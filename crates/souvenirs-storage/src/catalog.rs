use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use time::macros::format_description;
use time::OffsetDateTime;

use crate::capabilities::{StorageCapabilities, StorageCapabilityError};
use crate::durability::{sync_directory, sync_file, sync_parent};

pub const FORMAT_VERSION: u32 = 2;

const DEFAULT_MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "heic", "cr3", "nef", "arw", "dng", "mp4", "mov",
];
const DEFAULT_SIDECAR_EXTENSIONS: &[&str] = &["xmp", "aae", "dop", "pp3"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogConfig {
    pub format_version: u32,
    pub media_extensions: Vec<String>,
    pub sidecar_extensions: Vec<String>,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            media_extensions: DEFAULT_MEDIA_EXTENSIONS
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
            sidecar_extensions: DEFAULT_SIDECAR_EXTENSIONS
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
        }
    }
}

impl CatalogConfig {
    fn to_yaml(&self) -> Result<String, CatalogInitializationError> {
        if self.format_version != FORMAT_VERSION {
            return Err(CatalogInitializationError::UnsupportedFormatVersion(
                self.format_version,
            ));
        }
        validate_extensions(&self.media_extensions, &self.sidecar_extensions)?;

        Ok(format!(
            "format_version: {}\nmedia_extensions: [{}]\nsidecar_extensions: [{}]\n",
            self.format_version,
            self.media_extensions.join(", "),
            self.sidecar_extensions.join(", ")
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogLayout {
    root: PathBuf,
}

impl CatalogLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn initialize(
        root: impl Into<PathBuf>,
        config: &CatalogConfig,
    ) -> Result<Self, CatalogInitializationError> {
        let layout = Self::new(root);
        layout.initialize_inner(config)?;
        Ok(layout)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(".souvenirs.yml")
    }

    pub fn state_directory(&self) -> PathBuf {
        self.root.join(".souvenirs")
    }

    pub fn journal_directory(&self) -> PathBuf {
        self.state_directory().join("journal")
    }

    pub fn staging_directory(&self) -> PathBuf {
        self.state_directory().join("staging")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.state_directory().join("lock")
    }

    pub fn lock_takeover_path(&self) -> PathBuf {
        self.state_directory().join("lock.takeover")
    }

    pub fn capabilities(&self) -> Result<StorageCapabilities, StorageCapabilityError> {
        StorageCapabilities::detect(&self.root)
    }

    fn initialize_inner(&self, config: &CatalogConfig) -> Result<(), CatalogInitializationError> {
        let config_yaml = config.to_yaml()?;
        let root_metadata =
            fs::metadata(&self.root).map_err(|source| CatalogInitializationError::Io {
                action: "inspect catalog root",
                path: self.root.clone(),
                source,
            })?;
        if !root_metadata.is_dir() {
            return Err(CatalogInitializationError::RootIsNotDirectory(
                self.root.clone(),
            ));
        }

        let capabilities = self.capabilities()?;
        capabilities.ensure_writable(&self.root)?;

        ensure_absent(&self.config_path())?;
        ensure_absent(&self.state_directory())?;

        create_directory(&self.state_directory())?;
        create_directory(&self.journal_directory())?;

        let journal_path = self
            .journal_directory()
            .join(format!("{}-initialize.jsonl", operation_identifier()?));
        let mut journal = create_new_file(&journal_path)?;
        append_json_line(
            &mut journal,
            &journal_path,
            &InitializationRecord::Plan {
                operation: "initialize",
                format_version: config.format_version,
            },
        )?;

        create_directory(&self.staging_directory())?;
        write_new_file(&self.config_path(), config_yaml.as_bytes())?;

        append_json_line(
            &mut journal,
            &journal_path,
            &InitializationRecord::Outcome {
                outcome: "completed",
            },
        )?;
        sync_directory(&self.root).map_err(|source| CatalogInitializationError::Io {
            action: "persist catalog root",
            path: self.root.clone(),
            source,
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InitializationRecord<'a> {
    Plan {
        operation: &'a str,
        format_version: u32,
    },
    Outcome {
        outcome: &'a str,
    },
}

fn validate_extensions(
    media_extensions: &[String],
    sidecar_extensions: &[String],
) -> Result<(), CatalogInitializationError> {
    for extension in media_extensions.iter().chain(sidecar_extensions) {
        if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(CatalogInitializationError::InvalidExtension(
                extension.clone(),
            ));
        }
    }

    if media_extensions.iter().any(|media| {
        sidecar_extensions
            .iter()
            .any(|sidecar| media.eq_ignore_ascii_case(sidecar))
    }) {
        return Err(CatalogInitializationError::OverlappingExtensions);
    }
    Ok(())
}

fn ensure_absent(path: &Path) -> Result<(), CatalogInitializationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(CatalogInitializationError::AlreadyExists(
            path.to_path_buf(),
        )),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CatalogInitializationError::Io {
            action: "inspect initialization destination",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_directory(path: &Path) -> Result<(), CatalogInitializationError> {
    fs::create_dir(path).map_err(|source| CatalogInitializationError::Io {
        action: "create directory",
        path: path.to_path_buf(),
        source,
    })?;
    sync_parent(path).map_err(|source| CatalogInitializationError::Io {
        action: "persist parent directory",
        path: path.to_path_buf(),
        source,
    })
}

fn create_new_file(path: &Path) -> Result<File, CatalogInitializationError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| CatalogInitializationError::Io {
            action: "create file exclusively",
            path: path.to_path_buf(),
            source,
        })
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), CatalogInitializationError> {
    let mut file = create_new_file(path)?;
    file.write_all(bytes)
        .and_then(|()| sync_file(&file))
        .map_err(|source| CatalogInitializationError::Io {
            action: "write and persist file",
            path: path.to_path_buf(),
            source,
        })?;
    sync_parent(path).map_err(|source| CatalogInitializationError::Io {
        action: "persist parent directory",
        path: path.to_path_buf(),
        source,
    })
}

fn append_json_line(
    file: &mut File,
    path: &Path,
    record: &InitializationRecord<'_>,
) -> Result<(), CatalogInitializationError> {
    serde_json::to_writer(&mut *file, record).map_err(|source| {
        CatalogInitializationError::SerializeJournal {
            path: path.to_path_buf(),
            source,
        }
    })?;
    file.write_all(b"\n")
        .and_then(|()| sync_file(file))
        .map_err(|source| CatalogInitializationError::Io {
            action: "append and persist initialization journal",
            path: path.to_path_buf(),
            source,
        })?;
    sync_parent(path).map_err(|source| CatalogInitializationError::Io {
        action: "persist journal directory",
        path: path.to_path_buf(),
        source,
    })
}

fn operation_identifier() -> Result<String, CatalogInitializationError> {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .map_err(|source| CatalogInitializationError::Clock {
            details: source.to_string(),
        })
}

#[derive(Debug)]
pub enum CatalogInitializationError {
    AlreadyExists(PathBuf),
    Clock {
        details: String,
    },
    InvalidExtension(String),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    OverlappingExtensions,
    RootIsNotDirectory(PathBuf),
    SerializeJournal {
        path: PathBuf,
        source: serde_json::Error,
    },
    StorageCapability(StorageCapabilityError),
    UnsupportedFormatVersion(u32),
}

impl fmt::Display for CatalogInitializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists(path) => write!(
                formatter,
                "catalog initialization refuses to replace {}",
                path.display()
            ),
            Self::Clock { details } => {
                write!(formatter, "failed to read the system clock: {details}")
            }
            Self::InvalidExtension(extension) => {
                write!(formatter, "invalid catalog extension {extension:?}")
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::OverlappingExtensions => formatter.write_str(
                "media and sidecar extensions must not overlap under ASCII case folding",
            ),
            Self::RootIsNotDirectory(path) => {
                write!(
                    formatter,
                    "catalog root is not a directory: {}",
                    path.display()
                )
            }
            Self::SerializeJournal { path, source } => write!(
                formatter,
                "failed to serialize initialization journal {}: {source}",
                path.display()
            ),
            Self::StorageCapability(source) => source.fmt(formatter),
            Self::UnsupportedFormatVersion(version) => write!(
                formatter,
                "unsupported catalog format version {version}; expected {FORMAT_VERSION}"
            ),
        }
    }
}

impl Error for CatalogInitializationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::SerializeJournal { source, .. } => Some(source),
            Self::StorageCapability(source) => Some(source),
            Self::AlreadyExists(_)
            | Self::Clock { .. }
            | Self::InvalidExtension(_)
            | Self::OverlappingExtensions
            | Self::RootIsNotDirectory(_)
            | Self::UnsupportedFormatVersion(_) => None,
        }
    }
}

