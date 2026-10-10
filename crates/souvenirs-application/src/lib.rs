//! Caller-independent application services for catalog and import workflows.

pub use souvenirs_domain as domain;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use souvenirs_domain::names::CatalogLabel;
use souvenirs_storage::{
    CatalogConfig, CatalogInitializationError, CatalogLayout, StorageCapabilities,
    StorageCapabilityError, WriteSuitability, FORMAT_VERSION,
};
use time::{macros::format_description, Date, OffsetDateTime};

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationMode {
    Create,
    Existing,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DateResolution {
    AsShown,
    ConvertFromUtc,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportScanSummary {
    pub source_path: PathBuf,
    pub source_name: String,
    pub file_count: usize,
    pub media_count: usize,
    pub photo_count: usize,
    pub video_count: usize,
    pub unsupported_count: usize,
    pub uncertain_time_count: usize,
    pub total_bytes: u64,
    pub suggested_collection_name: String,
    pub suggested_accepted_date: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReviewRequest {
    pub catalog_path: PathBuf,
    pub source_path: PathBuf,
    pub collection_name: String,
    pub destination_mode: DestinationMode,
    pub accepted_date: String,
    pub date_resolution: Option<DateResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportReview {
    pub scan: ImportScanSummary,
    pub collection_name: String,
    pub destination_mode: DestinationMode,
    pub destination_path: PathBuf,
    pub accepted_date: String,
    pub date_resolution: Option<DateResolution>,
    pub required_decision_count: usize,
    pub transfer_count: usize,
    pub skipped_count: usize,
    pub left_on_source_count: usize,
    pub operation_count: usize,
    pub required_space_bytes: u64,
    pub can_start: bool,
}

pub struct ImportPlanningService;

impl ImportPlanningService {
    pub fn scan(source: impl AsRef<Path>) -> Result<ImportScanSummary, ImportPlanningError> {
        let source = source.as_ref();
        if !source.is_dir() {
            return Err(ImportPlanningError::SourceUnavailable(source.to_path_buf()));
        }
        let mut files = Vec::new();
        collect_source_files(source, &mut files)?;
        files.sort();

        let mut total_bytes = 0_u64;
        let mut photo_count = 0;
        let mut video_count = 0;
        let mut unsupported_count = 0;
        for path in &files {
            total_bytes = total_bytes
                .checked_add(
                    fs::metadata(path)
                        .map_err(|source| ImportPlanningError::Io {
                            path: path.clone(),
                            source,
                        })?
                        .len(),
                )
                .ok_or(ImportPlanningError::SourceTooLarge)?;
            match media_kind(path) {
                Some(MediaKind::Photo) => photo_count += 1,
                Some(MediaKind::Video) => video_count += 1,
                None => unsupported_count += 1,
            }
        }
        let source_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Import")
            .to_owned();
        Ok(ImportScanSummary {
            source_path: source.to_path_buf(),
            source_name: source_name.clone(),
            file_count: files.len(),
            media_count: photo_count + video_count,
            photo_count,
            video_count,
            unsupported_count,
            uncertain_time_count: video_count,
            total_bytes,
            suggested_collection_name: title_case_source_name(&source_name),
            suggested_accepted_date: OffsetDateTime::now_utc()
                .date()
                .format(format_description!("[year]-[month]-[day]"))
                .map_err(|error| ImportPlanningError::InvalidDate(error.to_string()))?,
        })
    }

    pub fn review(request: ImportReviewRequest) -> Result<ImportReview, ImportPlanningError> {
        CatalogService::open(&request.catalog_path).map_err(ImportPlanningError::Catalog)?;
        let scan = Self::scan(&request.source_path)?;
        if scan.media_count == 0 {
            return Err(ImportPlanningError::NoSupportedMedia);
        }
        let collection = CatalogLabel::new(&request.collection_name)
            .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?;
        validate_date(&request.accepted_date)?;
        let year = &request.accepted_date[..4];
        let directory = format!("{} {}", request.accepted_date, collection.as_str());
        let destination_path = request.catalog_path.join(year).join(directory);
        match request.destination_mode {
            DestinationMode::Create if destination_path.exists() => {
                return Err(ImportPlanningError::DestinationAlreadyExists(
                    destination_path,
                ));
            }
            DestinationMode::Existing if !destination_path.is_dir() => {
                return Err(ImportPlanningError::DestinationMissing(destination_path));
            }
            DestinationMode::Create | DestinationMode::Existing => {}
        }
        let required_decision_count =
            usize::from(scan.uncertain_time_count > 0 && request.date_resolution.is_none());
        Ok(ImportReview {
            collection_name: collection.to_string(),
            destination_mode: request.destination_mode,
            destination_path,
            accepted_date: request.accepted_date,
            date_resolution: request.date_resolution,
            required_decision_count,
            transfer_count: scan.media_count,
            skipped_count: 0,
            left_on_source_count: scan.unsupported_count,
            operation_count: scan.media_count + 2,
            required_space_bytes: scan.total_bytes,
            can_start: required_decision_count == 0,
            scan,
        })
    }
}

#[derive(Clone, Copy)]
enum MediaKind {
    Photo,
    Video,
}

fn collect_source_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), ImportPlanningError> {
    for entry in fs::read_dir(directory).map_err(|source| ImportPlanningError::Io {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ImportPlanningError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| ImportPlanningError::Io {
                path: entry.path(),
                source,
            })?;
        let path = entry.path();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_source_files(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn media_kind(path: &Path) -> Option<MediaKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "heic" | "png" | "tif" | "tiff" | "dng" | "cr2" | "cr3" | "nef"
    ) {
        Some(MediaKind::Photo)
    } else if matches!(extension.as_str(), "mov" | "mp4" | "m4v" | "avi") {
        Some(MediaKind::Video)
    } else {
        None
    }
}

fn title_case_source_name(value: &str) -> String {
    let words = value
        .split(|character: char| character == '_' || character == '-' || character.is_whitespace())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            characters
                .next()
                .map(|first| first.to_uppercase().chain(characters).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    if words.is_empty() {
        "Import".to_owned()
    } else {
        words.join(" ")
    }
}

fn validate_date(value: &str) -> Result<(), ImportPlanningError> {
    Date::parse(value, format_description!("[year]-[month]-[day]"))
        .map(|_| ())
        .map_err(|_| ImportPlanningError::InvalidDate(value.to_owned()))
}

#[derive(Debug)]
pub enum ImportPlanningError {
    Catalog(CatalogServiceError),
    DestinationAlreadyExists(PathBuf),
    DestinationMissing(PathBuf),
    InvalidCollection(String),
    InvalidDate(String),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NoSupportedMedia,
    SourceTooLarge,
    SourceUnavailable(PathBuf),
}

impl ImportPlanningError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Catalog(error) => error.code(),
            Self::DestinationAlreadyExists(_) => "destination_exists",
            Self::DestinationMissing(_) => "destination_missing",
            Self::InvalidCollection(_) => "invalid_collection",
            Self::InvalidDate(_) => "invalid_date",
            Self::Io { .. } => "source_io",
            Self::NoSupportedMedia => "no_supported_media",
            Self::SourceTooLarge => "source_too_large",
            Self::SourceUnavailable(_) => "source_unavailable",
        }
    }
}

impl fmt::Display for ImportPlanningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog(error) => error.fmt(formatter),
            Self::DestinationAlreadyExists(path) => write!(
                formatter,
                "A collection already exists at {}. Choose Add to Existing.",
                path.display()
            ),
            Self::DestinationMissing(path) => write!(
                formatter,
                "No collection exists at {}. Choose a new collection.",
                path.display()
            ),
            Self::InvalidCollection(message) => {
                write!(formatter, "Choose a different collection name: {message}")
            }
            Self::InvalidDate(value) => {
                write!(formatter, "{value:?} is not a valid YYYY-MM-DD date.")
            }
            Self::Io { path, source } => {
                write!(formatter, "Could not scan {}: {source}", path.display())
            }
            Self::NoSupportedMedia => {
                formatter.write_str("This source contains no supported photos or videos.")
            }
            Self::SourceTooLarge => formatter.write_str("The source is too large to summarize."),
            Self::SourceUnavailable(_) => formatter
                .write_str("The source is no longer available. Reconnect it and try again."),
        }
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
    use std::fs;

    use tempfile::tempdir;

    use super::{
        CatalogIntent, CatalogService, CatalogServiceError, DateResolution, DestinationMode,
        ImportPlanningError, ImportPlanningService, ImportReviewRequest,
    };

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

    #[test]
    fn scans_and_reviews_without_mutating_source_or_catalog() {
        let temporary = tempdir().expect("temporary directory");
        let catalog = temporary.path().join("catalog");
        let source = temporary.path().join("EOS_DIGITAL");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("photo.jpg"), b"photo").unwrap();
        fs::write(source.join("clip.mov"), b"video").unwrap();
        fs::write(source.join("notes.txt"), b"notes").unwrap();
        CatalogService::create(&catalog, "Family").unwrap();

        let scan = ImportPlanningService::scan(&source).unwrap();
        assert_eq!(scan.media_count, 2);
        assert_eq!(scan.uncertain_time_count, 1);
        assert_eq!(scan.unsupported_count, 1);

        let unresolved = ImportPlanningService::review(ImportReviewRequest {
            catalog_path: catalog.clone(),
            source_path: source.clone(),
            collection_name: "Dolomites".to_owned(),
            destination_mode: DestinationMode::Create,
            accepted_date: "2026-10-10".to_owned(),
            date_resolution: None,
        })
        .unwrap();
        assert!(!unresolved.can_start);
        assert_eq!(unresolved.required_decision_count, 1);

        let resolved = ImportPlanningService::review(ImportReviewRequest {
            catalog_path: catalog.clone(),
            source_path: source.clone(),
            collection_name: "Dolomites".to_owned(),
            destination_mode: DestinationMode::Create,
            accepted_date: "2026-10-10".to_owned(),
            date_resolution: Some(DateResolution::AsShown),
        })
        .unwrap();
        assert!(resolved.can_start);
        assert_eq!(resolved.transfer_count, 2);
        assert!(!resolved.destination_path.exists());
        assert!(source.join("photo.jpg").exists());
    }

    #[test]
    fn planning_reports_source_and_destination_failures() {
        let temporary = tempdir().expect("temporary directory");
        assert!(matches!(
            ImportPlanningService::scan(temporary.path().join("missing")),
            Err(ImportPlanningError::SourceUnavailable(_))
        ));

        let catalog = temporary.path().join("catalog");
        let source = temporary.path().join("source");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("photo.jpg"), b"photo").unwrap();
        CatalogService::create(&catalog, "Family").unwrap();
        fs::create_dir_all(catalog.join("2026").join("2026-10-10 Dolomites")).unwrap();

        assert!(matches!(
            ImportPlanningService::review(ImportReviewRequest {
                catalog_path: catalog,
                source_path: source,
                collection_name: "Dolomites".to_owned(),
                destination_mode: DestinationMode::Create,
                accepted_date: "2026-10-10".to_owned(),
                date_resolution: Some(DateResolution::AsShown),
            }),
            Err(ImportPlanningError::DestinationAlreadyExists(_))
        ));
    }
}
