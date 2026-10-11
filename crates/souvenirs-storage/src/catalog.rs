use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::macros::format_description;
use time::OffsetDateTime;

use crate::capabilities::{StorageCapabilities, StorageCapabilityError};
use crate::copy::{publish_path_exclusively, remove_apple_double_sidecar, PublishError};
use crate::durability::{sync_directory, sync_file};
use crate::lock::SystemLockEnvironment;
use crate::operation::{
    append_operation_record, is_operation_journal_file_name, read_parsed_operation_records,
    DurableTransition, Failpoint, OperationOutcome, OperationRecord,
};
use crate::{CatalogLock, LockError, OperationJournalError};

pub const FORMAT_VERSION: u32 = 2;

const BOOTSTRAP_LOCK_NAME: &str = ".souvenirs-initialize.lock";
const BOOTSTRAP_TAKEOVER_NAME: &str = ".souvenirs-initialize.lock.takeover";
const BOOTSTRAP_JOURNAL_NAME: &str = ".souvenirs-initialize.jsonl";
const CONFIG_STAGE_RELATIVE_PATH: &str = ".souvenirs/staging/initialize-config.yml.stage";
const INITIALIZE_OPERATION_ID: &str = "initialize";

macro_rules! stop_at {
    ($selected:expr, $current:ident) => {
        fail($selected, Failpoint::$current)
    };
}

