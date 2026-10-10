use std::fs;

use serde_json::json;
use sha2::{Digest as _, Sha256};
use souvenirs_storage::{
    CatalogConfig, CatalogLayout, CatalogLock, DurableTransition, ManifestReplacementError,
    ManifestReplacementRequest, OperationJournal, OperationOutcome, OperationRecord,
};
use tempfile::TempDir;

fn initialized_catalog() -> (TempDir, CatalogLayout, CatalogLock) {
    let temporary = TempDir::new().unwrap();
    let layout = CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap();
    let lock = CatalogLock::acquire(&layout).unwrap();
    (temporary, layout, lock)
}

fn journal(layout: &CatalogLayout, lock: &CatalogLock, id: &str) -> OperationJournal {
    OperationJournal::create(layout, lock, id, &json!({"confirmed": true})).unwrap()
}

fn records(journal: &OperationJournal) -> Vec<OperationRecord> {
    fs::read_to_string(journal.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn persists_plan_transitions_and_completed_outcome_as_json_lines() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import1");

    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    journal.complete(&layout, &lock).unwrap();

    assert!(layout.root().join("2026").is_dir());
    assert_eq!(
        records(&journal),
        vec![
            OperationRecord::Plan {
                operation_id: "import1".to_owned(),
                plan: json!({"confirmed": true}),
            },
            OperationRecord::Transition {
                transition: DurableTransition::DirectoryCreationStarted {
                    step_id: "directory1".to_owned(),
                    path: "2026".into(),
                },
            },
            OperationRecord::Transition {
                transition: DurableTransition::DirectoryCreated {
                    step_id: "directory1".to_owned(),
                    path: "2026".into(),
                },
            },
            OperationRecord::Outcome {
                outcome: OperationOutcome::Completed,
            },
        ]
    );
}

#[test]
fn persists_cancelled_outcome_and_rejects_later_mutation() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import2");

    journal.cancel(&layout, &lock).unwrap();
    let error = journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap_err();

    assert!(matches!(
        error,
        souvenirs_storage::OperationJournalError::AlreadyFinished
    ));
    assert_eq!(
        records(&journal).last(),
        Some(&OperationRecord::Outcome {
            outcome: OperationOutcome::Cancelled,
        })
    );
    assert!(!layout.root().join("2026").exists());
}

#[test]
fn unfinished_journal_blocks_a_new_mutation() {
    let (_temporary, layout, lock) = initialized_catalog();
    let journal = journal(&layout, &lock, "import-unfinished");

    let error =
        OperationJournal::create(&layout, &lock, "import-next", &json!({"confirmed": true}))
            .unwrap_err();

    assert!(matches!(
        error,
        souvenirs_storage::OperationJournalError::UnfinishedOperation(path)
            if path == journal.path()
    ));
}

#[test]
fn completed_journal_allows_the_next_mutation() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut completed = journal(&layout, &lock, "import-complete");
    completed.complete(&layout, &lock).unwrap();

    let next = journal(&layout, &lock, "import-next");

    assert_eq!(next.operation_id(), "import-next");
}

#[test]
fn apple_double_sidecars_do_not_look_like_unfinished_journals() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut completed = journal(&layout, &lock, "import-complete");
    completed.complete(&layout, &lock).unwrap();
    fs::write(
        layout
            .journal_directory()
            .join("._20260101-000000-import.jsonl"),
        b"filesystem metadata",
    )
    .unwrap();

    let next = journal(&layout, &lock, "import-next");

    assert_eq!(next.operation_id(), "import-next");
}

#[test]
fn invalid_operation_identifier_is_rejected_before_journal_creation() {
    let (_temporary, layout, lock) = initialized_catalog();
    let before = fs::read_dir(layout.journal_directory()).unwrap().count();

    let error =
        OperationJournal::create(&layout, &lock, "../operation", &json!({"confirmed": true}))
            .unwrap_err();

    assert!(matches!(
        error,
        souvenirs_storage::OperationJournalError::InvalidIdentifier {
            kind: "operation",
            ..
        }
    ));
    assert_eq!(
        fs::read_dir(layout.journal_directory()).unwrap().count(),
        before
    );
}

