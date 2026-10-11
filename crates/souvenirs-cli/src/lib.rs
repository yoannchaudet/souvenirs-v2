use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{error::ErrorKind, Args, Parser, Subcommand, ValueEnum};
use schemars::{schema::RootSchema, schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use souvenirs_domain::catalog::{
    allocate_collection_counters, year_directory, CanonicalFilename, CanonicalStem,
    DatedCollectionRange, NewItemCounterRequest,
};
use souvenirs_domain::names::CatalogLabel;
use souvenirs_domain::plan::{
    constrain_import_plan_schema, CatalogSnapshot, ContentDigest, ImportPlan, ManifestFingerprint,
    PlanDateTime, PlanDateTimeProvenance, PlanDocument, PlanFile, PlanFileRole, PlanId, PlanItem,
    PlanItemDisposition, PlanItemFinding, PlanStep, PlanStepKind, PlanTimestamp, SourceSnapshot,
    UnhandledContent, UnhandledContentFinding, IMPORT_PLAN_SCHEMA_VERSION,
};
use souvenirs_domain::relationships::{
    resolve_directory_relationships, sidecar_suffix_for_item, RelationshipFinding,
    RelationshipStatus,
};
use souvenirs_domain::source::{
    CameraIdentity, OriginalRole, SourceFile, SourceFileId, SourceFileKind, SourceMetadata,
    SourcePath,
};
use souvenirs_storage::{
    is_operation_journal_file_name, CatalogConfig, CatalogLayout, CatalogLock, CatalogPathError,
    ManifestReplacementRequest, OperationJournal, OperationOutcome, OperationRecord,
    PortableDestinationObserver, RecoveryCopy, RecoveryDirectory, RecoveryManifest, RecoveryMode,
    RecoveryOutcome, RecoveryRequest, FORMAT_VERSION,
};
use time::{Date, Month, PrimitiveDateTime, Time};

const SOURCE_SCAN_SCHEMA_VERSION: u32 = 2;

pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_NOT_FOUND: u8 = 3;
pub const EXIT_CONFLICT: u8 = 4;
pub const EXIT_INVALID_DATA: u8 = 5;
pub const EXIT_IO: u8 = 6;
pub const EXIT_OPERATION_FAILED: u8 = 7;

#[derive(Debug, Parser)]
#[command(name = "souvenirs", version, disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Catalog(CatalogArgs),
    Import(ImportArgs),
    Operation(OperationArgs),
    Integrity(IntegrityArgs),
}

#[derive(Debug, Args)]
struct CatalogArgs {
    #[command(subcommand)]
    command: CatalogCommand,
}

#[derive(Debug, Subcommand)]
enum CatalogCommand {
    Initialize {
        #[arg(long)]
        catalog: PathBuf,
    },
    Inspect {
        #[arg(long)]
        catalog: PathBuf,
    },
}

#[derive(Debug, Args)]
struct ImportArgs {
    #[command(subcommand)]
    command: ImportCommand,
}

