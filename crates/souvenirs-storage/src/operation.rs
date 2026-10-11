use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    StageRemovalStarted {
        step_id: String,
        staged_path: PathBuf,
    },
    StageRemoved {
        step_id: String,
        staged_path: PathBuf,
    },
    InitializationJournalRelocationStarted {
        destination: PathBuf,
    },
    InitializationJournalRelocated {
        destination: PathBuf,
    },
    InitializationConfigStageStarted {
        staged_path: PathBuf,
        sha256: String,
    },
    InitializationConfigStaged {
        staged_path: PathBuf,
        sha256: String,
    },
    InitializationConfigPublicationStarted {
        staged_path: PathBuf,
        destination: PathBuf,
        sha256: String,
    },
    InitializationConfigPublished {
        destination: PathBuf,
        sha256: String,
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
    Metadata {
        collection_name: String,
        source_path: PathBuf,
        total_count: usize,
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
        static JOURNAL_CREATION: Mutex<()> = Mutex::new(());
        let _creation = JOURNAL_CREATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        ensure_lock(layout, lock)?;
        validate_new_operation_identifier(&operation_id)?;
        let plan = serde_json::to_value(plan).map_err(OperationJournalError::SerializePlan)?;
        ensure_no_unfinished_journal(layout)?;
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
        journal.check_failpoint(Failpoint::JournalCreated)?;
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

    pub(crate) fn open_for_recovery(
        layout: &CatalogLayout,
        lock: &CatalogLock,
        path: &Path,
    ) -> Result<(Self, Vec<OperationRecord>), OperationJournalError> {
        ensure_lock(layout, lock)?;
        let canonical_journal_directory =
            fs::canonicalize(layout.journal_directory()).map_err(|source| {
                OperationJournalError::Io {
                    action: "resolve operation journal directory",
                    path: layout.journal_directory(),
                    source,
                }
            })?;
        let parent = path
            .parent()
            .ok_or_else(|| OperationJournalError::InvalidJournalPath(path.to_path_buf()))?;
        let canonical_parent =
            fs::canonicalize(parent).map_err(|source| OperationJournalError::Io {
                action: "resolve operation journal parent",
                path: parent.to_path_buf(),
                source,
            })?;
        if canonical_parent != canonical_journal_directory || !is_operation_journal(path) {
            return Err(OperationJournalError::InvalidJournalPath(
                path.to_path_buf(),
            ));
        }
        let metadata = fs::symlink_metadata(path).map_err(|source| OperationJournalError::Io {
            action: "inspect operation journal for recovery",
            path: path.to_path_buf(),
            source,
        })?;
        if !metadata.file_type().is_file() {
            return Err(OperationJournalError::InvalidJournalPath(
                path.to_path_buf(),
            ));
        }

        let ParsedOperationRecords {
            records,
            complete_length,
            interrupted_tail,
            ..
        } = read_parsed_operation_records(path, "read operation journal for recovery")?;
        let operation_id = match records.first() {
            Some(OperationRecord::Plan { operation_id, .. }) => operation_id.clone(),
            _ => return Err(OperationJournalError::MissingPlan(path.to_path_buf())),
        };
        validate_identifier("operation", &operation_id)?;
        if is_reserved_initialization_identifier(&operation_id) {
            return Err(OperationJournalError::ReservedIdentifier {
                kind: "operation",
                value: operation_id,
            });
        }
        let finished = records
            .last()
            .is_some_and(|record| matches!(record, OperationRecord::Outcome { .. }));
        let file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|source| OperationJournalError::Io {
                action: "open operation journal for recovery",
                path: path.to_path_buf(),
                source,
            })?;
        if interrupted_tail {
            file.set_len(complete_length)
                .and_then(|()| sync_file(&file))
                .map_err(|source| OperationJournalError::Io {
                    action: "discard interrupted operation journal append",
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        let catalog_root =
            fs::canonicalize(layout.root()).map_err(|source| OperationJournalError::Io {
                action: "resolve operation catalog root",
                path: layout.root().to_path_buf(),
                source,
            })?;
        Ok((
            Self {
                operation_id,
                catalog_root,
                path: path.to_path_buf(),
                file,
                finished,
                #[cfg(feature = "test-failpoints")]
                failpoint: None,
            },
            records,
        ))
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

    pub fn record_metadata(
        &mut self,
        layout: &CatalogLayout,
        lock: &CatalogLock,
        collection_name: String,
        source_path: PathBuf,
        total_count: usize,
    ) -> Result<(), OperationJournalError> {
        self.ensure_active(layout, lock)?;
        self.append(&OperationRecord::Metadata {
            collection_name,
            source_path,
            total_count,
        })
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
        append_operation_record(&mut self.file, &self.path, record)
    }
}

pub fn read_operation_records(
    path: impl AsRef<Path>,
) -> Result<Vec<OperationRecord>, OperationJournalError> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path).map_err(|source| OperationJournalError::Io {
        action: "inspect operation journal",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() || !is_operation_journal(path) {
        return Err(OperationJournalError::InvalidJournalPath(
            path.to_path_buf(),
        ));
    }
    Ok(read_parsed_operation_records(path, "read operation journal")?.records)
}

pub(crate) struct ParsedOperationRecords {
    pub records: Vec<OperationRecord>,
    pub complete_length: u64,
    pub interrupted_tail: bool,
    pub legacy_initialization_format: Option<u32>,
}

pub(crate) fn read_parsed_operation_records(
    path: &Path,
    action: &'static str,
) -> Result<ParsedOperationRecords, OperationJournalError> {
    let bytes = fs::read(path).map_err(|source| OperationJournalError::Io {
        action,
        path: path.to_path_buf(),
        source,
    })?;
    if let Some((records, format_version)) = parse_completed_legacy_initialization(path, &bytes) {
        return Ok(ParsedOperationRecords {
            records,
            complete_length: bytes.len() as u64,
            interrupted_tail: false,
            legacy_initialization_format: Some(format_version),
        });
    }
    let mut records = Vec::new();
    let mut start = 0;
    let mut line_number = 1;
    for (end, _) in bytes.iter().enumerate().filter(|(_, byte)| **byte == b'\n') {
        let line = &bytes[start..end];
        if line.is_empty() {
            start = end + 1;
            line_number += 1;
            continue;
        }

        records.push(serde_json::from_slice(line).map_err(|source| {
            OperationJournalError::DeserializeRecord {
                path: path.to_path_buf(),
                line: line_number,
                source,
            }
        })?);
        start = end + 1;
        line_number += 1;
    }
    Ok(ParsedOperationRecords {
        records,
        complete_length: start as u64,
        interrupted_tail: start < bytes.len(),
        legacy_initialization_format: None,
    })
}

fn parse_completed_legacy_initialization(
    path: &Path,
    bytes: &[u8],
) -> Option<(Vec<OperationRecord>, u32)> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPlan {
        kind: String,
        operation: String,
        format_version: u32,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyOutcome {
        kind: String,
        outcome: String,
    }

    if !is_initialization_journal(path) || !bytes.ends_with(b"\n") {
        return None;
    }
    let mut lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty());
    let plan: LegacyPlan = serde_json::from_slice(lines.next()?).ok()?;
    let outcome: LegacyOutcome = serde_json::from_slice(lines.next()?).ok()?;
    if lines.next().is_some()
        || plan.kind != "plan"
        || plan.operation != INITIALIZATION_OPERATION_ID
        || outcome.kind != "outcome"
        || outcome.outcome != "completed"
    {
        return None;
    }
    Some((
        vec![
            OperationRecord::Plan {
                operation_id: INITIALIZATION_OPERATION_ID.to_owned(),
                plan: serde_json::json!({
                    "legacy_initialization": true,
                    "format_version": plan.format_version,
                }),
            },
            OperationRecord::Outcome {
                outcome: OperationOutcome::Completed,
            },
        ],
        plan.format_version,
    ))
}

pub(crate) fn append_operation_record(
    file: &mut File,
    path: &Path,
    record: &OperationRecord,
) -> Result<(), OperationJournalError> {
    let mut encoded =
        serde_json::to_vec(record).map_err(|source| OperationJournalError::SerializeRecord {
            path: path.to_path_buf(),
            source,
        })?;
    encoded.push(b'\n');
    file.write_all(&encoded)
        .and_then(|()| sync_file(file))
        .map_err(|source| OperationJournalError::Io {
            action: "append and persist operation journal",
            path: path.to_path_buf(),
            source,
        })
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
        if !is_operation_journal(&path) {
            continue;
        }
        if is_initialization_journal(&path) {
            let parsed = read_parsed_operation_records(&path, "read initialization journal")?;
            if parsed.records.last().is_some_and(|record| {
                matches!(
                    record,
                    OperationRecord::Outcome {
                        outcome: OperationOutcome::Completed
                    }
                )
            }) {
                continue;
            }
            return Err(OperationJournalError::UnfinishedOperation(path));
        }
        if journal_tail_has_outcome(&path)? {
            continue;
        }
        let parsed = read_parsed_operation_records(&path, "read operation journal outcome")?;
        if parsed.records.is_empty() {
            fs::remove_file(&path).map_err(|source| OperationJournalError::Io {
                action: "remove interrupted pre-plan operation journal",
                path: path.clone(),
                source,
            })?;
            sync_directory(&layout.journal_directory()).map_err(|source| {
                OperationJournalError::Io {
                    action: "persist interrupted pre-plan journal removal",
                    path: layout.journal_directory(),
                    source,
                }
            })?;
            continue;
        }
        let finished = parsed
            .records
            .last()
            .is_some_and(|record| matches!(record, OperationRecord::Outcome { .. }));
        if !finished {
            return Err(OperationJournalError::UnfinishedOperation(path));
        }
    }
    Ok(())
}

fn is_operation_journal(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
        && !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("._"))
}

fn is_initialization_journal(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("-initialize.jsonl"))
}

