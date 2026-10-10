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
use souvenirs_domain::plan::PlanFileRole;
use souvenirs_storage::{
    read_operation_records, CatalogLayout, CatalogLock, CopyExpectation,
    ManifestReplacementRequest, OperationJournal, OperationRecord, RecoveryCopy, RecoveryDirectory,
    RecoveryManifest, RecoveryMode, RecoveryRequest,
};
use time::Date;

use crate::{directory_identity, planning::PlannedImport, CatalogService, SourceSnapshotEntry};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportExecutionRequest {
    pub reviewed_plan: String,
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
        let envelope: ReviewedPlanEnvelope = serde_json::from_str(&request.reviewed_plan)
            .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
        if envelope.schema_version != 1 {
            return Err(ImportExecutionError::Planning(format!(
                "unsupported reviewed plan schema version {}",
                envelope.schema_version
            )));
        }
        let payload = serde_json::to_vec(&envelope.plan)
            .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
        if digest_bytes(&payload) != envelope.digest {
            return Err(ImportExecutionError::Planning(
                "reviewed plan digest does not match its contents".to_owned(),
            ));
        }
        validate_reviewed_plan(&envelope.plan)?;
        let operation_id = operation_id()?;
        Ok(PreparedImport {
            operation_id,
            plan: envelope.plan,
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

pub(crate) fn encode_reviewed_plan(
    planned: &PlannedImport,
) -> Result<String, ImportExecutionError> {
    if !planned.review.can_start {
        return Err(ImportExecutionError::DecisionRequired);
    }
    let mut copies = Vec::new();
    for item in &planned.items {
        for file in &item.files {
            copies.push(ExecutionCopy {
                step_id: format!("copy-{}", copies.len() + 1),
                source: file.source.clone(),
                destination: file.destination.clone(),
                digest: file
                    .digest
                    .clone()
                    .expect("review planning hashes every transferred file"),
                size_bytes: file.size_bytes,
                manifested: file.role != PlanFileRole::Sidecar,
                role: Some(file.role),
                accepted_datetime: Some(item.accepted_datetime.clone()),
                date_source: Some(item.date_source),
            });
        }
    }
    if copies.is_empty() {
        return Err(ImportExecutionError::NoSupportedMedia);
    }
    let plan = ExecutionPlan {
        schema_version: 1,
        catalog_path: planned.catalog_path.clone(),
        source_path: planned.source_path.clone(),
        source_snapshot: planned.source_snapshot.clone(),
        exclusions: planned
            .source_snapshot
            .iter()
            .filter(|entry| {
                !copies.iter().any(|copy| {
                    copy.source
                        .strip_prefix(&planned.source_path)
                        .is_ok_and(|path| path == entry.path)
                })
            })
            .map(|entry| entry.path.clone())
            .collect(),
        collection_name: planned.collection_name.clone(),
        directories: planned
            .directories
            .iter()
            .enumerate()
            .map(|(index, directory)| ExecutionDirectory {
                step_id: format!("directory-{}", index + 1),
                path: directory.path.clone(),
                create: directory.create,
                identity: directory.identity.clone(),
            })
            .collect(),
        copies,
        manifest: ExecutionManifest {
            step_id: "manifest".to_owned(),
            path: planned.manifest.path.clone(),
            previous_digest: planned.manifest.previous_digest.clone(),
            resulting_digest: planned.manifest.resulting_digest.clone(),
            contents: planned.manifest.contents.clone(),
        },
        left_on_source_count: planned.review.left_on_source_count,
    };
    let payload = serde_json::to_vec(&plan)
        .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
    serde_json::to_string(&ReviewedPlanEnvelope {
        schema_version: 1,
        digest: digest_bytes(&payload),
        plan,
    })
    .map_err(|error| ImportExecutionError::Planning(error.to_string()))
}

const MANIFEST_CHANGED: &str =
    "The destination manifest changed after review. Review the import again.";

fn validate_reviewed_plan(plan: &ExecutionPlan) -> Result<(), ImportExecutionError> {
    CatalogService::open(&plan.catalog_path)
        .map_err(|error| ImportExecutionError::Catalog(error.to_string()))?;
    if source_snapshot(&plan.source_path)? != plan.source_snapshot {
        return Err(ImportExecutionError::Conflict(
            "The source changed after review. Review the import again.".to_owned(),
        ));
    }
    for directory in &plan.directories {
        let path = plan.catalog_path.join(&directory.path);
        let matches = path.metadata().is_ok_and(|metadata| {
            metadata.is_dir()
                && directory
                    .identity
                    .as_ref()
                    .map_or(true, |identity| directory_identity(&metadata) == *identity)
        });
        if (directory.create && path.exists()) || (!directory.create && !matches) {
            return Err(ImportExecutionError::Conflict(
                "The catalog changed after review. Review the import again.".to_owned(),
            ));
        }
    }
    if plan
        .copies
        .iter()
        .any(|copy| plan.catalog_path.join(&copy.destination).exists())
    {
        return Err(ImportExecutionError::Conflict(
            "A planned destination is no longer free. Review the import again.".to_owned(),
        ));
    }
    let manifest_path = plan.catalog_path.join(&plan.manifest.path);
    match &plan.manifest.previous_digest {
        Some(expected) => {
            if !manifest_path.is_file() {
                return Err(ImportExecutionError::Conflict(MANIFEST_CHANGED.to_owned()));
            }
            let (actual, _) = digest_file(&manifest_path)?;
            if &actual != expected {
                return Err(ImportExecutionError::Conflict(MANIFEST_CHANGED.to_owned()));
            }
        }
        None if manifest_path.exists() => {
            return Err(ImportExecutionError::Conflict(MANIFEST_CHANGED.to_owned()));
        }
        None => {}
    }
    Ok(())
}

fn source_snapshot(root: &Path) -> Result<Vec<SourceSnapshotEntry>, ImportExecutionError> {
    if !root.is_dir() {
        return Err(ImportExecutionError::InvalidSource(root.to_path_buf()));
    }
    let mut paths = Vec::new();
    crate::collect_source_files(root, &mut paths)
        .map_err(|error| ImportExecutionError::Planning(error.to_string()))?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .expect("collected source remains below root")
                .to_path_buf();
            let (sha256, size_bytes) = digest_file(&path)?;
            Ok(SourceSnapshotEntry {
                path: relative,
                size_bytes,
                sha256,
            })
        })
        .collect()
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
struct ReviewedPlanEnvelope {
    schema_version: u32,
    digest: String,
    plan: ExecutionPlan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecutionPlan {
    schema_version: u32,
    catalog_path: PathBuf,
    source_path: PathBuf,
    #[serde(default)]
    source_snapshot: Vec<SourceSnapshotEntry>,
    #[serde(default)]
    exclusions: Vec<PathBuf>,
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
    #[serde(default)]
    identity: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecutionCopy {
    step_id: String,
    source: PathBuf,
    destination: PathBuf,
    digest: String,
    size_bytes: u64,
    #[serde(default = "default_manifested")]
    manifested: bool,
    #[serde(default)]
    role: Option<PlanFileRole>,
    #[serde(default)]
    accepted_datetime: Option<String>,
    #[serde(default)]
    date_source: Option<crate::ImportDateSource>,
}

const fn default_manifested() -> bool {
    true
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
        if copy.manifested {
            let filename = copy
                .destination
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| ImportExecutionError::InvalidSource(copy.destination.clone()))?;
            lines.insert(filename.to_owned(), journal_digest.to_owned());
        }
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

pub(crate) fn existing_manifest(
    path: &Path,
) -> Result<BTreeMap<String, String>, ImportExecutionError> {
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

pub(crate) fn existing_counters(
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

pub(crate) fn encode_manifest(lines: &BTreeMap<String, String>) -> Vec<u8> {
    let mut output = String::new();
    for (filename, digest) in lines {
        writeln!(output, "{digest}  {filename}").expect("writing to a string cannot fail");
    }
    output.into_bytes()
}

pub(crate) fn digest_file(path: &Path) -> Result<(String, u64), ImportExecutionError> {
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

pub(crate) fn digest_bytes(contents: &[u8]) -> String {
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
        let prepared = prep(&fixture, DestinationMode::Create);
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
        assert!(manifest.contains("2026-10-10 10-11-12 001.jpg"));
        assert!(manifest.contains("2026-10-10 10-11-13 002.jpg"));

        let prepared = prep(&fixture, DestinationMode::Existing);
        ImportExecutionService::execute(prepared, |_| {}).unwrap();
        let manifest = fs::read_to_string(collection.join("SHA256SUMS")).unwrap();
        assert_eq!(manifest.lines().count(), 4);
        assert!(manifest.contains("2026-10-10 10-11-12 003.jpg"));
        assert!(manifest.contains("2026-10-10 10-11-13 004.jpg"));
    }

    #[test]
    fn executes_compound_item_with_shared_stem_and_original_only_manifest() {
        let fixture = compound_fixture();
        let prepared = prep(&fixture, DestinationMode::Create);
        let outcome = ImportExecutionService::execute(prepared, |_| {}).unwrap();

        assert_eq!(outcome.copied_count, 3);
        assert_eq!(outcome.left_on_source_count, 0);
        let collection = fixture
            .catalog
            .join("2026")
            .join("2026-10-10 Recovery Import");
        let raw = collection.join("2026-10-10 12-00-00 001.CR3");
        let jpeg = collection.join("2026-10-10 12-00-00 001.JPG");
        let sidecar = collection.join("2026-10-10 12-00-00 001.JPG.xmp");
        assert!(raw.is_file());
        assert!(jpeg.is_file());
        assert!(sidecar.is_file());

        let manifest = fs::read_to_string(collection.join("SHA256SUMS")).unwrap();
        assert_eq!(manifest.lines().count(), 2);
        assert!(manifest.contains(&format!(
            "{}  {}",
            super::digest_file(&raw).unwrap().0,
            raw.file_name().unwrap().to_string_lossy()
        )));
        assert!(manifest.contains(&format!(
            "{}  {}",
            super::digest_file(&jpeg).unwrap().0,
            jpeg.file_name().unwrap().to_string_lossy()
        )));
        assert!(!manifest.contains(".xmp"));

        let prepared = prep(&fixture, DestinationMode::Existing);
        ImportExecutionService::execute(prepared, |_| {}).unwrap();
        assert!(collection.join("2026-10-10 12-00-00 002.CR3").is_file());
        assert!(collection.join("2026-10-10 12-00-00 002.JPG").is_file());
        assert!(collection.join("2026-10-10 12-00-00 002.JPG.xmp").is_file());
    }

    #[test]
    fn cancellation_keeps_compound_sidecar_out_of_manifest() {
        let fixture = compound_fixture();
        let prepared = prep(&fixture, DestinationMode::Create);
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
        for copy in &prepared.plan.copies {
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
                .unwrap();
        }
        drop(journal);
        lock.release().unwrap();

        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            prepared.operation_id(),
            ImportRecoveryMode::Cancel,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, RecoveryOutcome::Cancelled);
        assert_eq!(result.copied_count, 3);
        let manifest =
            fs::read_to_string(layout.root().join(&prepared.plan.manifest.path)).unwrap();
        assert_eq!(manifest.lines().count(), 2);
        assert!(!manifest.contains(".xmp"));
    }

    #[test]
    fn compound_sidecar_drift_is_not_published_or_manifested() {
        let fixture = compound_fixture();
        let prepared = prep(&fixture, DestinationMode::Create);
        let operation_id = prepared.operation_id().to_owned();
        let sidecar = prepared
            .plan
            .copies
            .iter()
            .find(|copy| !copy.manifested)
            .unwrap();
        let sidecar_destination = fixture.catalog.join(&sidecar.destination);
        fs::write(
            fixture.source.join("IMG_20261010_120000.JPG.xmp"),
            b"changed",
        )
        .unwrap();

        let error = ImportExecutionService::execute(prepared, |_| {}).unwrap_err();

        assert!(error
            .to_string()
            .contains("does not match the confirmed plan"));
        assert!(!sidecar_destination.exists());
        let resume_error = ImportRecoveryService::recover(
            &fixture.catalog,
            &operation_id,
            ImportRecoveryMode::Resume,
            |_| {},
        )
        .unwrap_err();
        assert!(resume_error.to_string().contains("source changed"));
        let (outcome, result) = ImportRecoveryService::recover(
            &fixture.catalog,
            &operation_id,
            ImportRecoveryMode::Cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome, RecoveryOutcome::Cancelled);
        assert_eq!(result.copied_count, 2);
        let manifest = fs::read_to_string(
            fixture
                .catalog
                .join("2026/2026-10-10 Recovery Import/SHA256SUMS"),
        )
        .unwrap();
        assert_eq!(manifest.lines().count(), 2);
        assert!(!manifest.contains(".xmp"));
    }

    #[test]
    fn compound_sidecar_destination_is_never_overwritten() {
        let fixture = compound_fixture();
        let collection = fixture
            .catalog
            .join("2026")
            .join("2026-10-10 Recovery Import");
        fs::create_dir_all(&collection).unwrap();
        let prepared = prep(&fixture, DestinationMode::Existing);
        let sidecar = prepared
            .plan
            .copies
            .iter()
            .find(|copy| !copy.manifested)
            .unwrap();
        let sidecar_destination = fixture.catalog.join(&sidecar.destination);
        fs::write(&sidecar_destination, b"existing sidecar").unwrap();

        let error = ImportExecutionService::execute(prepared, |_| {}).unwrap_err();

        assert!(error.to_string().contains("occupied destination"));
        assert_eq!(fs::read(sidecar_destination).unwrap(), b"existing sidecar");
    }

    #[test]
    fn legacy_execution_plan_defaults_copies_to_manifested() {
        let fixture = fixture();
        let prepared = prep(&fixture, DestinationMode::Create);
        let mut value = serde_json::to_value(&prepared.plan).unwrap();
        value.as_object_mut().unwrap().remove("source_snapshot");
        for copy in value["copies"].as_array_mut().unwrap() {
            copy.as_object_mut().unwrap().remove("manifested");
        }

        let legacy: super::ExecutionPlan = serde_json::from_value(value).unwrap();

        assert!(legacy.copies.iter().all(|copy| copy.manifested));
    }

    #[test]
    fn recovers_an_import_interrupted_after_a_published_copy() {
        let fixture = fixture();
        let prepared = prep(&fixture, DestinationMode::Create);
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
        let prepared = prep(&fixture, DestinationMode::Create);
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
        fs::write(fixture.source.join("IMG_20261010_101112.jpg"), b"changed").unwrap();

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
        let prepared = prep(&fixture, DestinationMode::Create);
        let operation_id = prepared.operation_id().to_owned();
        let destination = fixture.catalog.join(&prepared.plan.copies[0].destination);
        fs::write(
            fixture.source.join("IMG_20261010_101112.jpg"),
            b"changed after planning",
        )
        .unwrap();

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
        let prepared = prep(&fixture, DestinationMode::Create);
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
        let prepared = prep(&fixture, DestinationMode::Create);
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
        let prepared = prep(&fixture, DestinationMode::Create);
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
        let prepared = prep(&fixture, DestinationMode::Create);
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

    #[test]
    fn reviewed_plan_rejects_source_drift_before_journaling() {
        for change in ["add", "remove", "change"] {
            let fixture = fixture();
            let path = fixture.source.join("IMG_20261010_101113.jpg");
            if change == "add" {
                fs::remove_file(&path).unwrap();
            }
            let request = fixture.request(DestinationMode::Create);
            match change {
                "add" => fs::write(path, b"two").unwrap(),
                "remove" => fs::remove_file(path).unwrap(),
                _ => fs::write(path, b"changed").unwrap(),
            }
            let error = ImportExecutionService::prepare(request).unwrap_err();
            assert_eq!(error.code(), "operation_conflict");
            assert_eq!(import_journal_count(&fixture), 0);
        }
    }

    fn import_journal_count(fixture: &Fixture) -> usize {
        fs::read_dir(CatalogLayout::new(&fixture.catalog).journal_directory())
            .unwrap()
            .flatten()
            .filter(|entry| {
                !entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with("-initialize.jsonl")
            })
            .count()
    }

    struct Fixture {
        _temporary: tempfile::TempDir,
        catalog: std::path::PathBuf,
        source: std::path::PathBuf,
    }

    impl Fixture {
        fn request(&self, destination_mode: DestinationMode) -> ImportExecutionRequest {
            let review = crate::ImportPlanningService::review(crate::ImportReviewRequest {
                catalog_path: self.catalog.clone(),
                source_path: self.source.clone(),
                collection_name: "Recovery Import".to_owned(),
                destination_mode,
                accepted_date: "2026-10-10".to_owned(),
                date_resolution: Some(DateResolution::AsShown),
            })
            .unwrap();
            ImportExecutionRequest {
                reviewed_plan: review.reviewed_plan.unwrap(),
            }
        }
    }

    fn prep(fixture: &Fixture, mode: DestinationMode) -> super::PreparedImport {
        ImportExecutionService::prepare(fixture.request(mode)).unwrap()
    }

    fn fixture() -> Fixture {
        let temporary = tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let source = temporary.path().join("source");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("IMG_20261010_101112.jpg"), b"one").unwrap();
        fs::write(source.join("IMG_20261010_101113.jpg"), b"two").unwrap();
        CatalogService::create(&catalog, "Test Catalog").unwrap();
        Fixture {
            _temporary: temporary,
            catalog,
            source,
        }
    }

    fn compound_fixture() -> Fixture {
        let temporary = tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let source = temporary.path().join("source");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("IMG_20261010_120000.CR3"), b"raw").unwrap();
        fs::write(source.join("IMG_20261010_120000.JPG"), b"jpeg").unwrap();
        fs::write(source.join("IMG_20261010_120000.JPG.xmp"), b"xmp").unwrap();
        CatalogService::create(&catalog, "Test Catalog").unwrap();
        Fixture {
            _temporary: temporary,
            catalog,
            source,
        }
    }
}
