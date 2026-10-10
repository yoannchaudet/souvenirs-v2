use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::OffsetDateTime;

use crate::catalog_path::{validate_catalog_relative, CatalogPathError};
use crate::durability::{sync_directory, sync_file};
use crate::{CatalogLayout, CatalogLock, LockError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DurableTransition {
    DirectoryCreationStarted {
        step_id: String,
        path: PathBuf,
    },
    DirectoryCreated {
        step_id: String,
        path: PathBuf,
    },
    CopyStarted {
        step_id: String,
        source: PathBuf,
        staged_path: PathBuf,
        destination: PathBuf,
    },
    CopyVerified {
        step_id: String,
        staged_path: PathBuf,
        destination: PathBuf,
        sha256: String,
        bytes_copied: u64,
    },
    CopyPublicationStarted {
        step_id: String,
        staged_path: PathBuf,
        destination: PathBuf,
        sha256: String,
    },
    CopyPublished {
        step_id: String,
        destination: PathBuf,
        sha256: String,
    },
    ManifestReplacementStarted {
        step_id: String,
        staged_path: PathBuf,
        manifest_path: PathBuf,
        previous_sha256: Option<String>,
        resulting_sha256: String,
    },
    ManifestStaged {
        step_id: String,
        staged_path: PathBuf,
        manifest_path: PathBuf,
        resulting_sha256: String,
    },
    ManifestPublicationStarted {
        step_id: String,
        staged_path: PathBuf,
        manifest_path: PathBuf,
        resulting_sha256: String,
    },
    ManifestReplaced {
        step_id: String,
        manifest_path: PathBuf,
        resulting_sha256: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OperationRecord {
    Plan {
        operation_id: String,
        plan: serde_json::Value,
    },
    Transition {
        transition: DurableTransition,
    },
    Outcome {
        outcome: OperationOutcome,
    },
}

#[derive(Debug)]
pub struct OperationJournal {
    operation_id: String,
    catalog_root: PathBuf,
    path: PathBuf,
    file: File,
    finished: bool,
    #[cfg(feature = "test-failpoints")]
    failpoint: Option<Failpoint>,
}

impl OperationJournal {
    pub fn create(
        layout: &CatalogLayout,
        lock: &CatalogLock,
        operation_id: impl Into<String>,
        plan: &impl Serialize,
    ) -> Result<Self, OperationJournalError> {
        Self::create_inner(layout, lock, operation_id.into(), plan, None)
    }

    #[cfg(feature = "test-failpoints")]
    pub fn create_failing_at(
        layout: &CatalogLayout,
        lock: &CatalogLock,
        operation_id: impl Into<String>,
        plan: &impl Serialize,
        failpoint: Failpoint,
    ) -> Result<Self, OperationJournalError> {
        Self::create_inner(layout, lock, operation_id.into(), plan, Some(failpoint))
    }

    fn create_inner(
        layout: &CatalogLayout,
        lock: &CatalogLock,
        operation_id: String,
        plan: &impl Serialize,
        initial_failpoint: Option<Failpoint>,
    ) -> Result<Self, OperationJournalError> {
        ensure_lock(layout, lock)?;
        ensure_no_unfinished_journal(layout)?;
        validate_identifier("operation", &operation_id)?;
        let plan = serde_json::to_value(plan).map_err(OperationJournalError::SerializePlan)?;
        let catalog_root =
            fs::canonicalize(layout.root()).map_err(|source| OperationJournalError::Io {
                action: "resolve operation catalog root",
                path: layout.root().to_path_buf(),
                source,
            })?;
        let timestamp = OffsetDateTime::now_utc()
            .format(format_description!(
                "[year][month][day]-[hour][minute][second]"
            ))
            .map_err(|source| OperationJournalError::Clock(source.to_string()))?;
        let path = layout
            .journal_directory()
            .join(format!("{timestamp}-{operation_id}.jsonl"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| OperationJournalError::Io {
                action: "create operation journal exclusively",
                path: path.clone(),
                source,
            })?;
        sync_directory(&layout.journal_directory()).map_err(|source| {
            OperationJournalError::Io {
                action: "persist new operation journal entry",
                path: layout.journal_directory(),
                source,
            }
        })?;

        let mut journal = Self {
            operation_id: operation_id.clone(),
            catalog_root,
            path,
            file,
            finished: false,
            #[cfg(feature = "test-failpoints")]
            failpoint: initial_failpoint,
        };
        #[cfg(not(feature = "test-failpoints"))]
        let _ = initial_failpoint;
        journal.append(&OperationRecord::Plan { operation_id, plan })?;
        journal.check_failpoint(Failpoint::PlanPersisted)?;
        Ok(journal)
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn create_destination_directory(
        &mut self,
        layout: &CatalogLayout,
        lock: &CatalogLock,
        step_id: &str,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, OperationJournalError> {
        self.ensure_active(layout, lock)?;
        validate_identifier("step", step_id)?;
        let relative_path = relative_path.as_ref();
        validate_catalog_relative(relative_path).map_err(OperationJournalError::CatalogPath)?;
        let path = layout.root().join(relative_path);
        let parent = path
            .parent()
            .expect("validated destination directory has a parent");
        let canonical_root =
            fs::canonicalize(layout.root()).map_err(|source| OperationJournalError::Io {
                action: "resolve catalog root",
                path: layout.root().to_path_buf(),
                source,
            })?;
        let canonical_parent =
            fs::canonicalize(parent).map_err(|source| OperationJournalError::Io {
                action: "resolve destination parent",
                path: parent.to_path_buf(),
                source,
            })?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(OperationJournalError::CatalogPath(
                CatalogPathError::EscapesCatalog(relative_path.to_path_buf()),
            ));
        }

        self.record_transition(DurableTransition::DirectoryCreationStarted {
            step_id: step_id.to_owned(),
            path: relative_path.to_path_buf(),
        })?;
        self.check_failpoint(Failpoint::DirectoryIntentPersisted)?;
        fs::create_dir(&path).map_err(|source| OperationJournalError::Io {
            action: "create destination directory exclusively",
            path: path.clone(),
            source,
        })?;
        sync_directory(&canonical_parent).map_err(|source| OperationJournalError::Io {
            action: "persist destination parent directory",
            path: canonical_parent,
            source,
        })?;
        self.record_transition(DurableTransition::DirectoryCreated {
            step_id: step_id.to_owned(),
            path: relative_path.to_path_buf(),
        })?;
        self.check_failpoint(Failpoint::DirectoryPersisted)?;
        Ok(path)
    }

    pub fn complete(
        &mut self,
        layout: &CatalogLayout,
        lock: &CatalogLock,
    ) -> Result<(), OperationJournalError> {
        self.record_outcome(layout, lock, OperationOutcome::Completed)
    }

    /// Records cancellation after the caller has finished required manifest updates and cleanup.
    pub fn cancel(
        &mut self,
        layout: &CatalogLayout,
        lock: &CatalogLock,
    ) -> Result<(), OperationJournalError> {
        self.record_outcome(layout, lock, OperationOutcome::Cancelled)
    }

    #[cfg(feature = "test-failpoints")]
    pub fn fail_at(&mut self, failpoint: Failpoint) {
        self.failpoint = Some(failpoint);
    }

    pub(crate) fn ensure_active(
        &self,
        layout: &CatalogLayout,
        lock: &CatalogLock,
    ) -> Result<(), OperationJournalError> {
        if self.finished {
            return Err(OperationJournalError::AlreadyFinished);
        }
        ensure_lock(layout, lock)?;
        let catalog_root =
            fs::canonicalize(layout.root()).map_err(|source| OperationJournalError::Io {
                action: "resolve operation catalog root",
                path: layout.root().to_path_buf(),
                source,
            })?;
        if catalog_root != self.catalog_root {
            return Err(OperationJournalError::WrongCatalogJournal);
        }
        Ok(())
    }

    pub(crate) fn record_transition(
        &mut self,
        transition: DurableTransition,
    ) -> Result<(), OperationJournalError> {
        self.append(&OperationRecord::Transition { transition })
    }

    pub(crate) fn check_failpoint(
        &mut self,
        failpoint: Failpoint,
    ) -> Result<(), OperationJournalError> {
        #[cfg(feature = "test-failpoints")]
        if self.failpoint == Some(failpoint) {
            self.failpoint = None;
            return Err(OperationJournalError::Failpoint(failpoint));
        }
        let _ = failpoint;
        Ok(())
    }

    fn record_outcome(
        &mut self,
        layout: &CatalogLayout,
        lock: &CatalogLock,
        outcome: OperationOutcome,
    ) -> Result<(), OperationJournalError> {
        self.ensure_active(layout, lock)?;
        self.append(&OperationRecord::Outcome { outcome })?;
        self.finished = true;
        self.check_failpoint(Failpoint::OutcomePersisted)
    }

    fn append(&mut self, record: &OperationRecord) -> Result<(), OperationJournalError> {
        let mut encoded = serde_json::to_vec(record).map_err(|source| {
            OperationJournalError::SerializeRecord {
                path: self.path.clone(),
                source,
            }
        })?;
        encoded.push(b'\n');
        self.file
            .write_all(&encoded)
            .and_then(|()| sync_file(&self.file))
            .map_err(|source| OperationJournalError::Io {
                action: "append and persist operation journal",
                path: self.path.clone(),
                source,
            })
    }
}

fn ensure_lock(layout: &CatalogLayout, lock: &CatalogLock) -> Result<(), OperationJournalError> {
    if lock.guards(layout).map_err(OperationJournalError::Lock)? {
        Ok(())
    } else {
        Err(OperationJournalError::WrongCatalogLock)
    }
}

fn ensure_no_unfinished_journal(layout: &CatalogLayout) -> Result<(), OperationJournalError> {
    let entries =
        fs::read_dir(layout.journal_directory()).map_err(|source| OperationJournalError::Io {
            action: "scan operation journals",
            path: layout.journal_directory(),
            source,
        })?;
    for entry in entries {
        let entry = entry.map_err(|source| OperationJournalError::Io {
            action: "read operation journal directory entry",
            path: layout.journal_directory(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let finished = journal_has_outcome(&path)?;
        if !finished {
            return Err(OperationJournalError::UnfinishedOperation(path));
        }
    }
    Ok(())
}

fn journal_has_outcome(path: &Path) -> Result<bool, OperationJournalError> {
    const OUTCOME_TAIL_BYTES: u64 = 4 * 1024;

    let mut file = File::open(path).map_err(|source| OperationJournalError::Io {
        action: "open operation journal outcome",
        path: path.to_path_buf(),
        source,
    })?;
    let length = file
        .metadata()
        .map_err(|source| OperationJournalError::Io {
            action: "inspect operation journal outcome",
            path: path.to_path_buf(),
            source,
        })?
        .len();
    file.seek(SeekFrom::Start(length.saturating_sub(OUTCOME_TAIL_BYTES)))
        .map_err(|source| OperationJournalError::Io {
            action: "seek to operation journal outcome",
            path: path.to_path_buf(),
            source,
        })?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)
        .map_err(|source| OperationJournalError::Io {
            action: "read operation journal outcome",
            path: path.to_path_buf(),
            source,
        })?;
    let last_line = tail
        .rsplit(|byte| *byte == b'\n')
        .find(|line| !line.is_empty());
    Ok(last_line
        .and_then(|line| serde_json::from_slice::<OperationRecord>(line).ok())
        .is_some_and(|record| matches!(record, OperationRecord::Outcome { .. })))
}

pub(crate) fn validate_identifier(
    kind: &'static str,
    identifier: &str,
) -> Result<(), OperationJournalError> {
    if identifier.is_empty()
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(OperationJournalError::InvalidIdentifier {
            kind,
            value: identifier.to_owned(),
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failpoint {
    PlanPersisted,
    DirectoryIntentPersisted,
    DirectoryPersisted,
    CopyIntentPersisted,
    CopyStagePersisted,
    CopyPublicationIntentPersisted,
    CopyPublished,
    ManifestIntentPersisted,
    ManifestStagePersisted,
    ManifestPublicationIntentPersisted,
    ManifestPublished,
    OutcomePersisted,
}

#[derive(Debug)]
pub enum OperationJournalError {
    AlreadyFinished,
    CatalogPath(CatalogPathError),
    Clock(String),
    #[cfg(feature = "test-failpoints")]
    Failpoint(Failpoint),
    InvalidIdentifier {
        kind: &'static str,
        value: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Lock(LockError),
    SerializePlan(serde_json::Error),
    SerializeRecord {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnfinishedOperation(PathBuf),
    WrongCatalogJournal,
    WrongCatalogLock,
}

impl fmt::Display for OperationJournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyFinished => formatter.write_str("operation journal already has an outcome"),
            Self::CatalogPath(error) => error.fmt(formatter),
            Self::Clock(details) => write!(formatter, "failed to read system clock: {details}"),
            #[cfg(feature = "test-failpoints")]
            Self::Failpoint(failpoint) => {
                write!(formatter, "test failpoint reached: {failpoint:?}")
            }
            Self::InvalidIdentifier { kind, value } => write!(
                formatter,
                "{kind} identifier must contain only ASCII letters, digits, hyphens, or underscores: {value:?}"
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Lock(source) => write!(formatter, "failed to validate catalog lock: {source}"),
            Self::SerializePlan(source) => {
                write!(formatter, "failed to serialize confirmed operation plan: {source}")
            }
            Self::SerializeRecord { path, source } => write!(
                formatter,
                "failed to serialize operation journal {}: {source}",
                path.display()
            ),
            Self::UnfinishedOperation(path) => write!(
                formatter,
                "unfinished operation journal blocks a new mutation: {}",
                path.display()
            ),
            Self::WrongCatalogJournal => {
                formatter.write_str("operation journal belongs to a different catalog")
            }
            Self::WrongCatalogLock => {
                formatter.write_str("operation requires the active lock for this catalog")
            }
        }
    }
}

impl Error for OperationJournalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CatalogPath(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Lock(source) => Some(source),
            Self::SerializePlan(source) | Self::SerializeRecord { source, .. } => Some(source),
            Self::AlreadyFinished
            | Self::Clock(_)
            | Self::InvalidIdentifier { .. }
            | Self::UnfinishedOperation(_)
            | Self::WrongCatalogJournal
            | Self::WrongCatalogLock => None,
            #[cfg(feature = "test-failpoints")]
            Self::Failpoint(_) => None,
        }
    }
}
