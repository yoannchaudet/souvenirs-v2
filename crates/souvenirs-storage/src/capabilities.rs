use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

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
        if self.write_suitability == WriteSuitability::SynchronizedFolder {
            return Err(StorageCapabilityError::SynchronizedFolder {
                path: path.as_ref().to_path_buf(),
            });
        }
        Ok(())
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
    Inspect { path: PathBuf, source: io::Error },
    SynchronizedFolder { path: PathBuf },
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
            Self::UnsupportedPlatform => {
                formatter.write_str("volume identity is unsupported on this platform")
            }
        }
    }
}

impl Error for StorageCapabilityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspect { source, .. } => Some(source),
            Self::SynchronizedFolder { .. } | Self::UnsupportedPlatform => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_synchronized_folder, WriteSuitability};
    use crate::StorageCapabilities;
    use std::path::Path;

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
}
