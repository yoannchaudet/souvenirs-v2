use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rustix::fs::{renameat_with, RenameFlags, CWD};
use rustix::io::Errno;
use sha2::{Digest as _, Sha256};

use crate::catalog_path::{resolve_catalog_entry, CatalogEntry, CatalogPathError};
use crate::durability::{sync_directory, sync_file};
use crate::operation::{validate_identifier, Failpoint};
use crate::{
    CatalogLayout, CatalogLock, DurableTransition, LockError, OperationJournal,
    OperationJournalError,
};

const COPY_BUFFER_SIZE: usize = 128 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedCopy {
    pub destination: PathBuf,
    pub staged_path: PathBuf,
    pub sha256: String,
    pub bytes_copied: u64,
}

impl CatalogLayout {
    pub fn copy_verified(
        &self,
        lock: &CatalogLock,
        journal: &mut OperationJournal,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        step_id: &str,
    ) -> Result<VerifiedCopy, VerifiedCopyError> {
        journal
            .ensure_active(self, lock)
            .map_err(VerifiedCopyError::Journal)?;
        validate_identifier("step", step_id).map_err(VerifiedCopyError::Journal)?;

        let source = source.as_ref();
        let relative_destination = destination.as_ref();
        let destination =
            resolve_catalog_entry(self, relative_destination).map_err(VerifiedCopyError::Path)?;
        let staged_relative_path = staging_relative_path(journal.operation_id(), step_id);
        let staged_path = self.root().join(&staged_relative_path);

        journal
            .record_transition(DurableTransition::CopyStarted {
                step_id: step_id.to_owned(),
                source: source.to_path_buf(),
                staged_path: staged_relative_path.clone(),
                destination: relative_destination.to_path_buf(),
            })
            .map_err(VerifiedCopyError::Journal)?;
        journal
            .check_failpoint(Failpoint::CopyIntentPersisted)
            .map_err(VerifiedCopyError::Journal)?;

        let (copy_digest, bytes_copied) = copy_to_stage(source, &staged_path)?;
        let read_back_digest = hash_staged_file(&staged_path)?;
        if copy_digest != read_back_digest {
            return Err(VerifiedCopyError::DigestMismatch {
                staged_path,
                copy_sha256: encode_digest(copy_digest),
                read_back_sha256: encode_digest(read_back_digest),
            });
        }

        let sha256 = encode_digest(copy_digest);
        journal
            .record_transition(DurableTransition::CopyVerified {
                step_id: step_id.to_owned(),
                staged_path: staged_relative_path.clone(),
                destination: relative_destination.to_path_buf(),
                sha256: sha256.clone(),
                bytes_copied,
            })
            .map_err(VerifiedCopyError::Journal)?;
        journal
            .check_failpoint(Failpoint::CopyStagePersisted)
            .map_err(VerifiedCopyError::Journal)?;
        journal
            .record_transition(DurableTransition::CopyPublicationStarted {
                step_id: step_id.to_owned(),
                staged_path: staged_relative_path,
                destination: relative_destination.to_path_buf(),
                sha256: sha256.clone(),
            })
            .map_err(VerifiedCopyError::Journal)?;
        journal
            .check_failpoint(Failpoint::CopyPublicationIntentPersisted)
            .map_err(VerifiedCopyError::Journal)?;
        publish_exclusively(&staged_path, &destination).map_err(|error| match error {
            PublishError::Occupied => VerifiedCopyError::DestinationOccupied {
                destination: destination.path.clone(),
                staged_path: staged_path.clone(),
                sha256: sha256.clone(),
            },
            PublishError::Io(source) => VerifiedCopyError::Io {
                action: "publish verified file exclusively",
                path: destination.path.clone(),
                source,
            },
        })?;
        sync_directory(&self.staging_directory()).map_err(|source| VerifiedCopyError::Io {
            action: "persist staging directory after publication",
            path: self.staging_directory(),
            source,
        })?;
        sync_file(&destination.directory).map_err(|source| VerifiedCopyError::Io {
            action: "persist destination directory",
            path: destination.directory_path.clone(),
            source,
        })?;
        journal
            .record_transition(DurableTransition::CopyPublished {
                step_id: step_id.to_owned(),
                destination: relative_destination.to_path_buf(),
                sha256: sha256.clone(),
            })
            .map_err(VerifiedCopyError::Journal)?;
        journal
            .check_failpoint(Failpoint::CopyPublished)
            .map_err(VerifiedCopyError::Journal)?;

        Ok(VerifiedCopy {
            destination: destination.path,
            staged_path,
            sha256,
            bytes_copied,
        })
    }
}

fn staging_relative_path(operation_id: &str, step_id: &str) -> PathBuf {
    Path::new(".souvenirs").join("staging").join(format!(
        "{}-{operation_id}-{step_id}.copy",
        operation_id.len()
    ))
}

