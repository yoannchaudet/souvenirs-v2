#![cfg(feature = "test-failpoints")]

use std::fs;

use serde_json::json;
use sha2::{Digest as _, Sha256};
use souvenirs_storage::{
    CatalogConfig, CatalogLayout, CatalogLock, CopyExpectation, Failpoint,
    ManifestReplacementRequest, OperationJournal, OperationJournalError, VerifiedCopyError,
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

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn expectation(bytes: &[u8]) -> CopyExpectation {
    CopyExpectation {
        sha256: digest(bytes),
        size_bytes: bytes.len() as u64,
    }
}

#[test]
fn plan_failpoint_leaves_the_confirmed_plan_as_the_first_record() {
    let (_temporary, layout, lock) = initialized_catalog();

    let error = OperationJournal::create_failing_at(
        &layout,
        &lock,
        "plan",
        &json!({"confirmed": true}),
        Failpoint::PlanPersisted,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        OperationJournalError::Failpoint(Failpoint::PlanPersisted)
    ));
    let journal = fs::read_dir(layout.journal_directory())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-plan.jsonl")
        })
        .unwrap();
    assert_eq!(fs::read_to_string(journal).unwrap().lines().count(), 1);
}

#[test]
fn outcome_failpoint_leaves_a_terminal_record_that_unblocks_the_catalog() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "outcome");
    journal.fail_at(Failpoint::OutcomePersisted);

    let error = journal.complete(&layout, &lock).unwrap_err();

    assert!(matches!(
        error,
        OperationJournalError::Failpoint(Failpoint::OutcomePersisted)
    ));
    assert!(OperationJournal::create(&layout, &lock, "next", &json!({"confirmed": true})).is_ok());
}

#[test]
fn directory_intent_failpoint_precedes_directory_mutation() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "directory");
    journal.fail_at(Failpoint::DirectoryIntentPersisted);

    let error = journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap_err();

    assert!(matches!(
        error,
        OperationJournalError::Failpoint(Failpoint::DirectoryIntentPersisted)
    ));
    assert!(!layout.root().join("2026").exists());
    assert!(fs::read_to_string(journal.path())
        .unwrap()
        .contains("\"kind\":\"directory_creation_started\""));
}

#[test]
fn copy_publication_intent_failpoint_preserves_verified_stage() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "copy");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"verified").unwrap();
    journal.fail_at(Failpoint::CopyPublicationIntentPersisted);

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "copy1",
            expectation(b"verified"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Journal(OperationJournalError::Failpoint(
            Failpoint::CopyPublicationIntentPersisted
        ))
    ));
    assert!(!layout.root().join("2026/original.bin").exists());
    assert_eq!(
        fs::read(layout.staging_directory().join("4-copy-copy1.copy")).unwrap(),
        b"verified"
    );
    let journal_text = fs::read_to_string(journal.path()).unwrap();
    assert!(journal_text.contains("\"staged_path\":\".souvenirs/staging/4-copy-copy1.copy\""));
    assert!(!journal_text.contains(layout.root().to_string_lossy().as_ref()));
}

#[test]
fn copy_published_failpoint_leaves_verified_destination_and_record() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "copy2");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"verified").unwrap();
    journal.fail_at(Failpoint::CopyPublished);

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "copy1",
            expectation(b"verified"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Journal(OperationJournalError::Failpoint(Failpoint::CopyPublished))
    ));
    assert_eq!(
        fs::read(layout.root().join("2026/original.bin")).unwrap(),
        b"verified"
    );
    assert!(fs::read_to_string(journal.path())
        .unwrap()
        .contains("\"kind\":\"copy_published\""));
}

#[test]
fn manifest_publication_intent_failpoint_preserves_old_manifest_and_stage() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "manifest");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let manifest = layout.root().join("2026/SHA256SUMS");
    fs::write(&manifest, b"old\n").unwrap();
    let next = b"next\n";
    journal.fail_at(Failpoint::ManifestPublicationIntentPersisted);

    let error = layout
        .replace_manifest(
            &lock,
            &mut journal,
            ManifestReplacementRequest {
                path: std::path::Path::new("2026/SHA256SUMS"),
                previous_sha256: Some(&digest(b"old\n")),
                resulting_sha256: &digest(next),
                step_id: "manifest1",
            },
            next,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        souvenirs_storage::ManifestReplacementError::Journal(OperationJournalError::Failpoint(
            Failpoint::ManifestPublicationIntentPersisted
        ))
    ));
    assert_eq!(fs::read(manifest).unwrap(), b"old\n");
    assert_eq!(
        fs::read(
            layout
                .staging_directory()
                .join("8-manifest-manifest1.manifest")
        )
        .unwrap(),
        next
    );
}

#[test]
fn manifest_published_failpoint_leaves_new_manifest_and_record() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "manifest2");
    journal
        .create_destination_directory(&layout, &lock, "directory1", "2026")
        .unwrap();
    let next = b"next\n";
    journal.fail_at(Failpoint::ManifestPublished);

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
        souvenirs_storage::ManifestReplacementError::Journal(OperationJournalError::Failpoint(
            Failpoint::ManifestPublished
        ))
    ));
    assert_eq!(
        fs::read(layout.root().join("2026/SHA256SUMS")).unwrap(),
        next
    );
    assert!(fs::read_to_string(journal.path())
        .unwrap()
        .contains("\"kind\":\"manifest_replaced\""));
}
