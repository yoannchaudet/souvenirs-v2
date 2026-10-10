use std::fs;

use sha2::{Digest as _, Sha256};
use souvenirs_storage::{CatalogConfig, CatalogLayout, CatalogLock, VerifiedCopyError};
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

#[test]
fn publishes_only_after_copy_and_read_back_hashes_match() {
    let (_temporary, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"verified bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let copied = layout
        .copy_verified(&lock, &source, "2026/original.bin", "import1", "original1")
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
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    let bytes: Vec<_> = (0..300_000).map(|index| (index % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let copied = layout
        .copy_verified(&lock, &source, "2026/large.bin", "import1", "large")
        .unwrap();

    assert_eq!(copied.bytes_copied, bytes.len() as u64);
    assert_eq!(copied.sha256, format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(fs::read(copied.destination).unwrap(), bytes);
}

#[test]
fn never_overwrites_an_occupied_destination() {
    let (_temporary, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"new bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();
    let destination = layout.root().join("2026/original.bin");
    fs::write(&destination, b"existing bytes").unwrap();

    let error = layout
        .copy_verified(&lock, &source, "2026/original.bin", "import2", "original1")
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
        .copy_verified(&lock, &source, "2026/original.bin", "import3", "original1")
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
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();
    fs::create_dir(layout.root().join("2026")).unwrap();

    let error = layout
        .copy_verified(
            &other_lock,
            &source,
            "2026/original.bin",
            "import4",
            "original1",
        )
        .unwrap_err();

    assert!(matches!(error, VerifiedCopyError::WrongCatalogLock));
    assert!(!layout.root().join("2026/original.bin").exists());
    drop(other_layout);
}

#[test]
fn rejects_destination_traversal_before_staging_bytes() {
    let (_temporary, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();

    let error = layout
        .copy_verified(&lock, &source, "../outside.bin", "import5", "original1")
        .unwrap_err();

    assert!(matches!(error, VerifiedCopyError::InvalidDestination(_)));
    assert!(!staged_path(&layout, "import5", "original1").exists());
}

#[test]
fn rejects_invalid_identifiers_before_staging_bytes() {
    let (_temporary, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();

    let error = layout
        .copy_verified(&lock, &source, "original.bin", "../import", "original1")
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::InvalidIdentifier {
            kind: "operation",
            ..
        }
    ));
    assert!(fs::read_dir(layout.staging_directory())
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn rejects_catalog_state_destinations() {
    let (_temporary, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &source,
            ".souvenirs/journal/injected",
            "import6",
            "original1",
        )
        .unwrap_err();

    assert!(matches!(error, VerifiedCopyError::ReservedDestination(_)));
}

#[cfg(unix)]
#[test]
fn rejects_a_destination_directory_symlink_outside_the_catalog() {
    use std::os::unix::fs::symlink;

    let (_temporary, layout, lock) = initialized_catalog();
    let source_directory = TempDir::new().unwrap();
    let source = source_directory.path().join("source.bin");
    fs::write(&source, b"bytes").unwrap();
    let outside = TempDir::new().unwrap();
    symlink(outside.path(), layout.root().join("escaped")).unwrap();

    let error = layout
        .copy_verified(
            &lock,
            &source,
            "escaped/original.bin",
            "import6",
            "original1",
        )
        .unwrap_err();

    assert!(matches!(
        error,
        VerifiedCopyError::DestinationEscapesCatalog(_)
    ));
    assert!(!outside.path().join("original.bin").exists());
    assert!(!staged_path(&layout, "import6", "original1").exists());
}