impl From<StorageCapabilityError> for CatalogInitializationError {
    fn from(source: StorageCapabilityError) -> Self {
        Self::StorageCapability(source)
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogConfig, CatalogInitializationError, CatalogLayout, FORMAT_VERSION};
    use std::fs;

    use crate::test_support::TempDirectory;

    #[test]
    fn initializes_catalog_layout_and_persisted_journal() {
        let temporary = TempDirectory::new("initialize");
        let layout =
            CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap();

        assert!(layout.state_directory().is_dir());
        assert!(layout.journal_directory().is_dir());
        assert!(layout.staging_directory().is_dir());
        assert_eq!(
            fs::read_to_string(layout.config_path()).unwrap(),
            "format_version: 2\n\
             media_extensions: [jpg, jpeg, heic, cr3, nef, arw, dng, mp4, mov]\n\
             sidecar_extensions: [xmp, aae, dop, pp3]\n"
        );

        let journals: Vec<_> = fs::read_dir(layout.journal_directory())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(journals.len(), 1);
        let journal = fs::read_to_string(&journals[0]).unwrap();
        assert_eq!(
            journal,
            format!(
                "{{\"kind\":\"plan\",\"operation\":\"initialize\",\"format_version\":{FORMAT_VERSION}}}\n\
                 {{\"kind\":\"outcome\",\"outcome\":\"completed\"}}\n"
            )
        );
    }