fn journal_tail_has_outcome(path: &Path) -> Result<bool, OperationJournalError> {
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

fn validate_new_operation_identifier(identifier: &str) -> Result<(), OperationJournalError> {
    validate_identifier("operation", identifier)?;
    if is_reserved_initialization_identifier(identifier) {
        return Err(OperationJournalError::ReservedIdentifier {
            kind: "operation",
            value: identifier.to_owned(),
        });
    }
    Ok(())
}

fn is_reserved_initialization_identifier(identifier: &str) -> bool {
    identifier == INITIALIZATION_OPERATION_ID || identifier.ends_with("-initialize")
}

const INITIALIZATION_OPERATION_ID: &str = "initialize";

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
    JournalCreated,
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
    StageRemovalIntentPersisted,
    StageRemovalPersisted,
    RecoveryDirectoryObservedBeforePersistence,
    RecoveryCopyStageObservedBeforePersistence,
    RecoveryCopyDestinationObservedBeforePersistence,
    RecoveryManifestStageObservedBeforePersistence,
    RecoveryManifestDestinationObservedBeforePersistence,
    RecoveryStageRemovalObservedBeforePersistence,
    OutcomePersisted,
    InitializationJournalCreated,
    InitializationPlanPersisted,
    InitializationStateDirectoryIntentPersisted,
    InitializationStateDirectoryMutationPersisted,
    InitializationStateDirectoryCompleted,
    InitializationJournalDirectoryIntentPersisted,
    InitializationJournalDirectoryMutationPersisted,
    InitializationJournalDirectoryCompleted,
    InitializationJournalRelocationIntentPersisted,
    InitializationJournalRelocationMutationPersisted,
    InitializationJournalRelocated,
    InitializationStagingDirectoryIntentPersisted,
    InitializationStagingDirectoryMutationPersisted,
    InitializationStagingDirectoryCompleted,
    InitializationConfigStageIntentPersisted,
    InitializationConfigStageMutationPersisted,
    InitializationConfigStaged,
    InitializationConfigPublicationIntentPersisted,
    InitializationConfigPublicationMutationPersisted,
    InitializationConfigPublished,
    InitializationRecoveryDirectoryObservedBeforePersistence,
    InitializationRecoveryRelocationObservedBeforePersistence,
    InitializationRecoveryConfigStageObservedBeforePersistence,
    InitializationRecoveryConfigObservedBeforePersistence,
    InitializationRecoveryCleanupObservedBeforePersistence,
    InitializationOutcomePersisted,
}

