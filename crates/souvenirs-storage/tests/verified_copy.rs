use std::fs;

use sha2::{Digest as _, Sha256};
use souvenirs_storage::{
    CatalogConfig, CatalogLayout, CatalogLock, CatalogPathError, CopyExpectation, OperationJournal,
    OperationJournalError, VerifiedCopyError,
};
use tempfile::TempDir;

fn initialized_catalog() -> (TempDir, CatalogLayout, CatalogLock) {
    let temporary = TempDir::new().unwrap();
    let layout = CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap();
    let lock = CatalogLock::acquire(&layout).unwrap();
    (temporary, layout, lock)
}

fn staged_path(layout: &CatalogLayout, operation_id: &str, step_id: &str) -> std::path::PathBuf {
    layout.staging_directory().join(format!(
        "{}-{operation_id}-{step_id}.copy",
        operation_id.len()
    ))
}

fn journal(layout: &CatalogLayout, lock: &CatalogLock, operation_id: &str) -> OperationJournal {
    OperationJournal::create(
        layout,
        lock,
        operation_id,
        &serde_json::json!({"plan": operation_id}),
    )
    .unwrap()
}

fn expectation(bytes: &[u8]) -> CopyExpectation {
    CopyExpectation {
        sha256: format!("{:x}", Sha256::digest(bytes)),
        size_bytes: bytes.len() as u64,
    }
}

#[test]
fn publishes_only_after_copy_and_read_back_hashes_match() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import1");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"verified bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let copied = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "original1",
            expectation(b"verified bytes"),
        )
        .unwrap();

    assert_eq!(
        copied.sha256,
        "186287b2d987891f027b4bc8baaf621a3e5a4a73ec78e04b0f65dc309b1ccc03"
    );
    assert_eq!(copied.bytes_copied, 14);
    assert_eq!(fs::read(&copied.destination).unwrap(), b"verified bytes");
    assert!(!copied.staged_path.exists());
}

#[test]
fn copies_and_verifies_multiple_buffers() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import1");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    let bytes: Vec<_> = (0..300_000).map(|index| (index % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let copied = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/large.bin",
            "large",
            expectation(&bytes),
        )
        .unwrap();

    assert_eq!(copied.bytes_copied, bytes.len() as u64);
    assert_eq!(copied.sha256, format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(fs::read(copied.destination).unwrap(), bytes);
}

#[test]
fn planned_content_mismatch_remains_staged_and_is_never_published() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "drift");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"changed bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "original1",
            expectation(b"planned bytes"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::PlannedContentMismatch { .. }
    ));
    assert!(!layout.root().join("2026/original.bin").exists());
    assert_eq!(
        fs::read(staged_path(&layout, "drift", "original1")).unwrap(),
        b"changed bytes"
    );
    let journal_text = fs::read_to_string(journal.path()).unwrap();
    assert!(journal_text.contains("\"kind\":\"copy_started\""));
    assert!(!journal_text.contains("\"kind\":\"copy_verified\""));
    assert!(!journal_text.contains("\"kind\":\"copy_publication_started\""));
}

#[test]
fn never_overwrites_an_occupied_destination() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import2");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"new bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();
    let destination = layout.root().join("2026/original.bin");
    fs::write(&destination, b"existing bytes").unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "original1",
            expectation(b"new bytes"),
        )
        .unwrap_err();

    match error {
        VerifiedCopyError::DestinationOccupied {
            destination: occupied,
            staged_path: staged,
            sha256,
        } => {
            assert_eq!(occupied, destination);
            assert_eq!(staged, staged_path(&layout, "import2", "original1"));
            assert_eq!(sha256, format!("{:x}", Sha256::digest(b"new bytes")));
        }
        error => panic!("unexpected copy error: {error}"),
    }
    assert_eq!(fs::read(destination).unwrap(), b"existing bytes");
    assert_eq!(
        fs::read(staged_path(&layout, "import2", "original1")).unwrap(),
        b"new bytes"
    );
}

#[test]
fn never_overwrites_an_occupied_staging_path() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import3");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"new bytes").unwrap();
    fs::write(
        staged_path(&layout, "import3", "original1"),
        b"recovery bytes",
    )
    .unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "original1",
            expectation(b"new bytes"),
        )
        .unwrap_err();

    assert!(matches!(error, VerifiedCopyError::StageOccupied(_)));
    assert_eq!(
        fs::read(staged_path(&layout, "import3", "original1")).unwrap(),
        b"recovery bytes"
    );
    assert!(!layout.root().join("2026/original.bin").exists());
}

#[test]
fn requires_the_active_lock_for_the_same_catalog() {
    let (_temporary, layout, _lock) = initialized_catalog();
    let (_other_temporary, other_layout, other_lock) = initialized_catalog();
    let mut journal = journal(&other_layout, &other_lock, "import4");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let error = layout
        .copy_verified(
            &other_lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "original1",
            expectation(b"bytes"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Journal(OperationJournalError::WrongCatalogLock)
    ));
    assert!(!layout.root().join("2026/original.bin").exists());
    drop(other_layout);
}

#[test]
fn rejects_destination_traversal_before_staging_bytes() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import5");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "../outside.bin",
            "original1",
            expectation(b"bytes"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Path(CatalogPathError::Invalid(_))
    ));
    assert!(!staged_path(&layout, "import5", "original1").exists());
}

#[test]
fn rejects_invalid_identifiers_before_staging_bytes() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import5");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "original.bin",
            "../step",
            expectation(b"bytes"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Journal(OperationJournalError::InvalidIdentifier { kind: "step", .. })
    ));
    assert!(fs::read_dir(layout.staging_directory())
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn rejects_catalog_state_destinations() {
    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import6");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            ".souvenirs/journal/injected",
            "original1",
            expectation(b"bytes"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Path(CatalogPathError::Reserved(_))
    ));
}

#[cfg(unix)]
#[test]
fn rejects_a_destination_directory_symlink_outside_the_catalog() {
    use std::os::unix::fs::symlink;

    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "import6");
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();
    let outside = TempDir::new().unwrap();
    symlink(outside.path(), layout.root().join("escaped")).unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "escaped/original.bin",
            "original1",
            expectation(b"bytes"),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Path(CatalogPathError::EscapesCatalog(_))
    ));
    assert!(!outside.path().join("original.bin").exists());
    assert!(!staged_path(&layout, "import6", "original1").exists());
}

#[cfg(unix)]
#[test]
fn serialization_failure_does_not_append_a_partial_journal_record() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let (_temporary, layout, lock) = initialized_catalog();
    let mut journal = journal(&layout, &lock, "nonutf8");
    fs::create_dir(layout.root().join("2026")).unwrap();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory
        .path()
        .join(OsString::from_vec(vec![b's', b'o', 0xff]));
    let before = fs::read(journal.path()).unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &mut journal,
            &source,
            "2026/original.bin",
            "copy1",
            expectation(b""),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::Journal(OperationJournalError::SerializeRecord { .. })
    ));
    assert_eq!(fs::read(journal.path()).unwrap(), before);
    assert!(fs::read_dir(layout.staging_directory())
        .unwrap()
        .next()
        .is_none());
}
