#![cfg(feature = "test-failpoints")]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest as _, Sha256};
use souvenirs_storage::{
    CatalogConfig, CatalogLayout, CatalogLock, CopyExpectation, Failpoint, OperationJournal,
    OperationJournalError, OperationOutcome, OperationRecord, RecoveryCopy, RecoveryDirectory,
    RecoveryManifest, RecoveryMode, RecoveryOutcome, RecoveryRequest, VerifiedCopyError,
};
use tempfile::{Builder, TempDir};

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn target_root() -> Option<PathBuf> {
    std::env::var_os("SOUVENIRS_STORAGE_TEST_ROOT").map(PathBuf::from)
}

fn create_catalog_root(root: &Path) -> TempDir {
    Builder::new()
        .prefix("souvenirs-representative-")
        .tempdir_in(root)
        .expect("create a disposable catalog on the representative target")
}

#[test]
fn interruption_recovery_preserves_bytes_manifest_and_terminal_outcome() {
    let Some(target_root) = target_root() else {
        eprintln!("skipped: set SOUVENIRS_STORAGE_TEST_ROOT to a disposable mounted target");
        return;
    };
    let catalog_root = create_catalog_root(&target_root);
    let source_root = TempDir::new().unwrap();
    let source = source_root.path().join("source.bin");
    let contents = b"representative storage validation\n";
    fs::write(&source, contents).unwrap();

    let layout = CatalogLayout::initialize(catalog_root.path(), &CatalogConfig::default()).unwrap();
    let lock = CatalogLock::acquire(&layout).unwrap();
    let mut journal = OperationJournal::create(
        &layout,
        &lock,
        "representative",
        &json!({"confirmed": true}),
    )
    .unwrap();
    journal
        .create_destination_directory(&layout, &lock, "directory", "2026")
        .unwrap();
    journal.fail_at(Failpoint::CopyPublicationIntentPersisted);

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "copy",
            CopyExpectation {
                sha256: digest(contents),
                size_bytes: contents.len() as u64,
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        VerifiedCopyError::Journal(OperationJournalError::Failpoint(
            Failpoint::CopyPublicationIntentPersisted
        ))
    ));
    let journal_path = journal.path().to_path_buf();
    drop(journal);

    let manifest = format!("{}  original.bin\n", digest(contents)).into_bytes();
    let outcome = layout
        .recover_operation(
            &lock,
            &journal_path,
            RecoveryRequest {
                mode: RecoveryMode::Resume,
                directories: vec![RecoveryDirectory {
                    step_id: "directory".into(),
                    path: "2026".into(),
                }],
                copies: vec![RecoveryCopy {
                    step_id: "copy".into(),
                    source,
                    destination: "2026/original.bin".into(),
                    expected_sha256: digest(contents),
                    expected_size_bytes: contents.len() as u64,
                }],
                manifests: vec![RecoveryManifest {
                    step_id: "manifest".into(),
                    path: "2026/SHA256SUMS".into(),
                    previous_sha256: None,
                    contents: manifest.clone(),
                }],
            },
        )
        .unwrap();

    assert_eq!(outcome, RecoveryOutcome::Completed);
    assert_eq!(
        fs::read(layout.root().join("2026/original.bin")).unwrap(),
        contents
    );
    assert_eq!(
        fs::read(layout.root().join("2026/SHA256SUMS")).unwrap(),
        manifest
    );
    assert!(fs::read_dir(layout.staging_directory())
        .unwrap()
        .next()
        .is_none());
    let records: Vec<OperationRecord> = fs::read_to_string(&journal_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        records.last(),
        Some(&OperationRecord::Outcome {
            outcome: OperationOutcome::Completed,
        })
    );
}

#[test]
fn verified_copy_never_replaces_an_occupied_destination() {
    let Some(target_root) = target_root() else {
        eprintln!("skipped: set SOUVENIRS_STORAGE_TEST_ROOT to a disposable mounted target");
        return;
    };
    let catalog_root = create_catalog_root(&target_root);
    let source_root = TempDir::new().unwrap();
    let source = source_root.path().join("source.bin");
    fs::write(&source, b"planned").unwrap();

    let layout = CatalogLayout::initialize(catalog_root.path(), &CatalogConfig::default()).unwrap();
    let lock = CatalogLock::acquire(&layout).unwrap();
    let mut journal =
        OperationJournal::create(&layout, &lock, "occupied", &json!({"confirmed": true})).unwrap();
    journal
        .create_destination_directory(&layout, &lock, "directory", "2026")
        .unwrap();
    let destination = layout.root().join("2026/original.bin");
    fs::write(&destination, b"occupied").unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "copy",
            CopyExpectation {
                sha256: digest(b"planned"),
                size_bytes: 7,
            },
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::DestinationOccupied { .. }
    ));
    assert_eq!(fs::read(destination).unwrap(), b"occupied");
}