const DEFAULT_MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "heic", "cr3", "nef", "arw", "dng", "mp4", "mov",
];
const DEFAULT_SIDECAR_EXTENSIONS: &[&str] = &["xmp", "aae", "dop", "pp3"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogConfig {
    pub format_version: u32,
    pub display_name: Option<String>,
    pub media_extensions: Vec<String>,
    pub sidecar_extensions: Vec<String>,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            display_name: None,
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

        let mut yaml = format!(
            "format_version: {}\nmedia_extensions: [{}]\nsidecar_extensions: [{}]\n",
            self.format_version,
            self.media_extensions.join(", "),
            self.sidecar_extensions.join(", ")
        );
        if let Some(display_name) = &self.display_name {
            yaml.push_str("display_name: ");
            yaml.push_str(
                &serde_json::to_string(display_name)
                    .map_err(CatalogInitializationError::SerializeConfig)?,
            );
            yaml.push('\n');
        }
        Ok(yaml)
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
        Self::initialize_with_failpoint(root.into(), config, None)
    }

    pub fn resume_initialization(
        root: impl Into<PathBuf>,
    ) -> Result<Option<Self>, CatalogInitializationError> {
        let layout = Self::new(root);
        layout.validate_root()?;
        if !layout.has_pending_initialization()? {
            return Ok(None);
        }
        let capabilities = layout.capabilities()?;
        capabilities.ensure_writable(&layout.root)?;
        validate_bootstrap_preparation(&layout)?;
        let resumed = layout.with_bootstrap_lock(|| layout.resume_initialization_locked())?;
        Ok(resumed.then_some(layout))
    }

    pub fn has_pending_initialization(&self) -> Result<bool, CatalogInitializationError> {
        let Some(path) = initialization_journal_path(self)? else {
            return Ok(false);
        };
        let parsed = read_parsed_operation_records(&path, "inspect initialization journal")
            .map_err(CatalogInitializationError::Journal)?;
        if let Some((format_version, completed)) = parsed.legacy_initialization {
            verify_legacy_initialization(self, &path, format_version, !completed)?;
            return Ok(!completed);
        }
        if parsed.records.is_empty() {
            return Ok(false);
        }
        let plan = initialization_plan(&path, &parsed.records)?;
        plan.validate(&path)?;
        if validate_initialization_outcome(&path, &parsed.records)? {
            return Ok(false);
        }
        if path_exists(&self.config_path())? {
            verify_file_digest(&self.config_path(), &plan.config_sha256)?;
        }
        Ok(true)
    }

    #[cfg(feature = "test-failpoints")]
    pub fn initialize_failing_at(
        root: impl Into<PathBuf>,
        config: &CatalogConfig,
        failpoint: crate::Failpoint,
    ) -> Result<Self, CatalogInitializationError> {
        Self::initialize_with_failpoint(root.into(), config, Some(failpoint))
    }

    fn initialize_with_failpoint(
        root: PathBuf,
        config: &CatalogConfig,
        failpoint: Option<Failpoint>,
    ) -> Result<Self, CatalogInitializationError> {
        let layout = Self::new(root);
        layout.initialize_inner(config, failpoint)?;
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

    pub fn index_path(&self) -> PathBuf {
        self.state_directory().join("index.sqlite")
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

    fn bootstrap_lock_path(&self) -> PathBuf {
        self.root.join(BOOTSTRAP_LOCK_NAME)
    }

    fn bootstrap_takeover_path(&self) -> PathBuf {
        self.root.join(BOOTSTRAP_TAKEOVER_NAME)
    }

    fn bootstrap_journal_path(&self) -> PathBuf {
        self.root.join(BOOTSTRAP_JOURNAL_NAME)
    }

    fn initialize_inner(
        &self,
        config: &CatalogConfig,
        failpoint: Option<Failpoint>,
    ) -> Result<(), CatalogInitializationError> {
        let config_yaml = config.to_yaml()?;
        self.validate_root()?;
        let capabilities = self.capabilities()?;
        capabilities.ensure_writable(&self.root)?;
        validate_bootstrap_preparation(self)?;
        self.has_pending_initialization()?;
        self.with_bootstrap_lock(|| {
            self.initialize_locked(config, config_yaml.as_bytes(), failpoint)
        })
    }

    fn with_bootstrap_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, CatalogInitializationError>,
    ) -> Result<T, CatalogInitializationError> {
        let (lock, takeover) = CatalogLock::acquire_paths_with_takeover(
            self.bootstrap_lock_path(),
            self.bootstrap_takeover_path(),
            &SystemLockEnvironment,
        )
        .map_err(CatalogInitializationError::Lock)?;
        let result = operation();
        #[cfg(feature = "test-failpoints")]
        if matches!(result, Err(CatalogInitializationError::Failpoint(_))) {
            if let Some(takeover) = takeover {
                let _ = takeover.remove();
            }
            lock.abandon();
            return result;
        }
        let cleanup = takeover
            .map(|guard| guard.remove())
            .transpose()
            .map_err(CatalogInitializationError::Lock);
        match result {
            Ok(value) => {
                cleanup?;
                lock.release().map_err(CatalogInitializationError::Lock)?;
                Ok(value)
            }
            Err(error) => {
                let _ = cleanup;
                drop(lock);
                Err(error)
            }
        }
    }

    fn validate_root(&self) -> Result<(), CatalogInitializationError> {
        let root_metadata = fs::metadata(&self.root)
            .map_err(|source| io_error("inspect catalog root", &self.root, source))?;
        if !root_metadata.is_dir() {
            return Err(CatalogInitializationError::RootIsNotDirectory(
                self.root.clone(),
            ));
        }
        Ok(())
    }

    fn initialize_locked(
        &self,
        config: &CatalogConfig,
        config_bytes: &[u8],
        failpoint: Option<Failpoint>,
    ) -> Result<(), CatalogInitializationError> {
        let discovered = discover_initialization_journal(self)?;
        let (journal, plan) = match discovered {
            Some(DiscoveredJournal::Legacy(path, format_version, unfinished)) => {
                let confirmed =
                    verify_legacy_initialization(self, &path, format_version, unfinished)?;
                if config_bytes != confirmed.as_bytes() {
                    return Err(CatalogInitializationError::ConfigurationMismatch {
                        confirmed_display_name: config_display_name(&confirmed),
                        confirmed_config_yaml: confirmed,
                    });
                }
                normalize_legacy_initialization(&path, unfinished)?;
                return Ok(());
            }
            Some(DiscoveredJournal::Preparation(path)) => {
                fs::remove_file(&path).map_err(|source| {
                    io_error(
                        "remove interrupted bootstrap journal preparation",
                        &path,
                        source,
                    )
                })?;
                stop_at!(
                    failpoint,
                    InitializationRecoveryCleanupObservedBeforePersistence
                )?;
                persist(
                    path.parent().expect("bootstrap preparation has a parent"),
                    "persist bootstrap preparation cleanup",
                )?;
                ensure_absent(&self.config_path())?;
                ensure_absent(&self.state_directory())?;
                self.start_initialization(config, config_bytes, failpoint)?
            }
            Some(DiscoveredJournal::Valid(journal, plan)) => (journal, plan),
            None => {
                ensure_absent(&self.config_path())?;
                ensure_absent(&self.state_directory())?;
                self.start_initialization(config, config_bytes, failpoint)?
            }
        };

        if plan.config_yaml.as_bytes() != config_bytes
            || plan.format_version != config.format_version
            || plan.config_sha256 != digest_bytes(config_bytes)
        {
            return Err(CatalogInitializationError::ConfigurationMismatch {
                confirmed_display_name: config_display_name(&plan.config_yaml),
                confirmed_config_yaml: plan.config_yaml.clone(),
            });
        }
        self.resume_initialization_journal(journal, plan, failpoint)
    }

    fn resume_initialization_locked(&self) -> Result<bool, CatalogInitializationError> {
        let (journal, plan) = match discover_initialization_journal(self)? {
            Some(DiscoveredJournal::Valid(journal, plan)) => (journal, plan),
            Some(DiscoveredJournal::Legacy(path, format_version, unfinished)) => {
                verify_legacy_initialization(self, &path, format_version, unfinished)?;
                normalize_legacy_initialization(&path, unfinished)?;
                return Ok(unfinished);
            }
            _ => return Ok(false),
        };
        let pending = !validate_initialization_outcome(&journal.path, &journal.records)?;
        self.resume_initialization_journal(journal, plan, None)?;
        Ok(pending)
    }

    fn resume_initialization_journal(
        &self,
        mut journal: InitializationJournal,
        plan: InitializationPlan,
        failpoint: Option<Failpoint>,
    ) -> Result<(), CatalogInitializationError> {
        plan.validate(&journal.path)?;
        if validate_initialization_outcome(&journal.path, &journal.records)? {
            verify_completed_initialization(self, &journal, &plan)?;
            return Ok(());
        }

        for (path, step, intent, mutation, completed) in [
            (
                ".souvenirs",
                "initialize-state",
                Failpoint::InitializationStateDirectoryIntentPersisted,
                Failpoint::InitializationStateDirectoryMutationPersisted,
                Failpoint::InitializationStateDirectoryCompleted,
            ),
            (
                ".souvenirs/journal",
                "initialize-journal",
                Failpoint::InitializationJournalDirectoryIntentPersisted,
                Failpoint::InitializationJournalDirectoryMutationPersisted,
                Failpoint::InitializationJournalDirectoryCompleted,
            ),
        ] {
            ensure_initialization_directory(
                self,
                &mut journal,
                path,
                step,
                failpoint,
                intent,
                mutation,
                completed,
            )?;
        }
        relocate_initialization_journal(self, &mut journal, &plan, failpoint)?;
        ensure_initialization_directory(
            self,
            &mut journal,
            ".souvenirs/staging",
            "initialize-staging",
            failpoint,
            Failpoint::InitializationStagingDirectoryIntentPersisted,
            Failpoint::InitializationStagingDirectoryMutationPersisted,
            Failpoint::InitializationStagingDirectoryCompleted,
        )?;
        stage_configuration(self, &mut journal, &plan, failpoint)?;
        publish_configuration(self, &mut journal, &plan, failpoint)?;

        journal.append(OperationRecord::Outcome {
            outcome: OperationOutcome::Completed,
        })?;
        stop_at!(failpoint, InitializationOutcomePersisted)?;
        verify_completed_initialization(self, &journal, &plan)?;
        Ok(())
    }

    fn start_initialization(
        &self,
        config: &CatalogConfig,
        config_bytes: &[u8],
        failpoint: Option<Failpoint>,
    ) -> Result<(InitializationJournal, InitializationPlan), CatalogInitializationError> {
        let plan = InitializationPlan {
            format_version: config.format_version,
            config_yaml: String::from_utf8(config_bytes.to_vec())
                .expect("catalog configuration is UTF-8"),
            config_sha256: digest_bytes(config_bytes),
            journal_file_name: format!("{}-initialize.jsonl", operation_identifier()?),
            staged_config_path: PathBuf::from(CONFIG_STAGE_RELATIVE_PATH),
        };
        let path = self.bootstrap_journal_path();
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| io_error("create bootstrap journal exclusively", &path, source))?;
        persist(&self.root, "persist bootstrap journal entry")?;
        stop_at!(failpoint, InitializationJournalCreated)?;
        let mut journal = InitializationJournal {
            path,
            file,
            records: Vec::new(),
        };
        journal.append(OperationRecord::Plan {
            operation_id: INITIALIZE_OPERATION_ID.to_owned(),
            plan: serde_json::to_value(&plan).map_err(CatalogInitializationError::SerializePlan)?,
        })?;
        stop_at!(failpoint, InitializationPlanPersisted)?;
        Ok((journal, plan))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct InitializationPlan {
    format_version: u32,
    config_yaml: String,
    config_sha256: String,
    journal_file_name: String,
    staged_config_path: PathBuf,
}

impl InitializationPlan {
    fn validate(&self, path: &Path) -> Result<(), CatalogInitializationError> {
        if self.staged_config_path != Path::new(CONFIG_STAGE_RELATIVE_PATH)
            || !self.journal_file_name.ends_with("-initialize.jsonl")
            || self.journal_file_name.contains('/')
            || self.journal_file_name.contains('\\')
        {
            return Err(CatalogInitializationError::InvalidPlan);
        }
        if self.format_version != FORMAT_VERSION {
            return Err(CatalogInitializationError::UnsupportedFormatVersion(
                self.format_version,
            ));
        }
        let config_version = parse_config_format_version(&self.config_yaml).ok_or_else(|| {
            invalid_journal(
                path,
                "initialization plan contains an invalid catalog configuration",
            )
        })?;
        if config_version != self.format_version {
            return Err(invalid_journal(
                path,
                "initialization plan and catalog configuration versions differ",
            ));
        }
        if self.config_sha256 != digest_bytes(self.config_yaml.as_bytes()) {
            return Err(CatalogInitializationError::InvalidPlan);
        }
        Ok(())
    }

    fn journal_relative_path(&self) -> PathBuf {
        Path::new(".souvenirs")
            .join("journal")
            .join(&self.journal_file_name)
    }
}

struct InitializationJournal {
    path: PathBuf,
    file: File,
    records: Vec<OperationRecord>,
}

impl InitializationJournal {
    fn open(
        path: PathBuf,
        parsed: crate::operation::ParsedOperationRecords,
    ) -> Result<Self, CatalogInitializationError> {
        discard_interrupted_initialization_append(&path, &parsed)?;
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|source| {
                io_error("open initialization journal for recovery", &path, source)
            })?;
        Ok(Self {
            path,
            file,
            records: parsed.records,
        })
    }

    fn append(&mut self, record: OperationRecord) -> Result<(), CatalogInitializationError> {
        append_operation_record(&mut self.file, &self.path, &record)
            .map_err(CatalogInitializationError::Journal)?;
        self.records.push(record);
        Ok(())
    }

    fn transition(
        &mut self,
        transition: DurableTransition,
    ) -> Result<(), CatalogInitializationError> {
        self.append(OperationRecord::Transition { transition })
    }
    fn has_transition(&self, expected: &DurableTransition) -> bool {
        self.records.iter().any(|record| {
            matches!(
                record,
                OperationRecord::Transition { transition } if transition == expected
            )
        })
    }
}

enum DiscoveredJournal {
    Legacy(PathBuf, u32, bool),
    Preparation(PathBuf),
    Valid(InitializationJournal, InitializationPlan),
}

fn discover_initialization_journal(
    layout: &CatalogLayout,
) -> Result<Option<DiscoveredJournal>, CatalogInitializationError> {
    let Some(path) = initialization_journal_path(layout)? else {
        return Ok(None);
    };
    let parsed = read_parsed_operation_records(&path, "read initialization journal for recovery")
        .map_err(CatalogInitializationError::Journal)?;
    if let Some((format_version, completed)) = parsed.legacy_initialization {
        return Ok(Some(DiscoveredJournal::Legacy(
            path,
            format_version,
            !completed,
        )));
    }
    if parsed.records.is_empty() && path == layout.bootstrap_journal_path() {
        return Ok(Some(DiscoveredJournal::Preparation(path)));
    }
    if parsed.records.is_empty() {
        return Err(invalid_journal(
            &path,
            "relocated initialization journal is empty",
        ));
    }
    let plan = initialization_plan(&path, &parsed.records)?;
    plan.validate(&path)?;
    let journal = InitializationJournal::open(path, parsed)?;
    Ok(Some(DiscoveredJournal::Valid(journal, plan)))
}

fn verify_legacy_initialization(
    layout: &CatalogLayout,
    path: &Path,
    format_version: u32,
    unfinished: bool,
) -> Result<String, CatalogInitializationError> {
    if unfinished && !path_exists(&layout.config_path())? {
        return Err(invalid_journal(
            path,
            "legacy initialization stopped before configuration publication and cannot be resumed because its full configuration was not recorded",
        ));
    }
    let mut required = vec![layout.state_directory(), layout.journal_directory()];
    if unfinished {
        required.push(layout.staging_directory());
    }
    if path.parent() != Some(layout.journal_directory().as_path())
        || required.iter().any(|required| {
            !fs::symlink_metadata(required).is_ok_and(|metadata| metadata.file_type().is_dir())
        })
        || !fs::symlink_metadata(layout.config_path())
            .is_ok_and(|metadata| metadata.file_type().is_file())
    {
        return Err(invalid_journal(
            path,
            "legacy initialization journal conflicts with published catalog state",
        ));
    }
    let config = fs::read_to_string(layout.config_path()).map_err(|source| {
        io_error(
            "read legacy catalog configuration",
            &layout.config_path(),
            source,
        )
    })?;
    let config_version = parse_config_format_version(&config);
    if format_version != FORMAT_VERSION || config_version != Some(format_version) {
        return Err(invalid_journal(
            path,
            "legacy initialization journal does not match the catalog configuration",
        ));
    }
    Ok(config)
}
fn normalize_legacy_initialization(
    path: &Path,
    unfinished: bool,
) -> Result<(), CatalogInitializationError> {
    let parsed = read_parsed_operation_records(path, "read legacy initialization journal")
        .map_err(CatalogInitializationError::Journal)?;
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|source| io_error("open legacy initialization journal", path, source))?;
    if parsed.interrupted_tail {
        file.set_len(parsed.complete_length)
            .and_then(|()| sync_file(&file))
            .map_err(|source| io_error("discard interrupted journal append", path, source))?;
    }
    if unfinished {
        append_operation_record(
            &mut file,
            path,
            &OperationRecord::Outcome {
                outcome: OperationOutcome::Completed,
            },
        )
        .map_err(CatalogInitializationError::Journal)?;
    }
    Ok(())
}
fn config_display_name(config: &str) -> Option<String> {
    config.lines().find_map(|line| {
        line.strip_prefix("display_name: ")
            .and_then(|value| serde_json::from_str(value).ok())
    })
}

