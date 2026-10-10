use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::catalog_path::{resolve_catalog_entry, CatalogPathError};
use crate::durability::{sync_directory, sync_file};
use crate::operation::{validate_identifier, Failpoint};
use crate::{
    CatalogLayout, CatalogLock, DurableTransition, OperationJournal, OperationJournalError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestReplacement {
    pub path: PathBuf,
    pub staged_path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ManifestReplacementRequest<'a> {
    pub path: &'a Path,
    pub previous_sha256: Option<&'a str>,
    pub resulting_sha256: &'a str,
    pub step_id: &'a str,
}

impl CatalogLayout {
    pub fn replace_manifest(
        &self,
        lock: &CatalogLock,
        journal: &mut OperationJournal,
        request: ManifestReplacementRequest<'_>,
        contents: &[u8],
    ) -> Result<ManifestReplacement, ManifestReplacementError> {
        journal
            .ensure_active(self, lock)
            .map_err(ManifestReplacementError::Journal)?;
        validate_identifier("step", request.step_id).map_err(ManifestReplacementError::Journal)?;
        validate_digest(request.resulting_sha256)?;
        if let Some(previous) = request.previous_sha256 {
            validate_digest(previous)?;
        }
        let relative_path = request.path;
        if relative_path.file_name() != Some(std::ffi::OsStr::new("SHA256SUMS")) {
            return Err(ManifestReplacementError::InvalidManifestPath(
                relative_path.to_path_buf(),
            ));
        }
        let manifest =
            resolve_catalog_entry(self, relative_path).map_err(ManifestReplacementError::Path)?;
        verify_resulting_digest(contents, request.resulting_sha256)?;
        verify_previous_manifest(&manifest.resolved_path, request.previous_sha256)?;

        let staged_relative_path = Path::new(".souvenirs").join("staging").join(format!(
            "{}-{}-{}.manifest",
            journal.operation_id().len(),
            journal.operation_id(),
            request.step_id
        ));
        let staged_path = self.root().join(&staged_relative_path);
        journal
            .record_transition(DurableTransition::ManifestReplacementStarted {
                step_id: request.step_id.to_owned(),
                staged_path: staged_relative_path.clone(),
                manifest_path: relative_path.to_path_buf(),
                previous_sha256: request.previous_sha256.map(str::to_owned),
                resulting_sha256: request.resulting_sha256.to_owned(),
            })
            .map_err(ManifestReplacementError::Journal)?;
        journal
            .check_failpoint(Failpoint::ManifestIntentPersisted)
            .map_err(ManifestReplacementError::Journal)?;

        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    ManifestReplacementError::StageOccupied(staged_path.clone())
                } else {
                    ManifestReplacementError::Io {
                        action: "create staged manifest exclusively",
                        path: staged_path.clone(),
                        source,
                    }
                }
            })?;
        sync_directory(&self.staging_directory()).map_err(|source| {
            ManifestReplacementError::Io {
                action: "persist new staged manifest entry",
                path: self.staging_directory(),
                source,
            }
        })?;
        staged
            .write_all(contents)
            .and_then(|()| sync_file(&staged))
            .map_err(|source| ManifestReplacementError::Io {
                action: "write and persist staged manifest",
                path: staged_path.clone(),
                source,
            })?;
        journal
            .record_transition(DurableTransition::ManifestStaged {
                step_id: request.step_id.to_owned(),
                staged_path: staged_relative_path.clone(),
                manifest_path: relative_path.to_path_buf(),
                resulting_sha256: request.resulting_sha256.to_owned(),
            })
            .map_err(ManifestReplacementError::Journal)?;
        journal
            .check_failpoint(Failpoint::ManifestStagePersisted)
            .map_err(ManifestReplacementError::Journal)?;

        verify_previous_manifest(&manifest.resolved_path, request.previous_sha256)?;
        journal
            .record_transition(DurableTransition::ManifestPublicationStarted {
                step_id: request.step_id.to_owned(),
                staged_path: staged_relative_path,
                manifest_path: relative_path.to_path_buf(),
                resulting_sha256: request.resulting_sha256.to_owned(),
            })
            .map_err(ManifestReplacementError::Journal)?;
        journal
            .check_failpoint(Failpoint::ManifestPublicationIntentPersisted)
            .map_err(ManifestReplacementError::Journal)?;
        fs::rename(&staged_path, &manifest.resolved_path).map_err(|source| {
            ManifestReplacementError::Io {
                action: "replace manifest atomically",
                path: manifest.path.clone(),
                source,
            }
        })?;
        sync_directory(&self.staging_directory()).map_err(|source| {
            ManifestReplacementError::Io {
                action: "persist staging directory after manifest replacement",
                path: self.staging_directory(),
                source,
            }
        })?;
        sync_file(&manifest.directory).map_err(|source| ManifestReplacementError::Io {
            action: "persist manifest directory",
            path: manifest.directory_path,
            source,
        })?;
        journal
            .record_transition(DurableTransition::ManifestReplaced {
                step_id: request.step_id.to_owned(),
                manifest_path: relative_path.to_path_buf(),
                resulting_sha256: request.resulting_sha256.to_owned(),
            })
            .map_err(ManifestReplacementError::Journal)?;
        journal
            .check_failpoint(Failpoint::ManifestPublished)
            .map_err(ManifestReplacementError::Journal)?;

        Ok(ManifestReplacement {
            path: manifest.path,
            staged_path,
            sha256: request.resulting_sha256.to_owned(),
        })
    }
}

