//! Caller-independent application services for catalog and import workflows.

mod execution;
mod planning;
mod source_datetime;

pub use execution::{
    ImportCancellation, ImportExecutionError, ImportExecutionOutcome, ImportExecutionRequest,
    ImportExecutionResult, ImportExecutionService, ImportRecoveryMode, ImportRecoveryService,
    OperationProgress, PreparedImport, RecoveryOutcome,
};
pub use souvenirs_domain as domain;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use souvenirs_domain::catalog::{year_directory, DatedCollectionRange};
use souvenirs_domain::datetime::{
    confirm_candidate, AcceptedDateTime, AcceptedDateTimeSource, ConfirmationDecision,
    ConfirmationReason, DateTimeResolution, DateTimeSource,
};
use souvenirs_domain::names::CatalogLabel;
use souvenirs_domain::relationships::{
    resolve_directory_relationships, RelationshipFinding, RelationshipStatus,
};
use souvenirs_domain::source::{SourceFile, SourceFileId, SourcePath};
use souvenirs_storage::{
    is_operation_journal_file_name, read_operation_records, CatalogConfig,
    CatalogInitializationError, CatalogLayout, DurableTransition, OperationRecord,
    StorageCapabilities, StorageCapabilityError, WriteSuitability, FORMAT_VERSION,
};
use time::{Date, UtcOffset};

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
                let layout = CatalogLayout::new(path);
                if layout.config_path().exists() && !layout.has_pending_initialization()? {
                    return Err(CatalogServiceError::AlreadyCatalog(path.to_path_buf()));
                }
                capabilities.ensure_writable(path)?;
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
        let display_name = CatalogLabel::new(display_name.as_ref())
            .map_err(|error| CatalogServiceError::InvalidName(error.to_string()))?;
        let config = CatalogConfig {
            display_name: Some(display_name.to_string()),
            ..CatalogConfig::default()
        };
        Self::validate(path, CatalogIntent::Create)?;
        CatalogLayout::initialize(path, &config)?;
        Ok(location(path))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<CatalogLocation, CatalogServiceError> {
        let path = path.as_ref();
        if !fs::metadata(path)
            .map_err(|source| CatalogServiceError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .is_dir()
        {
            return Err(CatalogServiceError::NotDirectory(path.to_path_buf()));
        }
        CatalogLayout::resume_initialization(path)?;
        Self::validate(path, CatalogIntent::Open)
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
    pub suggested_accepted_date: Option<String>,
    pub suggested_range_end: Option<String>,
    pub pending_item_count: usize,
    pub unresolved_item_count: usize,
    pub datetime_sources: Vec<ImportDateSourceSummary>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDateSource {
    ExifDateTimeOriginal,
    ExifDateTimeDigitized,
    QuickTimeCreationDate,
    XmpDateCreated,
    XmpExifDateTimeOriginal,
    ContainerCreationTime,
    GoogleTakeoutPhotoTakenTime,
    FilenamePattern,
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDateState {
    Accepted,
    NeedsConfirmation,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportDateSourceSummary {
    pub source: ImportDateSource,
    pub state: ImportDateState,
    pub item_count: usize,
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
    pub reviewed_plan: Option<String>,
}

pub struct ImportPlanningService;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UnfinishedOperation {
    pub operation_id: String,
    pub collection_name: String,
    pub source_name: String,
    pub copied_count: usize,
    pub total_count: usize,
    pub source_available: bool,
}

pub struct OperationInspectionService;

impl OperationInspectionService {
    pub fn unfinished(
        catalog: impl AsRef<Path>,
    ) -> Result<Option<UnfinishedOperation>, CatalogServiceError> {
        let catalog = catalog.as_ref();
        CatalogService::open(catalog)?;
        let journals = CatalogLayout::new(catalog).journal_directory();
        let entries = match fs::read_dir(&journals) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(CatalogServiceError::Io {
                    path: journals,
                    source,
                });
            }
        };
        let mut paths = entries
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|source| CatalogServiceError::Io {
                        path: journals.clone(),
                        source,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        paths.sort();
        for path in paths {
            if !path.file_name().is_some_and(is_operation_journal_file_name) {
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-initialize.jsonl"))
            {
                continue;
            }
            let records = read_operation_records(&path).map_err(|error| {
                CatalogServiceError::InvalidOperationJournal {
                    path: path.clone(),
                    message: error.to_string(),
                }
            })?;
            if matches!(records.last(), Some(OperationRecord::Outcome { .. })) {
                continue;
            }
            let Some(OperationRecord::Plan { operation_id, .. }) = records.first() else {
                continue;
            };
            let Some((collection_name, source_path, total_count)) =
                records.iter().find_map(|record| match record {
                    OperationRecord::Metadata {
                        collection_name,
                        source_path,
                        total_count,
                    } => Some((collection_name, source_path, total_count)),
                    _ => None,
                })
            else {
                continue;
            };
            let copied_count = records
                .iter()
                .filter(|record| {
                    matches!(
                        record,
                        OperationRecord::Transition {
                            transition: DurableTransition::CopyPublished { .. }
                        }
                    )
                })
                .count();
            return Ok(Some(UnfinishedOperation {
                operation_id: operation_id.clone(),
                collection_name: collection_name.clone(),
                source_name: source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("the source")
                    .to_owned(),
                copied_count,
                total_count: *total_count,
                source_available: source_path.is_dir(),
            }));
        }
        Ok(None)
    }
}

impl ImportPlanningService {
    pub fn scan(source: impl AsRef<Path>) -> Result<ImportScanSummary, ImportPlanningError> {
        let source = source.as_ref();
        let inspected = inspect_source(source, None, false)?;
        scan_summary(source, &inspected)
    }

    pub fn review(request: ImportReviewRequest) -> Result<ImportReview, ImportPlanningError> {
        planning::review_import(request)
    }
}

fn scan_summary(
    source: &Path,
    inspected: &InspectedSource,
) -> Result<ImportScanSummary, ImportPlanningError> {
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Import")
        .to_owned();
    Ok(ImportScanSummary {
        source_path: source.to_path_buf(),
        source_name: source_name.clone(),
        file_count: inspected.file_count,
        media_count: inspected.photo_count + inspected.video_count,
        photo_count: inspected.photo_count,
        video_count: inspected.video_count,
        unsupported_count: inspected.unsupported_count,
        uncertain_time_count: inspected.pending_item_count,
        total_bytes: inspected.total_bytes,
        suggested_collection_name: title_case_source_name(&source_name),
        suggested_accepted_date: inspected.range_start.map(|date| date.to_string()),
        suggested_range_end: inspected.range_end.map(|date| date.to_string()),
        pending_item_count: inspected.pending_item_count,
        unresolved_item_count: inspected.unresolved_item_count,
        datetime_sources: inspected.datetime_sources.clone(),
    })
}
#[derive(Clone, Debug)]
pub(crate) struct PlannedSourceItem {
    pub primary: SourceFile,
    pub companion: Option<SourceFile>,
    pub sidecars: Vec<SourceFile>,
    pub accepted: AcceptedDateTime,
}
#[derive(Clone, Debug)]
pub(crate) struct InspectedSource {
    file_count: usize,
    photo_count: usize,
    video_count: usize,
    unsupported_count: usize,
    total_bytes: u64,
    pending_item_count: usize,
    unresolved_item_count: usize,
    transfer_media_count: usize,
    transfer_file_count: usize,
    excluded_file_count: usize,
    transfer_bytes: u64,
    range_start: Option<Date>,
    range_end: Option<Date>,
    datetime_sources: Vec<ImportDateSourceSummary>,
    pub source_snapshot: Vec<SourceSnapshotEntry>,
    pub items: Vec<PlannedSourceItem>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SourceSnapshotEntry {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}
pub(crate) fn directory_identity(metadata: &fs::Metadata) -> String {
    format!(
        "{:?}",
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    )
}
pub(crate) fn inspect_source(
    source: &Path,
    decision: Option<DateResolution>,
    include_snapshot: bool,
) -> Result<InspectedSource, ImportPlanningError> {
    if !source.is_dir() {
        return Err(ImportPlanningError::SourceUnavailable(source.to_path_buf()));
    }
    let mut paths = Vec::new();
    collect_source_files(source, &mut paths)?;
    paths.sort();
    let mut source_files = Vec::new();
    let mut sizes = BTreeMap::new();
    let mut source_snapshot = Vec::new();
    let mut total_bytes = 0_u64;
    let mut photo_count = 0;
    let mut video_count = 0;
    for path in &paths {
        let size = fs::metadata(path)
            .map_err(|error| ImportPlanningError::Io {
                path: path.clone(),
                source: error,
            })?
            .len();
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or(ImportPlanningError::SourceTooLarge)?;
        let relative = path
            .strip_prefix(source)
            .expect("collected paths are below the source");
        let components = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        ImportPlanningError::InvalidSource(
                            "source path is not valid UTF-8".to_owned(),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let source_path = SourcePath::new(components)
            .map_err(|error| ImportPlanningError::InvalidSource(error.to_string()))?;
        let id = SourceFileId::new(source_path.to_string())
            .map_err(|error| ImportPlanningError::InvalidSource(error.to_string()))?;
        let kind = souvenirs_metadata::classify_path(path);
        match kind {
            souvenirs_metadata::AssetKind::Original {
                media: souvenirs_metadata::MediaKind::Photo,
                ..
            } => photo_count += 1,
            souvenirs_metadata::AssetKind::Original {
                media: souvenirs_metadata::MediaKind::Video,
                ..
            } => video_count += 1,
            souvenirs_metadata::AssetKind::Sidecar { .. }
            | souvenirs_metadata::AssetKind::Unsupported => {}
        }
        let filename = source_path.filename();
        let extension = Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_owned();
        let basename = filename
            .strip_suffix(&format!(".{extension}"))
            .unwrap_or(filename)
            .to_owned();
        sizes.insert(id.clone(), size);
        if include_snapshot {
            source_snapshot.push(SourceSnapshotEntry {
                path: relative.to_path_buf(),
                size_bytes: size,
                sha256: execution::digest_file(path)
                    .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                    .0,
            });
        }
        source_files.push(SourceFile {
            id,
            path: source_path,
            basename,
            extension,
            kind: kind.source_file_kind(),
            metadata: souvenirs_metadata::normalized_metadata(path)
                .map_err(|error| ImportPlanningError::InvalidMetadata(error.to_string()))?,
        });
    }

    let by_id = source_files
        .iter()
        .map(|file| (file.id.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let relationships = resolve_directory_relationships(&source_files);
    let resolved = source_datetime::assemble_source_datetimes(&source_files, relationships);
    let unhandled_ids = resolved
        .findings
        .iter()
        .filter_map(unhandled_file_id)
        .collect::<BTreeSet<_>>();
    let unsupported_count = resolved
        .findings
        .iter()
        .filter(|finding| matches!(finding, RelationshipFinding::UnknownContent(_)))
        .count();
    let mut pending_item_count = 0;
    let mut unresolved_item_count = 0;
    let mut excluded_file_count = 0;
    let mut transfer_bytes = 0_u64;
    let mut source_counts = BTreeMap::new();
    let mut items = Vec::new();
    for resolved_item in resolved.items {
        let item = resolved_item.logical_item;
        let item_files = 1 + usize::from(item.companion.is_some()) + item.sidecars.len();
        if item.status == RelationshipStatus::NeedsDecision {
            unresolved_item_count += 1;
            excluded_file_count += item_files;
            continue;
        }
        let resolution = resolved_item
            .datetime
            .expect("resolved relationships always produce a datetime resolution");
        let accepted = match resolution {
            DateTimeResolution::Accepted { accepted, .. } => {
                let source = accepted_source(&accepted.source);
                *source_counts
                    .entry((source, ImportDateState::Accepted))
                    .or_insert(0) += 1;
                Some(accepted)
            }
            DateTimeResolution::NeedsConfirmation(pending) => {
                let source = import_date_source(pending.candidate.source);
                if decision.is_some() {
                    let accepted = confirm_pending(pending).ok();
                    let state = if accepted.is_some() {
                        ImportDateState::Accepted
                    } else {
                        pending_item_count += 1;
                        ImportDateState::NeedsConfirmation
                    };
                    *source_counts.entry((source, state)).or_insert(0) += 1;
                    accepted
                } else {
                    pending_item_count += 1;
                    *source_counts
                        .entry((source, ImportDateState::NeedsConfirmation))
                        .or_insert(0) += 1;
                    None
                }
            }
            DateTimeResolution::Missing => {
                unresolved_item_count += 1;
                None
            }
        };
        let Some(accepted) = accepted else {
            excluded_file_count += item_files;
            continue;
        };
        for id in std::iter::once(&item.primary)
            .chain(item.companion.iter())
            .chain(item.sidecars.iter())
        {
            transfer_bytes = transfer_bytes
                .checked_add(sizes[id])
                .ok_or(ImportPlanningError::SourceTooLarge)?;
        }
        items.push(PlannedSourceItem {
            primary: by_id[&item.primary].clone(),
            companion: item.companion.as_ref().map(|id| by_id[id].clone()),
            sidecars: item.sidecars.iter().map(|id| by_id[id].clone()).collect(),
            accepted,
        });
    }
    let range_start = items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .min();
    let range_end = items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .max();
    let transfer_file_count = items
        .iter()
        .map(|item| 1 + usize::from(item.companion.is_some()) + item.sidecars.len())
        .sum();
    let transfer_media_count = items
        .iter()
        .map(|item| 1 + usize::from(item.companion.is_some()))
        .sum();
    Ok(InspectedSource {
        file_count: paths.len(),
        photo_count,
        video_count,
        unsupported_count,
        total_bytes,
        pending_item_count,
        unresolved_item_count,
        transfer_media_count,
        transfer_file_count,
        excluded_file_count: excluded_file_count + unhandled_ids.len(),
        transfer_bytes,
        range_start,
        range_end,
        datetime_sources: source_counts
            .into_iter()
            .map(|((source, state), item_count)| ImportDateSourceSummary {
                source,
                state,
                item_count,
            })
            .collect(),
        source_snapshot,
        items,
    })
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

fn unhandled_file_id(finding: &RelationshipFinding) -> Option<&SourceFileId> {
    match finding {
        RelationshipFinding::OrphanSidecar(file) | RelationshipFinding::UnknownContent(file) => {
            Some(file)
        }
        RelationshipFinding::AmbiguousSidecar { sidecar, .. } => Some(sidecar),
        RelationshipFinding::AmbiguousRawJpeg(_)
        | RelationshipFinding::AmbiguousLivePhoto { .. }
        | RelationshipFinding::AmbiguousCanonicalCompound(_)
        | RelationshipFinding::ConflictingCompound(_) => None,
    }
}

fn confirm_pending(
    pending: souvenirs_domain::datetime::PendingDateTime,
) -> Result<AcceptedDateTime, souvenirs_domain::datetime::DateTimeError> {
    let decision = match pending.reason {
        ConfirmationReason::UtcInterpretation => ConfirmationDecision::InterpretUtc(UtcOffset::UTC),
        ConfirmationReason::MissingClock => {
            return Err(souvenirs_domain::datetime::DateTimeError::InvalidConfirmation)
        }
        ConfirmationReason::AmbiguousMetadata | ConfirmationReason::Proposal => {
            ConfirmationDecision::AcceptCandidate
        }
    };
    confirm_candidate(pending, decision)
}
pub(crate) fn accepted_source(source: &AcceptedDateTimeSource) -> ImportDateSource {
    match source {
        AcceptedDateTimeSource::Metadata(source) | AcceptedDateTimeSource::Confirmed(source) => {
            import_date_source(*source)
        }
        AcceptedDateTimeSource::Corrected { original, .. } => accepted_source(original),
        AcceptedDateTimeSource::UserAssigned => {
            unreachable!("scan planning does not assign datetimes")
        }
    }
}
fn import_date_source(source: DateTimeSource) -> ImportDateSource {
    match source {
        DateTimeSource::ExifDateTimeOriginal => ImportDateSource::ExifDateTimeOriginal,
        DateTimeSource::ExifDateTimeDigitized => ImportDateSource::ExifDateTimeDigitized,
        DateTimeSource::QuickTimeCreationDate => ImportDateSource::QuickTimeCreationDate,
        DateTimeSource::XmpDateCreated => ImportDateSource::XmpDateCreated,
        DateTimeSource::XmpExifDateTimeOriginal => ImportDateSource::XmpExifDateTimeOriginal,
        DateTimeSource::ContainerCreationTime => ImportDateSource::ContainerCreationTime,
        DateTimeSource::GoogleTakeoutPhotoTakenTime => {
            ImportDateSource::GoogleTakeoutPhotoTakenTime
        }
        DateTimeSource::FilenamePattern => ImportDateSource::FilenamePattern,
    }
}

pub(crate) fn dated_collection_components(
    start: Date,
    end: Date,
    label: &CatalogLabel,
) -> Result<(String, String), ImportPlanningError> {
    let range = DatedCollectionRange::new(start, end)
        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?;
    let year = year_directory(start)
        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?;
    let folder = range
        .folder_name(label)
        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?;
    Ok((year.to_string(), folder.to_string()))
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

#[derive(Debug)]
pub enum ImportPlanningError {
    Catalog(CatalogServiceError),
    DestinationAlreadyExists(PathBuf),
    DestinationMissing(PathBuf),
    InvalidCollection(String),
    InvalidDate(String),
    InvalidMetadata(String),
    InvalidSource(String),
    InvalidPlan(String),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NoSupportedMedia,
    SourceTooLarge,
    SourceUnavailable(PathBuf),
    UnsupportedDateResolution,
}

impl ImportPlanningError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Catalog(error) => error.code(),
            Self::DestinationAlreadyExists(_) => "destination_exists",
            Self::DestinationMissing(_) => "destination_missing",
            Self::InvalidCollection(_) => "invalid_collection",
            Self::InvalidDate(_) => "invalid_date",
            Self::InvalidMetadata(_) => "invalid_metadata",
            Self::InvalidSource(_) => "invalid_source",
            Self::InvalidPlan(_) => "invalid_plan",
            Self::Io { .. } => "source_io",
            Self::NoSupportedMedia => "no_supported_media",
            Self::SourceTooLarge => "source_too_large",
            Self::SourceUnavailable(_) => "source_unavailable",
            Self::UnsupportedDateResolution => "unsupported_date_resolution",
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
            Self::InvalidMetadata(message) => {
                write!(formatter, "Could not read media metadata: {message}")
            }
            Self::InvalidSource(message) => write!(formatter, "Invalid source file: {message}"),
            Self::InvalidPlan(message) => formatter.write_str(message),
            Self::Io { path, source } => {
                write!(formatter, "Could not scan {}: {source}", path.display())
            }
            Self::NoSupportedMedia => {
                formatter.write_str("This source contains no supported photos or videos.")
            }
            Self::SourceTooLarge => formatter.write_str("The source is too large to summarize."),
            Self::SourceUnavailable(_) => formatter
                .write_str("The source is no longer available. Reconnect it and try again."),
            Self::UnsupportedDateResolution => formatter.write_str(
                "UTC conversion needs an explicit timezone and is not available in this review.",
            ),
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
    InvalidOperationJournal {
        path: PathBuf,
        message: String,
    },
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
            Self::Initialization(CatalogInitializationError::ConfigurationMismatch { .. }) => {
                "catalog_configuration_mismatch"
            }
            Self::Initialization(CatalogInitializationError::UnsupportedFormatVersion(_))
            | Self::UnsupportedFormat(_) => "unsupported_catalog_version",
            Self::Initialization(_) | Self::Io { .. } | Self::Storage(_) => "catalog_io",
            Self::InvalidConfig(_) => "invalid_catalog",
            Self::InvalidOperationJournal { .. } => "invalid_operation_journal",
            Self::InvalidName(_) => "invalid_catalog_name",
            Self::NotCatalog(_) => "not_a_catalog",
            Self::NotDirectory(_) => "not_a_directory",
            Self::SynchronizedFolder(_) => "synchronized_folder",
        }
    }
}

impl fmt::Display for CatalogServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCatalog(_) => formatter.write_str("This folder is already a catalog."),
            Self::Initialization(CatalogInitializationError::ConfigurationMismatch {
                confirmed_display_name,
                ..
            }) => match confirmed_display_name {
                Some(name) => write!(
                    formatter,
                    "This catalog was initialized as {name:?} with different settings."
                ),
                None => formatter
                    .write_str("This catalog was initialized with different settings."),
            },
            Self::Initialization(error) => error.fmt(formatter),
            Self::Io { path, source } => {
                write!(formatter, "Could not inspect {}: {source}", path.display())
            }
            Self::InvalidConfig(_) => {
                formatter.write_str("The catalog configuration is damaged or unreadable.")
            }
            Self::InvalidOperationJournal { path, message } => write!(
                formatter,
                "Could not read operation journal {}: {message}",
                path.display()
            ),
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

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::{
        CatalogInitializationError, CatalogIntent, CatalogService, CatalogServiceError,
        DateResolution, DestinationMode, ImportPlanningError, ImportPlanningService,
        ImportReviewRequest, OperationInspectionService,
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
        assert_eq!(
            OperationInspectionService::unfinished(directory.path()).unwrap(),
            None
        );
        assert!(matches!(
            CatalogService::validate(directory.path(), CatalogIntent::Create),
            Err(CatalogServiceError::AlreadyCatalog(_))
        ));
        let error = CatalogServiceError::Initialization(
            CatalogInitializationError::ConfigurationMismatch {
                confirmed_display_name: Some("Family Catalog".to_owned()),
                confirmed_config_yaml: "raw catalog configuration".to_owned(),
            },
        );
        assert_eq!(error.code(), "catalog_configuration_mismatch");
        assert_eq!(
            error.to_string(),
            "This catalog was initialized as \"Family Catalog\" with different settings."
        );
    }

    #[test]
    fn rejects_plain_folder_when_opening() {
        let directory = tempdir().expect("temporary directory");
        assert!(matches!(
            CatalogService::open(directory.path()),
            Err(CatalogServiceError::NotCatalog(_))
        ));
        let file = directory.path().join("catalog");
        fs::write(&file, []).unwrap();
        assert_eq!(
            CatalogService::open(file).unwrap_err().code(),
            "not_a_directory"
        );
        let future = tempdir().unwrap();
        fs::write(
            future.path().join(".souvenirs-initialize.jsonl"),
            b"{\"kind\":\"plan\",\"operation_id\":\"initialize\",\"plan\":{\"format_version\":3,\"config_yaml\":\"format_version: 3\\n\",\"config_sha256\":\"63cde216c8fae788bcc0892df26627eddc0b9c93bd26d67a9b6460cbf4d7f793\",\"journal_file_name\":\"future-initialize.jsonl\",\"staged_config_path\":\".souvenirs/staging/initialize-config.yml.stage\",\"future_field\":true}}\n",
        )
        .unwrap();
        assert_eq!(
            CatalogService::open(future.path()).unwrap_err().code(),
            "unsupported_catalog_version"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_validation_rejects_directory_without_write_access() {
        let directory = tempdir().expect("temporary directory");
        let original_permissions = fs::metadata(directory.path())
            .expect("read temporary directory metadata")
            .permissions();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o555))
            .expect("make temporary directory read-only");

        let error = CatalogService::validate(directory.path(), CatalogIntent::Create)
            .expect_err("read-only directory must fail create validation");

        fs::set_permissions(directory.path(), original_permissions)
            .expect("restore temporary directory permissions");

        assert_eq!(error.code(), "catalog_io");
        assert!(error.to_string().starts_with("Souvenirs cannot write to "));
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read temporary directory")
                .count(),
            0
        );
    }

    #[test]
    fn reports_typed_unfinished_operation_metadata() {
        let directory = tempdir().expect("temporary directory");
        let catalog = directory.path().join("catalog");
        let source = directory.path().join("EOS_DIGITAL");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        CatalogService::create(&catalog, "Family").unwrap();
        let layout = souvenirs_storage::CatalogLayout::new(&catalog);
        let lock = souvenirs_storage::CatalogLock::acquire(&layout).unwrap();
        let mut journal = souvenirs_storage::OperationJournal::create(
            &layout,
            &lock,
            "import-one",
            &serde_json::json!({"confirmed": true}),
        )
        .unwrap();
        journal
            .record_metadata(&layout, &lock, "Dolomites".to_owned(), source.clone(), 12)
            .unwrap();
        drop(journal);
        lock.release().unwrap();
        fs::write(
            layout
                .journal_directory()
                .join("._20260101-000000-import.jsonl"),
            b"not a journal",
        )
        .unwrap();
        fs::remove_dir(&source).unwrap();

        let operation = OperationInspectionService::unfinished(&catalog)
            .unwrap()
            .expect("unfinished operation");
        assert_eq!(operation.operation_id, "import-one");
        assert_eq!(operation.collection_name, "Dolomites");
        assert_eq!(operation.source_name, "EOS_DIGITAL");
        assert_eq!(operation.total_count, 12);
        assert!(!operation.source_available);
    }

    #[test]
    fn reports_malformed_non_appledouble_journal() {
        let directory = tempdir().expect("temporary directory");
        CatalogService::create(directory.path(), "Family").unwrap();
        let journals = souvenirs_storage::CatalogLayout::new(directory.path()).journal_directory();
        fs::write(
            journals.join("._20260101-000000-import.jsonl"),
            b"not a journal",
        )
        .unwrap();
        let malformed = journals.join("20260101-000000-import.jsonl");
        fs::write(&malformed, b"not a journal\n").unwrap();

        assert!(matches!(
            OperationInspectionService::unfinished(directory.path()),
            Err(CatalogServiceError::InvalidOperationJournal { path, .. }) if path == malformed
        ));
    }

    #[test]
    fn missing_journal_directory_means_no_unfinished_operation() {
        let directory = tempdir().expect("temporary directory");
        CatalogService::create(directory.path(), "Family").unwrap();
        let journals = souvenirs_storage::CatalogLayout::new(directory.path()).journal_directory();
        for entry in fs::read_dir(&journals).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(journals).unwrap();

        assert_eq!(
            OperationInspectionService::unfinished(directory.path()).unwrap(),
            None
        );
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
        fs::write(source.join("IMG_20241010_101112.jpg"), b"photo").unwrap();
        fs::write(source.join("VID_20241011_121314.mov"), b"video").unwrap();
        fs::write(source.join("undated.jpg"), b"undated").unwrap();
        fs::write(source.join("notes.txt"), b"notes").unwrap();
        CatalogService::create(&catalog, "Family").unwrap();

        let scan = ImportPlanningService::scan(&source).unwrap();
        assert_eq!(scan.uncertain_time_count, 2);
        assert_eq!(scan.unresolved_item_count, 1);
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
        assert!(unresolved.reviewed_plan.is_none());
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
        let reviewed: serde_json::Value =
            serde_json::from_str(resolved.reviewed_plan.as_deref().unwrap()).unwrap();
        assert_eq!(reviewed["plan"]["copies"][0]["role"], "primary");
        assert_eq!(
            reviewed["plan"]["copies"][0]["accepted_datetime"],
            "2024-10-10 10:11:12.0"
        );
        assert!(reviewed["plan"]["exclusions"]
            .to_string()
            .contains("undated.jpg"));
        assert_eq!(resolved.transfer_count, 2);
        assert!(resolved
            .destination_path
            .ends_with("2024-10-10 +1d Dolomites"));
        assert!(!resolved.destination_path.exists());
        fs::write(source.join("IMG-20241011-WA0001.jpg"), b"date only").unwrap();
        let pending = super::inspect_source(&source, Some(DateResolution::AsShown), false).unwrap();
        assert_eq!(pending.pending_item_count, 1);
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
        fs::write(source.join("IMG_20241010_101112.jpg"), b"photo").unwrap();
        CatalogService::create(&catalog, "Family").unwrap();
        fs::create_dir_all(catalog.join("2024").join("2024-10-10 Dolomites")).unwrap();

        assert!(matches!(
            ImportPlanningService::review(ImportReviewRequest {
                catalog_path: catalog.clone(),
                source_path: source.clone(),
                collection_name: "Dolomites".to_owned(),
                destination_mode: DestinationMode::Create,
                accepted_date: "2026-10-10".to_owned(),
                date_resolution: Some(DateResolution::AsShown),
            }),
            Err(ImportPlanningError::DestinationAlreadyExists(_))
        ));
        assert!(matches!(
            ImportPlanningService::review(ImportReviewRequest {
                catalog_path: catalog,
                source_path: source,
                collection_name: "Dolomites".to_owned(),
                destination_mode: DestinationMode::Create,
                accepted_date: String::new(),
                date_resolution: Some(DateResolution::ConvertFromUtc),
            }),
            Err(ImportPlanningError::UnsupportedDateResolution)
        ));
    }
}