fn parse_config_format_version(config: &str) -> Option<u32> {
    config
        .lines()
        .find_map(|line| line.strip_prefix("format_version: "))
        .and_then(|value| value.parse().ok())
}
fn initialization_journal_path(
    layout: &CatalogLayout,
) -> Result<Option<PathBuf>, CatalogInitializationError> {
    let root_path = layout.bootstrap_journal_path();
    let root_exists = regular_file_if_exists(&root_path)?;
    let mut final_paths = Vec::new();
    if directory_if_exists(&layout.state_directory())?
        && directory_if_exists(&layout.journal_directory())?
    {
        for entry in fs::read_dir(layout.journal_directory()).map_err(|source| {
            io_error(
                "scan initialization journals",
                &layout.journal_directory(),
                source,
            )
        })? {
            let path = entry
                .map_err(|source| {
                    io_error(
                        "read initialization journal entry",
                        &layout.journal_directory(),
                        source,
                    )
                })?
                .path();
            if path.file_name().is_some_and(|name| {
                is_operation_journal_file_name(name)
                    && name
                        .to_str()
                        .is_some_and(|name| name.ends_with("-initialize.jsonl"))
            }) {
                if !regular_file_if_exists(&path)? {
                    return Err(CatalogInitializationError::PathConflict(path));
                }
                final_paths.push(path);
            }
        }
    }
    if usize::from(root_exists) + final_paths.len() > 1 {
        return Err(CatalogInitializationError::MultipleBootstrapJournals);
    }
    let Some(path) = root_exists
        .then_some(root_path)
        .or_else(|| final_paths.pop())
    else {
        return Ok(None);
    };
    Ok(Some(path))
}