fn verify_resulting_digest(
    contents: &[u8],
    expected: &str,
) -> Result<(), ManifestReplacementError> {
    let actual = format!("{:x}", Sha256::digest(contents));
    if actual == expected {
        Ok(())
    } else {
        Err(ManifestReplacementError::ResultingDigestMismatch {
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn verify_previous_manifest(
    path: &Path,
    expected: Option<&str>,
) -> Result<(), ManifestReplacementError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(ManifestReplacementError::Io {
                action: "inspect previous manifest",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    match (metadata, expected) {
        (None, None) => Ok(()),
        (None, Some(expected)) => Err(ManifestReplacementError::PreviousManifestChanged {
            expected: Some(expected.to_owned()),
            actual: None,
        }),
        (Some(_), None) => Err(ManifestReplacementError::PreviousManifestChanged {
            expected: None,
            actual: Some(digest_file(path)?),
        }),
        (Some(metadata), Some(expected)) if metadata.file_type().is_file() => {
            let actual = digest_file(path)?;
            if actual == expected {
                Ok(())
            } else {
                Err(ManifestReplacementError::PreviousManifestChanged {
                    expected: Some(expected.to_owned()),
                    actual: Some(actual),
                })
            }
        }
        (Some(_), Some(expected)) => Err(ManifestReplacementError::PreviousManifestChanged {
            expected: Some(expected.to_owned()),
            actual: None,
        }),
    }
}

fn digest_file(path: &Path) -> Result<String, ManifestReplacementError> {
    let bytes = fs::read(path).map_err(|source| ManifestReplacementError::Io {
        action: "read previous manifest",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_digest(digest: &str) -> Result<(), ManifestReplacementError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ManifestReplacementError::InvalidDigest(digest.to_owned()))
    }
}

#[derive(Debug)]
pub enum ManifestReplacementError {
    InvalidDigest(String),
    InvalidManifestPath(PathBuf),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Journal(OperationJournalError),
    Path(CatalogPathError),
    PreviousManifestChanged {
        expected: Option<String>,
        actual: Option<String>,
    },
    ResultingDigestMismatch {
        expected: String,
        actual: String,
    },
    StageOccupied(PathBuf),
}

impl fmt::Display for ManifestReplacementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDigest(digest) => write!(formatter, "invalid SHA-256 digest: {digest:?}"),
            Self::InvalidManifestPath(path) => write!(
                formatter,
                "manifest path must end in SHA256SUMS: {}",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Journal(source) => source.fmt(formatter),
            Self::Path(error) => write!(formatter, "invalid manifest destination: {error}"),
            Self::PreviousManifestChanged { expected, actual } => write!(
                formatter,
                "manifest baseline changed: expected {expected:?}, found {actual:?}"
            ),
            Self::ResultingDigestMismatch { expected, actual } => write!(
                formatter,
                "resulting manifest digest mismatch: expected {expected}, computed {actual}"
            ),
            Self::StageOccupied(path) => write!(
                formatter,
                "manifest replacement refuses to overwrite staged path {}",
                path.display()
            ),
        }
    }
}

impl Error for ManifestReplacementError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Journal(source) => Some(source),
            Self::Path(source) => Some(source),
            Self::InvalidDigest(_)
            | Self::InvalidManifestPath(_)
            | Self::PreviousManifestChanged { .. }
            | Self::ResultingDigestMismatch { .. }
            | Self::StageOccupied(_) => None,
        }
    }
}
