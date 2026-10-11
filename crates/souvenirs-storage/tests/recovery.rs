#![cfg(feature = "test-failpoints")]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest as _, Sha256};
use souvenirs_storage::{
    CancellableRecoveryOutcome, CatalogConfig, CatalogLayout, CatalogLock, CopyExpectation,
    DurableTransition, Failpoint, ManifestReplacementRequest, OperationJournal,
    OperationJournalError, OperationRecord, RecoveryCheckpoint, RecoveryCopy, RecoveryDirectory,
    RecoveryError, RecoveryManifest, RecoveryMode, RecoveryOutcome, RecoveryRequest,
    RetainedDuplicate, RetainedDuplicateDisposition,
};
use tempfile::TempDir;

fn initialized_catalog() -> (TempDir, CatalogLayout, CatalogLock) {
    let temporary = TempDir::new().unwrap();
    let layout = CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap();
    let lock = CatalogLock::acquire(&layout).unwrap();
    (temporary, layout, lock)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn expectation(bytes: &[u8]) -> CopyExpectation {
    CopyExpectation {
        sha256: digest(bytes),
        size_bytes: bytes.len() as u64,
    }
}

fn journal_path(layout: &CatalogLayout, suffix: &str) -> PathBuf {
    fs::read_dir(layout.journal_directory())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(suffix)
        })
        .unwrap()
}

fn append_transitions(path: &Path, transitions: impl IntoIterator<Item = DurableTransition>) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    for transition in transitions {
        serde_json::to_writer(&mut file, &OperationRecord::Transition { transition }).unwrap();
        file.write_all(b"\n").unwrap();
    }
    file.sync_all().unwrap();
}

fn owned_stage(operation_id: &str, suffix: &str) -> PathBuf {
    format!(
        ".souvenirs/staging/{}-{operation_id}-{suffix}",
        operation_id.len()
    )
    .into()
}

fn operation_records(path: &Path) -> Vec<OperationRecord> {
    souvenirs_storage::read_operation_records(path).unwrap()
}

fn assert_no_outcome(records: &[OperationRecord]) {
    assert!(!records
        .iter()
        .any(|record| matches!(record, OperationRecord::Outcome { .. })));
}

#[test]
fn recovery_replays_retained_duplicates_idempotently() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal =
        OperationJournal::create(&layout, &lock, "retained", &json!({"confirmed": true})).unwrap();
    let journal_path = journal.path().to_path_buf();
    let retained = vec![
        RetainedDuplicate {
            source_file_id: "catalog-duplicate".to_owned(),
            disposition: RetainedDuplicateDisposition::CatalogDuplicate {
                catalog_path: vec![
                    "2024".to_owned(),
                    "2024-01-01 Existing".to_owned(),
                    "original.jpg".to_owned(),
                ],
            },
        },
        RetainedDuplicate {
            source_file_id: "source-duplicate".to_owned(),
            disposition: RetainedDuplicateDisposition::SourceDuplicate {
                selected_source_file_id: "selected-source".to_owned(),
            },
        },
    ];
    journal
        .record_retained_duplicate(&layout, &lock, &retained[0])
        .unwrap();
    drop(journal);

    let request = RecoveryRequest {
        retained_duplicates: retained.clone(),
        ..RecoveryRequest::default()
    };
    assert_eq!(
        layout
            .recover_operation(&lock, &journal_path, request)
            .unwrap(),
        RecoveryOutcome::Completed
    );

    let records = operation_records(&journal_path);
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, OperationRecord::Retained { .. }))
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    record,
                    OperationRecord::Retained { source_file_id, .. }
                        if source_file_id == "catalog-duplicate"
                )
            })
            .count(),
        1
    );
}

#[test]
fn recovery_rejects_duplicate_and_conflicting_retained_requests() {
    let (_temporary, layout, lock) = initialized_catalog();
    let retained = RetainedDuplicate {
        source_file_id: "source/identifier".to_owned(),
        disposition: RetainedDuplicateDisposition::SourceDuplicate {
            selected_source_file_id: "selected/identifier".to_owned(),
        },
    };
    let mut journal =
        OperationJournal::create(&layout, &lock, "retained-conflict", &json!({})).unwrap();
    journal
        .record_retained_duplicate(&layout, &lock, &retained)
        .unwrap();
    let journal_path = journal.path().to_path_buf();
    drop(journal);

    let duplicate = RecoveryRequest {
        retained_duplicates: vec![retained.clone(), retained.clone()],
        ..RecoveryRequest::default()
    };
    assert!(matches!(
        layout.recover_operation(&lock, &journal_path, duplicate),
        Err(RecoveryError::DuplicateRetainedDuplicate(source_file_id))
            if source_file_id == retained.source_file_id
    ));

    let conflicting = RecoveryRequest {
        retained_duplicates: vec![RetainedDuplicate {
            source_file_id: retained.source_file_id.clone(),
            disposition: RetainedDuplicateDisposition::CatalogDuplicate {
                catalog_path: vec!["2024".to_owned(), "different.jpg".to_owned()],
            },
        }],
        ..RecoveryRequest::default()
    };
    assert!(matches!(
        layout.recover_operation(&lock, &journal_path, conflicting),
        Err(RecoveryError::RetainedDuplicateMismatch(source_file_id))
            if source_file_id == retained.source_file_id
    ));
}