fn initialization_plan(
    path: &Path,
    records: &[OperationRecord],
) -> Result<InitializationPlan, CatalogInitializationError> {
    let raw_plan = match records.first() {
        Some(OperationRecord::Plan { operation_id, plan })
            if operation_id == INITIALIZE_OPERATION_ID =>
        {
            plan
        }
        _ => {
            return Err(invalid_journal(
                path,
                "initialization journal does not start with the initialize plan",
            ));
        }
    };
    if let Some(version) = raw_plan
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .filter(|version| *version != FORMAT_VERSION)
    {
        return Err(CatalogInitializationError::UnsupportedFormatVersion(
            version,
        ));
    }
    serde_json::from_value(raw_plan.clone()).map_err(|source| {
        CatalogInitializationError::DeserializePlan {
            path: path.to_path_buf(),
            source,
        }
    })
}
fn discard_interrupted_initialization_append(
    path: &Path,
    parsed: &crate::operation::ParsedOperationRecords,
) -> Result<(), CatalogInitializationError> {
    if !parsed.interrupted_tail {
        return Ok(());
    }
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| io_error("open initialization journal for recovery", path, source))?;
    file.set_len(parsed.complete_length)
        .and_then(|()| sync_file(&file))
        .map_err(|source| io_error("discard interrupted journal append", path, source))
}

