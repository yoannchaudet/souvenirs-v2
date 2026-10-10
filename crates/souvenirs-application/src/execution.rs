use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use souvenirs_domain::catalog::{
    allocate_collection_counters, CanonicalFilename, CanonicalStem, NewItemCounterRequest,
    SourcePathOrderKey,
};
use souvenirs_domain::names::CatalogLabel;
use souvenirs_storage::{
    read_operation_records, CatalogLayout, CatalogLock, CopyExpectation,
    ManifestReplacementRequest, OperationJournal, OperationRecord, RecoveryCopy, RecoveryDirectory,
    RecoveryManifest, RecoveryMode, RecoveryRequest,
};
use time::{macros::format_description, Date, PrimitiveDateTime, Time};

use crate::{
    CatalogService, DateResolution, DestinationMode, ImportPlanningService, ImportReviewRequest,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportExecutionRequest {
    pub catalog_path: PathBuf,
    pub source_path: PathBuf,
    pub collection_name: String,
    pub destination_mode: DestinationMode,
    pub accepted_date: String,
    pub date_resolution: Option<DateResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedImport {
    operation_id: String,
    plan: ExecutionPlan,
}

impl PreparedImport {
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub fn catalog_path(&self) -> &Path {
        &self.plan.catalog_path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationProgress {
    pub completed_units: u64,
    pub total_units: u64,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportExecutionOutcome {
    pub copied_count: usize,
    pub skipped_count: usize,
    pub left_on_source_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOutcome {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportRecoveryMode {
    Resume,
    Cancel,
}

pub struct ImportExecutionService;

impl ImportExecutionService {
    pub fn prepare(
        request: ImportExecutionRequest,
    ) -> Result<PreparedImport, ImportExecutionError> {
        let review = ImportPlanningService::review(ImportReviewRequest {
            catalog_path: request.catalog_path.clone(),
            source_path: request.source_path.clone(),
            collection_name: request.collection_name.clone(),
            destination_mode: request.destination_mode,
            accepted_date: request.accepted_date.clone(),
            date_resolution: request.date_resolution,
        })
        .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
        if !review.can_start {
            return Err(ImportExecutionError::DecisionRequired);
        }

        let accepted_date = Date::parse(
            &request.accepted_date,
            format_description!("[year]-[month]-[day]"),
        )
        .map_err(|_| ImportExecutionError::InvalidDate(request.accepted_date.clone()))?;
        let collection = CatalogLabel::new(&request.collection_name)
            .map_err(|error| ImportExecutionError::InvalidCollection(error.to_string()))?;
        let year = accepted_date.year().to_string();
        let collection_directory = format!("{accepted_date} {collection}");
        let collection_path = request.catalog_path.join(&year).join(&collection_directory);
        let mut source_files = Vec::new();
        collect_media_files(
            &request.source_path,
            &request.source_path,
            &mut source_files,
        )?;
        source_files.sort();
        let accepted_datetime = PrimitiveDateTime::new(
            accepted_date,
            Time::from_hms(12, 0, 0).expect("noon is valid"),
        );
        let allocations = allocate_collection_counters(
            &BTreeMap::from([(
                accepted_date,
                existing_counters(&collection_path, accepted_date)?,
            )]),
            source_files
                .iter()
                .map(|path| NewItemCounterRequest {
                    item_id: path.to_string_lossy().into_owned(),
                    source_path_order_key: SourcePathOrderKey::new(
                        path.components()
                            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
                    ),
                    accepted_datetime,
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
        let counters = allocations
            .into_iter()
            .map(|allocation| (allocation.item_id, allocation.counter))
            .collect::<BTreeMap<_, _>>();

        let mut copies = Vec::with_capacity(source_files.len());
        let mut manifest = existing_manifest(&collection_path.join("SHA256SUMS"))?;
        for (index, relative_source) in source_files.into_iter().enumerate() {
            let source = request.source_path.join(&relative_source);
            let extension = source
                .extension()
                .and_then(|value| value.to_str())
                .ok_or_else(|| ImportExecutionError::InvalidSource(source.clone()))?
                .to_ascii_lowercase();
            let item_id = relative_source.to_string_lossy();
            let stem = CanonicalStem::new(accepted_datetime, counters[item_id.as_ref()])
                .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
            let filename = CanonicalFilename::new(&stem, &extension)
                .map_err(|error| ImportExecutionError::Planning(error.to_string()))?
                .to_string();
            let (digest, size_bytes) = digest_file(&source)?;
            manifest.insert(filename.clone(), digest.clone());
            copies.push(ExecutionCopy {
                step_id: format!("copy-{}", index + 1),
                source,
                destination: PathBuf::from(&year)
                    .join(&collection_directory)
                    .join(filename),
                digest,
                size_bytes,
            });
        }
        if copies.is_empty() {
            return Err(ImportExecutionError::NoSupportedMedia);
        }

        let manifest_path = PathBuf::from(&year)
            .join(&collection_directory)
            .join("SHA256SUMS");
        let manifest_contents = encode_manifest(&manifest);
        let manifest_digest = digest_bytes(&manifest_contents);
        let previous_manifest_digest = if collection_path.join("SHA256SUMS").is_file() {
            Some(digest_file(&collection_path.join("SHA256SUMS"))?.0)
        } else {
            None
        };
        let directories = match request.destination_mode {
            DestinationMode::Create => vec![
                ExecutionDirectory {
                    step_id: "directory-year".to_owned(),
                    path: PathBuf::from(&year),
                    create: !request.catalog_path.join(&year).exists(),
                },
                ExecutionDirectory {
                    step_id: "directory-collection".to_owned(),
                    path: PathBuf::from(&year).join(&collection_directory),
                    create: true,
                },
            ],
            DestinationMode::Existing => Vec::new(),
        };
        let operation_id = operation_id()?;
        Ok(PreparedImport {
            operation_id,
            plan: ExecutionPlan {
                schema_version: 1,
                catalog_path: request.catalog_path,
                source_path: request.source_path,
                collection_name: request.collection_name,
                directories,
                copies,
                manifest: ExecutionManifest {
                    step_id: "manifest".to_owned(),
                    path: manifest_path,
                    previous_digest: previous_manifest_digest,
                    resulting_digest: manifest_digest,
                    contents: manifest_contents,
                },
                left_on_source_count: review.left_on_source_count,
            },
        })
    }

    pub fn execute(
        prepared: PreparedImport,
        mut progress: impl FnMut(OperationProgress),
    ) -> Result<ImportExecutionOutcome, ImportExecutionError> {
        CatalogService::open(&prepared.plan.catalog_path)
            .map_err(|error| ImportExecutionError::Catalog(error.to_string()))?;
        let layout = CatalogLayout::new(&prepared.plan.catalog_path);
        let lock = CatalogLock::acquire(&layout)
            .map_err(|error| ImportExecutionError::Conflict(error.to_string()))?;
        let mut journal =
            OperationJournal::create(&layout, &lock, &prepared.operation_id, &prepared.plan)
                .map_err(|error| ImportExecutionError::Conflict(error.to_string()))?;
        journal
            .record_metadata(
                &layout,
                &lock,
                prepared.plan.collection_name.clone(),
                prepared.plan.source_path.clone(),
                prepared.plan.copies.len(),
            )
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;

        for directory in prepared
            .plan
            .directories
            .iter()
            .filter(|directory| directory.create)
        {
            journal
                .create_destination_directory(&layout, &lock, &directory.step_id, &directory.path)
                .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        }

        let total_units = prepared.plan.copies.len() as u64;
        for (index, copy) in prepared.plan.copies.iter().enumerate() {
            layout
                .copy_verified(
                    &lock,
                    &mut journal,
                    &copy.source,
                    &copy.destination,
                    &copy.step_id,
                    CopyExpectation {
                        sha256: copy.digest.clone(),
                        size_bytes: copy.size_bytes,
                    },
                )
                .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
            progress(OperationProgress {
                completed_units: (index + 1) as u64,
                total_units,
                message: format!("Copied {} of {}", index + 1, total_units),
            });
        }

        layout
            .replace_manifest(
                &lock,
                &mut journal,
                ManifestReplacementRequest {
                    path: &prepared.plan.manifest.path,
                    previous_sha256: prepared.plan.manifest.previous_digest.as_deref(),
                    resulting_sha256: &prepared.plan.manifest.resulting_digest,
                    step_id: &prepared.plan.manifest.step_id,
                },
                &prepared.plan.manifest.contents,
            )
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        journal
            .complete(&layout, &lock)
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        lock.release()
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        Ok(ImportExecutionOutcome {
            copied_count: prepared.plan.copies.len(),
            skipped_count: 0,
            left_on_source_count: prepared.plan.left_on_source_count,
        })
    }
}

pub struct ImportRecoveryService;

impl ImportRecoveryService {
    pub fn recover(
        catalog_path: impl AsRef<Path>,
        operation_id: &str,
        mode: ImportRecoveryMode,
        mut progress: impl FnMut(OperationProgress),
    ) -> Result<(RecoveryOutcome, ImportExecutionOutcome), ImportExecutionError> {
        let catalog_path = catalog_path.as_ref();
        CatalogService::open(catalog_path)
            .map_err(|error| ImportExecutionError::Catalog(error.to_string()))?;
        let layout = CatalogLayout::new(catalog_path);
        let lock = CatalogLock::acquire(&layout)
            .map_err(|error| ImportExecutionError::Conflict(error.to_string()))?;
        let (journal_path, plan, published) = unfinished_plan(&layout, operation_id)?;
        if mode == ImportRecoveryMode::Resume {
            validate_recovery_sources(&layout, &plan, &published)?;
        }
        let (manifest_contents, cancelled_copy_count) = match mode {
            ImportRecoveryMode::Resume => (plan.manifest.contents.clone(), 0),
            ImportRecoveryMode::Cancel => cancellation_manifest(&layout, &plan, &published)?,
        };
        let request = RecoveryRequest {
            mode: match mode {
                ImportRecoveryMode::Resume => RecoveryMode::Resume,
                ImportRecoveryMode::Cancel => RecoveryMode::CancelRemainingCopies,
            },
            directories: plan
                .directories
                .iter()
                .filter(|directory| directory.create)
                .map(|directory| RecoveryDirectory {
                    step_id: directory.step_id.clone(),
                    path: directory.path.clone(),
                })
                .collect(),
            copies: plan
                .copies
                .iter()
                .map(|copy| RecoveryCopy {
                    step_id: copy.step_id.clone(),
                    source: copy.source.clone(),
                    destination: copy.destination.clone(),
                    expected_sha256: copy.digest.clone(),
                    expected_size_bytes: copy.size_bytes,
                })
                .collect(),
            manifests: vec![RecoveryManifest {
                step_id: plan.manifest.step_id.clone(),
                path: plan.manifest.path.clone(),
                previous_sha256: plan.manifest.previous_digest.clone(),
                contents: manifest_contents,
            }],
        };
        let outcome = layout
            .recover_operation(&lock, journal_path, request)
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        lock.release()
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        let copied_count = match outcome {
            souvenirs_storage::RecoveryOutcome::Completed => plan.copies.len(),
            souvenirs_storage::RecoveryOutcome::Cancelled => cancelled_copy_count,
        };
        progress(OperationProgress {
            completed_units: copied_count as u64,
            total_units: plan.copies.len() as u64,
            message: match outcome {
                souvenirs_storage::RecoveryOutcome::Completed => "Recovery completed".to_owned(),
                souvenirs_storage::RecoveryOutcome::Cancelled => "Recovery cancelled".to_owned(),
            },
        });
        Ok((
            match outcome {
                souvenirs_storage::RecoveryOutcome::Completed => RecoveryOutcome::Completed,
                souvenirs_storage::RecoveryOutcome::Cancelled => RecoveryOutcome::Cancelled,
            },
            ImportExecutionOutcome {
                copied_count,
                skipped_count: 0,
                left_on_source_count: plan.left_on_source_count,
            },
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecutionPlan {
    schema_version: u32,
    catalog_path: PathBuf,
    source_path: PathBuf,
    collection_name: String,
    directories: Vec<ExecutionDirectory>,
    copies: Vec<ExecutionCopy>,
    manifest: ExecutionManifest,
    left_on_source_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecutionDirectory {
    step_id: String,
    path: PathBuf,
    create: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecutionCopy {
    step_id: String,
    source: PathBuf,
    destination: PathBuf,
    digest: String,
    size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecutionManifest {
    step_id: String,
    path: PathBuf,
    previous_digest: Option<String>,
    resulting_digest: String,
    contents: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct JournalCopyState {
    staged_path: Option<PathBuf>,
    sha256: Option<String>,
    size_bytes: Option<u64>,
    published: bool,
}

#[derive(Debug)]
pub enum ImportExecutionError {
    Catalog(String),
    Conflict(String),
    DecisionRequired,
    InvalidCollection(String),
    InvalidDate(String),
    InvalidSource(PathBuf),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NoSupportedMedia,
    OperationNotFound(String),
    Planning(String),
    Storage(String),
}

impl ImportExecutionError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Catalog(_) => "catalog_error",
            Self::Conflict(_) => "operation_conflict",
            Self::DecisionRequired => "decision_required",
            Self::InvalidCollection(_) => "invalid_collection",
            Self::InvalidDate(_) => "invalid_date",
            Self::InvalidSource(_) => "source_unavailable",
            Self::Io { .. } => "source_io",
            Self::NoSupportedMedia => "no_supported_media",
            Self::OperationNotFound(_) => "operation_not_found",
            Self::Planning(_) => "invalid_plan",
            Self::Storage(_) => "operation_failed",
        }
    }
}

impl fmt::Display for ImportExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog(message)
            | Self::Conflict(message)
            | Self::InvalidCollection(message)
            | Self::InvalidDate(message)
            | Self::Planning(message)
            | Self::Storage(message) => formatter.write_str(message),
            Self::DecisionRequired => formatter.write_str("The import still requires a decision."),
            Self::InvalidSource(path) => {
                write!(formatter, "The source is unavailable: {}", path.display())
            }
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::NoSupportedMedia => {
                formatter.write_str("The source contains no supported media.")
            }
            Self::OperationNotFound(operation_id) => {
                write!(formatter, "Operation {operation_id} was not found.")
            }
        }
    }
}

fn collect_media_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), ImportExecutionError> {
    let entries = fs::read_dir(directory).map_err(|source| ImportExecutionError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ImportExecutionError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| ImportExecutionError::Io {
                path: entry.path(),
                source,
            })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_media_files(root, &entry.path(), files)?;
        } else if file_type.is_file() && super::media_kind(&entry.path()).is_some() {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .expect("walked source remains below root")
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn unfinished_plan(
    layout: &CatalogLayout,
    operation_id: &str,
) -> Result<(PathBuf, ExecutionPlan, BTreeMap<PathBuf, JournalCopyState>), ImportExecutionError> {
    let mut entries = fs::read_dir(layout.journal_directory())
        .map_err(|source| ImportExecutionError::Io {
            path: layout.journal_directory(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ImportExecutionError::Io {
            path: layout.journal_directory(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("-initialize.jsonl"))
        {
            continue;
        }
        let records = read_operation_records(&path)
            .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
        let Some(OperationRecord::Plan {
            operation_id: recorded,
            plan,
        }) = records.first()
        else {
            continue;
        };
        if recorded == operation_id
            && !matches!(records.last(), Some(OperationRecord::Outcome { .. }))
        {
            let plan: ExecutionPlan = serde_json::from_value(plan.clone())
                .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
            if plan.schema_version != 1 {
                return Err(ImportExecutionError::Storage(format!(
                    "unsupported execution plan schema version {}",
                    plan.schema_version
                )));
            }
            let mut published = BTreeMap::<PathBuf, JournalCopyState>::new();
            for record in &records {
                let OperationRecord::Transition { transition } = record else {
                    continue;
                };
                match transition {
                    souvenirs_storage::DurableTransition::CopyStarted {
                        destination,
                        staged_path,
                        ..
                    } => {
                        published
                            .entry(destination.clone())
                            .or_default()
                            .staged_path = Some(staged_path.clone());
                    }
                    souvenirs_storage::DurableTransition::CopyVerified {
                        destination,
                        staged_path,
                        sha256,
                        bytes_copied,
                        ..
                    } => {
                        let state = published.entry(destination.clone()).or_default();
                        state.staged_path = Some(staged_path.clone());
                        state.sha256 = Some(sha256.clone());
                        state.size_bytes = Some(*bytes_copied);
                    }
                    souvenirs_storage::DurableTransition::CopyPublished {
                        destination,
                        sha256,
                        ..
                    } => {
                        let state = published.entry(destination.clone()).or_default();
                        state.sha256 = Some(sha256.clone());
                        state.published = true;
                    }
                    _ => {}
                }
            }
            return Ok((path, plan, published));
        }
    }
    Err(ImportExecutionError::OperationNotFound(
        operation_id.to_owned(),
    ))
}

fn validate_recovery_sources(
    layout: &CatalogLayout,
    plan: &ExecutionPlan,
    published: &BTreeMap<PathBuf, JournalCopyState>,
) -> Result<(), ImportExecutionError> {
    for copy in &plan.copies {
        if let Some(state) = published.get(&copy.destination) {
            if state.published || verified_copy_can_publish(layout, copy, state)? {
                continue;
            }
        }
        let (digest, size_bytes) = digest_file(&copy.source)?;
        if digest != copy.digest || size_bytes != copy.size_bytes {
            return Err(ImportExecutionError::Storage(format!(
                "source changed after operation preparation: {}",
                copy.source.display()
            )));
        }
    }
    Ok(())
}

fn cancellation_manifest(
    layout: &CatalogLayout,
    plan: &ExecutionPlan,
    published: &BTreeMap<PathBuf, JournalCopyState>,
) -> Result<(Vec<u8>, usize), ImportExecutionError> {
    let mut lines = existing_manifest(&layout.root().join(&plan.manifest.path))?;
    let mut copied_count = 0;
    for copy in &plan.copies {
        let Some(state) = published.get(&copy.destination) else {
            continue;
        };
        let Some(journal_digest) = state.sha256.as_deref() else {
            continue;
        };
        let matches_plan =
            journal_digest == copy.digest && state.size_bytes == Some(copy.size_bytes);
        if state.published {
            let destination = layout.root().join(&copy.destination);
            if !destination.is_file() {
                return Err(ImportExecutionError::Storage(format!(
                    "published destination is missing during cancellation: {}",
                    destination.display()
                )));
            }
            let (actual_digest, actual_size) = digest_file(&destination)?;
            if !matches_plan || actual_digest != journal_digest || actual_size != copy.size_bytes {
                return Err(ImportExecutionError::Storage(format!(
                    "published destination differs from the confirmed plan: {}",
                    destination.display()
                )));
            }
        } else if !verified_copy_can_publish(layout, copy, state)? {
            continue;
        }
        let filename = copy
            .destination
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| ImportExecutionError::InvalidSource(copy.destination.clone()))?;
        lines.insert(filename.to_owned(), journal_digest.to_owned());
        copied_count += 1;
    }
    Ok((encode_manifest(&lines), copied_count))
}

fn verified_copy_can_publish(
    layout: &CatalogLayout,
    copy: &ExecutionCopy,
    state: &JournalCopyState,
) -> Result<bool, ImportExecutionError> {
    let matches_plan = state.sha256.as_deref() == Some(copy.digest.as_str())
        && state.size_bytes == Some(copy.size_bytes);
    if !matches_plan {
        return Ok(false);
    }
    let destination = layout.root().join(&copy.destination);
    if destination.is_file() {
        let (digest, size_bytes) = digest_file(&destination)?;
        return Ok(digest == copy.digest && size_bytes == copy.size_bytes);
    }
    let Some(staged_path) = state.staged_path.as_deref() else {
        return Ok(false);
    };
    let staged_path = layout.root().join(staged_path);
    if !staged_path.is_file() {
        return Ok(false);
    }
    let (digest, size_bytes) = digest_file(&staged_path)?;
    Ok(digest == copy.digest && size_bytes == copy.size_bytes)
}

fn existing_manifest(path: &Path) -> Result<BTreeMap<String, String>, ImportExecutionError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = fs::read_to_string(path).map_err(|source| ImportExecutionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    contents
        .lines()
        .map(|line| {
            line.split_once("  ")
                .map(|(digest, filename)| (filename.to_owned(), digest.to_owned()))
                .ok_or_else(|| ImportExecutionError::Storage("Invalid SHA256SUMS line.".to_owned()))
        })
        .collect()
}

fn existing_counters(
    collection: &Path,
    date: Date,
) -> Result<BTreeSet<NonZeroU32>, ImportExecutionError> {
    if !collection.exists() {
        return Ok(BTreeSet::new());
    }
    let prefix = format!("{date} ");
    let mut counters = BTreeSet::new();
    for entry in fs::read_dir(collection).map_err(|source| ImportExecutionError::Io {
        path: collection.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ImportExecutionError::Io {
            path: collection.to_path_buf(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(counter) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split_whitespace().nth(1))
            .and_then(|value| value.split('.').next())
            .and_then(|value| value.parse::<u32>().ok())
            .and_then(NonZeroU32::new)
        {
            counters.insert(counter);
        }
    }
    Ok(counters)
}

fn encode_manifest(lines: &BTreeMap<String, String>) -> Vec<u8> {
    let mut output = String::new();
    for (filename, digest) in lines {
        writeln!(output, "{digest}  {filename}").expect("writing to a string cannot fail");
    }
    output.into_bytes()
}

fn digest_file(path: &Path) -> Result<(String, u64), ImportExecutionError> {
    let mut file = File::open(path).map_err(|source| ImportExecutionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut size = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| ImportExecutionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn digest_bytes(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
}

fn operation_id() -> Result<String, ImportExecutionError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ImportExecutionError::Storage(error.to_string()))?;
    Ok(format!(
        "import-{}-{}",
        duration.as_secs(),
        duration.subsec_nanos()
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::{
        CopyExpectation, ImportExecutionRequest, ImportExecutionService, ImportRecoveryMode,
        ImportRecoveryService, RecoveryOutcome,
    };
    use crate::{CatalogService, DateResolution, DestinationMode};
    use souvenirs_storage::{
        CatalogLayout, CatalogLock, DurableTransition, OperationJournal, OperationRecord,
    };

    fn append_transitions(
        journal_path: &Path,
        transitions: impl IntoIterator<Item = DurableTransition>,
    ) {
        let mut journal_file = OpenOptions::new().append(true).open(journal_path).unwrap();
        for transition in transitions {
            serde_json::to_writer(
                &mut journal_file,
                &OperationRecord::Transition { transition },
            )
            .unwrap();
            journal_file.write_all(b"\n").unwrap();
        }
        journal_file.sync_all().unwrap();
    }

    #[test]
    fn executes_prepared_import_and_writes_manifest() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let mut progress = Vec::new();

        let outcome =
            ImportExecutionService::execute(prepared, |event| progress.push(event)).unwrap();

        assert_eq!(outcome.copied_count, 2);
        assert_eq!(
            progress
                .iter()
                .map(|event| event.completed_units)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        let collection = fixture
            .catalog
            .join("2026")
            .join("2026-10-10 Recovery Import");
        let manifest = fs::read_to_string(collection.join("SHA256SUMS")).unwrap();
        assert_eq!(manifest.lines().count(), 2);
        assert!(manifest.contains("2026-10-10 12-00-00 001.jpg"));
        assert!(manifest.contains("2026-10-10 12-00-00 002.jpg"));

        let mut existing_request = fixture.request();
        existing_request.destination_mode = DestinationMode::Existing;
        let prepared = ImportExecutionService::prepare(existing_request).unwrap();
        ImportExecutionService::execute(prepared, |_| {}).unwrap();
        let manifest = fs::read_to_string(collection.join("SHA256SUMS")).unwrap();
        assert_eq!(manifest.lines().count(), 4);
        assert!(manifest.contains("2026-10-10 12-00-00 003.jpg"));
        assert!(manifest.contains("2026-10-10 12-00-00 004.jpg"));
    }

    #[test]
    fn recovers_an_import_interrupted_after_a_published_copy() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let layout = CatalogLayout::new(&fixture.catalog);
        let lock = CatalogLock::acquire(&layout).unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, prepared.operation_id(), &prepared.plan)
                .unwrap();
        journal
            .record_metadata(
                &layout,
                &lock,
                prepared.plan.collection_name.clone(),
                prepared.plan.source_path.clone(),
                prepared.plan.copies.len(),
            )
            .unwrap();
        for directory in prepared
            .plan
            .directories
            .iter()
            .filter(|directory| directory.create)
        {
            journal
                .create_destination_directory(&layout, &lock, &directory.step_id, &directory.path)
                .unwrap();
        }
        let first = &prepared.plan.copies[0];
        layout
            .copy_verified(
                &lock,
                &mut journal,
                &first.source,
                &first.destination,
                &first.step_id,
                CopyExpectation {
                    sha256: first.digest.clone(),
                    size_bytes: first.size_bytes,
                },
            )
            .unwrap();
        drop(journal);
        lock.release().unwrap();
        fs::write(
            layout.journal_directory().join(".DS_Store"),
            b"not a journal",
        )
        .unwrap();

        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Resume,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, RecoveryOutcome::Completed);
        assert_eq!(result.copied_count, 2);
        let collection = fixture
            .catalog
            .join("2026")
            .join("2026-10-10 Recovery Import");
        assert_eq!(
            fs::read_to_string(collection.join("SHA256SUMS"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn rejects_recovery_when_an_unpublished_source_changed() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let layout = CatalogLayout::new(&fixture.catalog);
        let lock = CatalogLock::acquire(&layout).unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, prepared.operation_id(), &prepared.plan)
                .unwrap();
        journal
            .record_metadata(
                &layout,
                &lock,
                prepared.plan.collection_name.clone(),
                prepared.plan.source_path.clone(),
                prepared.plan.copies.len(),
            )
            .unwrap();
        drop(journal);
        lock.release().unwrap();
        fs::write(fixture.source.join("one.jpg"), b"changed").unwrap();

        let error = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Resume,
            |_| {},
        )
        .unwrap_err();

        assert!(error.to_string().contains("source changed"));
    }

    #[test]
    fn source_drift_never_publishes_and_cancellation_records_no_stale_entry() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let operation_id = prepared.operation_id().to_owned();
        let destination = fixture.catalog.join(&prepared.plan.copies[0].destination);
        fs::write(fixture.source.join("one.jpg"), b"changed after planning").unwrap();

        let error = ImportExecutionService::execute(prepared, |_| {}).unwrap_err();

        assert!(error
            .to_string()
            .contains("does not match the confirmed plan"));
        assert!(!destination.exists());
        let resume_error = ImportRecoveryService::recover(
            &fixture.catalog,
            &operation_id,
            ImportRecoveryMode::Resume,
            |_| {},
        )
        .unwrap_err();
        assert!(resume_error.to_string().contains("source changed"));
        assert!(!destination.exists());
        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            &operation_id,
            ImportRecoveryMode::Cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome, RecoveryOutcome::Cancelled);
        assert_eq!(result.copied_count, 0);
        assert!(!destination.exists());
        let manifest = fixture
            .catalog
            .join("2026")
            .join("2026-10-10 Recovery Import")
            .join("SHA256SUMS");
        assert_eq!(fs::read_to_string(manifest).unwrap(), "");
        assert!(
            fs::read_dir(CatalogLayout::new(&fixture.catalog).staging_directory())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn cancellation_manifests_a_matching_verified_stage_that_recovery_publishes() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let first = &prepared.plan.copies[0];
        let layout = CatalogLayout::new(&fixture.catalog);
        let lock = CatalogLock::acquire(&layout).unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, prepared.operation_id(), &prepared.plan)
                .unwrap();
        for directory in prepared
            .plan
            .directories
            .iter()
            .filter(|directory| directory.create)
        {
            journal
                .create_destination_directory(&layout, &lock, &directory.step_id, &directory.path)
                .unwrap();
        }
        let journal_path = journal.path().to_path_buf();
        drop(journal);
        lock.release().unwrap();

        let staged_path = PathBuf::from(".souvenirs/staging").join(format!(
            "{}-{}-{}.copy",
            prepared.operation_id().len(),
            prepared.operation_id(),
            first.step_id
        ));
        let source_bytes = fs::read(&first.source).unwrap();
        fs::write(layout.root().join(&staged_path), &source_bytes).unwrap();
        let transitions = [
            DurableTransition::CopyStarted {
                step_id: first.step_id.clone(),
                source: first.source.clone(),
                staged_path: staged_path.clone(),
                destination: first.destination.clone(),
            },
            DurableTransition::CopyVerified {
                step_id: first.step_id.clone(),
                staged_path,
                destination: first.destination.clone(),
                sha256: first.digest.clone(),
                bytes_copied: first.size_bytes,
            },
        ];
        append_transitions(&journal_path, transitions);

        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Cancel,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, RecoveryOutcome::Cancelled);
        assert_eq!(result.copied_count, 1);
        assert_eq!(
            fs::read(layout.root().join(&first.destination)).unwrap(),
            source_bytes
        );
        let manifest =
            fs::read_to_string(layout.root().join(&prepared.plan.manifest.path)).unwrap();
        assert!(manifest.contains(&first.digest));
        assert!(manifest.contains(first.destination.file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn cancellation_does_not_manifest_a_missing_verified_stage() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let first = &prepared.plan.copies[0];
        let layout = CatalogLayout::new(&fixture.catalog);
        let lock = CatalogLock::acquire(&layout).unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, prepared.operation_id(), &prepared.plan)
                .unwrap();
        for directory in prepared
            .plan
            .directories
            .iter()
            .filter(|directory| directory.create)
        {
            journal
                .create_destination_directory(&layout, &lock, &directory.step_id, &directory.path)
                .unwrap();
        }
        let journal_path = journal.path().to_path_buf();
        drop(journal);
        lock.release().unwrap();
        let staged_path = PathBuf::from(".souvenirs/staging").join(format!(
            "{}-{}-{}.copy",
            prepared.operation_id().len(),
            prepared.operation_id(),
            first.step_id
        ));
        append_transitions(
            &journal_path,
            [
                DurableTransition::CopyStarted {
                    step_id: first.step_id.clone(),
                    source: first.source.clone(),
                    staged_path: staged_path.clone(),
                    destination: first.destination.clone(),
                },
                DurableTransition::CopyVerified {
                    step_id: first.step_id.clone(),
                    staged_path,
                    destination: first.destination.clone(),
                    sha256: first.digest.clone(),
                    bytes_copied: first.size_bytes,
                },
            ],
        );

        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Cancel,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, RecoveryOutcome::Cancelled);
        assert_eq!(result.copied_count, 0);
        assert!(!layout.root().join(&first.destination).exists());
        assert_eq!(
            fs::read_to_string(layout.root().join(&prepared.plan.manifest.path)).unwrap(),
            ""
        );
    }

    #[test]
    fn resume_publishes_a_matching_verified_stage_without_the_source() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let first = &prepared.plan.copies[0];
        let layout = CatalogLayout::new(&fixture.catalog);
        let lock = CatalogLock::acquire(&layout).unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, prepared.operation_id(), &prepared.plan)
                .unwrap();
        for directory in prepared
            .plan
            .directories
            .iter()
            .filter(|directory| directory.create)
        {
            journal
                .create_destination_directory(&layout, &lock, &directory.step_id, &directory.path)
                .unwrap();
        }
        let journal_path = journal.path().to_path_buf();
        drop(journal);
        lock.release().unwrap();
        let staged_path = PathBuf::from(".souvenirs/staging").join(format!(
            "{}-{}-{}.copy",
            prepared.operation_id().len(),
            prepared.operation_id(),
            first.step_id
        ));
        let source_bytes = fs::read(&first.source).unwrap();
        fs::write(layout.root().join(&staged_path), &source_bytes).unwrap();
        append_transitions(
            &journal_path,
            [
                DurableTransition::CopyStarted {
                    step_id: first.step_id.clone(),
                    source: first.source.clone(),
                    staged_path: staged_path.clone(),
                    destination: first.destination.clone(),
                },
                DurableTransition::CopyVerified {
                    step_id: first.step_id.clone(),
                    staged_path,
                    destination: first.destination.clone(),
                    sha256: first.digest.clone(),
                    bytes_copied: first.size_bytes,
                },
            ],
        );
        fs::remove_file(&first.source).unwrap();

        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Resume,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, RecoveryOutcome::Completed);
        assert_eq!(result.copied_count, prepared.plan.copies.len());
        assert_eq!(
            fs::read(layout.root().join(&first.destination)).unwrap(),
            source_bytes
        );
    }

    #[test]
    fn cancellation_rejects_a_legacy_published_copy_that_drifted_from_the_plan() {
        let fixture = fixture();
        let prepared = ImportExecutionService::prepare(fixture.request()).unwrap();
        let first = &prepared.plan.copies[0];
        let layout = CatalogLayout::new(&fixture.catalog);
        let lock = CatalogLock::acquire(&layout).unwrap();
        let mut journal =
            OperationJournal::create(&layout, &lock, prepared.operation_id(), &prepared.plan)
                .unwrap();
        for directory in prepared
            .plan
            .directories
            .iter()
            .filter(|directory| directory.create)
        {
            journal
                .create_destination_directory(&layout, &lock, &directory.step_id, &directory.path)
                .unwrap();
        }
        let journal_path = journal.path().to_path_buf();
        drop(journal);
        lock.release().unwrap();

        let drifted = b"legacy drifted bytes";
        fs::write(layout.root().join(&first.destination), drifted).unwrap();
        let drifted_digest = super::digest_bytes(drifted);
        let staged_path = PathBuf::from(".souvenirs/staging/legacy-drift.copy");
        let transitions = [
            DurableTransition::CopyStarted {
                step_id: first.step_id.clone(),
                source: first.source.clone(),
                staged_path: staged_path.clone(),
                destination: first.destination.clone(),
            },
            DurableTransition::CopyVerified {
                step_id: first.step_id.clone(),
                staged_path: staged_path.clone(),
                destination: first.destination.clone(),
                sha256: drifted_digest.clone(),
                bytes_copied: drifted.len() as u64,
            },
            DurableTransition::CopyPublicationStarted {
                step_id: first.step_id.clone(),
                staged_path,
                destination: first.destination.clone(),
                sha256: drifted_digest.clone(),
            },
            DurableTransition::CopyPublished {
                step_id: first.step_id.clone(),
                destination: first.destination.clone(),
                sha256: drifted_digest,
            },
        ];
        append_transitions(&journal_path, transitions);

        let error = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Cancel,
            |_| {},
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("published destination differs from the confirmed plan"));
        assert!(!layout.root().join(&prepared.plan.manifest.path).exists());
    }

    struct Fixture {
        _temporary: tempfile::TempDir,
        catalog: std::path::PathBuf,
        source: std::path::PathBuf,
    }

    impl Fixture {
        fn request(&self) -> ImportExecutionRequest {
            ImportExecutionRequest {
                catalog_path: self.catalog.clone(),
                source_path: self.source.clone(),
                collection_name: "Recovery Import".to_owned(),
                destination_mode: DestinationMode::Create,
                accepted_date: "2026-10-10".to_owned(),
                date_resolution: Some(DateResolution::AsShown),
            }
        }
    }

    fn fixture() -> Fixture {
        let temporary = tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let source = temporary.path().join("source");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("one.jpg"), b"one").unwrap();
        fs::write(source.join("two.jpg"), b"two").unwrap();
        CatalogService::create(&catalog, "Test Catalog").unwrap();
        Fixture {
            _temporary: temporary,
            catalog,
            source,
        }
    }
}
