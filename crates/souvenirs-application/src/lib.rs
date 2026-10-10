//! Caller-independent application services for catalog and import workflows.

pub use souvenirs_domain as domain;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use souvenirs_domain::names::CatalogLabel;
use souvenirs_storage::{
    CatalogConfig, CatalogInitializationError, CatalogLayout, StorageCapabilities,
    StorageCapabilityError, WriteSuitability, FORMAT_VERSION,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogIntent {
    Create,
    Open,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogLocation {
    pub path: PathBuf,
    pub volume_name: String,
    pub display_name: Option<String>,
    pub detail: String,
}

pub struct CatalogService;

impl CatalogService {
    pub fn validate(
        path: impl AsRef<Path>,
        intent: CatalogIntent,
    ) -> Result<CatalogLocation, CatalogServiceError> {
        let path = path.as_ref();
        let metadata = fs::metadata(path).map_err(|source| CatalogServiceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(CatalogServiceError::NotDirectory(path.to_path_buf()));
        }

        match intent {
            CatalogIntent::Create => {
                let capabilities = StorageCapabilities::detect(path)?;
                if capabilities.write_suitability == WriteSuitability::SynchronizedFolder {
                    return Err(CatalogServiceError::SynchronizedFolder(path.to_path_buf()));
                }
                if CatalogLayout::new(path).config_path().exists() {
                    return Err(CatalogServiceError::AlreadyCatalog(path.to_path_buf()));
                }
            }
            CatalogIntent::Open => {
                let layout = CatalogLayout::new(path);
                if !layout.config_path().is_file() || !layout.state_directory().is_dir() {
                    return Err(CatalogServiceError::NotCatalog(path.to_path_buf()));
                }
                read_catalog_identity(&layout.config_path())?;
            }
        }

        Ok(location(path))
    }

    pub fn create(
        path: impl AsRef<Path>,
        display_name: impl AsRef<str>,
    ) -> Result<CatalogLocation, CatalogServiceError> {
        let path = path.as_ref();
        Self::validate(path, CatalogIntent::Create)?;
        let display_name = CatalogLabel::new(display_name.as_ref())
            .map_err(|error| CatalogServiceError::InvalidName(error.to_string()))?;
        CatalogLayout::initialize(
            path,
            &CatalogConfig {
                display_name: Some(display_name.to_string()),
                ..CatalogConfig::default()
            },
        )?;
        Ok(location(path))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<CatalogLocation, CatalogServiceError> {
        Self::validate(path.as_ref(), CatalogIntent::Open)
    }
}

fn location(path: &Path) -> CatalogLocation {
    let identity = read_catalog_identity(&CatalogLayout::new(path).config_path()).ok();
    CatalogLocation {
        path: path.to_path_buf(),
        volume_name: volume_name(path),
        display_name: identity.as_ref().and_then(|(_, name)| name.clone()),
        detail: format!(
            "Catalog format {}",
            identity.map_or(FORMAT_VERSION, |(version, _)| version)
        ),
    }
}

fn volume_name(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    parts
        .windows(2)
        .find_map(|window| (window[0] == "Volumes").then(|| window[1].to_owned()))
        .or_else(|| {
            parts.windows(3).find_map(|window| {
                (window[0] == "Network" && window[1] == "Servers").then(|| window[2].to_owned())
            })
        })
        .unwrap_or_else(|| "Mac".to_owned())
}

fn read_catalog_identity(config_path: &Path) -> Result<(u32, Option<String>), CatalogServiceError> {
    let contents = fs::read_to_string(config_path).map_err(|source| CatalogServiceError::Io {
        path: config_path.to_path_buf(),
        source,
    })?;
    let version = contents
        .lines()
        .find_map(|line| line.strip_prefix("format_version: "))
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| CatalogServiceError::InvalidConfig(config_path.to_path_buf()))?;
    if version != FORMAT_VERSION {
        return Err(CatalogServiceError::UnsupportedFormat(version));
    }
    let display_name = contents
        .lines()
        .find_map(|line| line.strip_prefix("display_name: "))
        .map(serde_json::from_str::<String>)
        .transpose()
        .map_err(|_| CatalogServiceError::InvalidConfig(config_path.to_path_buf()))?;
    Ok((version, display_name))
}

#[derive(Debug)]
pub enum CatalogServiceError {
    AlreadyCatalog(PathBuf),
    Initialization(CatalogInitializationError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidConfig(PathBuf),
    InvalidName(String),
    NotCatalog(PathBuf),
    NotDirectory(PathBuf),
    Storage(StorageCapabilityError),
    SynchronizedFolder(PathBuf),
    UnsupportedFormat(u32),
}

impl CatalogServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AlreadyCatalog(_) => "catalog_already_exists",
            Self::Initialization(_) | Self::Io { .. } | Self::Storage(_) => "catalog_io",
            Self::InvalidConfig(_) => "invalid_catalog",
            Self::InvalidName(_) => "invalid_catalog_name",
            Self::NotCatalog(_) => "not_a_catalog",
            Self::NotDirectory(_) => "not_a_directory",
            Self::SynchronizedFolder(_) => "synchronized_folder",
            Self::UnsupportedFormat(_) => "unsupported_catalog_version",
        }
    }
}