struct Scenario {
    _catalog: TempDir,
    _source: TempDir,
    layout: CatalogLayout,
    lock: CatalogLock,
    source: PathBuf,
    manifest: Vec<u8>,
    journal_path: PathBuf,
}

impl Scenario {
    fn new(failpoint: Failpoint) -> Self {
        let (catalog, layout, lock) = initialized_catalog();
        let source_directory = TempDir::new().unwrap();
        let source = source_directory.path().join("source.bin");
        fs::write(&source, b"verified bytes").unwrap();
        let manifest = format!("{}  original.bin\n", digest(b"verified bytes")).into_bytes();

        assert_ne!(failpoint, Failpoint::JournalCreated);
        if failpoint == Failpoint::PlanPersisted {
            let error = OperationJournal::create_failing_at(
                &layout,
                &lock,
                "recover",
                &json!({"confirmed": true}),
                failpoint,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                OperationJournalError::Failpoint(Failpoint::PlanPersisted)
            ));
        } else {
            let mut journal =
                OperationJournal::create(&layout, &lock, "recover", &json!({"confirmed": true}))
                    .unwrap();
            journal.fail_at(failpoint);

            let interrupted = match failpoint {
                Failpoint::DirectoryIntentPersisted | Failpoint::DirectoryPersisted => journal
                    .create_destination_directory(&layout, &lock, "directory", "2026")
                    .is_err(),
                _ => {
                    journal
                        .create_destination_directory(&layout, &lock, "directory", "2026")
                        .unwrap();
                    false
                }
            };
            if !interrupted {
                let interrupted = match failpoint {
                    Failpoint::CopyIntentPersisted
                    | Failpoint::CopyStagePersisted
                    | Failpoint::CopyPublicationIntentPersisted
                    | Failpoint::CopyPublished => layout
                        .copy_verified(
                            &lock,
                            &mut journal,
                            &source,
                            "2026/original.bin",
                            "copy",
                            expectation(b"verified bytes"),
                        )
                        .is_err(),
                    _ => {
                        layout
                            .copy_verified(
                                &lock,
                                &mut journal,
                                &source,
                                "2026/original.bin",
                                "copy",
                                expectation(b"verified bytes"),
                            )
                            .unwrap();
                        false
                    }
                };
                if !interrupted {
                    let interrupted = match failpoint {
                        Failpoint::ManifestIntentPersisted
                        | Failpoint::ManifestStagePersisted
                        | Failpoint::ManifestPublicationIntentPersisted
                        | Failpoint::ManifestPublished => layout
                            .replace_manifest(
                                &lock,
                                &mut journal,
                                ManifestReplacementRequest {
                                    path: Path::new("2026/SHA256SUMS"),
                                    previous_sha256: None,
                                    resulting_sha256: &digest(&manifest),
                                    step_id: "manifest",
                                },
                                &manifest,
                            )
                            .is_err(),
                        _ => {
                            layout
                                .replace_manifest(
                                    &lock,
                                    &mut journal,
                                    ManifestReplacementRequest {
                                        path: Path::new("2026/SHA256SUMS"),
                                        previous_sha256: None,
                                        resulting_sha256: &digest(&manifest),
                                        step_id: "manifest",
                                    },
                                    &manifest,
                                )
                                .unwrap();
                            false
                        }
                    };
                    if !interrupted {
                        let error = journal.complete(&layout, &lock).unwrap_err();
                        assert!(matches!(
                            error,
                            OperationJournalError::Failpoint(Failpoint::OutcomePersisted)
                        ));
                    }
                }
            }
        }

        let journal_path = journal_path(&layout, "-recover.jsonl");
        Self {
            _catalog: catalog,
            _source: source_directory,
            layout,
            lock,
            source,
            manifest,
            journal_path,
        }
    }

    fn request(&self) -> RecoveryRequest {
        RecoveryRequest {
            mode: RecoveryMode::Resume,
            retained_duplicates: Vec::new(),
            directories: vec![RecoveryDirectory {
                step_id: "directory".into(),
                path: "2026".into(),
            }],
            copies: vec![RecoveryCopy {
                step_id: "copy".into(),
                source: self.source.clone(),
                destination: "2026/original.bin".into(),
                expected_sha256: digest(b"verified bytes"),
                expected_size_bytes: 14,
            }],
            manifests: vec![RecoveryManifest {
                step_id: "manifest".into(),
                path: "2026/SHA256SUMS".into(),
                previous_sha256: None,
                contents: self.manifest.clone(),
            }],
        }
    }
}

#[test]
fn recovery_discards_an_interrupted_final_append_before_continuing() {
    for tail in [
        br#"{"kind":"transition","transition":{"kind":"directory_created""#.as_slice(),
        br#"{"kind":"outcome","outcome":"completed""#.as_slice(),
    ] {
        let (_temporary, layout, lock) = initialized_catalog();
        let journal =
            OperationJournal::create(&layout, &lock, "tail", &json!({"confirmed": true})).unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(tail).unwrap();
        file.sync_all().unwrap();
        drop(file);

        let outcome = layout
            .recover_operation(&lock, &path, RecoveryRequest::default())
            .unwrap();

        assert_eq!(outcome, RecoveryOutcome::Completed);
        assert_eq!(
            souvenirs_storage::read_operation_records(&path).unwrap(),
            vec![
                OperationRecord::Plan {
                    operation_id: "tail".into(),
                    plan: json!({"confirmed": true}),
                },
                OperationRecord::Outcome {
                    outcome: souvenirs_storage::OperationOutcome::Completed,
                },
            ]
        );
        assert!(fs::read(&path).unwrap().ends_with(b"\n"));
    }
}

