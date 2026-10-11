use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::durability::{sync_directory, sync_file};

static WRITE_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VolumeIdentity(u64);

impl VolumeIdentity {
    pub fn for_path(path: impl AsRef<Path>) -> Result<Self, StorageCapabilityError> {
        let path = path.as_ref();
        let metadata = fs::metadata(path).map_err(|source| StorageCapabilityError::Inspect {
            path: path.to_path_buf(),
            source,
        })?;

        #[cfg(unix)]
        {
            Ok(Self(metadata.dev()))
        }

        #[cfg(not(unix))]
        {
            let _ = metadata;
            Err(StorageCapabilityError::UnsupportedPlatform)
        }
    }

    pub fn same_volume(self, other: Self) -> bool {
        self == other
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteSuitability {
    Writable,
    SynchronizedFolder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageCapabilities {
    pub volume: VolumeIdentity,
    pub write_suitability: WriteSuitability,
}

impl StorageCapabilities {
    pub fn detect(path: impl AsRef<Path>) -> Result<Self, StorageCapabilityError> {
        let path = path.as_ref();
        let canonical_path =
            fs::canonicalize(path).map_err(|source| StorageCapabilityError::Inspect {
                path: path.to_path_buf(),
                source,
            })?;

        Ok(Self {
            volume: VolumeIdentity::for_path(&canonical_path)?,
            write_suitability: if is_synchronized_folder(&canonical_path) {
                WriteSuitability::SynchronizedFolder
            } else {
                WriteSuitability::Writable
            },
        })
    }

    pub fn ensure_writable(&self, path: impl AsRef<Path>) -> Result<(), StorageCapabilityError> {
        let path = path.as_ref();
        if self.write_suitability == WriteSuitability::SynchronizedFolder {
            return Err(StorageCapabilityError::SynchronizedFolder {
                path: path.to_path_buf(),
            });
        }
        probe_write_capability(path)
    }

    pub fn available_space_bytes(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<u64, StorageCapabilityError> {
        let path = path.as_ref();
        if !self.volume.same_volume(VolumeIdentity::for_path(path)?) {
            return Err(StorageCapabilityError::Inspect {
                path: path.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "available space must be observed on the detected volume",
                ),
            });
        }
        let statistics =
            rustix::fs::statvfs(path).map_err(|source| StorageCapabilityError::Inspect {
                path: path.to_path_buf(),
                source: source.into(),
            })?;
        statistics
            .f_bavail
            .checked_mul(statistics.f_frsize)
            .ok_or_else(|| StorageCapabilityError::Inspect {
                path: path.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "available filesystem space exceeds the supported range",
                ),
            })
    }
}

fn probe_write_capability(path: &Path) -> Result<(), StorageCapabilityError> {
    let probe_directory = create_probe_directory(path)?;
    let pending_file = probe_directory.join("pending");
    let published_file = probe_directory.join("published");

    let probe_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending_file)
            .map_err(|source| write_probe_error(path, "create a probe file", source))?;
        file.write_all(b"souvenirs write capability probe\n")
            .and_then(|()| sync_file(&file))
            .map_err(|source| write_probe_error(path, "write and persist a probe file", source))?;
        sync_directory(&probe_directory)
            .map_err(|source| write_probe_error(path, "persist the probe file entry", source))?;
        fs::rename(&pending_file, &published_file)
            .map_err(|source| write_probe_error(path, "rename a probe file", source))?;
        sync_directory(&probe_directory)
            .map_err(|source| write_probe_error(path, "persist the probe rename", source))
    })();

    let cleanup_result = cleanup_probe(path, &probe_directory, &pending_file, &published_file);
    cleanup_result.and(probe_result)
}

fn create_probe_directory(path: &Path) -> Result<PathBuf, StorageCapabilityError> {
    for _ in 0..16 {
        let sequence = WRITE_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let probe_directory = path.join(format!(
            ".souvenirs-write-probe-{}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&probe_directory) {
            Ok(()) => {
                if let Err(source) = sync_directory(path) {
                    let probe_error =
                        write_probe_error(path, "persist the probe directory", source);
                    let cleanup_result = cleanup_probe(
                        path,
                        &probe_directory,
                        &probe_directory.join("pending"),
                        &probe_directory.join("published"),
                    );
                    return cleanup_result.and(Err(probe_error));
                }
                return Ok(probe_directory);
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(write_probe_error(path, "create a probe directory", source));
            }
        }
    }

    Err(write_probe_error(
        path,
        "create a uniquely named probe directory",
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "write probe names repeatedly collided",
        ),
    ))
}