fn validate_initialization_outcome(
    path: &Path,
    records: &[OperationRecord],
) -> Result<bool, CatalogInitializationError> {
    let Some((index, outcome)) =
        records
            .iter()
            .enumerate()
            .find_map(|(index, record)| match record {
                OperationRecord::Outcome { outcome } => Some((index, *outcome)),
                _ => None,
            })
    else {
        return Ok(false);
    };
    if index + 1 != records.len() || outcome != OperationOutcome::Completed {
        return Err(invalid_journal(
            path,
            "initialization outcome is not one terminal completed record",
        ));
    }
    Ok(true)
}

fn invalid_journal(path: &Path, details: &'static str) -> CatalogInitializationError {
    CatalogInitializationError::InvalidBootstrapJournal {
        path: path.to_path_buf(),
        details,
    }
}

#[allow(clippy::too_many_arguments)]
fn ensure_initialization_directory(
    layout: &CatalogLayout,
    journal: &mut InitializationJournal,
    relative_path: &str,
    step_id: &str,
    failpoint: Option<Failpoint>,
    intent_failpoint: Failpoint,
    mutation_failpoint: Failpoint,
    completed_failpoint: Failpoint,
) -> Result<(), CatalogInitializationError> {
    let relative_path = PathBuf::from(relative_path);
    let path = layout.root.join(&relative_path);
    let intent = DurableTransition::DirectoryCreationStarted {
        step_id: step_id.to_owned(),
        path: relative_path.clone(),
    };
    let completed = DurableTransition::DirectoryCreated {
        step_id: step_id.to_owned(),
        path: relative_path.clone(),
    };
    if journal.has_transition(&completed) {
        ensure_directory(&path)?;
        return Ok(());
    }
    if !journal.has_transition(&intent) {
        if path_exists(&path)? {
            return Err(CatalogInitializationError::PathConflict(path));
        }
        journal.transition(intent)?;
        fail(failpoint, intent_failpoint)?;
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            stop_at!(
                failpoint,
                InitializationRecoveryDirectoryObservedBeforePersistence
            )?;
            persist(
                path.parent()
                    .expect("initialization directory has a parent"),
                "persist observed initialization directory",
            )?;
        }
        Ok(_) => return Err(CatalogInitializationError::PathConflict(path)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&path).map_err(|source| {
                io_error("create initialization directory exclusively", &path, source)
            })?;
            persist(
                path.parent()
                    .expect("initialization directory has a parent"),
                "persist initialization directory",
            )?;
            fail(failpoint, mutation_failpoint)?;
        }
        Err(source) => return Err(io_error("inspect initialization directory", &path, source)),
    }
    journal.transition(completed)?;
    fail(failpoint, completed_failpoint)
}

fn relocate_initialization_journal(
    layout: &CatalogLayout,
    journal: &mut InitializationJournal,
    plan: &InitializationPlan,
    failpoint: Option<Failpoint>,
) -> Result<(), CatalogInitializationError> {
    let relative_destination = plan.journal_relative_path();
    let destination = layout.root.join(&relative_destination);
    let intent = DurableTransition::InitializationJournalRelocationStarted {
        destination: relative_destination.clone(),
    };
    let completed = DurableTransition::InitializationJournalRelocated {
        destination: relative_destination,
    };
    if journal.has_transition(&completed) {
        if journal.path != destination || !regular_file_if_exists(&destination)? {
            return Err(CatalogInitializationError::PathConflict(destination));
        }
        ensure_absent(&layout.bootstrap_journal_path())?;
        return Ok(());
    }
    if !journal.has_transition(&intent) {
        if journal.path != layout.bootstrap_journal_path() || path_exists(&destination)? {
            return Err(CatalogInitializationError::PathConflict(destination));
        }
        journal.transition(intent)?;
        stop_at!(failpoint, InitializationJournalRelocationIntentPersisted)?;
    }
    let root_exists = regular_file_if_exists(&layout.bootstrap_journal_path())?;
    let final_exists = regular_file_if_exists(&destination)?;
    match (root_exists, final_exists) {
        (true, false) => {
            publish_path_exclusively(&layout.bootstrap_journal_path(), &destination)
                .map_err(|error| publication_error(error, &destination))?;
            remove_sidecar(
                &layout.bootstrap_journal_path(),
                "remove bootstrap journal AppleDouble sidecar",
            )?;
            persist(&layout.root, "persist bootstrap journal removal")?;
            persist(
                &layout.journal_directory(),
                "persist relocated initialization journal",
            )?;
            journal.path = destination.clone();
            stop_at!(failpoint, InitializationJournalRelocationMutationPersisted)?;
        }
        (false, true) => {
            if journal.path != destination {
                return Err(CatalogInitializationError::PathConflict(destination));
            }
            remove_sidecar(
                &layout.bootstrap_journal_path(),
                "remove observed bootstrap journal AppleDouble sidecar",
            )?;
            stop_at!(
                failpoint,
                InitializationRecoveryRelocationObservedBeforePersistence
            )?;
            persist(&layout.root, "persist observed bootstrap journal removal")?;
            persist(
                &layout.journal_directory(),
                "persist observed relocated initialization journal",
            )?;
        }
        _ => return Err(CatalogInitializationError::MultipleBootstrapJournals),
    }
    journal.transition(completed)?;
    stop_at!(failpoint, InitializationJournalRelocated)
}