#[test]
fn resumes_forward_from_every_durable_failpoint() {
    let failpoints = [
        Failpoint::PlanPersisted,
        Failpoint::DirectoryIntentPersisted,
        Failpoint::DirectoryPersisted,
        Failpoint::CopyIntentPersisted,
        Failpoint::CopyStagePersisted,
        Failpoint::CopyPublicationIntentPersisted,
        Failpoint::CopyPublished,
        Failpoint::ManifestIntentPersisted,
        Failpoint::ManifestStagePersisted,
        Failpoint::ManifestPublicationIntentPersisted,
        Failpoint::ManifestPublished,
        Failpoint::OutcomePersisted,
    ];

    for failpoint in failpoints {
        let scenario = Scenario::new(failpoint);
        let outcome = scenario
            .layout
            .recover_operation(&scenario.lock, &scenario.journal_path, scenario.request())
            .unwrap_or_else(|error| panic!("recovery failed after {failpoint:?}: {error}"));

        assert_eq!(outcome, RecoveryOutcome::Completed);
        assert_eq!(
            fs::read(scenario.layout.root().join("2026/original.bin")).unwrap(),
            b"verified bytes",
            "copy mismatch after {failpoint:?}"
        );
        assert_eq!(
            fs::read(scenario.layout.root().join("2026/SHA256SUMS")).unwrap(),
            scenario.manifest,
            "manifest mismatch after {failpoint:?}"
        );
        assert!(
            fs::read_dir(scenario.layout.staging_directory())
                .unwrap()
                .next()
                .is_none(),
            "owned stage remained after {failpoint:?}"
        );
        assert_eq!(
            scenario
                .layout
                .recover_operation(
                    &scenario.lock,
                    &scenario.journal_path,
                    RecoveryRequest::default(),
                )
                .unwrap(),
            RecoveryOutcome::Completed
        );
    }
}

#[test]
fn interruption_after_manifest_recovery_leaves_a_durable_nonterminal_boundary() {
    let scenario = Scenario::new(Failpoint::ManifestIntentPersisted);
    let manifest_path = scenario.layout.root().join("2026/SHA256SUMS");

    let outcome = scenario
        .layout
        .recover_operation_cancellable(
            &scenario.lock,
            &scenario.journal_path,
            scenario.request(),
            |checkpoint| {
                checkpoint == RecoveryCheckpoint::DurableBoundary && manifest_path.is_file()
            },
        )
        .unwrap();

    assert_eq!(outcome, CancellableRecoveryOutcome::Interrupted);
    assert_eq!(fs::read(&manifest_path).unwrap(), scenario.manifest);
    assert_no_outcome(&operation_records(&scenario.journal_path));

    let mut cancellation = scenario.request();
    cancellation.mode = RecoveryMode::CancelRemainingCopies;
    assert_eq!(
        scenario
            .layout
            .recover_operation(&scenario.lock, &scenario.journal_path, cancellation)
            .unwrap(),
        RecoveryOutcome::Cancelled
    );
}

#[test]
fn interruption_wins_before_the_terminal_recovery_boundary() {
    let scenario = Scenario::new(Failpoint::ManifestPublished);

    let outcome = scenario
        .layout
        .recover_operation_cancellable(
            &scenario.lock,
            &scenario.journal_path,
            scenario.request(),
            |checkpoint| checkpoint == RecoveryCheckpoint::CompletionBoundary,
        )
        .unwrap();

    assert_eq!(outcome, CancellableRecoveryOutcome::Interrupted);
    assert_no_outcome(&operation_records(&scenario.journal_path));

    let mut cancellation = scenario.request();
    cancellation.mode = RecoveryMode::CancelRemainingCopies;
    assert_eq!(
        scenario
            .layout
            .recover_operation(&scenario.lock, &scenario.journal_path, cancellation)
            .unwrap(),
        RecoveryOutcome::Cancelled
    );
}

#[test]
fn observed_directory_is_persisted_before_recovery_records_completion() {
    let (_catalog, layout, lock) = initialized_catalog();
    let journal =
        OperationJournal::create(&layout, &lock, "observed-directory", &json!({})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    append_transitions(
        &path,
        [DurableTransition::DirectoryCreationStarted {
            step_id: "directory".into(),
            path: "2026".into(),
        }],
    );
    fs::create_dir(layout.root().join("2026")).unwrap();
    let request = RecoveryRequest {
        directories: vec![RecoveryDirectory {
            step_id: "directory".into(),
            path: "2026".into(),
        }],
        ..RecoveryRequest::default()
    };

    let error = layout
        .recover_operation_failing_at(
            &lock,
            &path,
            request.clone(),
            Failpoint::RecoveryDirectoryObservedBeforePersistence,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Journal(OperationJournalError::Failpoint(
            Failpoint::RecoveryDirectoryObservedBeforePersistence
        ))
    ));
    let records = operation_records(&path);
    assert!(!records.iter().any(|record| matches!(
        record,
        OperationRecord::Transition {
            transition: DurableTransition::DirectoryCreated { .. }
        }
    )));
    assert_no_outcome(&records);

    assert_eq!(
        layout.recover_operation(&lock, &path, request).unwrap(),
        RecoveryOutcome::Completed
    );
}