#[test]
fn journal_cannot_mutate_a_different_catalog() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import");
    let (_other_temporary, other_layout, other_lock) = initialized_catalog();

    let error = journal
        .create_destination_directory(&other_layout, &other_lock, "directory1", "2026")
        .unwrap_err();

    assert!(matches!(
        error,
        souvenirs_storage::OperationJournalError::WrongCatalogJournal
    ));
    assert!(!other_layout.root().join("2026").exists());
}

#[test]
fn replaces_manifest_only_when_the_observed_baseline_matches() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import3");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let manifest_path = layout.root().join("2026/SHA256SUMS");
    let previous = b"old digest  old.jpg\n";
    let next = b"new digest  new.jpg\n";
    fs::write(&manifest_path, previous).unwrap();

    let replacement = layout
        .replace_manifest(
            &lock,
            &mut journal,
            ManifestReplacementRequest {
                path: std::path::Path::new("2026/SHA256SUMS"),
                previous_sha256: Some(&digest(previous)),
                resulting_sha256: &digest(next),
                step_id: "manifest1",
            },
            next,
        )
        .unwrap();

    assert_eq!(fs::read(&manifest_path).unwrap(), next);
    assert_eq!(replacement.path, manifest_path);
    assert!(!replacement.staged_path.exists());
    for record in records(&journal) {
        let staged_path = match record {
            OperationRecord::Transition {
                transition:
                    DurableTransition::ManifestReplacementStarted { staged_path, .. }
                    | DurableTransition::ManifestStaged { staged_path, .. }
                    | DurableTransition::ManifestPublicationStarted { staged_path, .. },
            } => Some(staged_path),
            _ => None,
        };
        if let Some(staged_path) = staged_path {
            assert!(!staged_path.is_absolute());
            assert!(staged_path.starts_with(".souvenirs/staging"));
        }
    }
    assert!(matches!(
        records(&journal).last(),
        Some(OperationRecord::Transition {
            transition: DurableTransition::ManifestReplaced { .. },
        })
    ));
}

#[test]
fn refuses_manifest_replacement_after_baseline_drift() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import4");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let manifest_path = layout.root().join("2026/SHA256SUMS");
    fs::write(&manifest_path, b"changed\n").unwrap();
    let next = b"next\n";

    let error = layout
        .replace_manifest(
            &lock,
            &mut journal,
            ManifestReplacementRequest {
                path: std::path::Path::new("2026/SHA256SUMS"),
                previous_sha256: Some(&digest(b"planned\n")),
                resulting_sha256: &digest(next),
                step_id: "manifest1",
            },
            next,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ManifestReplacementError::PreviousManifestChanged { .. }
    ));
    assert_eq!(fs::read(manifest_path).unwrap(), b"changed\n");
    assert_eq!(records(&journal).len(), 3);
}

#[test]
fn refuses_to_replace_an_unexpected_existing_manifest() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import5");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let manifest_path = layout.root().join("2026/SHA256SUMS");
    fs::write(&manifest_path, b"existing\n").unwrap();
    let next = b"next\n";

    let error = layout
        .replace_manifest(
            &lock,
            &mut journal,
            ManifestReplacementRequest {
                path: std::path::Path::new("2026/SHA256SUMS"),
                previous_sha256: None,
                resulting_sha256: &digest(next),
                step_id: "manifest1",
            },
            next,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ManifestReplacementError::PreviousManifestChanged {
            expected: None,
            actual: Some(_),
        }
    ));
    assert_eq!(fs::read(manifest_path).unwrap(), b"existing\n");
}

#[test]
fn rejects_manifest_bytes_that_do_not_match_the_planned_digest() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import6");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();

    let error = layout
        .replace_manifest(
            &lock,
            &mut journal,
            ManifestReplacementRequest {
                path: std::path::Path::new("2026/SHA256SUMS"),
                previous_sha256: None,
                resulting_sha256: &digest(b"planned\n"),
                step_id: "manifest1",
            },
            b"actual\n",
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ManifestReplacementError::ResultingDigestMismatch { .. }
    ));
    assert!(!layout.root().join("2026/SHA256SUMS").exists());
}
