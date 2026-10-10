use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{error::ErrorKind, Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use souvenirs_domain::catalog::{
    allocate_collection_counters, year_directory, CanonicalFilename, CanonicalStem,
    DatedCollectionRange, NewItemCounterRequest, SourcePathOrderKey,
};
use souvenirs_domain::names::CatalogLabel;
use souvenirs_domain::plan::{
    CatalogSnapshot, ContentDigest, ImportPlan, ManifestFingerprint, PlanDateTime,
    PlanDateTimeProvenance, PlanDocument, PlanFile, PlanFileRole, PlanId, PlanItem,
    PlanItemDisposition, PlanStep, PlanStepKind, PlanTimestamp, SourceSnapshot, UnhandledContent,
    UnhandledContentFinding, IMPORT_PLAN_SCHEMA_VERSION,
};
use souvenirs_storage::{
    CatalogConfig, CatalogLayout, CatalogLock, ManifestReplacementRequest, OperationJournal,
    OperationOutcome, OperationRecord, RecoveryCopy, RecoveryDirectory, RecoveryManifest,
    RecoveryMode, RecoveryOutcome, RecoveryRequest, FORMAT_VERSION,
};
use time::{Date, PrimitiveDateTime};

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
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScannedFileKind {
    Media,
    Sidecar,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPlan {
    pub schema_version: u32,
    pub confirmed: bool,
    pub source_root: PathBuf,
    pub import_plan: ImportPlan,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStoredPlan {
    schema_version: u32,
    confirmed: bool,
    source_root: PathBuf,
    import_plan: serde_json::Value,
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
        let (digest, size_bytes) = digest_file(&absolute)?;
        let id = digest_text(&path.join("/"));
        files.push(ScannedFile {
            id,
            path,
            digest,
            size_bytes,
            kind: classify_path(&absolute),
        });
    }
    let fingerprint = scan_fingerprint(&files)?;
    let source_id = digest_text(&source_root.to_string_lossy());
    let scan = SourceScan {
        schema_version: 1,
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
    let current_scan = scan_source(&scan.source_root)?;
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
    let media: Vec<_> = scan
        .files
        .iter()
        .filter(|file| file.kind == ScannedFileKind::Media)
        .collect();
    if media.is_empty() {
        return Err(CliError::invalid_data(
            "source scan contains no supported media files",
        ));
    }
    let allocations = allocate_collection_counters(
        &BTreeMap::from([(accepted_at.date(), existing_counters)]),
        media.iter().map(|file| NewItemCounterRequest {
            item_id: file.id.clone(),
            source_path_order_key: SourcePathOrderKey::new(file.path.clone()),
            accepted_datetime: accepted_at,
        }),
    )
    .map_err(|error| CliError::invalid_data(error.to_string()))?;
    let counters: BTreeMap<_, _> = allocations
        .into_iter()
        .map(|allocation| (allocation.item_id, allocation.counter))
        .collect();

    let mut steps = Vec::new();
    let mut sequence = 1_u32;
    if !layout.root().join(&year_directory).exists() {
        steps.push(PlanStep {
            sequence,
            action: PlanStepKind::CreateDirectory {
                path: vec![year_directory.clone()],
            },
        });
        sequence += 1;
    }
    if destination_mode == DestinationMode::Create {
        steps.push(PlanStep {
            sequence,
            action: PlanStepKind::CreateDirectory {
                path: vec![year_directory.clone(), collection_directory.clone()],
            },
        });
        sequence += 1;
    }

    let mut items = Vec::new();
    let unhandled_content = scan
        .files
        .iter()
        .filter(|file| file.kind != ScannedFileKind::Media)
        .map(|file| UnhandledContent {
            source_file_id: file.id.clone(),
            source_path: file.path.clone(),
            digest: file.digest.clone(),
            size_bytes: file.size_bytes,
            finding: match file.kind {
                ScannedFileKind::Sidecar => UnhandledContentFinding::OrphanSidecar,
                ScannedFileKind::Unknown => UnhandledContentFinding::UnknownContent,
                ScannedFileKind::Media => unreachable!("media files are filtered out"),
            },
        })
        .collect::<Vec<_>>();
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
    for file in media {
        let extension = Path::new(file.path.last().expect("scanned file has a filename"))
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| CliError::invalid_data("media file has no UTF-8 extension"))?;
        let stem = CanonicalStem::new(accepted_at, counters[&file.id])
            .map_err(|error| CliError::invalid_data(error.to_string()))?;
        let filename = CanonicalFilename::new(&stem, extension)
            .map_err(|error| CliError::invalid_data(error.to_string()))?
            .to_string();
        let destination = vec![
            year_directory.clone(),
            collection_directory.clone(),
            filename.clone(),
        ];
        manifest_lines.insert(filename, file.digest.as_str().to_owned());
        steps.push(PlanStep {
            sequence,
            action: PlanStepKind::CopyFile {
                source_file_id: file.id.clone(),
                destination,
                expected_digest: file.digest.clone(),
                size_bytes: file.size_bytes,
            },
        });
        sequence += 1;
        required_space_bytes = required_space_bytes
            .checked_add(file.size_bytes)
            .ok_or_else(|| CliError::invalid_data("planned size exceeds supported range"))?;
        items.push(PlanItem {
            item_id: file.id.clone(),
            files: vec![PlanFile {
                source_file_id: file.id.clone(),
                source_path: file.path.clone(),
                digest: file.digest.clone(),
                size_bytes: file.size_bytes,
                role: PlanFileRole::Primary,
            }],
            accepted_datetime: Some(plan_datetime(accepted_at)),
            disposition: PlanItemDisposition::Transfer,
        });
    }
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
        schema_version: 1,
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
    let operation_id = stored.import_plan.document().plan_id.as_str().to_owned();
    let mut progress = ProgressWriter::new(progress_path, &operation_id)?;
    progress.write("started", None)?;

    let lock = CatalogLock::acquire(&layout)
        .map_err(|error| CliError::conflict(format!("acquire catalog lock: {error}")))?;
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
                let copied = layout
                    .copy_verified(
                        &lock,
                        &mut journal,
                        source,
                        pathbuf(destination),
                        &format!("step-{}", step.sequence),
                    )
                    .map_err(|error| CliError::operation(error.to_string()))?;
                if copied.sha256 != expected_digest.as_str() || copied.bytes_copied != *size_bytes {
                    return Err(CliError::operation(format!(
                        "verified copy result does not match plan step {}",
                        step.sequence
                    )));
                }
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
    let (journal_path, records) = find_operation_journal(&layout, operation_id)?;
    validate_recovery_state(&layout, &stored, &records, mode)?;
    let request = recovery_request(&layout, &stored, mode)?;
    let mut progress = ProgressWriter::new(progress_path, operation_id)?;
    progress.write("recovery_started", None)?;
    let lock = CatalogLock::acquire(&layout)
        .map_err(|error| CliError::conflict(format!("acquire catalog lock: {error}")))?;
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
    let mut findings = Vec::new();
    for collection in collection_directories(layout.root())? {
        let manifest_path = collection.join("SHA256SUMS");
        if !manifest_path.is_file() {
            findings.push(IntegrityFinding::MissingManifest {
                collection: relative_to(layout.root(), &collection)?,
            });
            continue;
        }
        checked_manifests += 1;
        let manifest = existing_manifest_lines(&manifest_path)?;
        let mut actual_files = BTreeSet::new();
        for entry in fs::read_dir(&collection)
            .map_err(|error| CliError::io(format!("read collection: {error}")))?
        {
            let entry = entry.map_err(|error| CliError::io(error.to_string()))?;
            if entry
                .file_type()
                .map_err(|error| CliError::io(error.to_string()))?
                .is_file()
                && entry.file_name() != "SHA256SUMS"
            {
                actual_files.insert(
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| CliError::invalid_data("catalog filename is not UTF-8"))?,
                );
            }
        }
        for (filename, expected) in &manifest {
            let path = collection.join(filename);
            if !path.is_file() {
                findings.push(IntegrityFinding::MissingFile {
                    path: relative_to(layout.root(), &path)?,
                });
                continue;
            }
            let (actual, _) = digest_file(&path)?;
            checked_files += 1;
            if actual.as_str() != expected {
                findings.push(IntegrityFinding::DigestMismatch {
                    path: relative_to(layout.root(), &path)?,
                    expected: expected.clone(),
                    actual: actual.as_str().to_owned(),
                });
            }
        }
        for filename in actual_files.difference(&manifest.keys().cloned().collect()) {
            findings.push(IntegrityFinding::UnexpectedFile {
                path: relative_to(layout.root(), &collection.join(filename))?,
            });
        }
    }
    write_result(
        "integrity.check",
        IntegrityResult {
            catalog: canonical_path(layout.root())?,
            valid: findings.is_empty(),
            checked_files,
            checked_manifests,
            findings,
        },
    )
}

fn read_stored_plan(path: &Path) -> Result<StoredPlan, CliError> {
    let raw: RawStoredPlan = read_json(path)?;
    if raw.schema_version != 1 {
        return Err(CliError::invalid_data(format!(
            "unsupported stored plan schema version {}",
            raw.schema_version
        )));
    }
    let plan_bytes = serde_json::to_vec(&raw.import_plan)
        .map_err(|error| CliError::invalid_data(format!("serialize import plan: {error}")))?;
    let import_plan = ImportPlan::from_json(&plan_bytes)
        .map_err(|error| CliError::invalid_data(format!("validate import plan: {error}")))?;
    Ok(StoredPlan {
        schema_version: raw.schema_version,
        confirmed: raw.confirmed,
        source_root: raw.source_root,
        import_plan,
    })
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
    for step in &plan.steps {
        if let PlanStepKind::CopyFile { destination, .. } = &step.action {
            let path = join_components(layout.root(), destination);
            if path.exists() {
                return Err(CliError::conflict(format!(
                    "planned destination is occupied: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
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
            destination,
            expected_digest,
            ..
        } = &step.action
        {
            if destination[..destination.len() - 1] == *parent {
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
                ..
            } => request.copies.push(RecoveryCopy {
                step_id,
                source: source_path_for_id(stored, source_file_id)?,
                destination: pathbuf(destination),
            }),
            PlanStepKind::WriteManifest {
                path,
                previous_digest,
                resulting_digest,
                ..
            } => {
                let contents =
                    manifest_contents_for_recovery(layout, &stored.import_plan, path, mode)?;
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
) -> Result<Vec<u8>, CliError> {
    if mode == RecoveryChoice::Resume {
        return manifest_contents_for_step(layout, plan, manifest_path);
    }
    let absolute = join_components(layout.root(), manifest_path);
    let mut lines = existing_manifest_lines(&absolute)?;
    let parent = &manifest_path[..manifest_path.len() - 1];
    for step in &plan.document().steps {
        if let PlanStepKind::CopyFile {
            destination,
            expected_digest,
            ..
        } = &step.action
        {
            if destination[..destination.len() - 1] != *parent {
                continue;
            }
            let destination_path = join_components(layout.root(), destination);
            if !destination_path.exists() {
                continue;
            }
            let (actual, _) = digest_file(&destination_path)?;
            if actual != *expected_digest {
                return Err(CliError::conflict(format!(
                    "published destination differs from the confirmed plan: {}",
                    destination_path.display()
                )));
            }
            lines.insert(
                destination
                    .last()
                    .expect("copy destination has filename")
                    .clone(),
                expected_digest.as_str().to_owned(),
            );
        }
    }
    Ok(encode_manifest(&lines))
}

fn validate_recovery_state(
    layout: &CatalogLayout,
    stored: &StoredPlan,
    records: &[OperationRecord],
    mode: RecoveryChoice,
) -> Result<(), CliError> {
    let plan = stored.import_plan.document();
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
    fn new(path: Option<&Path>, operation_id: &str) -> Result<Self, CliError> {
        let file = path
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
    if scan.schema_version != 1 {
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

fn scan_source(source: &Path) -> Result<SourceScan, CliError> {
    let source_root = canonical_path(source)?;
    let mut paths = Vec::new();
    collect_files(&source_root, &source_root, &mut paths)?;
    paths.sort();
    let mut files = Vec::new();
    for path in paths {
        let absolute = join_components(&source_root, &path);
        let (digest, size_bytes) = digest_file(&absolute)?;
        files.push(ScannedFile {
            id: digest_text(&path.join("/")),
            path,
            digest,
            size_bytes,
            kind: classify_path(&absolute),
        });
    }
    Ok(SourceScan {
        schema_version: 1,
        source_id: digest_text(&source_root.to_string_lossy()),
        fingerprint: scan_fingerprint(&files)?,
        source_root,
        files,
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
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if [
        "jpg", "jpeg", "heic", "cr3", "nef", "arw", "dng", "mp4", "mov",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    {
        ScannedFileKind::Media
    } else if ["xmp", "aae", "dop", "pp3"]
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    {
        ScannedFileKind::Sidecar
    } else {
        ScannedFileKind::Unknown
    }
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
    let mut disk_files = BTreeSet::new();
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
        disk_files.insert(filename);
    }
    if disk_files != manifest.keys().cloned().collect() {
        return Err(CliError::conflict(
            "existing collection files do not exactly match SHA256SUMS",
        ));
    }
    Ok(())
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
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
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
}