#[test]
fn observed_copy_rename_is_persisted_on_both_sides_before_completion() {
    for failpoint in [
        Failpoint::RecoveryCopyStageObservedBeforePersistence,
        Failpoint::RecoveryCopyDestinationObservedBeforePersistence,
    ] {
        let (_catalog, layout, lock) = initialized_catalog();
        fs::create_dir(layout.root().join("2026")).unwrap();
        let operation_id = "observed-copy";
        let journal = OperationJournal::create(&layout, &lock, operation_id, &json!({})).unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);
        let staged_path = owned_stage(operation_id, "copy.copy");
        let staged_absolute = layout.root().join(&staged_path);
        fs::write(&staged_absolute, b"published").unwrap();
        append_transitions(
            &path,
            [
                DurableTransition::CopyStarted {
                    step_id: "copy".into(),
                    source: "unavailable".into(),
                    staged_path: staged_path.clone(),
                    destination: "2026/original.bin".into(),
                },
                DurableTransition::CopyVerified {
                    step_id: "copy".into(),
                    staged_path: staged_path.clone(),
                    destination: "2026/original.bin".into(),
                    sha256: digest(b"published"),
                    bytes_copied: 9,
                },
                DurableTransition::CopyPublicationStarted {
                    step_id: "copy".into(),
                    staged_path: staged_path.clone(),
                    destination: "2026/original.bin".into(),
                    sha256: digest(b"published"),
                },
            ],
        );
        fs::rename(staged_absolute, layout.root().join("2026/original.bin")).unwrap();
        let request = RecoveryRequest {
            copies: vec![RecoveryCopy {
                step_id: "copy".into(),
                source: "unavailable".into(),
                destination: "2026/original.bin".into(),
                expected_sha256: digest(b"published"),
                expected_size_bytes: 9,
            }],
            ..RecoveryRequest::default()
        };

        let error = layout
            .recover_operation_failing_at(&lock, &path, request.clone(), failpoint)
            .unwrap_err();
        assert!(matches!(
            error,
            RecoveryError::Journal(OperationJournalError::Failpoint(actual))
                if actual == failpoint
        ));
        let records = operation_records(&path);
        assert!(!records.iter().any(|record| matches!(
            record,
            OperationRecord::Transition {
                transition: DurableTransition::CopyPublished { .. }
            }
        )));
        assert_no_outcome(&records);

        assert_eq!(
            layout.recover_operation(&lock, &path, request).unwrap(),
            RecoveryOutcome::Completed
        );
        assert_eq!(
            fs::read(layout.root().join("2026/original.bin")).unwrap(),
            b"published"
        );
    }
}

#[test]
fn recorded_copy_publication_persists_observed_stage_cleanup_before_outcome() {
    let (_catalog, layout, lock) = initialized_catalog();
    fs::create_dir(layout.root().join("2026")).unwrap();
    let operation_id = "published-copy";
    let journal = OperationJournal::create(&layout, &lock, operation_id, &json!({})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    let staged_path = owned_stage(operation_id, "copy.copy");
    let staged_absolute = layout.root().join(&staged_path);
    let staged_name = staged_absolute.file_name().unwrap().to_string_lossy();
    let apple_double = staged_absolute.with_file_name(format!("._{staged_name}"));
    fs::write(layout.root().join("2026/original.bin"), b"published").unwrap();
    fs::write(&apple_double, b"metadata").unwrap();
    append_transitions(
        &path,
        [
            DurableTransition::CopyStarted {
                step_id: "copy".into(),
                source: "unavailable".into(),
                staged_path: staged_path.clone(),
                destination: "2026/original.bin".into(),
            },
            DurableTransition::CopyVerified {
                step_id: "copy".into(),
                staged_path,
                destination: "2026/original.bin".into(),
                sha256: digest(b"published"),
                bytes_copied: 9,
            },
            DurableTransition::CopyPublished {
                step_id: "copy".into(),
                destination: "2026/original.bin".into(),
                sha256: digest(b"published"),
            },
        ],
    );
    let request = RecoveryRequest {
        copies: vec![RecoveryCopy {
            step_id: "copy".into(),
            source: "unavailable".into(),
            destination: "2026/original.bin".into(),
            expected_sha256: digest(b"published"),
            expected_size_bytes: 9,
        }],
        ..RecoveryRequest::default()
    };

    let error = layout
        .recover_operation_failing_at(
            &lock,
            &path,
            request.clone(),
            Failpoint::RecoveryCopyStageObservedBeforePersistence,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Journal(OperationJournalError::Failpoint(
            Failpoint::RecoveryCopyStageObservedBeforePersistence
        ))
    ));
    assert!(!apple_double.exists());
    assert_no_outcome(&operation_records(&path));

    assert_eq!(
        layout.recover_operation(&lock, &path, request).unwrap(),
        RecoveryOutcome::Completed
    );
}