fn cleanup_probe(
    root: &Path,
    probe_directory: &Path,
    pending_file: &Path,
    published_file: &Path,
) -> Result<(), StorageCapabilityError> {
    remove_probe_file(root, pending_file)?;
    remove_probe_file(root, published_file)?;
    fs::remove_dir(probe_directory)
        .map_err(|source| write_probe_error(root, "remove the probe directory", source))?;
    sync_directory(root).map_err(|source| write_probe_error(root, "persist probe cleanup", source))
}

fn remove_probe_file(root: &Path, path: &Path) -> Result<(), StorageCapabilityError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(write_probe_error(root, "remove a probe file", source)),
    }
}

fn write_probe_error(
    path: &Path,
    action: &'static str,
    source: io::Error,
) -> StorageCapabilityError {
    StorageCapabilityError::WriteProbe {
        path: path.to_path_buf(),
        action,
        source,
    }
}

fn is_synchronized_folder(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();

    components
        .windows(2)
        .any(|window| window == ["Library", "CloudStorage"])
        || components
            .windows(2)
            .any(|window| window == ["Library", "Mobile Documents"])
        || components.iter().any(|component| {
            component.eq_ignore_ascii_case("dropbox") || component.eq_ignore_ascii_case(".dropbox")
        })
}

#[derive(Debug)]
pub enum StorageCapabilityError {
    Inspect {
        path: PathBuf,
        source: io::Error,
    },
    SynchronizedFolder {
        path: PathBuf,
    },
    WriteProbe {
        path: PathBuf,
        action: &'static str,
        source: io::Error,
    },
    UnsupportedPlatform,
}

impl fmt::Display for StorageCapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inspect { path, source } => {
                write!(
                    formatter,
                    "failed to inspect storage at {}: {source}",
                    path.display()
                )
            }
            Self::SynchronizedFolder { path } => write!(
                formatter,
                "catalog mutations are not supported in synchronized folder {}",
                path.display()
            ),
            Self::WriteProbe { path, .. } => write!(
                formatter,
                "Souvenirs cannot write to {}. Check that the folder or volume allows changes.",
                path.display()
            ),
            Self::UnsupportedPlatform => {
                formatter.write_str("volume identity is unsupported on this platform")
            }
        }
    }
}

impl Error for StorageCapabilityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspect { source, .. } | Self::WriteProbe { source, .. } => Some(source),
            Self::SynchronizedFolder { .. } | Self::UnsupportedPlatform => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_synchronized_folder, StorageCapabilityError, WriteSuitability};
    use crate::StorageCapabilities;
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn identifies_known_synchronized_folder_locations() {
        assert!(is_synchronized_folder(Path::new(
            "/Users/person/Library/CloudStorage/provider/catalog"
        )));
        assert!(is_synchronized_folder(Path::new(
            "/Users/person/Library/Mobile Documents/catalog"
        )));
        assert!(is_synchronized_folder(Path::new(
            "/Users/person/Dropbox/catalog"
        )));
        assert!(!is_synchronized_folder(Path::new(
            "/Volumes/photos/catalog"
        )));
    }

    #[test]
    fn detects_volume_identity_for_real_directory() {
        let capabilities = StorageCapabilities::detect(std::env::temp_dir())
            .expect("temporary directory should have detectable storage");

        assert_eq!(capabilities.write_suitability, WriteSuitability::Writable);
        assert!(capabilities.volume.same_volume(capabilities.volume));
    }

    #[test]
    fn observes_available_space_for_destination() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let capabilities =
            StorageCapabilities::detect(directory.path()).expect("detect capabilities");

        assert!(
            capabilities
                .available_space_bytes(directory.path())
                .expect("available space")
                > 0
        );
    }

    #[test]
    fn write_probe_cleans_up_after_success() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let capabilities =
            StorageCapabilities::detect(directory.path()).expect("detect capabilities");

        capabilities
            .ensure_writable(directory.path())
            .expect("temporary directory should be writable");

        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read temporary directory")
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_probe_rejects_directory_without_write_access() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let original_permissions = fs::metadata(directory.path())
            .expect("read temporary directory metadata")
            .permissions();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o555))
            .expect("make temporary directory read-only");

        let capabilities =
            StorageCapabilities::detect(directory.path()).expect("detect capabilities");
        let result = capabilities.ensure_writable(directory.path());

        fs::set_permissions(directory.path(), original_permissions)
            .expect("restore temporary directory permissions");

        assert!(matches!(
            result,
            Err(StorageCapabilityError::WriteProbe { .. })
        ));
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read temporary directory")
                .count(),
            0
        );
    }
}