#[derive(Debug, Subcommand)]
enum ImportCommand {
    Scan {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Plan {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        collection: String,
        #[arg(long, value_enum)]
        destination_mode: DestinationMode,
        #[arg(long, value_parser = parse_datetime)]
        accepted_at: PrimitiveDateTime,
        #[arg(long)]
        confirm: bool,
    },
    Execute {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        progress: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DestinationMode {
    Create,
    Existing,
}

#[derive(Debug, Args)]
struct OperationArgs {
    #[command(subcommand)]
    command: OperationCommand,
}

#[derive(Debug, Subcommand)]
enum OperationCommand {
    Status {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        operation_id: String,
    },
    Recover {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        operation_id: String,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long, value_enum, default_value = "resume")]
        mode: RecoveryChoice,
        #[arg(long)]
        progress: Option<PathBuf>,
    },
    Cancel {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        operation_id: String,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        progress: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RecoveryChoice {
    Resume,
    Cancel,
}

#[derive(Debug, Args)]
struct IntegrityArgs {
    #[command(subcommand)]
    command: IntegrityCommand,
}

#[derive(Debug, Subcommand)]
enum IntegrityCommand {
    Check {
        #[arg(long)]
        catalog: PathBuf,
    },
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResultEnvelope<T> {
    pub version: u32,
    pub command: &'static str,
    pub result: T,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    pub version: u32,
    pub error: StructuredError,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceScan {
    pub schema_version: u32,
    pub source_root: PathBuf,
    pub source_id: String,
    pub fingerprint: ContentDigest,
    pub files: Vec<ScannedFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScannedFile {
    pub id: String,
    pub path: Vec<String>,
    pub digest: ContentDigest,
    pub size_bytes: u64,
    pub kind: ScannedFileKind,
    #[serde(default, skip_serializing_if = "ScannedMetadata::is_empty")]
    pub metadata: ScannedMetadata,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScannedMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apple_content_identifier: Option<String>,
}

impl ScannedMetadata {
    fn is_empty(&self) -> bool {
        self.apple_content_identifier.is_none()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScannedFileKind {
    Media,
    Sidecar,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPlan {
    #[schemars(range(min = 1, max = 2))]
    pub schema_version: u32,
    pub confirmed: bool,
    #[schemars(with = "String")]
    pub source_root: PathBuf,
    pub import_plan: ImportPlan,
}

pub fn stored_plan_schema() -> RootSchema {
    stored_plan_schema_for_version(IMPORT_PLAN_SCHEMA_VERSION)
        .expect("current stored plan schema version is supported")
}

pub fn stored_plan_schema_for_version(version: u32) -> Result<RootSchema, String> {
    let mut schema = schema_for!(StoredPlan);
    constrain_import_plan_schema(&mut schema, version).map_err(|error| error.to_string())?;
    let number = schema
        .schema
        .object
        .as_mut()
        .and_then(|object| object.properties.get_mut("schema_version"))
        .and_then(|schema| match schema {
            schemars::schema::Schema::Object(schema) => schema.number.as_mut(),
            schemars::schema::Schema::Bool(_) => None,
        })
        .ok_or_else(|| "stored plan schema version field is missing".to_owned())?;
    number.minimum = Some(f64::from(version));
    number.maximum = Some(f64::from(version));
    Ok(schema)
}

#[derive(Debug, Serialize)]
struct CatalogInitialized {
    catalog: PathBuf,
    format_version: u32,
}

#[derive(Debug, Serialize)]
struct CatalogInspection {
    catalog: PathBuf,
    catalog_id: String,
    format_version: u32,
    journal_count: usize,
    unfinished_operation_count: usize,
    manifest_count: usize,
    managed_file_count: usize,
}

#[derive(Debug, Serialize)]
struct ScanResult {
    snapshot: PathBuf,
    source_id: String,
    fingerprint: ContentDigest,
    file_count: usize,
    media_count: usize,
    sidecar_count: usize,
    unknown_count: usize,
}

#[derive(Debug, Serialize)]
struct PlanResult {
    plan: PathBuf,
    plan_id: String,
    digest: ContentDigest,
    confirmed: bool,
    item_count: usize,
    transfer_count: usize,
    unhandled_count: usize,
    required_space_bytes: u64,
}

#[derive(Debug, Serialize)]
struct ExecutionResult {
    operation_id: String,
    outcome: &'static str,
    completed_steps: usize,
}

#[derive(Debug, Serialize)]
struct OperationStatusResult {
    operation_id: String,
    state: &'static str,
    transition_count: usize,
    journal: PathBuf,
}

#[derive(Debug, Serialize)]
struct RecoveryResult {
    operation_id: String,
    outcome: &'static str,
}

#[derive(Debug, Serialize)]
struct IntegrityResult {
    catalog: PathBuf,
    valid: bool,
    coverage_complete: bool,
    checked_files: usize,
    checked_manifests: usize,
    findings: Vec<IntegrityFinding>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum IntegrityFinding {
    MissingManifest {
        collection: PathBuf,
    },
    MissingFile {
        path: PathBuf,
    },
    UnexpectedFile {
        path: PathBuf,
    },
    DigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    UnreadableFile {
        path: PathBuf,
        error: String,
    },
    UnreadableCollection {
        collection: PathBuf,
        error: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ProgressEvent {
    version: u32,
    operation_id: String,
    sequence: u32,
    event: &'static str,
    step: Option<u32>,
}

struct ValidatedProgressPath(Option<PathBuf>);

struct ProgressWriter {
    file: Option<File>,
    operation_id: String,
    sequence: u32,
    terminal: bool,
}

#[derive(Debug)]
struct CliError {
    exit_code: u8,
    code: &'static str,
    message: String,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self::new(EXIT_USAGE, "usage", message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(EXIT_NOT_FOUND, "not_found", message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(EXIT_CONFLICT, "conflict", message)
    }

    fn invalid_data(message: impl Into<String>) -> Self {
        Self::new(EXIT_INVALID_DATA, "invalid_data", message)
    }

    fn io(message: impl Into<String>) -> Self {
        Self::new(EXIT_IO, "io", message)
    }

    fn operation(message: impl Into<String>) -> Self {
        Self::new(EXIT_OPERATION_FAILED, "operation_failed", message)
    }

    fn new(exit_code: u8, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            code,
            message: message.into(),
        }
    }
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return ExitCode::SUCCESS;
            }
            let structured = CliError::usage(error.to_string());
            write_error(&structured);
            return ExitCode::from(structured.exit_code);
        }
    };

    match execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            write_error(&error);
            ExitCode::from(error.exit_code)
        }
    }
}

fn execute(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Catalog(args) => match args.command {
            CatalogCommand::Initialize { catalog } => catalog_initialize(&catalog),
            CatalogCommand::Inspect { catalog } => catalog_inspect(&catalog),
        },
        Command::Import(args) => match args.command {
            ImportCommand::Scan { source, output } => import_scan(&source, &output),
            ImportCommand::Plan {
                catalog,
                snapshot,
                output,
                collection,
                destination_mode,
                accepted_at,
                confirm,
            } => import_plan(
                &catalog,
                &snapshot,
                &output,
                &collection,
                destination_mode,
                accepted_at,
                confirm,
            ),
            ImportCommand::Execute {
                catalog,
                plan,
                progress,
            } => import_execute(&catalog, &plan, progress.as_deref()),
        },
        Command::Operation(args) => match args.command {
            OperationCommand::Status {
                catalog,
                operation_id,
            } => operation_status(&catalog, &operation_id),
            OperationCommand::Recover {
                catalog,
                operation_id,
                plan,
                mode,
                progress,
            } => operation_recover(&catalog, &operation_id, &plan, mode, progress.as_deref()),
            OperationCommand::Cancel {
                catalog,
                operation_id,
                plan,
                progress,
            } => operation_recover(
                &catalog,
                &operation_id,
                &plan,
                RecoveryChoice::Cancel,
                progress.as_deref(),
            ),
        },
        Command::Integrity(args) => match args.command {
            IntegrityCommand::Check { catalog } => integrity_check(&catalog),
        },
    }
}

fn catalog_initialize(catalog: &Path) -> Result<(), CliError> {
    if !catalog.is_dir() {
        return Err(CliError::not_found(format!(
            "catalog root is not a directory: {}",
            catalog.display()
        )));
    }
    let pending = CatalogLayout::new(catalog)
        .has_pending_initialization()
        .map_err(|error| CliError::conflict(error.to_string()))?;
    if !pending && require_catalog(catalog).is_ok() {
        return Err(CliError::conflict(format!(
            "catalog is already initialized: {}",
            catalog.display()
        )));
    }
    CatalogLayout::initialize(catalog, &CatalogConfig::default())
        .map_err(|error| CliError::conflict(error.to_string()))?;
    write_result(
        "catalog.initialize",
        CatalogInitialized {
            catalog: canonical_path(catalog)?,
            format_version: FORMAT_VERSION,
        },
    )
}

fn catalog_inspect(catalog: &Path) -> Result<(), CliError> {
    if !catalog.is_dir() {
        return Err(CliError::not_found(format!(
            "initialized catalog not found: {}",
            catalog.display()
        )));
    }
    CatalogLayout::resume_initialization(catalog)
        .map_err(|error| CliError::conflict(error.to_string()))?;
    let layout = require_catalog(catalog)?;
    let config = fs::read_to_string(layout.config_path())
        .map_err(|error| CliError::io(format!("read catalog configuration: {error}")))?;
    let format_version = parse_format_version(&config)?;
    let journals = jsonl_files(&layout.journal_directory())?;
    let unfinished_operation_count = journals
        .iter()
        .filter(|path| journal_is_unfinished(path))
        .count();
    let (manifest_count, managed_file_count) = inspect_managed_files(layout.root())?;
    let catalog = canonical_path(layout.root())?;

    write_result(
        "catalog.inspect",
        CatalogInspection {
            catalog_id: digest_text(&catalog.to_string_lossy()),
            catalog,
            format_version,
            journal_count: journals.len(),
            unfinished_operation_count,
            manifest_count,
            managed_file_count,
        },
    )
}

fn import_scan(source: &Path, output: &Path) -> Result<(), CliError> {
    if !source.is_dir() {
        return Err(CliError::not_found(format!(
            "source root is not a directory: {}",
            source.display()
        )));
    }
    ensure_output_absent(output)?;
    let source_root = canonical_path(source)?;
    let mut paths = Vec::new();
    collect_files(&source_root, &source_root, &mut paths)?;
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let absolute = join_components(&source_root, &path);
        files.push(scan_file(&absolute, path, SOURCE_SCAN_SCHEMA_VERSION)?);
    }
    let fingerprint = scan_fingerprint(&files)?;
    let source_id = digest_text(&source_root.to_string_lossy());
    let scan = SourceScan {
        schema_version: SOURCE_SCAN_SCHEMA_VERSION,
        source_root,
        source_id: source_id.clone(),
        fingerprint: fingerprint.clone(),
        files,
    };
    write_new_json(output, &scan)?;

    write_result(
        "import.scan",
        ScanResult {
            snapshot: canonical_output_path(output)?,
            source_id,
            fingerprint,
            file_count: scan.files.len(),
            media_count: scan
                .files
                .iter()
                .filter(|file| file.kind == ScannedFileKind::Media)
                .count(),
            sidecar_count: scan
                .files
                .iter()
                .filter(|file| file.kind == ScannedFileKind::Sidecar)
                .count(),
            unknown_count: scan
                .files
                .iter()
                .filter(|file| file.kind == ScannedFileKind::Unknown)
                .count(),
        },
    )
}

fn import_plan(
    catalog: &Path,
    snapshot: &Path,
    output: &Path,
    collection: &str,
    destination_mode: DestinationMode,
    accepted_at: PrimitiveDateTime,
    confirmed: bool,
) -> Result<(), CliError> {
    let layout = require_catalog(catalog)?;
    ensure_output_absent(output)?;
    let label = CatalogLabel::new(collection)
        .map_err(|error| CliError::invalid_data(format!("invalid collection: {error}")))?;
    let scan: SourceScan = read_json(snapshot)?;
    validate_scan(&scan)?;
    let current_scan = scan_source(&scan.source_root, scan.schema_version)?;
    if current_scan.fingerprint != scan.fingerprint {
        return Err(CliError::conflict(
            "source contents changed after the saved scan",
        ));
    }

    let year_directory = year_directory(accepted_at.date())
        .map_err(|error| CliError::invalid_data(error.to_string()))?
        .to_string();
    let collection_directory = DatedCollectionRange::new(accepted_at.date(), accepted_at.date())
        .and_then(|range| range.folder_name(&label))
        .map_err(|error| CliError::invalid_data(error.to_string()))?
        .to_string();
    let collection_path = layout
        .root()
        .join(&year_directory)
        .join(&collection_directory);
    match destination_mode {
        DestinationMode::Create if collection_path.exists() => {
            return Err(CliError::conflict(format!(
                "create destination already exists: {}",
                collection_path.display()
            )));
        }
        DestinationMode::Existing if !collection_path.is_dir() => {
            return Err(CliError::not_found(format!(
                "existing destination is not a collection directory: {}",
                collection_path.display()
            )));
        }
        DestinationMode::Create | DestinationMode::Existing => {}
    }
    if destination_mode == DestinationMode::Existing {
        validate_existing_collection(&collection_path)?;
    }
    let existing_counters = existing_counters(&collection_path, accepted_at.date())?;
    let source_files = scan
        .files
        .iter()
        .map(scanned_source_file)
        .collect::<Result<Vec<_>, _>>()?;
    let source_by_id: BTreeMap<_, _> = source_files
        .iter()
        .map(|file| (file.id.clone(), file))
        .collect();
    let scanned_by_id: BTreeMap<_, _> = scan
        .files
        .iter()
        .map(|file| {
            SourceFileId::new(file.id.clone())
                .map(|id| (id, file))
                .map_err(|error| CliError::invalid_data(error.to_string()))
        })
        .collect::<Result<_, _>>()?;
    let relationships = resolve_directory_relationships(&source_files);
    if relationships.items.is_empty() {
        return Err(CliError::invalid_data(
            "source scan contains no supported media files",
        ));
    }
    let mut unresolved_items = BTreeMap::new();
    let mut unhandled_content = Vec::new();
    for finding in &relationships.findings {
        match finding {
            RelationshipFinding::AmbiguousRawJpeg(files) => {
                record_item_findings(
                    &mut unresolved_items,
                    files,
                    PlanItemFinding::AmbiguousRawJpeg,
                );
            }
            RelationshipFinding::AmbiguousLivePhoto { files, .. } => {
                record_item_findings(
                    &mut unresolved_items,
                    files,
                    PlanItemFinding::AmbiguousLivePhoto,
                );
            }
            RelationshipFinding::AmbiguousCanonicalCompound(files) => {
                record_item_findings(
                    &mut unresolved_items,
                    files,
                    PlanItemFinding::AmbiguousCanonicalCompound,
                );
            }
            RelationshipFinding::ConflictingCompound(files) => {
                record_item_findings(
                    &mut unresolved_items,
                    files,
                    PlanItemFinding::ConflictingCompound,
                );
            }
            RelationshipFinding::OrphanSidecar(file) => {
                unhandled_content.push(unhandled_file(
                    scanned_by_id[file],
                    UnhandledContentFinding::OrphanSidecar,
                ));
            }
            RelationshipFinding::AmbiguousSidecar {
                sidecar,
                candidate_items,
            } => {
                record_item_findings(
                    &mut unresolved_items,
                    candidate_items,
                    PlanItemFinding::AmbiguousSidecar,
                );
                unhandled_content.push(unhandled_file(
                    scanned_by_id[sidecar],
                    UnhandledContentFinding::AmbiguousSidecar {
                        candidate_item_ids: candidate_items
                            .iter()
                            .map(|id| id.as_str().to_owned())
                            .collect(),
                    },
                ));
            }
            RelationshipFinding::UnknownContent(file) => {
                unhandled_content.push(unhandled_file(
                    scanned_by_id[file],
                    UnhandledContentFinding::UnknownContent,
                ));
            }
        }
    }
    let allocations = allocate_collection_counters(
        &BTreeMap::from([(accepted_at.date(), existing_counters)]),
        relationships
            .items
            .iter()
            .filter(|item| item.status == RelationshipStatus::Resolved)
            .map(|item| NewItemCounterRequest {
                item_id: item.primary.as_str().to_owned(),
                source_path_order_key: source_by_id[&item.primary].path.order_key(),
                accepted_datetime: accepted_at,
            }),
    )
    .map_err(|error| CliError::invalid_data(error.to_string()))?;
    let has_transfers = !allocations.is_empty();
    let counters: BTreeMap<_, _> = allocations
        .into_iter()
        .map(|allocation| (allocation.item_id, allocation.counter))
        .collect();

    let mut steps = Vec::new();
    let mut sequence = 1_u32;
    if has_transfers && !layout.root().join(&year_directory).exists() {
        steps.push(PlanStep {
            sequence,
            action: PlanStepKind::CreateDirectory {
                path: vec![year_directory.clone()],
            },
        });
        sequence += 1;
    }
    if has_transfers && destination_mode == DestinationMode::Create {
        steps.push(PlanStep {
            sequence,
            action: PlanStepKind::CreateDirectory {
                path: vec![year_directory.clone(), collection_directory.clone()],
            },
        });
        sequence += 1;
    }

    let mut items = Vec::new();
    let mut manifest_lines = existing_manifest_lines(&collection_path.join("SHA256SUMS"))?;
    let previous_manifest = manifest_fingerprint(
        layout.root(),
        &[
            year_directory.clone(),
            collection_directory.clone(),
            "SHA256SUMS".to_owned(),
        ],
    )?;
    let mut required_space_bytes = 0_u64;
    for item in relationships.items {
        let disposition = match item.status {
            RelationshipStatus::Resolved => PlanItemDisposition::Transfer,
            RelationshipStatus::NeedsDecision => PlanItemDisposition::Unresolved {
                finding: item_relationship_finding(&item, &unresolved_items).ok_or_else(|| {
                    CliError::invalid_data(format!(
                        "logical item '{}' needs a relationship decision without a finding",
                        item.primary.as_str()
                    ))
                })?,
            },
        };
        let mut files = Vec::new();
        let originals = std::iter::once((&item.primary, PlanFileRole::Primary))
            .chain(
                item.companion
                    .iter()
                    .map(|id| (id, PlanFileRole::Companion)),
            )
            .collect::<Vec<_>>();
        let stem = counters
            .get(item.primary.as_str())
            .map(|counter| {
                CanonicalStem::new(accepted_at, *counter)
                    .map_err(|error| CliError::invalid_data(error.to_string()))
            })
            .transpose()?;
        for (file_id, role) in originals {
            let source = source_by_id[file_id];
            let scanned = scanned_by_id[file_id];
            files.push(plan_file(scanned, role));
            if let Some(stem) = &stem {
                let filename = CanonicalFilename::new(stem, &source.extension)
                    .map_err(|error| CliError::invalid_data(error.to_string()))?
                    .to_string();
                manifest_lines.insert(filename.clone(), scanned.digest.as_str().to_owned());
                push_copy_step(
                    &mut steps,
                    &mut sequence,
                    &mut required_space_bytes,
                    scanned,
                    vec![
                        year_directory.clone(),
                        collection_directory.clone(),
                        filename,
                    ],
                )?;
            }
        }
        for sidecar_id in &item.sidecars {
            let source = source_by_id[sidecar_id];
            let scanned = scanned_by_id[sidecar_id];
            files.push(plan_file(scanned, PlanFileRole::Sidecar));
            if let Some(stem) = &stem {
                let suffix = sidecar_suffix_for_item(
                    source,
                    std::iter::once(&item.primary)
                        .chain(item.companion.iter())
                        .map(|id| source_by_id[id]),
                )
                .ok_or_else(|| {
                    CliError::invalid_data(format!(
                        "resolved sidecar has no unambiguous preserved suffix: {}",
                        source.path
                    ))
                })?;
                push_copy_step(
                    &mut steps,
                    &mut sequence,
                    &mut required_space_bytes,
                    scanned,
                    vec![
                        year_directory.clone(),
                        collection_directory.clone(),
                        format!("{stem}{suffix}"),
                    ],
                )?;
            }
        }
        items.push(PlanItem {
            item_id: item.primary.as_str().to_owned(),
            files,
            accepted_datetime: Some(plan_datetime(accepted_at)),
            disposition,
        });
    }
    if has_transfers {
        let manifest_contents = encode_manifest(&manifest_lines);
        let manifest_digest = ContentDigest::from_bytes(&manifest_contents);
        required_space_bytes = required_space_bytes
            .checked_add(manifest_contents.len() as u64)
            .ok_or_else(|| CliError::invalid_data("planned size exceeds supported range"))?;
        steps.push(PlanStep {
            sequence,
            action: PlanStepKind::WriteManifest {
                path: vec![
                    year_directory,
                    collection_directory,
                    "SHA256SUMS".to_owned(),
                ],
                previous_digest: previous_manifest.as_ref().map(|entry| entry.digest.clone()),
                resulting_digest: manifest_digest,
                resulting_size_bytes: manifest_contents.len() as u64,
            },
        });
    }

    let created_at = system_timestamp()?;
    let plan_id = PlanId::new(format!(
        "plan-{}-{}-{}",
        created_at.unix_seconds,
        created_at.nanosecond,
        &scan.fingerprint.as_str()[..12]
    ))
    .map_err(|error| CliError::invalid_data(error.to_string()))?;
    let plan = ImportPlan::new(PlanDocument {
        schema_version: IMPORT_PLAN_SCHEMA_VERSION,
        plan_id,
        created_at,
        catalog: CatalogSnapshot {
            catalog_id: digest_text(&canonical_path(layout.root())?.to_string_lossy()),
            manifests: previous_manifest.into_iter().collect(),
        },
        source: SourceSnapshot {
            source_id: scan.source_id.clone(),
            fingerprint: scan.fingerprint.clone(),
        },
        items,
        unhandled_content,
        steps,
        required_space_bytes,
    })
    .map_err(|error| CliError::invalid_data(error.to_string()))?;
    let stored = StoredPlan {
        schema_version: IMPORT_PLAN_SCHEMA_VERSION,
        confirmed,
        source_root: scan.source_root,
        import_plan: plan,
    };
    write_new_json(output, &stored)?;

    write_result(
        "import.plan",
        PlanResult {
            plan: canonical_output_path(output)?,
            plan_id: stored.import_plan.document().plan_id.as_str().to_owned(),
            digest: stored.import_plan.digest().clone(),
            confirmed,
            item_count: stored.import_plan.document().items.len(),
            transfer_count: stored
                .import_plan
                .document()
                .items
                .iter()
                .filter(|item| matches!(item.disposition, PlanItemDisposition::Transfer))
                .count(),
            unhandled_count: stored.import_plan.document().unhandled_content.len(),
            required_space_bytes,
        },
    )
}

fn import_execute(
    catalog: &Path,
    plan_path: &Path,
    progress_path: Option<&Path>,
) -> Result<(), CliError> {
    let layout = require_catalog(catalog)?;
    let stored = read_stored_plan(plan_path)?;
    if !stored.confirmed {
        return Err(CliError::conflict(
            "import plan is not confirmed; create it with --confirm",
        ));
    }
    validate_plan_state(&layout, &stored)?;
    let progress_path = validate_progress_path(&layout, plan_path, &stored, progress_path)?;
    validate_plan_destinations(&layout, &stored.import_plan)?;
    let operation_id = stored.import_plan.document().plan_id.as_str().to_owned();
    let mut progress = ProgressWriter::new(&progress_path, &operation_id)?;
    progress.write("started", None)?;

    let lock = CatalogLock::acquire(&layout)
        .map_err(|error| CliError::conflict(format!("acquire catalog lock: {error}")))?;
    // Recheck under the lock to close the race after the read-only preflight.
    validate_plan_destinations(&layout, &stored.import_plan)?;
    let mut journal = OperationJournal::create(&layout, &lock, &operation_id, &stored.import_plan)
        .map_err(|error| CliError::conflict(format!("create operation journal: {error}")))?;
    let collection_name = stored
        .import_plan
        .document()
        .steps
        .iter()
        .find_map(|step| match &step.action {
            PlanStepKind::CopyFile { destination, .. } => destination
                .get(1)
                .map(|directory| collection_name_from_directory(directory)),
            _ => None,
        })
        .unwrap_or_else(|| "Import".to_owned());
    let total_count = stored
        .import_plan
        .document()
        .steps
        .iter()
        .filter(|step| matches!(step.action, PlanStepKind::CopyFile { .. }))
        .count();
    journal
        .record_metadata(
            &layout,
            &lock,
            collection_name,
            stored.source_root.clone(),
            total_count,
        )
        .map_err(|error| CliError::operation(error.to_string()))?;

    let mut completed_steps = 0_usize;
    for step in &stored.import_plan.document().steps {
        progress.write("step_started", Some(step.sequence))?;
        match &step.action {
            PlanStepKind::CreateDirectory { path } => {
                journal
                    .create_destination_directory(
                        &layout,
                        &lock,
                        &format!("step-{}", step.sequence),
                        pathbuf(path),
                    )
                    .map_err(|error| CliError::operation(error.to_string()))?;
            }

            PlanStepKind::CopyFile {
                source_file_id,
                destination,
                expected_digest,
                size_bytes,
            } => {
                let source = source_path_for_id(&stored, source_file_id)?;
                layout
                    .copy_verified(
                        &lock,
                        &mut journal,
                        source,
                        pathbuf(destination),
                        &format!("step-{}", step.sequence),
                        souvenirs_storage::CopyExpectation {
                            sha256: expected_digest.as_str().to_owned(),
                            size_bytes: *size_bytes,
                        },
                    )
                    .map_err(|error| CliError::operation(error.to_string()))?;
            }
            PlanStepKind::WriteManifest {
                path,
                previous_digest,
                resulting_digest,
                resulting_size_bytes,
            } => {
                let contents = manifest_contents_for_step(&layout, &stored.import_plan, path)?;
                if contents.len() as u64 != *resulting_size_bytes
                    || ContentDigest::from_bytes(&contents) != *resulting_digest
                {
                    return Err(CliError::conflict(format!(
                        "manifest contents drifted from plan step {}",
                        step.sequence
                    )));
                }
                layout
                    .replace_manifest(
                        &lock,
                        &mut journal,
                        ManifestReplacementRequest {
                            path: &pathbuf(path),
                            previous_sha256: previous_digest.as_ref().map(ContentDigest::as_str),
                            resulting_sha256: resulting_digest.as_str(),
                            step_id: &format!("step-{}", step.sequence),
                        },
                        &contents,
                    )
                    .map_err(|error| CliError::operation(error.to_string()))?;
            }
        }
        completed_steps += 1;
        progress.write("step_completed", Some(step.sequence))?;
    }
    journal
        .complete(&layout, &lock)
        .map_err(|error| CliError::operation(error.to_string()))?;
    lock.release()
        .map_err(|error| CliError::operation(format!("release catalog lock: {error}")))?;
    progress.write("completed", None)?;

    write_result(
        "import.execute",
        ExecutionResult {
            operation_id,
            outcome: "completed",
            completed_steps,
        },
    )
}

fn collection_name_from_directory(directory: &str) -> String {
    let Some((date, name)) = directory.split_once(' ') else {
        return directory.to_owned();
    };
    let valid_date_prefix = date.len() == 10
        && date.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if valid_date_prefix && !name.is_empty() {
        name.to_owned()
    } else {
        directory.to_owned()
    }
}

fn operation_status(catalog: &Path, operation_id: &str) -> Result<(), CliError> {
    let layout = require_catalog(catalog)?;
    let (journal, records) = find_operation_journal(&layout, operation_id)?;
    let state = match records.last() {
        Some(OperationRecord::Outcome {
            outcome: OperationOutcome::Completed,
        }) => "completed",
        Some(OperationRecord::Outcome {
            outcome: OperationOutcome::Cancelled,
        }) => "cancelled",
        _ => "interrupted",
    };
    let transition_count = records
        .iter()
        .filter(|record| matches!(record, OperationRecord::Transition { .. }))
        .count();
    write_result(
        "operation.status",
        OperationStatusResult {
            operation_id: operation_id.to_owned(),
            state,
            transition_count,
            journal,
        },
    )
}

fn operation_recover(
    catalog: &Path,
    operation_id: &str,
    plan_path: &Path,
    mode: RecoveryChoice,
    progress_path: Option<&Path>,
) -> Result<(), CliError> {
    let layout = require_catalog(catalog)?;
    let stored = read_stored_plan(plan_path)?;
    if !stored.confirmed {
        return Err(CliError::conflict(
            "operation recovery requires the confirmed plan",
        ));
    }
    if stored.import_plan.document().plan_id.as_str() != operation_id {
        return Err(CliError::invalid_data(
            "operation identifier does not match the saved plan",
        ));
    }
    let progress_path = validate_progress_path(&layout, plan_path, &stored, progress_path)?;
    let lock = CatalogLock::acquire(&layout)
        .map_err(|error| CliError::conflict(format!("acquire catalog lock: {error}")))?;
    let (journal_path, records) = find_operation_journal(&layout, operation_id)?;
    validate_recovery_state(&layout, &stored, &records, mode)?;
    let request = recovery_request(&layout, &stored, mode, &records)?;
    let mut progress = ProgressWriter::new(&progress_path, operation_id)?;
    progress.write("recovery_started", None)?;
    let outcome = layout
        .recover_operation(&lock, journal_path, request)
        .map_err(|error| CliError::operation(error.to_string()))?;
    lock.release()
        .map_err(|error| CliError::operation(format!("release catalog lock: {error}")))?;
    let outcome_name = match outcome {
        RecoveryOutcome::Completed => "completed",
        RecoveryOutcome::Cancelled => "cancelled",
    };
    progress.write(outcome_name, None)?;
    write_result(
        match mode {
            RecoveryChoice::Resume => "operation.recover",
            RecoveryChoice::Cancel => "operation.cancel",
        },
        RecoveryResult {
            operation_id: operation_id.to_owned(),
            outcome: outcome_name,
        },
    )
}

fn integrity_check(catalog: &Path) -> Result<(), CliError> {
    let layout = require_catalog(catalog)?;
    let mut checked_files = 0_usize;
    let mut checked_manifests = 0_usize;
    let mut coverage_complete = true;
    let mut findings = Vec::new();
    for collection in collection_directories(layout.root())? {
        let manifest_path = collection.join("SHA256SUMS");
        let manifest_is_file = match fs::metadata(&manifest_path) {
            Ok(metadata) => metadata.is_file(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                coverage_complete = false;
                findings.push(IntegrityFinding::UnreadableFile {
                    path: relative_to(layout.root(), &manifest_path)?,
                    error: error.to_string(),
                });
                continue;
            }
        };
        if !manifest_is_file {
            findings.push(IntegrityFinding::MissingManifest {
                collection: relative_to(layout.root(), &collection)?,
            });
            continue;
        }
        let manifest = match existing_manifest_lines(&manifest_path) {
            Ok(manifest) => manifest,
            Err(error) if error.code == "io" => {
                coverage_complete = false;
                findings.push(IntegrityFinding::UnreadableFile {
                    path: relative_to(layout.root(), &manifest_path)?,
                    error: error.message,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        checked_manifests += 1;
        for (filename, expected) in &manifest {
            let path = collection.join(filename);
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    findings.push(IntegrityFinding::MissingFile {
                        path: relative_to(layout.root(), &path)?,
                    });
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    findings.push(IntegrityFinding::MissingFile {
                        path: relative_to(layout.root(), &path)?,
                    });
                    continue;
                }
                Err(error) => {
                    coverage_complete = false;
                    findings.push(IntegrityFinding::UnreadableFile {
                        path: relative_to(layout.root(), &path)?,
                        error: error.to_string(),
                    });
                    continue;
                }
            }
            let (actual, _) = match digest_file(&path) {
                Ok(digest) => digest,
                Err(error) if error.code == "io" => {
                    coverage_complete = false;
                    findings.push(IntegrityFinding::UnreadableFile {
                        path: relative_to(layout.root(), &path)?,
                        error: error.message,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            checked_files += 1;
            if actual.as_str() != expected {
                findings.push(IntegrityFinding::DigestMismatch {
                    path: relative_to(layout.root(), &path)?,
                    expected: expected.clone(),
                    actual: actual.as_str().to_owned(),
                });
            }
        }
        let files = match inspect_collection_files(&collection) {
            Ok(files) => files,
            Err(error) if error.code == "io" => {
                coverage_complete = false;
                findings.push(IntegrityFinding::UnreadableCollection {
                    collection: relative_to(layout.root(), &collection)?,
                    error: error.message,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut unexpected = files
            .originals
            .difference(&manifest.keys().cloned().collect())
            .cloned()
            .collect::<BTreeSet<_>>();
        unexpected.extend(files.unowned_or_unsupported);
        for filename in unexpected {
            findings.push(IntegrityFinding::UnexpectedFile {
                path: relative_to(layout.root(), &collection.join(&filename))?,
            });
        }
    }
    write_result(
        "integrity.check",
        IntegrityResult {
            catalog: canonical_path(layout.root())?,
            valid: coverage_complete && findings.is_empty(),
            coverage_complete,
            checked_files,
            checked_manifests,
            findings,
        },
    )
}

fn read_stored_plan(path: &Path) -> Result<StoredPlan, CliError> {
    let stored: StoredPlan = read_json(path)?;
    if !(1..=IMPORT_PLAN_SCHEMA_VERSION).contains(&stored.schema_version) {
        return Err(CliError::invalid_data(format!(
            "unsupported stored plan schema version {}",
            stored.schema_version
        )));
    }
    if stored.schema_version != stored.import_plan.document().schema_version {
        return Err(CliError::invalid_data(
            "stored plan envelope and import plan schema versions differ",
        ));
    }
    Ok(stored)
}

fn validate_plan_state(layout: &CatalogLayout, stored: &StoredPlan) -> Result<(), CliError> {
    let plan = stored.import_plan.document();
    let catalog_id = digest_text(&canonical_path(layout.root())?.to_string_lossy());
    if catalog_id != plan.catalog.catalog_id {
        return Err(CliError::conflict("plan belongs to a different catalog"));
    }
    for manifest in &plan.catalog.manifests {
        let path = join_components(layout.root(), &manifest.path);
        let (actual, _) = digest_file(&path)?;
        if actual != manifest.digest {
            return Err(CliError::conflict(format!(
                "catalog manifest changed after planning: {}",
                path.display()
            )));
        }
    }
    for item in &plan.items {
        for file in &item.files {
            let path = join_components(&stored.source_root, &file.source_path);
            let (actual, size) = digest_file(&path)?;
            if actual != file.digest || size != file.size_bytes {
                return Err(CliError::conflict(format!(
                    "source file changed after planning: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn validate_plan_destinations(layout: &CatalogLayout, plan: &ImportPlan) -> Result<(), CliError> {
    let mut observer =
        PortableDestinationObserver::new(layout).map_err(destination_observation_cli_error)?;
    for destination in plan.destinations_requiring_free() {
        let relative_path = pathbuf(destination);
        if observer
            .is_occupied(&relative_path)
            .map_err(destination_observation_cli_error)?
        {
            return Err(CliError::conflict(format!(
                "planned destination is occupied: {}",
                layout.root().join(relative_path).display()
            )));
        }
    }
    Ok(())
}

fn destination_observation_cli_error(error: CatalogPathError) -> CliError {
    match error {
        CatalogPathError::Io { .. } => {
            CliError::io(format!("inspect planned destination: {error}"))
        }
        other => CliError::conflict(format!("invalid planned destination: {other}")),
    }
}

fn validate_progress_path(
    layout: &CatalogLayout,
    plan_path: &Path,
    stored: &StoredPlan,
    progress_path: Option<&Path>,
) -> Result<ValidatedProgressPath, CliError> {
    let Some(progress_path) = progress_path else {
        return Ok(ValidatedProgressPath(None));
    };

    let observer =
        PortableDestinationObserver::new(layout).map_err(progress_observation_cli_error)?;
    let observation = observer
        .observe_output_path(progress_path)
        .map_err(progress_observation_cli_error)?;

    for step in &stored.import_plan.document().steps {
        let planned_path = match &step.action {
            PlanStepKind::CreateDirectory { path } | PlanStepKind::WriteManifest { path, .. } => {
                path
            }
            PlanStepKind::CopyFile { destination, .. } => destination,
        };
        if observation
            .overlaps_catalog_path(&pathbuf(planned_path))
            .map_err(progress_observation_cli_error)?
        {
            return Err(CliError::conflict(format!(
                "progress path overlaps a planned destination: {}",
                progress_path.display()
            )));
        }
    }

    if observation.is_in_catalog() {
        return Err(CliError::conflict(format!(
            "progress path is inside the catalog: {}",
            progress_path.display()
        )));
    }

    let canonical_plan = canonical_path(plan_path)?;
    if observation.resolved_path() == canonical_plan {
        return Err(CliError::conflict(format!(
            "progress path overlaps the saved plan: {}",
            progress_path.display()
        )));
    }

    match fs::canonicalize(&stored.source_root) {
        Ok(canonical_source) if observation.resolved_path().starts_with(&canonical_source) => {
            return Err(CliError::conflict(format!(
                "progress path overlaps the source: {}",
                progress_path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::io(format!(
                "resolve {}: {error}",
                stored.source_root.display()
            )));
        }
    }

    Ok(ValidatedProgressPath(Some(
        observation.resolved_path().to_path_buf(),
    )))
}

fn progress_observation_cli_error(error: CatalogPathError) -> CliError {
    match error {
        CatalogPathError::Io {
            action: "resolve output directory",
            path,
            source,
        } if source.kind() == io::ErrorKind::NotFound => CliError::not_found(format!(
            "output directory does not exist: {}",
            path.display()
        )),
        CatalogPathError::Io { .. } => CliError::io(format!("inspect progress path: {error}")),
        other => CliError::conflict(format!("invalid progress path: {other}")),
    }
}

fn source_path_for_id(stored: &StoredPlan, source_file_id: &str) -> Result<PathBuf, CliError> {
    stored
        .import_plan
        .document()
        .items
        .iter()
        .flat_map(|item| &item.files)
        .find(|file| file.source_file_id == source_file_id)
        .map(|file| join_components(&stored.source_root, &file.source_path))
        .ok_or_else(|| CliError::invalid_data("plan copy references an unknown source file"))
}

fn manifest_contents_for_step(
    layout: &CatalogLayout,
    plan: &ImportPlan,
    manifest_path: &[String],
) -> Result<Vec<u8>, CliError> {
    let absolute = join_components(layout.root(), manifest_path);
    let mut lines = existing_manifest_lines(&absolute)?;
    let parent = &manifest_path[..manifest_path.len() - 1];
    for step in &plan.document().steps {
        if let PlanStepKind::CopyFile {
            source_file_id,
            destination,
            expected_digest,
            ..
        } = &step.action
        {
            if destination[..destination.len() - 1] == *parent
                && plan_file_role(plan, source_file_id) != Some(PlanFileRole::Sidecar)
            {
                lines.insert(
                    destination
                        .last()
                        .expect("copy destination has filename")
                        .clone(),
                    expected_digest.as_str().to_owned(),
                );
            }
        }
    }
    Ok(encode_manifest(&lines))
}

fn recovery_request(
    layout: &CatalogLayout,
    stored: &StoredPlan,
    mode: RecoveryChoice,
    records: &[OperationRecord],
) -> Result<RecoveryRequest, CliError> {
    let mut request = RecoveryRequest {
        mode: match mode {
            RecoveryChoice::Resume => RecoveryMode::Resume,
            RecoveryChoice::Cancel => RecoveryMode::CancelRemainingCopies,
        },
        ..RecoveryRequest::default()
    };
    for step in &stored.import_plan.document().steps {
        let step_id = format!("step-{}", step.sequence);
        match &step.action {
            PlanStepKind::CreateDirectory { path } => {
                request.directories.push(RecoveryDirectory {
                    step_id,
                    path: pathbuf(path),
                });
            }
            PlanStepKind::CopyFile {
                source_file_id,
                destination,
                expected_digest,
                size_bytes,
            } => request.copies.push(RecoveryCopy {
                step_id,
                source: source_path_for_id(stored, source_file_id)?,
                destination: pathbuf(destination),
                expected_sha256: expected_digest.as_str().to_owned(),
                expected_size_bytes: *size_bytes,
            }),
            PlanStepKind::WriteManifest {
                path,
                previous_digest,
                resulting_digest,
                ..
            } => {
                let contents = manifest_contents_for_recovery(
                    layout,
                    &stored.import_plan,
                    path,
                    mode,
                    records,
                )?;
                if mode == RecoveryChoice::Resume
                    && ContentDigest::from_bytes(&contents) != *resulting_digest
                {
                    return Err(CliError::conflict(
                        "recovery manifest no longer matches the confirmed plan",
                    ));
                }
                request.manifests.push(RecoveryManifest {
                    step_id,
                    path: pathbuf(path),
                    previous_sha256: previous_digest
                        .as_ref()
                        .map(|digest| digest.as_str().to_owned()),
                    contents,
                });
            }
        }
    }
    Ok(request)
}

fn manifest_contents_for_recovery(
    layout: &CatalogLayout,
    plan: &ImportPlan,
    manifest_path: &[String],
    mode: RecoveryChoice,
    records: &[OperationRecord],
) -> Result<Vec<u8>, CliError> {
    if mode == RecoveryChoice::Resume {
        return manifest_contents_for_step(layout, plan, manifest_path);
    }
    let absolute = join_components(layout.root(), manifest_path);
    let mut lines = existing_manifest_lines(&absolute)?;
    let parent = &manifest_path[..manifest_path.len() - 1];
    let copy_states = journal_copy_states(records);
    for step in &plan.document().steps {
        if let PlanStepKind::CopyFile {
            source_file_id,
            destination,
            expected_digest,
            size_bytes,
            ..
        } = &step.action
        {
            if destination[..destination.len() - 1] != *parent {
                continue;
            }
            let step_id = format!("step-{}", step.sequence);
            let state = copy_states.get(&step_id);
            let destination_path = join_components(layout.root(), destination);
            let Some(state) = state else {
                if destination_path.exists() {
                    return Err(CliError::conflict(format!(
                        "unverified bytes appeared at copy destination: {}",
                        destination_path.display()
                    )));
                }
                continue;
            };
            let Some(journal_digest) = state.sha256.as_deref() else {
                continue;
            };
            let matches_plan =
                journal_digest == expected_digest.as_str() && state.size_bytes == Some(*size_bytes);
            if destination_path.exists() {
                let (actual, actual_size) = digest_file(&destination_path)?;
                if !matches_plan || actual.as_str() != journal_digest || actual_size != *size_bytes
                {
                    return Err(CliError::conflict(format!(
                        "published destination differs from the confirmed plan: {}",
                        destination_path.display()
                    )));
                }
            } else if state.published {
                return Err(CliError::conflict(format!(
                    "published destination is missing during cancellation: {}",
                    destination_path.display()
                )));
            } else if !verified_copy_can_publish(
                layout,
                state,
                expected_digest,
                *size_bytes,
                &destination_path,
            )? {
                continue;
            }
            if plan_file_role(plan, source_file_id) != Some(PlanFileRole::Sidecar) {
                lines.insert(
                    destination
                        .last()
                        .expect("copy destination has filename")
                        .clone(),
                    journal_digest.to_owned(),
                );
            }
        }
    }
    Ok(encode_manifest(&lines))
}

fn plan_file_role(plan: &ImportPlan, source_file_id: &str) -> Option<PlanFileRole> {
    plan.document()
        .items
        .iter()
        .flat_map(|item| &item.files)
        .find(|file| file.source_file_id == source_file_id)
        .map(|file| file.role)
}

#[derive(Default)]
struct JournalCopyState {
    staged_path: Option<PathBuf>,
    sha256: Option<String>,
    size_bytes: Option<u64>,
    published: bool,
}

fn journal_copy_states(records: &[OperationRecord]) -> BTreeMap<String, JournalCopyState> {
    let mut states = BTreeMap::<String, JournalCopyState>::new();
    for record in records {
        let OperationRecord::Transition { transition } = record else {
            continue;
        };
        match transition {
            souvenirs_storage::DurableTransition::CopyStarted {
                step_id,
                staged_path,
                ..
            } => {
                states.entry(step_id.clone()).or_default().staged_path = Some(staged_path.clone());
            }
            souvenirs_storage::DurableTransition::CopyVerified {
                step_id,
                staged_path,
                sha256,
                bytes_copied,
                ..
            } => {
                let state = states.entry(step_id.clone()).or_default();
                state.staged_path = Some(staged_path.clone());
                state.sha256 = Some(sha256.clone());
                state.size_bytes = Some(*bytes_copied);
            }
            souvenirs_storage::DurableTransition::CopyPublished {
                step_id, sha256, ..
            } => {
                let state = states.entry(step_id.clone()).or_default();
                state.sha256 = Some(sha256.clone());
                state.published = true;
            }
            _ => {}
        }
    }
    states
}

fn verified_copy_can_publish(
    layout: &CatalogLayout,
    state: &JournalCopyState,
    expected_digest: &ContentDigest,
    expected_size_bytes: u64,
    destination_path: &Path,
) -> Result<bool, CliError> {
    if state.sha256.as_deref() != Some(expected_digest.as_str())
        || state.size_bytes != Some(expected_size_bytes)
    {
        return Ok(false);
    }
    if destination_path.is_file() {
        let (digest, size_bytes) = digest_file(destination_path)?;
        return Ok(digest == *expected_digest && size_bytes == expected_size_bytes);
    }
    let Some(staged_path) = state.staged_path.as_deref() else {
        return Ok(false);
    };
    let staged_path = layout.root().join(staged_path);
    if !staged_path.is_file() {
        return Ok(false);
    }
    let (digest, size_bytes) = digest_file(&staged_path)?;
    Ok(digest == *expected_digest && size_bytes == expected_size_bytes)
}

fn validate_recovery_state(
    layout: &CatalogLayout,
    stored: &StoredPlan,
    records: &[OperationRecord],
    mode: RecoveryChoice,
) -> Result<(), CliError> {
    let plan = stored.import_plan.document();
    let copy_states = journal_copy_states(records);
    let catalog_id = digest_text(&canonical_path(layout.root())?.to_string_lossy());
    if catalog_id != plan.catalog.catalog_id {
        return Err(CliError::conflict(
            "recovery plan belongs to a different catalog",
        ));
    }
    let expected_plan = serde_json::to_value(&stored.import_plan)
        .map_err(|error| CliError::invalid_data(format!("serialize recovery plan: {error}")))?;
    match records.first() {
        Some(OperationRecord::Plan { plan, .. }) if plan == &expected_plan => {}
        Some(OperationRecord::Plan { .. }) => {
            return Err(CliError::conflict(
                "saved plan does not match the operation journal",
            ));
        }
        _ => {
            return Err(CliError::invalid_data(
                "operation journal has no plan record",
            ))
        }
    }

    for step in &plan.steps {
        match &step.action {
            PlanStepKind::CopyFile {
                source_file_id,
                destination,
                expected_digest,
                size_bytes,
            } => {
                let destination_path = join_components(layout.root(), destination);
                if destination_path.exists() {
                    let (actual, size) = digest_file(&destination_path)?;
                    if actual != *expected_digest || size != *size_bytes {
                        return Err(CliError::conflict(format!(
                            "published destination differs from the confirmed plan: {}",
                            destination_path.display()
                        )));
                    }
                } else if mode == RecoveryChoice::Resume {
                    let step_id = format!("step-{}", step.sequence);
                    if let Some(state) = copy_states.get(&step_id) {
                        if verified_copy_can_publish(
                            layout,
                            state,
                            expected_digest,
                            *size_bytes,
                            &destination_path,
                        )? {
                            continue;
                        }
                    }
                    let source = source_path_for_id(stored, source_file_id)?;
                    let (actual, size) = digest_file(&source)?;
                    if actual != *expected_digest || size != *size_bytes {
                        return Err(CliError::conflict(format!(
                            "source file changed before recovery: {}",
                            source.display()
                        )));
                    }
                }
            }
            PlanStepKind::WriteManifest {
                path,
                previous_digest,
                resulting_digest,
                ..
            } => {
                let manifest_path = join_components(layout.root(), path);
                if manifest_path.exists() {
                    let (actual, _) = digest_file(&manifest_path)?;
                    if Some(&actual) != previous_digest.as_ref() && actual != *resulting_digest {
                        return Err(CliError::conflict(format!(
                            "catalog manifest changed outside the operation: {}",
                            manifest_path.display()
                        )));
                    }
                } else if previous_digest.is_some() {
                    return Err(CliError::conflict(format!(
                        "catalog manifest disappeared before recovery: {}",
                        manifest_path.display()
                    )));
                }
            }
            PlanStepKind::CreateDirectory { .. } => {}
        }
    }
    Ok(())
}

fn find_operation_journal(
    layout: &CatalogLayout,
    operation_id: &str,
) -> Result<(PathBuf, Vec<OperationRecord>), CliError> {
    for path in jsonl_files(&layout.journal_directory())? {
        let contents = fs::read_to_string(&path)
            .map_err(|error| CliError::io(format!("read operation journal: {error}")))?;
        let mut records = Vec::new();
        for (index, line) in contents.lines().enumerate() {
            match serde_json::from_str::<OperationRecord>(line) {
                Ok(record) => records.push(record),
                Err(error)
                    if matches!(
                        records.first(),
                        Some(OperationRecord::Plan {
                            operation_id: recorded,
                            ..
                        }) if recorded == operation_id
                    ) =>
                {
                    return Err(CliError::invalid_data(format!(
                        "operation journal {} line {} is invalid: {error}",
                        path.display(),
                        index + 1
                    )));
                }
                Err(_) => {
                    records.clear();
                    break;
                }
            }
        }
        if matches!(
            records.first(),
            Some(OperationRecord::Plan {
                operation_id: recorded,
                ..
            }) if recorded == operation_id
        ) {
            return Ok((path, records));
        }
    }
    Err(CliError::not_found(format!(
        "operation journal not found: {operation_id}"
    )))
}

impl ProgressWriter {
    fn new(path: &ValidatedProgressPath, operation_id: &str) -> Result<Self, CliError> {
        let file = path
            .0
            .as_deref()
            .map(|path| {
                ensure_output_absent(path)?;
                File::options()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|error| {
                        CliError::io(format!("create progress file {}: {error}", path.display()))
                    })
            })
            .transpose()?;
        Ok(Self {
            file,
            operation_id: operation_id.to_owned(),
            sequence: 0,
            terminal: false,
        })
    }

    fn write(&mut self, event: &'static str, step: Option<u32>) -> Result<(), CliError> {
        let Some(file) = &mut self.file else {
            return Ok(());
        };
        self.sequence += 1;
        if matches!(event, "completed" | "cancelled" | "failed") {
            self.terminal = true;
        }
        serde_json::to_writer(
            &mut *file,
            &ProgressEvent {
                version: 1,
                operation_id: self.operation_id.clone(),
                sequence: self.sequence,
                event,
                step,
            },
        )
        .map_err(|error| CliError::io(format!("write progress event: {error}")))?;
        file.write_all(b"\n")
            .and_then(|()| file.flush())
            .map_err(|error| CliError::io(format!("flush progress event: {error}")))
    }
}

impl Drop for ProgressWriter {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        let Some(file) = &mut self.file else {
            return;
        };
        self.sequence += 1;
        let _ = serde_json::to_writer(
            &mut *file,
            &ProgressEvent {
                version: 1,
                operation_id: self.operation_id.clone(),
                sequence: self.sequence,
                event: "failed",
                step: None,
            },
        );
        let _ = file.write_all(b"\n");
        let _ = file.flush();
    }
}

fn collection_directories(root: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut collections = Vec::new();
    for year in
        fs::read_dir(root).map_err(|error| CliError::io(format!("read catalog root: {error}")))?
    {
        let year = year.map_err(|error| CliError::io(error.to_string()))?;
        if !year
            .file_type()
            .map_err(|error| CliError::io(error.to_string()))?
            .is_dir()
            || year.file_name().to_string_lossy().starts_with('.')
        {
            continue;
        }
        for collection in
            fs::read_dir(year.path()).map_err(|error| CliError::io(error.to_string()))?
        {
            let collection = collection.map_err(|error| CliError::io(error.to_string()))?;
            if collection
                .file_type()
                .map_err(|error| CliError::io(error.to_string()))?
                .is_dir()
            {
                collections.push(collection.path());
            }
        }
    }
    collections.sort();
    Ok(collections)
}

fn relative_to(root: &Path, path: &Path) -> Result<PathBuf, CliError> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| CliError::operation("catalog path escaped its root"))
}

fn pathbuf(components: &[String]) -> PathBuf {
    components.iter().collect()
}

fn require_catalog(path: &Path) -> Result<CatalogLayout, CliError> {
    let layout = CatalogLayout::new(path);
    if !layout.root().is_dir()
        || !layout.config_path().is_file()
        || !layout.state_directory().is_dir()
    {
        return Err(CliError::not_found(format!(
            "initialized catalog not found: {}",
            path.display()
        )));
    }
    Ok(layout)
}

fn validate_scan(scan: &SourceScan) -> Result<(), CliError> {
    if !(1..=SOURCE_SCAN_SCHEMA_VERSION).contains(&scan.schema_version) {
        return Err(CliError::invalid_data(format!(
            "unsupported source scan schema version {}",
            scan.schema_version
        )));
    }
    if !scan.source_root.is_absolute() {
        return Err(CliError::invalid_data(
            "source scan root must be an absolute path",
        ));
    }
    let mut paths = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for file in &scan.files {
        if file.path.is_empty()
            || file
                .path
                .iter()
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !paths.insert(&file.path)
            || !ids.insert(&file.id)
        {
            return Err(CliError::invalid_data(
                "source scan contains invalid or duplicate file entries",
            ));
        }
    }
    if scan_fingerprint(&scan.files)? != scan.fingerprint {
        return Err(CliError::invalid_data(
            "source scan fingerprint does not match its file entries",
        ));
    }
    Ok(())
}

fn scan_source(source: &Path, schema_version: u32) -> Result<SourceScan, CliError> {
    let source_root = canonical_path(source)?;
    let mut paths = Vec::new();
    collect_files(&source_root, &source_root, &mut paths)?;
    paths.sort();
    let mut files = Vec::new();
    for path in paths {
        let absolute = join_components(&source_root, &path);
        files.push(scan_file(&absolute, path, schema_version)?);
    }
    Ok(SourceScan {
        schema_version,
        source_id: digest_text(&source_root.to_string_lossy()),
        fingerprint: scan_fingerprint(&files)?,
        source_root,
        files,
    })
}

fn scan_file(
    absolute: &Path,
    path: Vec<String>,
    schema_version: u32,
) -> Result<ScannedFile, CliError> {
    let (digest, size_bytes) = digest_file(absolute)?;
    let metadata = if schema_version >= 2 {
        let metadata = souvenirs_metadata::relationship_metadata(absolute)
            .map_err(|error| CliError::invalid_data(error.to_string()))?;
        ScannedMetadata {
            apple_content_identifier: metadata.apple_content_identifier,
        }
    } else {
        ScannedMetadata::default()
    };
    Ok(ScannedFile {
        id: digest_text(&path.join("/")),
        path,
        digest,
        size_bytes,
        kind: classify_path(absolute),
        metadata,
    })
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<Vec<String>>,
) -> Result<(), CliError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| {
            CliError::io(format!(
                "read source directory {}: {error}",
                directory.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| CliError::io(format!("read source directory entry: {error}")))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| CliError::io(format!("inspect source entry: {error}")))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .expect("walked source entry remains below source root")
                .components()
                .map(|part| {
                    part.as_os_str().to_str().map(str::to_owned).ok_or_else(|| {
                        CliError::invalid_data(format!(
                            "source path is not valid UTF-8: {}",
                            entry.path().display()
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            output.push(relative);
        }
    }
    Ok(())
}

fn classify_path(path: &Path) -> ScannedFileKind {
    match souvenirs_metadata::classify_path(path) {
        souvenirs_metadata::AssetKind::Original { .. } => ScannedFileKind::Media,
        souvenirs_metadata::AssetKind::Sidecar { .. } => ScannedFileKind::Sidecar,
        souvenirs_metadata::AssetKind::Unsupported => ScannedFileKind::Unknown,
    }
}

fn scanned_source_file(file: &ScannedFile) -> Result<SourceFile, CliError> {
    source_file(
        SourceFileId::new(file.id.clone())
            .map_err(|error| CliError::invalid_data(error.to_string()))?,
        SourcePath::new(file.path.clone())
            .map_err(|error| CliError::invalid_data(error.to_string()))?,
        file.metadata.clone(),
    )
}

fn source_file(
    id: SourceFileId,
    path: SourcePath,
    metadata: ScannedMetadata,
) -> Result<SourceFile, CliError> {
    let filename = path.filename();
    let filename_path = Path::new(filename);
    let basename = filename_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(filename)
        .to_owned();
    let extension = filename_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_owned();
    let kind = souvenirs_metadata::classify_extension(&extension).source_file_kind();
    Ok(SourceFile {
        id,
        path,
        basename,
        extension,
        kind,
        metadata: SourceMetadata {
            datetime_candidates: Vec::new(),
            camera: CameraIdentity {
                make: None,
                model: None,
                serial_number: None,
            },
            apple_content_identifier: metadata.apple_content_identifier,
        },
    })
}

fn record_item_findings(
    output: &mut BTreeMap<SourceFileId, PlanItemFinding>,
    files: &[SourceFileId],
    finding: PlanItemFinding,
) {
    for file in files {
        output
            .entry(file.clone())
            .and_modify(|existing| {
                if relationship_finding_priority(&finding) < relationship_finding_priority(existing)
                {
                    *existing = finding.clone();
                }
            })
            .or_insert_with(|| finding.clone());
    }
}

fn relationship_finding_priority(finding: &PlanItemFinding) -> u8 {
    match finding {
        PlanItemFinding::AmbiguousRawJpeg => 0,
        PlanItemFinding::AmbiguousLivePhoto => 1,
        PlanItemFinding::AmbiguousCanonicalCompound => 2,
        PlanItemFinding::ConflictingCompound => 3,
        PlanItemFinding::AmbiguousSidecar => 4,
        PlanItemFinding::MissingDate | PlanItemFinding::PendingDateConfirmation => 5,
    }
}

fn item_relationship_finding(
    item: &souvenirs_domain::relationships::LogicalItem,
    findings: &BTreeMap<SourceFileId, PlanItemFinding>,
) -> Option<PlanItemFinding> {
    std::iter::once(&item.primary)
        .chain(item.companion.iter())
        .find_map(|id| findings.get(id).cloned())
}

fn unhandled_file(file: &ScannedFile, finding: UnhandledContentFinding) -> UnhandledContent {
    UnhandledContent {
        source_file_id: file.id.clone(),
        source_path: file.path.clone(),
        digest: file.digest.clone(),
        size_bytes: file.size_bytes,
        finding,
    }
}

fn plan_file(file: &ScannedFile, role: PlanFileRole) -> PlanFile {
    PlanFile {
        source_file_id: file.id.clone(),
        source_path: file.path.clone(),
        digest: file.digest.clone(),
        size_bytes: file.size_bytes,
        role,
    }
}

fn push_copy_step(
    steps: &mut Vec<PlanStep>,
    sequence: &mut u32,
    required_space_bytes: &mut u64,
    file: &ScannedFile,
    destination: Vec<String>,
) -> Result<(), CliError> {
    steps.push(PlanStep {
        sequence: *sequence,
        action: PlanStepKind::CopyFile {
            source_file_id: file.id.clone(),
            destination,
            expected_digest: file.digest.clone(),
            size_bytes: file.size_bytes,
        },
    });
    *sequence += 1;
    *required_space_bytes = required_space_bytes
        .checked_add(file.size_bytes)
        .ok_or_else(|| CliError::invalid_data("planned size exceeds supported range"))?;
    Ok(())
}

fn scan_fingerprint(files: &[ScannedFile]) -> Result<ContentDigest, CliError> {
    serde_json::to_vec(files)
        .map(|bytes| ContentDigest::from_bytes(&bytes))
        .map_err(|error| CliError::invalid_data(format!("serialize source scan: {error}")))
}

fn existing_counters(path: &Path, date: Date) -> Result<BTreeSet<NonZeroU32>, CliError> {
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let prefix = format!("{date} ");
    let mut counters = BTreeSet::new();
    for entry in fs::read_dir(path)
        .map_err(|error| CliError::io(format!("read collection directory: {error}")))?
    {
        let name = entry
            .map_err(|error| CliError::io(format!("read collection entry: {error}")))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if let Some(rest) = name.strip_prefix(&prefix) {
            let mut parts = rest.split(' ');
            let _time = parts.next();
            if let Some(counter) = parts
                .next()
                .and_then(|value| value.split('.').next())
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(NonZeroU32::new)
            {
                counters.insert(counter);
            }
        }
    }
    Ok(counters)
}

fn existing_manifest_lines(path: &Path) -> Result<BTreeMap<String, String>, CliError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| CliError::io(format!("read existing manifest: {error}")))?;
    let mut lines = BTreeMap::new();
    for line in contents.lines() {
        let (digest, filename) = line
            .split_once("  ")
            .ok_or_else(|| CliError::invalid_data("existing SHA256SUMS line is malformed"))?;
        ContentDigest::parse(digest)
            .map_err(|error| CliError::invalid_data(format!("invalid manifest digest: {error}")))?;
        if lines
            .insert(filename.to_owned(), digest.to_owned())
            .is_some()
        {
            return Err(CliError::invalid_data(
                "existing SHA256SUMS contains duplicate filenames",
            ));
        }
    }
    Ok(lines)
}

fn validate_existing_collection(path: &Path) -> Result<(), CliError> {
    let manifest = existing_manifest_lines(&path.join("SHA256SUMS"))?;
    let files = inspect_collection_files(path)?;
    if files.originals != manifest.keys().cloned().collect() {
        return Err(CliError::conflict(
            "existing collection originals do not exactly match SHA256SUMS",
        ));
    }
    if files.has_relationship_findings || !files.unowned_or_unsupported.is_empty() {
        return Err(CliError::conflict(
            "existing collection contains unresolved or unsupported direct files",
        ));
    }
    Ok(())
}

struct InspectedCollectionFiles {
    originals: BTreeSet<String>,
    unowned_or_unsupported: BTreeSet<String>,
    has_relationship_findings: bool,
}

fn inspect_collection_files(path: &Path) -> Result<InspectedCollectionFiles, CliError> {
    let mut original_files = BTreeSet::new();
    let mut sidecar_files = BTreeSet::new();
    let mut unsupported_files = BTreeSet::new();
    let mut source_files = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|error| CliError::io(format!("read existing collection: {error}")))?
    {
        let entry = entry
            .map_err(|error| CliError::io(format!("read existing collection entry: {error}")))?;
        let file_type = entry
            .file_type()
            .map_err(|error| CliError::io(format!("inspect existing collection entry: {error}")))?;
        if file_type.is_symlink() || !file_type.is_file() || entry.file_name() == "SHA256SUMS" {
            continue;
        }
        let filename = entry
            .file_name()
            .into_string()
            .map_err(|_| CliError::invalid_data("collection filename is not valid UTF-8"))?;
        let file = source_file(
            SourceFileId::new(filename.clone())
                .map_err(|error| CliError::invalid_data(error.to_string()))?,
            SourcePath::new([filename.clone()])
                .map_err(|error| CliError::invalid_data(error.to_string()))?,
            ScannedMetadata::default(),
        )?;
        match file.kind {
            SourceFileKind::Original(_) => {
                original_files.insert(filename);
            }
            SourceFileKind::Sidecar { .. } => {
                sidecar_files.insert(filename);
            }
            SourceFileKind::Unknown => {
                unsupported_files.insert(filename);
            }
        }
        source_files.push(file);
    }
    mark_existing_canonical_compounds(&mut source_files);
    let relationships = resolve_directory_relationships(&source_files);
    let owned_sidecars = relationships
        .items
        .iter()
        .flat_map(|item| &item.sidecars)
        .map(|id| id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    for sidecar in sidecar_files.difference(&owned_sidecars) {
        unsupported_files.insert(sidecar.clone());
    }
    Ok(InspectedCollectionFiles {
        originals: original_files,
        unowned_or_unsupported: unsupported_files,
        has_relationship_findings: !relationships.findings.is_empty(),
    })
}

fn mark_existing_canonical_compounds(files: &mut [SourceFile]) {
    let mut by_stem = BTreeMap::<String, Vec<usize>>::new();
    for (index, file) in files.iter().enumerate() {
        if matches!(file.kind, SourceFileKind::Original(_)) && is_canonical_stem(&file.basename) {
            by_stem
                .entry(file.basename.clone())
                .or_default()
                .push(index);
        }
    }
    for indices in by_stem.into_values() {
        if indices.len() != 2 {
            continue;
        }
        let roles = indices
            .iter()
            .filter_map(|index| match files[*index].kind {
                SourceFileKind::Original(role) => Some(role),
                SourceFileKind::Sidecar { .. } | SourceFileKind::Unknown => None,
            })
            .collect::<Vec<_>>();
        if roles.iter().filter(|role| role.is_still()).count() == 1
            && roles.iter().filter(|role| role.is_video()).count() == 1
        {
            for index in indices {
                let SourceFileKind::Original(role) = files[index].kind else {
                    continue;
                };
                files[index].kind = SourceFileKind::Original(if role.is_still() {
                    OriginalRole::CanonicalStill
                } else {
                    OriginalRole::CanonicalVideo
                });
            }
        }
    }
}

fn is_canonical_stem(stem: &str) -> bool {
    let mut parts = stem.split(' ');
    let (Some(date), Some(clock), Some(counter), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if date.len() != 10 || clock.len() != 8 {
        return false;
    }
    let date = date.split('-').collect::<Vec<_>>();
    if date.len() != 3 {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        date[0].parse::<i32>(),
        date[1].parse::<u8>(),
        date[2].parse::<u8>(),
    ) else {
        return false;
    };
    let Some(month) = Month::try_from(month).ok() else {
        return false;
    };
    if Date::from_calendar_date(year, month, day).is_err() {
        return false;
    }
    let clock = clock.split('-').collect::<Vec<_>>();
    if clock.len() != 3 {
        return false;
    }
    let (Ok(hour), Ok(minute), Ok(second)) = (
        clock[0].parse::<u8>(),
        clock[1].parse::<u8>(),
        clock[2].parse::<u8>(),
    ) else {
        return false;
    };
    Time::from_hms(hour, minute, second).is_ok()
        && counter.len() >= 3
        && counter.bytes().all(|byte| byte.is_ascii_digit())
        && counter
            .parse::<u32>()
            .ok()
            .and_then(NonZeroU32::new)
            .is_some()
}

fn encode_manifest(lines: &BTreeMap<String, String>) -> Vec<u8> {
    let mut output = String::new();
    for (filename, digest) in lines {
        output.push_str(digest);
        output.push_str("  ");
        output.push_str(filename);
        output.push('\n');
    }
    output.into_bytes()
}

fn manifest_fingerprint(
    root: &Path,
    components: &[String],
) -> Result<Option<ManifestFingerprint>, CliError> {
    let path = join_components(root, components);
    if !path.exists() {
        return Ok(None);
    }
    let (digest, _) = digest_file(&path)?;
    Ok(Some(ManifestFingerprint {
        path: components.to_vec(),
        digest,
    }))
}

fn inspect_managed_files(root: &Path) -> Result<(usize, usize), CliError> {
    let mut manifest_count = 0;
    let mut managed_file_count = 0;
    for year in
        fs::read_dir(root).map_err(|error| CliError::io(format!("read catalog root: {error}")))?
    {
        let year = year.map_err(|error| CliError::io(format!("read catalog entry: {error}")))?;
        if !year
            .file_type()
            .map_err(|error| CliError::io(error.to_string()))?
            .is_dir()
            || year.file_name().to_string_lossy().starts_with('.')
        {
            continue;
        }
        for collection in fs::read_dir(year.path())
            .map_err(|error| CliError::io(format!("read catalog year: {error}")))?
        {
            let collection =
                collection.map_err(|error| CliError::io(format!("read collection: {error}")))?;
            if !collection
                .file_type()
                .map_err(|error| CliError::io(error.to_string()))?
                .is_dir()
            {
                continue;
            }
            for file in fs::read_dir(collection.path())
                .map_err(|error| CliError::io(format!("read collection files: {error}")))?
            {
                let file = file.map_err(|error| CliError::io(error.to_string()))?;
                if file
                    .file_type()
                    .map_err(|error| CliError::io(error.to_string()))?
                    .is_file()
                {
                    if file.file_name() == "SHA256SUMS" {
                        manifest_count += 1;
                    } else {
                        managed_file_count += 1;
                    }
                }
            }
        }
    }
    Ok((manifest_count, managed_file_count))
}

fn jsonl_files(path: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut files = fs::read_dir(path)
        .map_err(|error| CliError::io(format!("read journal directory: {error}")))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.file_name().is_some_and(is_operation_journal_file_name))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn journal_is_unfinished(path: &Path) -> bool {
    fs::read_to_string(path).map_or(true, |contents| {
        contents.lines().last().map_or(true, |line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| value["kind"].as_str().map(str::to_owned))
                .as_deref()
                != Some("outcome")
        })
    })
}

fn parse_format_version(config: &str) -> Result<u32, CliError> {
    config
        .lines()
        .find_map(|line| line.strip_prefix("format_version: "))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| CliError::invalid_data("catalog configuration has no format_version"))
}

fn plan_datetime(value: PrimitiveDateTime) -> PlanDateTime {
    PlanDateTime {
        year: value.year(),
        ordinal: value.ordinal(),
        hour: value.hour(),
        minute: value.minute(),
        second: value.second(),
        recorded_offset_seconds: None,
        provenance: PlanDateTimeProvenance::UserAssigned,
    }
}

fn parse_datetime(value: &str) -> Result<PrimitiveDateTime, String> {
    PrimitiveDateTime::parse(
        value,
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
    )
    .map_err(|_| "expected YYYY-MM-DDTHH:MM:SS".to_owned())
}

fn system_timestamp() -> Result<PlanTimestamp, CliError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| CliError::operation(format!("read system clock: {error}")))?;
    PlanTimestamp::new(duration.as_secs() as i64, duration.subsec_nanos())
        .map_err(|error| CliError::operation(error.to_string()))
}

fn digest_file(path: &Path) -> Result<(ContentDigest, u64), CliError> {
    let mut file = File::open(path)
        .map_err(|error| CliError::io(format!("open {}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut size = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| CliError::io(format!("read {}: {error}", path.display())))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| CliError::invalid_data("file size exceeds supported range"))?;
    }
    ContentDigest::parse(format!("{:x}", hasher.finalize()))
        .map(|digest| (digest, size))
        .map_err(|error| CliError::operation(error.to_string()))
}

fn digest_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn canonical_path(path: &Path) -> Result<PathBuf, CliError> {
    fs::canonicalize(path)
        .map_err(|error| CliError::io(format!("resolve {}: {error}", path.display())))
}

fn canonical_output_path(path: &Path) -> Result<PathBuf, CliError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = canonical_path(parent)?;
    Ok(parent.join(
        path.file_name()
            .ok_or_else(|| CliError::usage("output path has no filename"))?,
    ))
}

fn join_components(root: &Path, components: &[String]) -> PathBuf {
    components
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn ensure_output_absent(path: &Path) -> Result<(), CliError> {
    if path.exists() {
        Err(CliError::conflict(format!(
            "refusing to replace existing output: {}",
            path.display()
        )))
    } else if !path.parent().unwrap_or_else(|| Path::new(".")).is_dir() {
        Err(CliError::not_found(format!(
            "output directory does not exist: {}",
            path.parent().unwrap_or_else(|| Path::new(".")).display()
        )))
    } else {
        Ok(())
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CliError> {
    let bytes = fs::read(path)
        .map_err(|error| CliError::io(format!("read {}: {error}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| CliError::invalid_data(format!("parse {}: {error}", path.display())))
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), CliError> {
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| CliError::io(format!("create {}: {error}", path.display())))?;
    serde_json::to_writer_pretty(&mut file, value)
        .map_err(|error| CliError::io(format!("write {}: {error}", path.display())))?;
    file.write_all(b"\n")
        .map_err(|error| CliError::io(format!("finish {}: {error}", path.display())))
}

fn write_result(command: &'static str, result: impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer(
        io::stdout().lock(),
        &ResultEnvelope {
            version: 1,
            command,
            result,
        },
    )
    .and_then(|()| writeln!(io::stdout().lock()).map_err(serde_json::Error::io))
    .map_err(|error| CliError::io(format!("write result: {error}")))
}

fn write_error(error: &CliError) {
    let envelope = ErrorEnvelope {
        version: 1,
        error: StructuredError {
            code: error.code,
            message: error.message.clone(),
        },
    };
    let mut stderr = io::stderr().lock();
    let _ = serde_json::to_writer(&mut stderr, &envelope);
    let _ = writeln!(stderr);
}

#[cfg(test)]
mod tests {
    use jsonschema::JSONSchema;

    use super::*;

    #[test]
    fn datetime_requires_the_stable_cli_shape() {
        assert!(parse_datetime("2026-10-10T12:34:56").is_ok());
        assert!(parse_datetime("2026-10-10 12:34:56").is_err());
    }

    #[test]
    fn manifest_encoding_is_deterministic() {
        let lines = BTreeMap::from([
            ("b.jpg".to_owned(), "b".repeat(64)),
            ("a.jpg".to_owned(), "a".repeat(64)),
        ]);
        assert_eq!(
            String::from_utf8(encode_manifest(&lines)).unwrap(),
            format!("{}  a.jpg\n{}  b.jpg\n", "a".repeat(64), "b".repeat(64))
        );
    }

    #[test]
    fn committed_import_plan_schemas_match_generated_schemas() {
        for version in 1..=IMPORT_PLAN_SCHEMA_VERSION {
            let expected =
                serde_json::to_string_pretty(&stored_plan_schema_for_version(version).unwrap())
                    .unwrap()
                    + "\n";
            let actual = fs::read_to_string(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("schema/import-plan/v{version}.json")),
            )
            .expect("committed import plan schema should be readable");

            assert_eq!(
                actual, expected,
                "regenerate the committed import plan schemas"
            );
        }
    }

    #[test]
    fn representative_saved_plans_match_the_schema_and_production_reader() {
        let schema_value: serde_json::Value = serde_json::from_slice(
            &fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/import-plan/v1.json"))
                .unwrap(),
        )
        .unwrap();
        let validator = JSONSchema::compile(&schema_value).expect("schema should compile");
        let confirmed = fixture_path("confirmed-v1.json");
        let unconfirmed = fixture_path("unconfirmed-v1.json");

        for path in [&confirmed, &unconfirmed] {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            if let Err(errors) = validator.validate(&value) {
                let errors = errors.map(|error| error.to_string()).collect::<Vec<_>>();
                panic!("fixture does not match the committed schema: {errors:?}");
            }
            read_stored_plan(path).expect("fixture should pass production validation");
        }

        let confirmed = read_stored_plan(&confirmed).unwrap();
        let unconfirmed = read_stored_plan(&unconfirmed).unwrap();
        assert!(confirmed.confirmed);
        assert!(!unconfirmed.confirmed);
        assert_ne!(confirmed.source_root, unconfirmed.source_root);
        assert_eq!(
            confirmed.import_plan.digest(),
            unconfirmed.import_plan.digest()
        );

        let schema_value: serde_json::Value = serde_json::from_slice(
            &fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/import-plan/v2.json"))
                .unwrap(),
        )
        .unwrap();
        let validator = JSONSchema::compile(&schema_value).expect("schema should compile");
        let version_two = fixture_path("confirmed-v2.json");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&version_two).unwrap()).unwrap();
        assert!(validator.is_valid(&value));
        let version_two = read_stored_plan(&version_two).unwrap();
        assert!(matches!(
            version_two.import_plan.document().items[0].disposition,
            PlanItemDisposition::Unresolved {
                finding: PlanItemFinding::AmbiguousSidecar
            }
        ));
    }

    #[test]
    fn version_one_schema_rejects_other_outer_and_inner_versions() {
        let schema_value: serde_json::Value = serde_json::from_slice(
            &fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/import-plan/v1.json"))
                .unwrap(),
        )
        .unwrap();
        let validator = JSONSchema::compile(&schema_value).expect("schema should compile");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture_path("confirmed-v1.json")).unwrap()).unwrap();

        value["schema_version"] = serde_json::json!(2);
        assert!(!validator.is_valid(&value));
        value["schema_version"] = serde_json::json!(1);
        value["import_plan"]["document"]["schema_version"] = serde_json::json!(2);
        assert!(!validator.is_valid(&value));
        value["import_plan"]["document"]["schema_version"] = serde_json::json!(1);
        value["import_plan"]["document"]["items"][0]["disposition"] =
            serde_json::json!({"kind": "unresolved", "finding": "ambiguous_sidecar"});
        assert!(!validator.is_valid(&value));
    }

    #[test]
    fn production_reader_rejects_invalid_saved_plan_envelopes() {
        let fixture: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture_path("confirmed-v1.json")).unwrap()).unwrap();
        let temporary = tempfile::tempdir().unwrap();

        let mut unsupported = fixture.clone();
        unsupported["schema_version"] = serde_json::json!(IMPORT_PLAN_SCHEMA_VERSION + 1);
        let unsupported_path = temporary.path().join("unsupported.json");
        fs::write(&unsupported_path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert!(read_stored_plan(&unsupported_path).is_err());

        let mut unknown = fixture;
        unknown["unexpected"] = serde_json::json!(true);
        let unknown_path = temporary.path().join("unknown.json");
        fs::write(&unknown_path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(read_stored_plan(&unknown_path).is_err());

        let mut mismatched: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture_path("confirmed-v1.json")).unwrap()).unwrap();
        mismatched["schema_version"] = serde_json::json!(2);
        let mismatched_path = temporary.path().join("mismatched.json");
        fs::write(&mismatched_path, serde_json::to_vec(&mismatched).unwrap()).unwrap();
        assert!(read_stored_plan(&mismatched_path).is_err());
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/import-plan")
            .join(name)
    }
}