#[test]
fn observed_manifest_rename_is_persisted_on_both_sides_before_completion() {
    for failpoint in [
        Failpoint::RecoveryManifestStageObservedBeforePersistence,
        Failpoint::RecoveryManifestDestinationObservedBeforePersistence,
    ] {
        let (_catalog, layout, lock) = initialized_catalog();
        fs::create_dir(layout.root().join("2026")).unwrap();
        let operation_id = "observed-manifest";
        let journal = OperationJournal::create(&layout, &lock, operation_id, &json!({})).unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);
        let contents = format!("{}  original.bin\n", digest(b"published")).into_bytes();
        let resulting_sha256 = digest(&contents);
        let staged_path = owned_stage(operation_id, "manifest.tmp");
        let staged_absolute = layout.root().join(&staged_path);
        fs::write(&staged_absolute, &contents).unwrap();
        append_transitions(
            &path,
            [
                DurableTransition::ManifestReplacementStarted {
                    step_id: "manifest".into(),
                    staged_path: staged_path.clone(),
                    manifest_path: "2026/SHA256SUMS".into(),
                    previous_sha256: None,
                    resulting_sha256: resulting_sha256.clone(),
                },
                DurableTransition::ManifestStaged {
                    step_id: "manifest".into(),
                    staged_path: staged_path.clone(),
                    manifest_path: "2026/SHA256SUMS".into(),
                    resulting_sha256: resulting_sha256.clone(),
                },
                DurableTransition::ManifestPublicationStarted {
                    step_id: "manifest".into(),
                    staged_path: staged_path.clone(),
                    manifest_path: "2026/SHA256SUMS".into(),
                    resulting_sha256: resulting_sha256.clone(),
                },
            ],
        );
        fs::rename(staged_absolute, layout.root().join("2026/SHA256SUMS")).unwrap();
        let request = RecoveryRequest {
            manifests: vec![RecoveryManifest {
                step_id: "manifest".into(),
                path: "2026/SHA256SUMS".into(),
                previous_sha256: None,
                contents: contents.clone(),
            }],
            ..RecoveryRequest::default()
        };

        let error = layout
            .recover_operation_failing_at(&lock, &path, request.clone(), failpoint)
            .unwrap_err();
        assert!(matches!(
            error,
            RecoveryError::Journal(OperationJournalError::Failpoint(actual))
                if actual == failpoint
        ));
        let records = operation_records(&path);
        assert!(!records.iter().any(|record| matches!(
            record,
            OperationRecord::Transition {
                transition: DurableTransition::ManifestReplaced { .. }
            }
        )));
        assert_no_outcome(&records);

        assert_eq!(
            layout.recover_operation(&lock, &path, request).unwrap(),
            RecoveryOutcome::Completed
        );
        assert_eq!(
            fs::read(layout.root().join("2026/SHA256SUMS")).unwrap(),
            contents
        );
    }
}

#[test]
fn observed_absent_owned_stage_is_persisted_before_removal_completion() {
    let (_catalog, layout, lock) = initialized_catalog();
    let operation_id = "observed-removal";
    let journal = OperationJournal::create(&layout, &lock, operation_id, &json!({})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    let staged_path = owned_stage(operation_id, "copy.copy");
    append_transitions(
        &path,
        [
            DurableTransition::CopyStarted {
                step_id: "copy".into(),
                source: "unavailable".into(),
                staged_path: staged_path.clone(),
                destination: "original.bin".into(),
            },
            DurableTransition::StageRemovalStarted {
                step_id: "copy".into(),
                staged_path: staged_path.clone(),
            },
        ],
    );
    let unrelated_stage = layout.staging_directory().join("unrelated.tmp");
    fs::write(&unrelated_stage, b"keep").unwrap();
    let request = RecoveryRequest {
        mode: RecoveryMode::CancelRemainingCopies,
        retained_duplicates: Vec::new(),
        copies: vec![RecoveryCopy {
            step_id: "copy".into(),
            source: "unavailable".into(),
            destination: "original.bin".into(),
            expected_sha256: digest(b"unused"),
            expected_size_bytes: 6,
        }],
        ..RecoveryRequest::default()
    };

    let error = layout
        .recover_operation_failing_at(
            &lock,
            &path,
            request.clone(),
            Failpoint::RecoveryStageRemovalObservedBeforePersistence,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Journal(OperationJournalError::Failpoint(
            Failpoint::RecoveryStageRemovalObservedBeforePersistence
        ))
    ));
    let records = operation_records(&path);
    assert!(!records.iter().any(|record| matches!(
        record,
        OperationRecord::Transition {
            transition: DurableTransition::StageRemoved { .. }
        }
    )));
    assert_no_outcome(&records);

    assert_eq!(
        layout.recover_operation(&lock, &path, request).unwrap(),
        RecoveryOutcome::Cancelled
    );
    assert_eq!(fs::read(unrelated_stage).unwrap(), b"keep");
    let records = operation_records(&path);
    let removed = records
        .iter()
        .position(|record| {
            matches!(
                record,
                OperationRecord::Transition {
                    transition: DurableTransition::StageRemoved { .. }
                }
            )
        })
        .unwrap();
    let outcome = records
        .iter()
        .position(|record| matches!(record, OperationRecord::Outcome { .. }))
        .unwrap();
    assert!(removed < outcome);
}