fn stage_configuration(
    layout: &CatalogLayout,
    journal: &mut InitializationJournal,
    plan: &InitializationPlan,
    failpoint: Option<Failpoint>,
) -> Result<(), CatalogInitializationError> {
    let staged_path = layout.root.join(&plan.staged_config_path);
    let intent = DurableTransition::InitializationConfigStageStarted {
        staged_path: plan.staged_config_path.clone(),
        sha256: plan.config_sha256.clone(),
    };
    let completed = DurableTransition::InitializationConfigStaged {
        staged_path: plan.staged_config_path.clone(),
        sha256: plan.config_sha256.clone(),
    };
    if journal.has_transition(&completed) {
        if !path_exists(&layout.config_path())? {
            verify_file_digest(&staged_path, &plan.config_sha256)?;
        }
        return Ok(());
    }
    if !journal.has_transition(&intent) {
        if path_exists(&staged_path)? {
            return Err(CatalogInitializationError::PathConflict(staged_path));
        }
        journal.transition(intent)?;
        stop_at!(failpoint, InitializationConfigStageIntentPersisted)?;
    }
    if path_exists(&staged_path)? {
        match file_digest(&staged_path) {
            Ok(actual) if actual == plan.config_sha256 => {
                stop_at!(
                    failpoint,
                    InitializationRecoveryConfigStageObservedBeforePersistence
                )?;
                let file = File::open(&staged_path).map_err(|source| {
                    io_error("open observed staged configuration", &staged_path, source)
                })?;
                sync_file(&file).map_err(|source| {
                    io_error(
                        "persist observed staged configuration",
                        &staged_path,
                        source,
                    )
                })?;
                persist(
                    &layout.staging_directory(),
                    "persist observed configuration stage",
                )?;
            }
            Ok(actual) => {
                return Err(CatalogInitializationError::UnexpectedDigest {
                    path: staged_path,
                    expected: plan.config_sha256.clone(),
                    actual,
                });
            }
            Err(error) => return Err(error),
        }
    } else {
        write_stage(layout, &staged_path, plan.config_yaml.as_bytes())?;
        stop_at!(failpoint, InitializationConfigStageMutationPersisted)?;
    }
    journal.transition(completed)?;
    stop_at!(failpoint, InitializationConfigStaged)
}

fn publish_configuration(
    layout: &CatalogLayout,
    journal: &mut InitializationJournal,
    plan: &InitializationPlan,
    failpoint: Option<Failpoint>,
) -> Result<(), CatalogInitializationError> {
    let staged_path = layout.root.join(&plan.staged_config_path);
    let destination = layout.config_path();
    let destination_relative = PathBuf::from(".souvenirs.yml");
    let intent = DurableTransition::InitializationConfigPublicationStarted {
        staged_path: plan.staged_config_path.clone(),
        destination: destination_relative.clone(),
        sha256: plan.config_sha256.clone(),
    };
    let completed = DurableTransition::InitializationConfigPublished {
        destination: destination_relative,
        sha256: plan.config_sha256.clone(),
    };
    if journal.has_transition(&completed) {
        verify_file_digest(&destination, &plan.config_sha256)?;
        ensure_absent(&staged_path)?;
        return Ok(());
    }
    if !journal.has_transition(&intent) {
        if path_exists(&destination)? {
            return Err(CatalogInitializationError::PathConflict(destination));
        }
        journal.transition(intent)?;
        stop_at!(failpoint, InitializationConfigPublicationIntentPersisted)?;
    }
    match (path_exists(&staged_path)?, path_exists(&destination)?) {
        (true, false) => {
            verify_file_digest(&staged_path, &plan.config_sha256)?;
            publish_path_exclusively(&staged_path, &destination)
                .map_err(|error| publication_error(error, &destination))?;
            remove_sidecar(
                &staged_path,
                "remove initialization stage AppleDouble sidecar",
            )?;
            persist(
                &layout.staging_directory(),
                "persist configuration stage publication",
            )?;
            persist(&layout.root, "persist catalog configuration publication")?;
            stop_at!(failpoint, InitializationConfigPublicationMutationPersisted)?;
        }
        (false, true) => {
            verify_file_digest(&destination, &plan.config_sha256)?;
            remove_sidecar(
                &staged_path,
                "remove observed initialization stage AppleDouble sidecar",
            )?;
            stop_at!(
                failpoint,
                InitializationRecoveryConfigObservedBeforePersistence
            )?;
            persist(
                &layout.staging_directory(),
                "persist observed configuration stage removal",
            )?;
            persist(
                &layout.root,
                "persist observed catalog configuration publication",
            )?;
        }
        _ => return Err(CatalogInitializationError::PathConflict(destination)),
    }
    journal.transition(completed)?;
    stop_at!(failpoint, InitializationConfigPublished)
}

fn write_stage(
    layout: &CatalogLayout,
    path: &Path,
    bytes: &[u8],
) -> Result<(), CatalogInitializationError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| io_error("create configuration stage exclusively", path, source))?;
    file.write_all(bytes)
        .and_then(|()| sync_file(&file))
        .map_err(|source| io_error("write and persist staged configuration", path, source))?;
    persist(
        &layout.staging_directory(),
        "persist staged configuration entry",
    )
}