impl fmt::Display for CatalogServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCatalog(_) => formatter.write_str("This folder is already a catalog."),
            Self::Initialization(error) => error.fmt(formatter),
            Self::Io { path, source } => {
                write!(formatter, "Could not inspect {}: {source}", path.display())
            }
            Self::InvalidConfig(_) => {
                formatter.write_str("The catalog configuration is damaged or unreadable.")
            }
            Self::InvalidName(message) => write!(formatter, "Choose a different name: {message}"),
            Self::NotCatalog(_) => formatter.write_str(
                "This folder is not a Souvenirs catalog. Choose a folder containing .souvenirs.yml.",
            ),
            Self::NotDirectory(_) => formatter.write_str("Choose a folder, not a file."),
            Self::Storage(error) => error.fmt(formatter),
            Self::SynchronizedFolder(_) => formatter.write_str(
                "This synchronized folder cannot hold a catalog because it may rename files on its own.",
            ),
            Self::UnsupportedFormat(version) => write!(
                formatter,
                "This catalog uses format {version}, which this version of Souvenirs cannot open."
            ),
        }
    }
}

impl From<CatalogInitializationError> for CatalogServiceError {
    fn from(value: CatalogInitializationError) -> Self {
        Self::Initialization(value)
    }
}

impl From<StorageCapabilityError> for CatalogServiceError {
    fn from(value: StorageCapabilityError) -> Self {
        Self::Storage(value)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{CatalogIntent, CatalogService, CatalogServiceError};

    #[test]
    fn creates_and_opens_catalog_through_application_service() {
        let directory = tempdir().expect("temporary directory");

        CatalogService::validate(directory.path(), CatalogIntent::Create)
            .expect("empty directory should be valid");
        CatalogService::create(directory.path(), "Family Catalog")
            .expect("catalog should initialize");
        let location = CatalogService::open(directory.path()).expect("catalog should open");
        assert_eq!(location.display_name.as_deref(), Some("Family Catalog"));
        assert!(matches!(
            CatalogService::validate(directory.path(), CatalogIntent::Create),
            Err(CatalogServiceError::AlreadyCatalog(_))
        ));
    }

    #[test]
    fn rejects_plain_folder_when_opening() {
        let directory = tempdir().expect("temporary directory");
        assert!(matches!(
            CatalogService::open(directory.path()),
            Err(CatalogServiceError::NotCatalog(_))
        ));
    }

    #[test]
    fn derives_external_volume_name_from_catalog_path() {
        assert_eq!(
            super::volume_name(std::path::Path::new(
                "/Volumes/photos-nas.local/Family Catalog"
            )),
            "photos-nas.local"
        );
        assert_eq!(
            super::volume_name(std::path::Path::new("/Users/person/Pictures")),
            "Mac"
        );
    }
}