#[test]
fn cleanup_tracks_the_latest_lifecycle_for_a_reused_owned_stage_path() {
    let (_catalog, layout, lock) = initialized_catalog();
    let operation_id = "reused-stage";
    let journal = OperationJournal::create(&layout, &lock, operation_id, &json!({})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    let staged_path = owned_stage(operation_id, "copy.copy");
    fs::write(layout.root().join(&staged_path), b"new lifecycle").unwrap();
    append_transitions(
        &path,
        [
            DurableTransition::CopyStarted {
                step_id: "copy".into(),
                source: "unavailable".into(),
                staged_path: staged_path.clone(),
                destination: "original.bin".into(),
            },
            DurableTransition::StageRemovalStarted {
                step_id: "copy".into(),
                staged_path: staged_path.clone(),
            },
            DurableTransition::StageRemoved {
                step_id: "copy".into(),
                staged_path: staged_path.clone(),
            },
            DurableTransition::CopyStarted {
                step_id: "copy".into(),
                source: "unavailable".into(),
                staged_path: staged_path.clone(),
                destination: "original.bin".into(),
            },
        ],
    );
    let request = RecoveryRequest {
        mode: RecoveryMode::CancelRemainingCopies,
        retained_duplicates: Vec::new(),
        copies: vec![RecoveryCopy {
            step_id: "copy".into(),
            source: "unavailable".into(),
            destination: "original.bin".into(),
            expected_sha256: digest(b"new lifecycle"),
            expected_size_bytes: 13,
        }],
        ..RecoveryRequest::default()
    };

    let error = layout
        .recover_operation_failing_at(
            &lock,
            &path,
            request.clone(),
            Failpoint::StageRemovalIntentPersisted,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Journal(OperationJournalError::Failpoint(
            Failpoint::StageRemovalIntentPersisted
        ))
    ));
    assert!(layout.root().join(&staged_path).is_file());

    assert_eq!(
        layout
            .recover_operation(&lock, &path, request.clone())
            .unwrap(),
        RecoveryOutcome::Cancelled
    );
    assert!(!layout.root().join(staged_path).exists());
    assert_eq!(
        layout
            .recover_operation(&lock, &path, RecoveryRequest::default())
            .unwrap(),
        RecoveryOutcome::Cancelled
    );
}

#[test]
fn cancellation_publishes_verified_stages_updates_manifest_and_removes_only_owned_stages() {
    let (_catalog, layout, lock) = initialized_catalog();
    let sources = TempDir::new().unwrap();
    let first = sources.path().join("first.bin");
    let second = sources.path().join("second.bin");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    let mut journal =
        OperationJournal::create(&layout, &lock, "cancel", &json!({"confirmed": true})).unwrap();
    journal
        .create_destination_directory(&layout, &lock, "directory", "2026")
        .unwrap();
    layout
        .copy_verified(
            &lock,
            &mut journal,
            &first,
            "2026/first.bin",
            "copy-first",
            expectation(b"first"),
        )
        .unwrap();
    journal.fail_at(Failpoint::CopyStagePersisted);
    assert!(layout
        .copy_verified(
            &lock,
            &mut journal,
            &second,
            "2026/second.bin",
            "copy-second",
            expectation(b"second"),
        )
        .is_err());
    fs::remove_file(&second).unwrap();
    let unrelated_stage = layout.staging_directory().join("unrelated.tmp");
    fs::write(&unrelated_stage, b"do not remove").unwrap();
    let manifest = format!(
        "{}  first.bin\n{}  second.bin\n",
        digest(b"first"),
        digest(b"second")
    )
    .into_bytes();
    let path = journal.path().to_path_buf();
    drop(journal);

    let outcome = layout
        .recover_operation(
            &lock,
            &path,
            RecoveryRequest {
                mode: RecoveryMode::CancelRemainingCopies,
                retained_duplicates: Vec::new(),
                directories: vec![RecoveryDirectory {
                    step_id: "directory".into(),
                    path: "2026".into(),
                }],
                copies: vec![
                    RecoveryCopy {
                        step_id: "copy-first".into(),
                        source: first,
                        destination: "2026/first.bin".into(),
                        expected_sha256: digest(b"first"),
                        expected_size_bytes: 5,
                    },
                    RecoveryCopy {
                        step_id: "copy-second".into(),
                        source: second,
                        destination: "2026/second.bin".into(),
                        expected_sha256: digest(b"second"),
                        expected_size_bytes: 6,
                    },
                ],
                manifests: vec![RecoveryManifest {
                    step_id: "manifest".into(),
                    path: "2026/SHA256SUMS".into(),
                    previous_sha256: None,
                    contents: manifest.clone(),
                }],
            },
        )
        .unwrap();

    assert_eq!(outcome, RecoveryOutcome::Cancelled);
    assert_eq!(
        fs::read(layout.root().join("2026/first.bin")).unwrap(),
        b"first"
    );
    assert_eq!(
        fs::read(layout.root().join("2026/second.bin")).unwrap(),
        b"second"
    );
    assert_eq!(
        fs::read(layout.root().join("2026/SHA256SUMS")).unwrap(),
        manifest
    );
    assert_eq!(fs::read(unrelated_stage).unwrap(), b"do not remove");
    let journal_text = fs::read_to_string(&path).unwrap();
    assert!(
        journal_text.find("\"kind\":\"manifest_replaced\"").unwrap()
            < journal_text.find("\"outcome\":\"cancelled\"").unwrap()
    );
    assert_eq!(
        layout
            .recover_operation(&lock, &path, RecoveryRequest::default())
            .unwrap(),
        RecoveryOutcome::Cancelled
    );
}