fn remove_sidecar(path: &Path, action: &'static str) -> Result<(), CatalogInitializationError> {
    remove_apple_double_sidecar(path).map_err(|source| io_error(action, path, source))
}

fn validate_bootstrap_preparation(
    layout: &CatalogLayout,
) -> Result<(), CatalogInitializationError> {
    regular_file_if_exists(&layout.bootstrap_lock_path())?;
    regular_file_if_exists(&layout.bootstrap_journal_path())?;
    regular_file_if_exists(&layout.bootstrap_takeover_path())?;
    Ok(())
}

fn verify_completed_initialization(
    layout: &CatalogLayout,
    journal: &InitializationJournal,
    plan: &InitializationPlan,
) -> Result<(), CatalogInitializationError> {
    let expected_journal = layout.root.join(plan.journal_relative_path());
    if journal.path != expected_journal {
        return Err(CatalogInitializationError::PathConflict(expected_journal));
    }
    ensure_directory(&layout.state_directory())?;
    ensure_directory(&layout.journal_directory())?;
    ensure_directory(&layout.staging_directory())?;
    verify_file_digest(&layout.config_path(), &plan.config_sha256)?;
    ensure_absent(&layout.bootstrap_journal_path())?;
    ensure_absent(&layout.root.join(&plan.staged_config_path))
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

fn operation_identifier() -> Result<String, CatalogInitializationError> {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .map_err(|source| CatalogInitializationError::Clock {
            details: source.to_string(),
        })
}

fn fail(selected: Option<Failpoint>, current: Failpoint) -> Result<(), CatalogInitializationError> {
    #[cfg(feature = "test-failpoints")]
    if selected == Some(current) {
        return Err(CatalogInitializationError::Failpoint(current));
    }
    let _ = (selected, current);
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file_digest(path: &Path) -> Result<String, CatalogInitializationError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| io_error("inspect initialization file", path, source))?;
    if !metadata.file_type().is_file() {
        return Err(CatalogInitializationError::PathConflict(path.to_path_buf()));
    }
    let bytes =
        fs::read(path).map_err(|source| io_error("read initialization file", path, source))?;
    Ok(digest_bytes(&bytes))
}

fn verify_file_digest(path: &Path, expected: &str) -> Result<(), CatalogInitializationError> {
    let actual = file_digest(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CatalogInitializationError::UnexpectedDigest {
            path: path.to_path_buf(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn ensure_directory(path: &Path) -> Result<(), CatalogInitializationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(CatalogInitializationError::PathConflict(path.to_path_buf())),
        Err(source) => Err(io_error("inspect initialization directory", path, source)),
    }
}

fn ensure_absent(path: &Path) -> Result<(), CatalogInitializationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(CatalogInitializationError::AlreadyExists(
            path.to_path_buf(),
        )),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("inspect initialization destination", path, source)),
    }
}

fn path_exists(path: &Path) -> Result<bool, CatalogInitializationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("inspect initialization path", path, source)),
    }
}

fn regular_file_if_exists(path: &Path) -> Result<bool, CatalogInitializationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(CatalogInitializationError::PathConflict(path.to_path_buf())),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("inspect initialization journal", path, source)),
    }
}

fn directory_if_exists(path: &Path) -> Result<bool, CatalogInitializationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(CatalogInitializationError::PathConflict(path.to_path_buf())),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error(
            "inspect initialization journal directory",
            path,
            source,
        )),
    }
}

