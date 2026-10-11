//! Durable storage, filesystem, journal, lock, and index adapters.

mod capabilities;
mod catalog;
mod catalog_path;
mod copy;
mod durability;
mod lock;
mod manifest;
mod operation;
mod recovery;

pub use capabilities::{
    StorageCapabilities, StorageCapabilityError, VolumeIdentity, WriteSuitability,
};
pub use catalog::{CatalogConfig, CatalogInitializationError, CatalogLayout, FORMAT_VERSION};
pub use catalog_path::{
    validate_directory, CatalogPathError, OutputPathObservation, PortableDestinationObserver,
};
pub use copy::{is_source_unavailable_error, CopyExpectation, VerifiedCopy, VerifiedCopyError};
pub use lock::{CatalogLock, LockEnvironment, LockError, LockHolder, SystemLockEnvironment};
pub use manifest::{ManifestReplacement, ManifestReplacementError, ManifestReplacementRequest};
#[cfg(feature = "test-failpoints")]
pub use operation::Failpoint;
pub use operation::{
    is_operation_journal_file_name, read_operation_records, DurableTransition, OperationJournal,
    OperationJournalError, OperationOutcome, OperationRecord,
};
pub use recovery::{
    CancellableRecoveryOutcome, RecoveryCheckpoint, RecoveryCopy, RecoveryDirectory, RecoveryError,
    RecoveryManifest, RecoveryMode, RecoveryOutcome, RecoveryRequest,
};

#[cfg(test)]
mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    pub struct TempDirectory {
        path: PathBuf,
    }

    impl TempDirectory {
        pub fn new(label: &str) -> Self {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "souvenirs-storage-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary test directory should be created");
            Self { path }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).expect("temporary test directory should be removed");
        }
    }
}