#[test]
fn cancellation_removes_a_verified_stage_that_differs_from_the_confirmed_plan() {
    let (_catalog, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"drifted").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();
    let journal =
        OperationJournal::create(&layout, &lock, "drifted", &json!({"confirmed": true})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    let stage: PathBuf = ".souvenirs/staging/7-drifted-copy.copy".into();
    fs::write(layout.root().join(&stage), b"drifted").unwrap();
    append_transitions(
        &path,
        [
            DurableTransition::CopyStarted {
                step_id: "copy".into(),
                source: source.clone(),
                staged_path: stage,
                destination: "2026/original.bin".into(),
            },
            DurableTransition::CopyVerified {
                step_id: "copy".into(),
                staged_path: ".souvenirs/staging/7-drifted-copy.copy".into(),
                destination: "2026/original.bin".into(),
                sha256: digest(b"drifted"),
                bytes_copied: 7,
            },
        ],
    );

    let outcome = layout
        .recover_operation(
            &lock,
            &path,
            RecoveryRequest {
                mode: RecoveryMode::CancelRemainingCopies,
                retained_duplicates: Vec::new(),
                directories: vec![],
                copies: vec![RecoveryCopy {
                    step_id: "copy".into(),
                    source,
                    destination: "2026/original.bin".into(),
                    expected_sha256: digest(b"planned"),
                    expected_size_bytes: 7,
                }],
                manifests: vec![],
            },
        )
        .unwrap();

    assert_eq!(outcome, RecoveryOutcome::Cancelled);
    assert!(!layout.root().join("2026/original.bin").exists());
    assert!(!layout
        .root()
        .join(".souvenirs/staging/7-drifted-copy.copy")
        .exists());
}

#[test]
fn recovery_never_overwrites_an_occupied_destination() {
    let (_catalog, layout, lock) = initialized_catalog();
    let sources = TempDir::new().unwrap();
    let source = sources.path().join("source.bin");
    fs::write(&source, b"planned").unwrap();
    let mut journal =
        OperationJournal::create(&layout, &lock, "occupied", &json!({"confirmed": true})).unwrap();
    journal
        .create_destination_directory(&layout, &lock, "directory", "2026")
        .unwrap();
    journal.fail_at(Failpoint::CopyStagePersisted);
    assert!(layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "copy",
            expectation(b"planned"),
        )
        .is_err());
    let destination = layout.root().join("2026/original.bin");
    fs::write(&destination, b"occupied").unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);

    let error = layout
        .recover_operation(
            &lock,
            path,
            RecoveryRequest {
                mode: RecoveryMode::Resume,
                retained_duplicates: Vec::new(),
                directories: vec![RecoveryDirectory {
                    step_id: "directory".into(),
                    path: "2026".into(),
                }],
                copies: vec![RecoveryCopy {
                    step_id: "copy".into(),
                    source,
                    destination: "2026/original.bin".into(),
                    expected_sha256: digest(b"planned"),
                    expected_size_bytes: 7,
                }],
                manifests: vec![],
            },
        )
        .unwrap_err();

    assert!(matches!(error, RecoveryError::CopyConflict(_)));
    assert_eq!(fs::read(destination).unwrap(), b"occupied");
}

#[test]
fn recovery_preserves_journal_order_for_nested_directories_and_copy_publication() {
    let (_catalog, layout, lock) = initialized_catalog();
    let journal =
        OperationJournal::create(&layout, &lock, "ordered", &json!({"confirmed": true})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    append_transitions(
        &path,
        [
            DurableTransition::DirectoryCreationStarted {
                step_id: "z-parent".into(),
                path: "2026".into(),
            },
            DurableTransition::DirectoryCreationStarted {
                step_id: "a-child".into(),
                path: "2026/collection".into(),
            },
        ],
    );

    layout
        .recover_operation(
            &lock,
            &path,
            RecoveryRequest {
                mode: RecoveryMode::Resume,
                retained_duplicates: Vec::new(),
                directories: vec![
                    RecoveryDirectory {
                        step_id: "z-parent".into(),
                        path: "2026".into(),
                    },
                    RecoveryDirectory {
                        step_id: "a-child".into(),
                        path: "2026/collection".into(),
                    },
                ],
                copies: vec![],
                manifests: vec![],
            },
        )
        .unwrap();
    assert!(layout.root().join("2026/collection").is_dir());

    let journal =
        OperationJournal::create(&layout, &lock, "ordercopy", &json!({"confirmed": true})).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    let original_stage: PathBuf = ".souvenirs/staging/9-ordercopy-z-original.copy".into();
    let sidecar_stage: PathBuf = ".souvenirs/staging/9-ordercopy-a-sidecar.copy".into();
    fs::write(layout.root().join(&original_stage), b"original").unwrap();
    fs::write(layout.root().join(&sidecar_stage), b"sidecar").unwrap();
    append_transitions(
        &path,
        [
            DurableTransition::CopyStarted {
                step_id: "z-original".into(),
                source: "unavailable-original".into(),
                staged_path: original_stage.clone(),
                destination: "2026/collection/original.bin".into(),
            },
            DurableTransition::CopyVerified {
                step_id: "z-original".into(),
                staged_path: original_stage,
                destination: "2026/collection/original.bin".into(),
                sha256: digest(b"original"),
                bytes_copied: 8,
            },
            DurableTransition::CopyStarted {
                step_id: "a-sidecar".into(),
                source: "unavailable-sidecar".into(),
                staged_path: sidecar_stage.clone(),
                destination: "2026/collection/original.bin.xmp".into(),
            },
            DurableTransition::CopyVerified {
                step_id: "a-sidecar".into(),
                staged_path: sidecar_stage,
                destination: "2026/collection/original.bin.xmp".into(),
                sha256: digest(b"sidecar"),
                bytes_copied: 7,
            },
        ],
    );

    layout
        .recover_operation(
            &lock,
            &path,
            RecoveryRequest {
                mode: RecoveryMode::Resume,
                retained_duplicates: Vec::new(),
                directories: vec![],
                copies: vec![
                    RecoveryCopy {
                        step_id: "z-original".into(),
                        source: "unavailable-original".into(),
                        destination: "2026/collection/original.bin".into(),
                        expected_sha256: digest(b"original"),
                        expected_size_bytes: 8,
                    },
                    RecoveryCopy {
                        step_id: "a-sidecar".into(),
                        source: "unavailable-sidecar".into(),
                        destination: "2026/collection/original.bin.xmp".into(),
                        expected_sha256: digest(b"sidecar"),
                        expected_size_bytes: 7,
                    },
                ],
                manifests: vec![],
            },
        )
        .unwrap();
    let published: Vec<_> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<OperationRecord>(line).ok())
        .filter_map(|record| match record {
            OperationRecord::Transition {
                transition: DurableTransition::CopyPublished { step_id, .. },
            } => Some(step_id),
            _ => None,
        })
        .collect();
    assert_eq!(published, ["z-original", "a-sidecar"]);
}