fn copy_to_stage(source: &Path, staged_path: &Path) -> Result<([u8; 32], u64), VerifiedCopyError> {
    let mut source_file = File::open(source).map_err(|source_error| VerifiedCopyError::Io {
        action: "open copy source",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let metadata = source_file
        .metadata()
        .map_err(|source_error| VerifiedCopyError::Io {
            action: "inspect copy source",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if !metadata.is_file() {
        return Err(VerifiedCopyError::SourceIsNotFile(source.to_path_buf()));
    }

    let mut staged_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged_path)
        .map_err(|source_error| {
            if source_error.kind() == io::ErrorKind::AlreadyExists {
                VerifiedCopyError::StageOccupied(staged_path.to_path_buf())
            } else {
                VerifiedCopyError::Io {
                    action: "create staged file exclusively",
                    path: staged_path.to_path_buf(),
                    source: source_error,
                }
            }
        })?;
    disable_cache(&staged_file).map_err(|source_error| VerifiedCopyError::Io {
        action: "disable cache for staged writes",
        path: staged_path.to_path_buf(),
        source: source_error,
    })?;
    let staging_directory = staged_path
        .parent()
        .expect("staged path has a parent directory");
    sync_directory(staging_directory).map_err(|source_error| VerifiedCopyError::Io {
        action: "persist new staging entry",
        path: staging_directory.to_path_buf(),
        source: source_error,
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut bytes_copied = 0_u64;
    loop {
        let count =
            source_file
                .read(&mut buffer)
                .map_err(|source_error| VerifiedCopyError::Io {
                    action: "read copy source",
                    path: source.to_path_buf(),
                    source: source_error,
                })?;
        if count == 0 {
            break;
        }
        staged_file
            .write_all(&buffer[..count])
            .map_err(|source_error| VerifiedCopyError::Io {
                action: "write staged file",
                path: staged_path.to_path_buf(),
                source: source_error,
            })?;
        hasher.update(&buffer[..count]);
        bytes_copied += count as u64;
    }
    sync_file(&staged_file).map_err(|source_error| VerifiedCopyError::Io {
        action: "persist staged file",
        path: staged_path.to_path_buf(),
        source: source_error,
    })?;

    Ok((hasher.finalize().into(), bytes_copied))
}

fn hash_staged_file(staged_path: &Path) -> Result<[u8; 32], VerifiedCopyError> {
    let mut staged_file =
        File::open(staged_path).map_err(|source_error| VerifiedCopyError::Io {
            action: "open staged file for read-back verification",
            path: staged_path.to_path_buf(),
            source: source_error,
        })?;
    disable_cache(&staged_file).map_err(|source_error| VerifiedCopyError::Io {
        action: "disable cache for read-back verification",
        path: staged_path.to_path_buf(),
        source: source_error,
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let count =
            staged_file
                .read(&mut buffer)
                .map_err(|source_error| VerifiedCopyError::Io {
                    action: "read staged file for verification",
                    path: staged_path.to_path_buf(),
                    source: source_error,
                })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(target_vendor = "apple")]
fn disable_cache(file: &File) -> io::Result<()> {
    rustix::fs::fcntl_nocache(file, true).map_err(io::Error::from)
}

#[cfg(not(target_vendor = "apple"))]
fn disable_cache(_file: &File) -> io::Result<()> {
    Ok(())
}

pub(crate) fn publish_exclusively(
    staged_path: &Path,
    destination: &CatalogEntry,
) -> Result<(), PublishError> {
    match renameat_with(
        CWD,
        staged_path,
        &destination.directory,
        &destination.file_name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(PublishError::Occupied),
        Err(Errno::NOTSUP | Errno::INVAL | Errno::NOSYS) => {
            publish_with_locked_fallback(staged_path, &destination.resolved_path)
        }
        Err(source) => Err(PublishError::Io(io::Error::from(source))),
    }
}

fn publish_with_locked_fallback(
    staged_path: &Path,
    destination: &Path,
) -> Result<(), PublishError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(PublishError::Occupied),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(PublishError::Io(source)),
    }

    fs::rename(staged_path, destination).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            PublishError::Occupied
        } else {
            PublishError::Io(source)
        }
    })
}

pub(crate) enum PublishError {
    Occupied,
    Io(io::Error),
}

fn encode_digest(digest: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

#[derive(Debug)]
pub enum VerifiedCopyError {
    DestinationOccupied {
        destination: PathBuf,
        staged_path: PathBuf,
        sha256: String,
    },
    DigestMismatch {
        staged_path: PathBuf,
        copy_sha256: String,
        read_back_sha256: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Journal(OperationJournalError),
    Lock(LockError),
    Path(CatalogPathError),
    SourceIsNotFile(PathBuf),
    StageOccupied(PathBuf),
}

impl fmt::Display for VerifiedCopyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DestinationOccupied {
                destination,
                staged_path,
                sha256,
            } => write!(
                formatter,
                "verified copy refuses to replace occupied destination {}; verified stage {} has SHA-256 {sha256}",
                destination.display(),
                staged_path.display()
            ),
            Self::DigestMismatch {
                staged_path,
                copy_sha256,
                read_back_sha256,
            } => write!(
                formatter,
                "staged file {} changed between copy and read-back hashing: copied {copy_sha256}, read back {read_back_sha256}",
                staged_path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Journal(source) => source.fmt(formatter),
            Self::Lock(source) => write!(formatter, "failed to validate catalog lock: {source}"),
            Self::Path(source) => write!(formatter, "invalid copy destination: {source}"),
            Self::SourceIsNotFile(path) => {
                write!(formatter, "copy source is not a regular file: {}", path.display())
            }
            Self::StageOccupied(path) => write!(
                formatter,
                "verified copy refuses to replace occupied staging path {}",
                path.display()
            ),
        }
    }
}

impl Error for VerifiedCopyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Journal(source) => Some(source),
            Self::Lock(source) => Some(source),
            Self::Path(source) => Some(source),
            Self::DestinationOccupied { .. }
            | Self::DigestMismatch { .. }
            | Self::SourceIsNotFile(_)
            | Self::StageOccupied(_) => None,
        }
    }
}
