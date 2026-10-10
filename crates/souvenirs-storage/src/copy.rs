use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{renameat_with, RenameFlags, CWD};
use rustix::io::Errno;
use sha2::{Digest as _, Sha256};

use crate::durability::{sync_directory, sync_file};
use crate::{CatalogLayout, CatalogLock, LockError};

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
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        operation_id: &str,
        step_id: &str,
    ) -> Result<VerifiedCopy, VerifiedCopyError> {
        if !lock.guards(self).map_err(VerifiedCopyError::Lock)? {
            return Err(VerifiedCopyError::WrongCatalogLock);
        }
        validate_identifier("operation", operation_id)?;
        validate_identifier("step", step_id)?;

        let source = source.as_ref();
        let destination = catalog_destination(self, destination.as_ref())?;
        let staged_path = staging_path(self, operation_id, step_id);

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

        Ok(VerifiedCopy {
            destination: destination.path,
            staged_path,
            sha256,
            bytes_copied,
        })
    }
}

fn validate_identifier(kind: &'static str, identifier: &str) -> Result<(), VerifiedCopyError> {
    if identifier.is_empty()
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(VerifiedCopyError::InvalidIdentifier {
            kind,
            value: identifier.to_owned(),
        });
    }
    Ok(())
}

fn staging_path(layout: &CatalogLayout, operation_id: &str, step_id: &str) -> PathBuf {
    layout.staging_directory().join(format!(
        "{}-{operation_id}-{step_id}.copy",
        operation_id.len()
    ))
}

fn catalog_destination(
    layout: &CatalogLayout,
    relative_path: &Path,
) -> Result<CatalogDestination, VerifiedCopyError> {
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(VerifiedCopyError::InvalidDestination(
            relative_path.to_path_buf(),
        ));
    }

    let first_component = relative_path
        .components()
        .next()
        .expect("validated destination has a component")
        .as_os_str();
    if first_component == ".souvenirs" || relative_path == Path::new(".souvenirs.yml") {
        return Err(VerifiedCopyError::ReservedDestination(
            relative_path.to_path_buf(),
        ));
    }

    let destination = layout.root().join(relative_path);
    let parent = destination
        .parent()
        .expect("non-empty relative destination has a parent");
    let canonical_root =
        fs::canonicalize(layout.root()).map_err(|source| VerifiedCopyError::Io {
            action: "resolve catalog root",
            path: layout.root().to_path_buf(),
            source,
        })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|source| VerifiedCopyError::Io {
        action: "resolve destination directory",
        path: parent.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(VerifiedCopyError::DestinationEscapesCatalog(
            relative_path.to_path_buf(),
        ));
    }

    let directory = File::open(&canonical_parent).map_err(|source| VerifiedCopyError::Io {
        action: "open destination directory",
        path: canonical_parent.clone(),
        source,
    })?;
    let file_name = destination
        .file_name()
        .expect("validated destination has a file name")
        .to_os_string();
    Ok(CatalogDestination {
        path: destination,
        resolved_path: canonical_parent.join(&file_name),
        directory_path: canonical_parent,
        directory,
        file_name,
    })
}

struct CatalogDestination {
    path: PathBuf,
    resolved_path: PathBuf,
    directory_path: PathBuf,
    directory: File,
    file_name: OsString,
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

fn publish_exclusively(
    staged_path: &Path,
    destination: &CatalogDestination,
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

enum PublishError {
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
    DestinationEscapesCatalog(PathBuf),
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
    InvalidDestination(PathBuf),
    InvalidIdentifier {
        kind: &'static str,
        value: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Lock(LockError),
    ReservedDestination(PathBuf),
    SourceIsNotFile(PathBuf),
    StageOccupied(PathBuf),
    WrongCatalogLock,
}

impl fmt::Display for VerifiedCopyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DestinationEscapesCatalog(path) => write!(
                formatter,
                "copy destination escapes the catalog through a symbolic link: {}",
                path.display()
            ),
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
            Self::InvalidDestination(path) => write!(
                formatter,
                "copy destination must be a non-empty catalog-relative path without traversal: {}",
                path.display()
            ),
            Self::InvalidIdentifier { kind, value } => write!(
                formatter,
                "{kind} identifier must contain only ASCII letters, digits, hyphens, or underscores: {value:?}"
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Lock(source) => write!(formatter, "failed to validate catalog lock: {source}"),
            Self::ReservedDestination(path) => write!(
                formatter,
                "copy destination is reserved for catalog state: {}",
                path.display()
            ),
            Self::SourceIsNotFile(path) => {
                write!(formatter, "copy source is not a regular file: {}", path.display())
            }
            Self::StageOccupied(path) => write!(
                formatter,
                "verified copy refuses to replace occupied staging path {}",
                path.display()
            ),
            Self::WrongCatalogLock => {
                formatter.write_str("verified copy requires the active lock for this catalog")
            }
        }
    }
}

impl Error for VerifiedCopyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Lock(source) => Some(source),
            Self::DestinationEscapesCatalog(_)
            | Self::DestinationOccupied { .. }
            | Self::DigestMismatch { .. }
            | Self::InvalidDestination(_)
            | Self::InvalidIdentifier { .. }
            | Self::ReservedDestination(_)
            | Self::SourceIsNotFile(_)
            | Self::StageOccupied(_)
            | Self::WrongCatalogLock => None,
        }
    }
}