    #[test]
    fn refuses_to_replace_existing_catalog_state() {
        let temporary = TempDirectory::new("occupied");
        fs::write(temporary.path().join(".souvenirs.yml"), "existing").unwrap();

        let error =
            CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap_err();

        assert!(matches!(
            error,
            CatalogInitializationError::AlreadyExists(path)
                if path == temporary.path().join(".souvenirs.yml")
        ));
        assert_eq!(
            fs::read_to_string(temporary.path().join(".souvenirs.yml")).unwrap(),
            "existing"
        );
        assert!(!temporary.path().join(".souvenirs").exists());
    }

    #[test]
    fn rejects_overlapping_extension_roles_before_mutation() {
        let temporary = TempDirectory::new("extensions");
        let config = CatalogConfig {
            format_version: FORMAT_VERSION,
            media_extensions: vec!["jpg".to_owned()],
            sidecar_extensions: vec!["JPG".to_owned()],
        };

        let error = CatalogLayout::initialize(temporary.path(), &config).unwrap_err();

        assert!(matches!(
            error,
            CatalogInitializationError::OverlappingExtensions
        ));
        assert!(!temporary.path().join(".souvenirs").exists());
        assert!(!temporary.path().join(".souvenirs.yml").exists());
    }

    #[test]
    fn rejects_unsupported_format_version_before_mutation() {
        let temporary = TempDirectory::new("format-version");
        let config = CatalogConfig {
            format_version: FORMAT_VERSION + 1,
            ..CatalogConfig::default()
        };

        let error = CatalogLayout::initialize(temporary.path(), &config).unwrap_err();

        assert!(matches!(
            error,
            CatalogInitializationError::UnsupportedFormatVersion(version)
                if version == FORMAT_VERSION + 1
        ));
        assert!(!temporary.path().join(".souvenirs").exists());
    }
}