fn persist(path: &Path, action: &'static str) -> Result<(), CatalogInitializationError> {
    sync_directory(path).map_err(|source| io_error(action, path, source))
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> CatalogInitializationError {
    CatalogInitializationError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

fn publication_error(error: PublishError, path: &Path) -> CatalogInitializationError {
    match error {
        PublishError::Occupied => CatalogInitializationError::AlreadyExists(path.to_path_buf()),
        PublishError::Io(source) => {
            io_error("publish initialization artifact exclusively", path, source)
        }
    }
}

#[derive(Debug)]
pub enum CatalogInitializationError {
    AlreadyExists(PathBuf),
    Clock {
        details: String,
    },
    ConfigurationMismatch {
        confirmed_display_name: Option<String>,
        confirmed_config_yaml: String,
    },
    DeserializePlan {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[cfg(feature = "test-failpoints")]
    Failpoint(crate::Failpoint),
    InvalidBootstrapJournal {
        path: PathBuf,
        details: &'static str,
    },
    InvalidExtension(String),
    InvalidPlan,
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Journal(OperationJournalError),
    Lock(LockError),
    MultipleBootstrapJournals,
    OverlappingExtensions,
    PathConflict(PathBuf),
    RootIsNotDirectory(PathBuf),
    SerializeConfig(serde_json::Error),
    SerializePlan(serde_json::Error),
    StorageCapability(StorageCapabilityError),
    UnexpectedDigest {
        path: PathBuf,
        expected: String,
        actual: String,
    },
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
            Self::ConfigurationMismatch {
                confirmed_display_name,
                ..
            } => {
                formatter.write_str(
                    "catalog initialization configuration differs from the confirmed configuration",
                )?;
                if let Some(display_name) = confirmed_display_name {
                    write!(formatter, " for {display_name:?}")?;
                }
                Ok(())
            }
            Self::DeserializePlan { path, source } => write!(
                formatter,
                "failed to deserialize initialization plan {}: {source}",
                path.display()
            ),
            #[cfg(feature = "test-failpoints")]
            Self::Failpoint(failpoint) => {
                write!(formatter, "test failpoint reached: {failpoint:?}")
            }
            Self::InvalidBootstrapJournal { path, details } => write!(
                formatter,
                "invalid initialization journal {}: {details}",
                path.display()
            ),
            Self::InvalidExtension(extension) => {
                write!(formatter, "invalid catalog extension {extension:?}")
            }
            Self::InvalidPlan => formatter.write_str("initialization plan is invalid"),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Journal(source) => source.fmt(formatter),
            Self::Lock(source) => write!(formatter, "failed to manage bootstrap lock: {source}"),
            Self::MultipleBootstrapJournals => {
                formatter.write_str("multiple initialization journals are present")
            }
            Self::OverlappingExtensions => formatter.write_str(
                "media and sidecar extensions must not overlap under ASCII case folding",
            ),
            Self::PathConflict(path) => write!(
                formatter,
                "catalog initialization found unexpected content at {}",
                path.display()
            ),
            Self::RootIsNotDirectory(path) => {
                write!(
                    formatter,
                    "catalog root is not a directory: {}",
                    path.display()
                )
            }
            Self::SerializeConfig(source) => {
                write!(
                    formatter,
                    "failed to serialize catalog configuration: {source}"
                )
            }
            Self::SerializePlan(source) => {
                write!(
                    formatter,
                    "failed to serialize initialization plan: {source}"
                )
            }
            Self::StorageCapability(source) => source.fmt(formatter),
            Self::UnexpectedDigest {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "initialization content at {} has SHA-256 {actual}; expected {expected}",
                path.display()
            ),
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
            Self::DeserializePlan { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Journal(source) => Some(source),
            Self::Lock(source) => Some(source),
            Self::SerializeConfig(source) | Self::SerializePlan(source) => Some(source),
            Self::StorageCapability(source) => Some(source),
            Self::AlreadyExists(_)
            | Self::Clock { .. }
            | Self::ConfigurationMismatch { .. }
            | Self::InvalidBootstrapJournal { .. }
            | Self::InvalidExtension(_)
            | Self::InvalidPlan
            | Self::MultipleBootstrapJournals
            | Self::OverlappingExtensions
            | Self::PathConflict(_)
            | Self::RootIsNotDirectory(_)
            | Self::UnexpectedDigest { .. }
            | Self::UnsupportedFormatVersion(_) => None,
            #[cfg(feature = "test-failpoints")]
            Self::Failpoint(_) => None,
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

    #[rustfmt::skip]
    const INVALID_LEGACY_SUFFIXES: &[&str] = &["{\"kind\":\"outcome\",\"outcome\":\"cancelled\"}\n", "{\"kind\":\"outcome\",\"outcome\":\"completed\"}\n{\"kind\":\"outcome\",\"outcome\":\"completed\"}\n", "{\"kind\":\"metadata\",\"collection_name\":\"x\",\"source_path\":\"x\",\"total_count\":0}\n", "{\"kind\":\"transition\",\"transition\":{\"kind\":\"initialization_config_published\",\"destination\":\".souvenirs.yml\",\"sha256\":\"x\"}}\n", "{\"kind\":\"outcome\",\"outcome\":\"completed\"}\n{\"kind\":\"metadata\",\"collection_name\":\"x\",\"source_path\":\"x\",\"total_count\":0}\n"];

    #[test]
    fn initializes_catalog_layout_and_completed_journal() {
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
        let journals = fs::read_dir(layout.journal_directory())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(journals.len(), 1);
        let plan = format!(
            "{{\"kind\":\"plan\",\"operation\":\"initialize\",\"format_version\":{FORMAT_VERSION}}}\n"
        );
        fs::write(
            journals[0].path(),
            format!("{plan}{{\"kind\":\"outcome\",\"outcome\":\"completed\"}}\n"),
        )
        .unwrap();
        let mut different = CatalogConfig::default();
        different.display_name.replace("Different".to_owned());
        let error = CatalogLayout::initialize(temporary.path(), &different).unwrap_err();
        assert!(matches!(
            error,
            CatalogInitializationError::ConfigurationMismatch { .. }
        ));
        CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap();
        for suffix in INVALID_LEGACY_SUFFIXES {
            let contents = format!("{plan}{suffix}");
            fs::write(journals[0].path(), &contents).unwrap();
            assert!(CatalogLayout::resume_initialization(temporary.path()).is_err());
            assert_eq!(fs::read_to_string(journals[0].path()).unwrap(), contents);
        }
        fs::write(
            journals[0].path(),
            format!("{plan}{{\"kind\":\"outcome\",\"outcome\":\"completed\""),
        )
        .unwrap();
        let resumed = CatalogLayout::resume_initialization(temporary.path()).unwrap();
        let recovered = fs::read_to_string(journals[0].path()).unwrap();
        assert!(resumed.is_some());
        assert_eq!(
            recovered,
            format!("{plan}{{\"kind\":\"outcome\",\"outcome\":\"completed\"}}\n")
        );
        fs::write(journals[0].path(), &plan).unwrap();
        fs::remove_file(layout.config_path()).unwrap();
        let error = CatalogLayout::resume_initialization(temporary.path()).unwrap_err();
        assert!(matches!(error, CatalogInitializationError::InvalidBootstrapJournal {
            details: "legacy initialization stopped before configuration publication and cannot be resumed because its full configuration was not recorded",
            ..
        }));
        assert_eq!(fs::read_to_string(journals[0].path()).unwrap(), plan);
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
            display_name: None,
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