#[derive(Debug)]
pub enum OperationJournalError {
    AlreadyFinished,
    CatalogPath(CatalogPathError),
    Clock(String),
    DeserializeRecord {
        path: PathBuf,
        line: usize,
        source: serde_json::Error,
    },
    #[cfg(feature = "test-failpoints")]
    Failpoint(Failpoint),
    InvalidIdentifier {
        kind: &'static str,
        value: String,
    },
    InvalidJournalPath(PathBuf),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Lock(LockError),
    MissingPlan(PathBuf),
    ReservedIdentifier {
        kind: &'static str,
        value: String,
    },
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
            Self::DeserializeRecord { path, line, source } => write!(
                formatter,
                "failed to deserialize operation journal {} line {line}: {source}",
                path.display()
            ),
            #[cfg(feature = "test-failpoints")]
            Self::Failpoint(failpoint) => {
                write!(formatter, "test failpoint reached: {failpoint:?}")
            }
            Self::InvalidIdentifier { kind, value } => write!(
                formatter,
                "{kind} identifier must contain only ASCII letters, digits, hyphens, or underscores: {value:?}"
            ),
            Self::InvalidJournalPath(path) => write!(
                formatter,
                "operation journal must be a JSON Lines file directly inside the catalog journal directory: {}",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Lock(source) => write!(formatter, "failed to validate catalog lock: {source}"),
            Self::MissingPlan(path) => write!(
                formatter,
                "operation journal does not start with a confirmed plan: {}",
                path.display()
            ),
            Self::ReservedIdentifier { kind, value } => write!(
                formatter,
                "{kind} identifier is reserved for catalog initialization: {value:?}"
            ),
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
            Self::DeserializeRecord { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Lock(source) => Some(source),
            Self::SerializePlan(source) | Self::SerializeRecord { source, .. } => Some(source),
            Self::AlreadyFinished
            | Self::Clock(_)
            | Self::InvalidIdentifier { .. }
            | Self::InvalidJournalPath(_)
            | Self::MissingPlan(_)
            | Self::ReservedIdentifier { .. }
            | Self::UnfinishedOperation(_)
            | Self::WrongCatalogJournal
            | Self::WrongCatalogLock => None,
            #[cfg(feature = "test-failpoints")]
            Self::Failpoint(_) => None,
        }
    }
}