#[test]
fn staging_cleanup_recovers_after_each_cleanup_failpoint() {
    for failpoint in [
        Failpoint::StageRemovalIntentPersisted,
        Failpoint::StageRemovalPersisted,
    ] {
        let (_catalog, layout, lock) = initialized_catalog();
        let sources = TempDir::new().unwrap();
        let source = sources.path().join("missing.bin");
        fs::write(&source, b"partial").unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, "cleanup", &json!({"confirmed": true}))
                .unwrap();
        journal
            .create_destination_directory(&layout, &lock, "directory", "2026")
            .unwrap();
        journal.fail_at(Failpoint::CopyIntentPersisted);
        assert!(layout
            .copy_verified(
                &lock,
                &mut journal,
                &source,
                "2026/original.bin",
                "copy",
                expectation(b"partial"),
            )
            .is_err());
        let stage = layout.staging_directory().join("7-cleanup-copy.copy");
        fs::write(&stage, b"partial").unwrap();
        fs::remove_file(&source).unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);
        let request = RecoveryRequest {
            mode: RecoveryMode::CancelRemainingCopies,
            retained_duplicates: Vec::new(),
            directories: vec![RecoveryDirectory {
                step_id: "directory".into(),
                path: "2026".into(),
            }],
            copies: vec![RecoveryCopy {
                step_id: "copy".into(),
                source,
                destination: "2026/original.bin".into(),
                expected_sha256: digest(b"partial"),
                expected_size_bytes: 7,
            }],
            manifests: vec![],
        };

        let error = layout
            .recover_operation_failing_at(&lock, &path, request.clone(), failpoint)
            .unwrap_err();
        assert!(matches!(
            error,
            RecoveryError::Journal(OperationJournalError::Failpoint(actual))
                if actual == failpoint
        ));
        assert_eq!(
            layout.recover_operation(&lock, &path, request).unwrap(),
            RecoveryOutcome::Cancelled
        );
        assert!(!stage.exists());
    }
}

#[test]
fn cancellation_replaces_a_stale_planned_manifest_with_published_files_only() {
    let (_catalog, layout, lock) = initialized_catalog();
    let sources = TempDir::new().unwrap();
    let source = sources.path().join("first.bin");
    fs::write(&source, b"first").unwrap();
    let mut journal =
        OperationJournal::create(&layout, &lock, "correct", &json!({"confirmed": true})).unwrap();
    journal
        .create_destination_directory(&layout, &lock, "directory", "2026")
        .unwrap();
    layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/first.bin",
            "copy-first",
            expectation(b"first"),
        )
        .unwrap();
    let stale_manifest = format!(
        "{}  first.bin\n{}  second.bin\n",
        digest(b"first"),
        digest(b"second")
    )
    .into_bytes();
    journal.fail_at(Failpoint::ManifestIntentPersisted);
    assert!(layout
        .replace_manifest(
            &lock,
            &mut journal,
            ManifestReplacementRequest {
                path: Path::new("2026/SHA256SUMS"),
                previous_sha256: None,
                resulting_sha256: &digest(&stale_manifest),
                step_id: "manifest",
            },
            &stale_manifest,
        )
        .is_err());
    let path = journal.path().to_path_buf();
    drop(journal);
    let corrected_manifest = format!("{}  first.bin\n", digest(b"first")).into_bytes();

    assert_eq!(
        layout
            .recover_operation(
                &lock,
                path,
                RecoveryRequest {
                    mode: RecoveryMode::CancelRemainingCopies,
                    retained_duplicates: Vec::new(),
                    directories: vec![RecoveryDirectory {
                        step_id: "directory".into(),
                        path: "2026".into(),
                    }],
                    copies: vec![RecoveryCopy {
                        step_id: "copy-first".into(),
                        source,
                        destination: "2026/first.bin".into(),
                        expected_sha256: digest(b"first"),
                        expected_size_bytes: 5,
                    }],
                    manifests: vec![RecoveryManifest {
                        step_id: "manifest".into(),
                        path: "2026/SHA256SUMS".into(),
                        previous_sha256: None,
                        contents: corrected_manifest.clone(),
                    }],
                },
            )
            .unwrap(),
        RecoveryOutcome::Cancelled
    );
    assert_eq!(
        fs::read(layout.root().join("2026/SHA256SUMS")).unwrap(),
        corrected_manifest
    );
}
