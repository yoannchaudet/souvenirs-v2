use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::copy::{publish_exclusively, remove_apple_double_sidecar, PublishError};
use crate::durability::{sync_directory, sync_file};
use crate::manifest::verify_previous_manifest;
use crate::operation::validate_identifier;
use crate::{
    CatalogLayout, CatalogLock, CopyExpectation, DurableTransition, ManifestReplacementRequest,
    OperationJournal, OperationJournalError, OperationOutcome, OperationRecord,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecoveryMode {
    #[default]
    Resume,
    CancelRemainingCopies,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryDirectory {
    pub step_id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryCopy {
    pub step_id: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub expected_sha256: String,
    pub expected_size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryManifest {
    pub step_id: String,
    pub path: PathBuf,
    pub previous_sha256: Option<String>,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryRequest {
    pub mode: RecoveryMode,
    pub directories: Vec<RecoveryDirectory>,
    pub copies: Vec<RecoveryCopy>,
    pub manifests: Vec<RecoveryManifest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOutcome {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellableRecoveryOutcome {
    Finished(RecoveryOutcome),
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryCheckpoint {
    DurableBoundary,
    CompletionBoundary,
}

impl CatalogLayout {
    pub fn recover_operation(
        &self,
        lock: &CatalogLock,
        journal_path: impl AsRef<Path>,
        request: RecoveryRequest,
    ) -> Result<RecoveryOutcome, RecoveryError> {
        match self.recover_operation_inner(
            lock,
            journal_path.as_ref(),
            request,
            None,
            &mut |_| false,
        )? {
            CancellableRecoveryOutcome::Finished(outcome) => Ok(outcome),
            CancellableRecoveryOutcome::Interrupted => {
                unreachable!("non-cancellable recovery cannot be interrupted")
            }
        }
    }

    pub fn recover_operation_cancellable(
        &self,
        lock: &CatalogLock,
        journal_path: impl AsRef<Path>,
        request: RecoveryRequest,
        mut interrupt: impl FnMut(RecoveryCheckpoint) -> bool,
    ) -> Result<CancellableRecoveryOutcome, RecoveryError> {
        self.recover_operation_inner(lock, journal_path.as_ref(), request, None, &mut interrupt)
    }

    #[cfg(feature = "test-failpoints")]
    pub fn recover_operation_failing_at(
        &self,
        lock: &CatalogLock,
        journal_path: impl AsRef<Path>,
        request: RecoveryRequest,
        failpoint: crate::Failpoint,
    ) -> Result<RecoveryOutcome, RecoveryError> {
        match self.recover_operation_inner(
            lock,
            journal_path.as_ref(),
            request,
            Some(failpoint),
            &mut |_| false,
        )? {
            CancellableRecoveryOutcome::Finished(outcome) => Ok(outcome),
            CancellableRecoveryOutcome::Interrupted => {
                unreachable!("non-cancellable recovery cannot be interrupted")
            }
        }
    }

    fn recover_operation_inner(
        &self,
        lock: &CatalogLock,
        journal_path: &Path,
        request: RecoveryRequest,
        failpoint: Option<crate::operation::Failpoint>,
        interrupt: &mut impl FnMut(RecoveryCheckpoint) -> bool,
    ) -> Result<CancellableRecoveryOutcome, RecoveryError> {
        let (mut journal, records) = OperationJournal::open_for_recovery(self, lock, journal_path)
            .map_err(RecoveryError::Journal)?;
        if let Some(outcome) = terminal_outcome(&records) {
            return Ok(CancellableRecoveryOutcome::Finished(outcome));
        }
        #[cfg(feature = "test-failpoints")]
        if let Some(failpoint) = failpoint {
            journal.fail_at(failpoint);
        }
        #[cfg(not(feature = "test-failpoints"))]
        let _ = failpoint;

        validate_requests(&request)?;
        let manifests = validate_manifest_requests(&request.manifests)?;
        recover_directories(self, lock, &mut journal, &records, &request.directories)?;
        if interrupt(RecoveryCheckpoint::DurableBoundary)
            || recover_copies(
                self,
                lock,
                &mut journal,
                &records,
                request.mode,
                &request.copies,
                interrupt,
            )?
            || recover_recorded_manifests(
                self,
                lock,
                &mut journal,
                &records,
                request.mode,
                &manifests,
                interrupt,
            )?
            || recover_missing_manifests(
                self,
                lock,
                &mut journal,
                &records,
                &request.manifests,
                interrupt,
            )?
        {
            return Ok(CancellableRecoveryOutcome::Interrupted);
        }
        cleanup_owned_stages(self, lock, &mut journal, &records)?;
        if interrupt(RecoveryCheckpoint::DurableBoundary)
            || interrupt(RecoveryCheckpoint::CompletionBoundary)
        {
            return Ok(CancellableRecoveryOutcome::Interrupted);
        }

        let outcome = match request.mode {
            RecoveryMode::Resume => {
                journal
                    .complete(self, lock)
                    .map_err(RecoveryError::Journal)?;
                RecoveryOutcome::Completed
            }
            RecoveryMode::CancelRemainingCopies => {
                journal.cancel(self, lock).map_err(RecoveryError::Journal)?;
                RecoveryOutcome::Cancelled
            }
        };
        Ok(CancellableRecoveryOutcome::Finished(outcome))
    }
}

fn terminal_outcome(records: &[OperationRecord]) -> Option<RecoveryOutcome> {
    match records.last() {
        Some(OperationRecord::Outcome {
            outcome: OperationOutcome::Completed,
        }) => Some(RecoveryOutcome::Completed),
        Some(OperationRecord::Outcome {
            outcome: OperationOutcome::Cancelled,
        }) => Some(RecoveryOutcome::Cancelled),
        _ => None,
    }
}

fn recover_directories(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    records: &[OperationRecord],
    requests: &[RecoveryDirectory],
) -> Result<(), RecoveryError> {
    let mut started = BTreeMap::new();
    let mut order = Vec::new();
    let mut completed = BTreeSet::new();
    for transition in transitions(records) {
        match transition {
            DurableTransition::DirectoryCreationStarted { step_id, path } => {
                if !started.contains_key(step_id) {
                    order.push(step_id.clone());
                }
                started.insert(step_id.clone(), path.clone());
            }
            DurableTransition::DirectoryCreated { step_id, path } => {
                if started.get(step_id) != Some(path) {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                completed.insert(step_id.clone());
            }
            _ => {}
        }
    }

    for step_id in order {
        let relative_path = &started[&step_id];
        let path = resolve_directory(layout, relative_path)?;
        if completed.contains(&step_id) {
            ensure_directory(&path)?;
            continue;
        }
        journal
            .ensure_active(layout, lock)
            .map_err(RecoveryError::Journal)?;
        let observed = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => true,
            Ok(_) => return Err(RecoveryError::PathConflict(path)),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(|source| RecoveryError::Io {
                    action: "create interrupted destination directory",
                    path: path.clone(),
                    source,
                })?;
                false
            }
            Err(source) => {
                return Err(RecoveryError::Io {
                    action: "inspect interrupted destination directory",
                    path,
                    source,
                });
            }
        };
        if observed {
            journal
                .check_failpoint(
                    crate::operation::Failpoint::RecoveryDirectoryObservedBeforePersistence,
                )
                .map_err(RecoveryError::Journal)?;
        }
        let parent = path.parent().expect("recovered directory has a parent");
        sync_directory(parent).map_err(|source| RecoveryError::Io {
            action: "persist recovered destination directory",
            path: parent.to_path_buf(),
            source,
        })?;
        journal
            .record_transition(DurableTransition::DirectoryCreated {
                step_id,
                path: relative_path.clone(),
            })
            .map_err(RecoveryError::Journal)?;
    }
    for request in requests {
        if let Some(recorded_path) = started.get(&request.step_id) {
            if recorded_path != &request.path {
                return Err(RecoveryError::RequestMismatch(request.step_id.clone()));
            }
            continue;
        }
        journal
            .create_destination_directory(layout, lock, &request.step_id, &request.path)
            .map_err(RecoveryError::Journal)?;
    }
    Ok(())
}

#[derive(Clone)]
struct CopyState {
    source: PathBuf,
    staged_path: PathBuf,
    destination: PathBuf,
    sha256: Option<String>,
    bytes_copied: Option<u64>,
    published: bool,
}

fn recover_copies(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    records: &[OperationRecord],
    mode: RecoveryMode,
    requests: &[RecoveryCopy],
    interrupt: &mut impl FnMut(RecoveryCheckpoint) -> bool,
) -> Result<bool, RecoveryError> {
    let requests = requests
        .iter()
        .map(|request| (request.step_id.as_str(), request))
        .collect::<BTreeMap<_, _>>();
    let mut copies = BTreeMap::<String, CopyState>::new();
    let mut order = Vec::new();
    for transition in transitions(records) {
        match transition {
            DurableTransition::CopyStarted {
                step_id,
                source,
                staged_path,
                destination,
            } => {
                validate_owned_stage(layout, journal.operation_id(), staged_path)?;
                if !copies.contains_key(step_id) {
                    order.push(step_id.clone());
                }
                copies.insert(
                    step_id.clone(),
                    CopyState {
                        source: source.clone(),
                        staged_path: staged_path.clone(),
                        destination: destination.clone(),
                        sha256: None,
                        bytes_copied: None,
                        published: false,
                    },
                );
            }
            DurableTransition::CopyVerified {
                step_id,
                staged_path,
                destination,
                sha256,
                bytes_copied,
            } => {
                let state = copies
                    .get_mut(step_id)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.staged_path != *staged_path || state.destination != *destination {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.sha256 = Some(sha256.clone());
                state.bytes_copied = Some(*bytes_copied);
            }
            DurableTransition::CopyPublicationStarted {
                step_id,
                staged_path,
                destination,
                sha256,
            } => {
                let state = copies
                    .get(step_id)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.staged_path != *staged_path
                    || state.destination != *destination
                    || state.sha256.as_deref() != Some(sha256.as_str())
                {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
            }
            DurableTransition::CopyPublished {
                step_id,
                destination,
                sha256,
            } => {
                let state = copies
                    .get_mut(step_id)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.destination != *destination
                    || state.sha256.as_deref() != Some(sha256.as_str())
                {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.published = true;
            }
            _ => {}
        }
    }

    for step_id in order {
        if interrupt(RecoveryCheckpoint::DurableBoundary) {
            return Ok(true);
        }
        let state = &copies[&step_id];
        let request = requests
            .get(step_id.as_str())
            .copied()
            .ok_or_else(|| RecoveryError::RequestMismatch(step_id.clone()))?;
        if state.source != request.source || state.destination != request.destination {
            return Err(RecoveryError::RequestMismatch(step_id.clone()));
        }
        let staged_path = layout.root().join(&state.staged_path);
        let destination = crate::catalog_path::resolve_catalog_entry(layout, &state.destination)
            .map_err(RecoveryError::Path)?;
        let staged_digest = digest_if_file(&staged_path)?;
        let destination_digest = digest_if_file(&destination.resolved_path)?;
        if let Some(actual_sha256) = state.sha256.as_deref() {
            if actual_sha256 != request.expected_sha256
                || state.bytes_copied != Some(request.expected_size_bytes)
            {
                if mode == RecoveryMode::CancelRemainingCopies
                    && !state.published
                    && destination_digest.is_none()
                {
                    if staged_digest.is_some() {
                        remove_owned_stage(layout, lock, journal, &step_id, &state.staged_path)?;
                    }
                    continue;
                }
                return Err(RecoveryError::ExpectedContentMismatch {
                    step: step_id.clone(),
                    expected_sha256: request.expected_sha256.clone(),
                    actual_sha256: actual_sha256.to_owned(),
                    expected_size_bytes: request.expected_size_bytes,
                    actual_size_bytes: state.bytes_copied,
                });
            }
        }

        if state.published {
            ensure_digest(
                &destination.resolved_path,
                destination_digest,
                state.sha256.as_deref(),
            )?;
            if staged_digest.is_some() {
                return Err(RecoveryError::PathConflict(staged_path));
            }
            journal
                .ensure_active(layout, lock)
                .map_err(RecoveryError::Journal)?;
            remove_apple_double_sidecar(&staged_path).map_err(|source| RecoveryError::Io {
                action: "remove recovered staged AppleDouble sidecar",
                path: staged_path.clone(),
                source,
            })?;
            persist_observed_copy_stage_removal(layout, journal)?;
            continue;
        }

        if let Some(expected) = state.sha256.as_deref() {
            match (staged_digest.as_deref(), destination_digest.as_deref()) {
                (None, Some(actual)) if actual == expected => {
                    journal
                        .ensure_active(layout, lock)
                        .map_err(RecoveryError::Journal)?;
                    remove_apple_double_sidecar(&staged_path).map_err(|source| {
                        RecoveryError::Io {
                            action: "remove recovered staged AppleDouble sidecar",
                            path: staged_path.clone(),
                            source,
                        }
                    })?;
                    persist_observed_copy_stage_removal(layout, journal)?;
                    journal
                        .check_failpoint(
                            crate::operation::Failpoint::RecoveryCopyDestinationObservedBeforePersistence,
                        )
                        .map_err(RecoveryError::Journal)?;
                    sync_file(&destination.directory).map_err(|source| RecoveryError::Io {
                        action: "persist observed copy destination directory",
                        path: destination.directory_path.clone(),
                        source,
                    })?;
                    record_copy_published(
                        layout,
                        lock,
                        journal,
                        step_id.clone(),
                        state.destination.clone(),
                        expected,
                    )?;
                }
                (Some(actual), None) if actual == expected => {
                    journal
                        .ensure_active(layout, lock)
                        .map_err(RecoveryError::Journal)?;
                    journal
                        .record_transition(DurableTransition::CopyPublicationStarted {
                            step_id: step_id.clone(),
                            staged_path: state.staged_path.clone(),
                            destination: state.destination.clone(),
                            sha256: expected.to_owned(),
                        })
                        .map_err(RecoveryError::Journal)?;
                    publish_exclusively(&staged_path, &destination).map_err(
                        |error| match error {
                            PublishError::Occupied => {
                                RecoveryError::PathConflict(destination.resolved_path.clone())
                            }
                            PublishError::Io(source) => RecoveryError::Io {
                                action: "publish recovered verified file",
                                path: destination.resolved_path.clone(),
                                source,
                            },
                        },
                    )?;
                    remove_apple_double_sidecar(&staged_path).map_err(|source| {
                        RecoveryError::Io {
                            action: "remove recovered staged AppleDouble sidecar",
                            path: staged_path.clone(),
                            source,
                        }
                    })?;
                    sync_directory(&layout.staging_directory()).map_err(|source| {
                        RecoveryError::Io {
                            action: "persist staging directory after recovered publication",
                            path: layout.staging_directory(),
                            source,
                        }
                    })?;
                    sync_file(&destination.directory).map_err(|source| RecoveryError::Io {
                        action: "persist recovered copy destination directory",
                        path: destination.directory_path.clone(),
                        source,
                    })?;
                    record_copy_published(
                        layout,
                        lock,
                        journal,
                        step_id.clone(),
                        state.destination.clone(),
                        expected,
                    )?;
                }
                (None, None) if mode == RecoveryMode::Resume && state.source.is_file() => {
                    layout
                        .copy_verified(
                            lock,
                            journal,
                            &state.source,
                            &state.destination,
                            &step_id,
                            CopyExpectation {
                                sha256: request.expected_sha256.clone(),
                                size_bytes: request.expected_size_bytes,
                            },
                        )
                        .map_err(RecoveryError::Copy)?;
                }
                (None, None) if mode == RecoveryMode::Resume => {
                    return Err(RecoveryError::VerifiedStageMissing(staged_path));
                }
                (None, None) => {}
                (Some(_), None) if mode == RecoveryMode::CancelRemainingCopies => {
                    remove_owned_stage(layout, lock, journal, &step_id, &state.staged_path)?;
                }
                _ => return Err(RecoveryError::CopyConflict(state.destination.clone())),
            }
            continue;
        }

        if destination_digest.is_some() {
            return Err(RecoveryError::UnverifiedDestination(
                destination.resolved_path,
            ));
        }
        if staged_digest.is_some() {
            remove_owned_stage(layout, lock, journal, &step_id, &state.staged_path)?;
        }
        if mode == RecoveryMode::CancelRemainingCopies {
            continue;
        }
        if !state.source.is_file() {
            return Err(RecoveryError::SourceUnavailable(state.source.clone()));
        }
        layout
            .copy_verified(
                lock,
                journal,
                &state.source,
                &state.destination,
                &step_id,
                CopyExpectation {
                    sha256: request.expected_sha256.clone(),
                    size_bytes: request.expected_size_bytes,
                },
            )
            .map_err(RecoveryError::Copy)?;
    }
    for request in requests.values() {
        if interrupt(RecoveryCheckpoint::DurableBoundary) {
            return Ok(true);
        }
        if let Some(recorded) = copies.get(&request.step_id) {
            if recorded.source != request.source || recorded.destination != request.destination {
                return Err(RecoveryError::RequestMismatch(request.step_id.clone()));
            }
            continue;
        }
        if mode == RecoveryMode::CancelRemainingCopies {
            continue;
        }
        if !request.source.is_file() {
            return Err(RecoveryError::SourceUnavailable(request.source.clone()));
        }
        layout
            .copy_verified(
                lock,
                journal,
                &request.source,
                &request.destination,
                &request.step_id,
                CopyExpectation {
                    sha256: request.expected_sha256.clone(),
                    size_bytes: request.expected_size_bytes,
                },
            )
            .map_err(RecoveryError::Copy)?;
    }
    Ok(interrupt(RecoveryCheckpoint::DurableBoundary))
}

fn persist_observed_copy_stage_removal(
    layout: &CatalogLayout,
    journal: &mut OperationJournal,
) -> Result<(), RecoveryError> {
    journal
        .check_failpoint(crate::operation::Failpoint::RecoveryCopyStageObservedBeforePersistence)
        .map_err(RecoveryError::Journal)?;
    sync_directory(&layout.staging_directory()).map_err(|source| RecoveryError::Io {
        action: "persist staging directory after observed publication",
        path: layout.staging_directory(),
        source,
    })
}

fn record_copy_published(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    step_id: String,
    destination: PathBuf,
    sha256: &str,
) -> Result<(), RecoveryError> {
    journal
        .ensure_active(layout, lock)
        .map_err(RecoveryError::Journal)?;
    journal
        .record_transition(DurableTransition::CopyPublished {
            step_id,
            destination,
            sha256: sha256.to_owned(),
        })
        .map_err(RecoveryError::Journal)
}

fn recover_recorded_manifests(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    records: &[OperationRecord],
    mode: RecoveryMode,
    requests: &BTreeMap<PathBuf, &RecoveryManifest>,
    interrupt: &mut impl FnMut(RecoveryCheckpoint) -> bool,
) -> Result<bool, RecoveryError> {
    let mut manifests = BTreeMap::<PathBuf, ManifestState>::new();
    let mut order = Vec::new();
    for transition in transitions(records) {
        match transition {
            DurableTransition::ManifestReplacementStarted {
                step_id,
                staged_path,
                manifest_path,
                previous_sha256,
                resulting_sha256,
            } => {
                validate_owned_stage(layout, journal.operation_id(), staged_path)?;
                if !manifests.contains_key(manifest_path) {
                    order.push(manifest_path.clone());
                }
                manifests.insert(
                    manifest_path.clone(),
                    ManifestState {
                        step_id: step_id.clone(),
                        staged_path: staged_path.clone(),
                        previous_sha256: previous_sha256.clone(),
                        resulting_sha256: resulting_sha256.clone(),
                        staged: false,
                        replaced: false,
                    },
                );
            }
            DurableTransition::ManifestStaged {
                step_id,
                staged_path,
                manifest_path,
                resulting_sha256,
            }
            | DurableTransition::ManifestPublicationStarted {
                step_id,
                staged_path,
                manifest_path,
                resulting_sha256,
            } => {
                let state = manifests
                    .get_mut(manifest_path)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.step_id != *step_id
                    || state.staged_path != *staged_path
                    || state.resulting_sha256 != *resulting_sha256
                {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.staged = true;
            }
            DurableTransition::ManifestReplaced {
                step_id,
                manifest_path,
                resulting_sha256,
            } => {
                let state = manifests
                    .get_mut(manifest_path)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.step_id != *step_id || state.resulting_sha256 != *resulting_sha256 {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.replaced = true;
            }
            _ => {}
        }
    }

    for manifest_path in order {
        if interrupt(RecoveryCheckpoint::DurableBoundary) {
            return Ok(true);
        }
        let state = manifests
            .remove(&manifest_path)
            .expect("ordered manifest state exists");
        let manifest = crate::catalog_path::resolve_catalog_entry(layout, &manifest_path)
            .map_err(RecoveryError::Path)?;
        let staged_path = layout.root().join(&state.staged_path);
        let current_digest = digest_if_file(&manifest.resolved_path)?;
        let mut staged_digest = digest_if_file(&staged_path)?;
        if state.replaced {
            ensure_digest(
                &manifest.resolved_path,
                current_digest,
                Some(&state.resulting_sha256),
            )?;
            if staged_digest.is_some() {
                return Err(RecoveryError::PathConflict(staged_path));
            }
            continue;
        }
        if current_digest.as_deref() == Some(state.resulting_sha256.as_str())
            && staged_digest.is_none()
        {
            journal
                .ensure_active(layout, lock)
                .map_err(RecoveryError::Journal)?;
            journal
                .check_failpoint(
                    crate::operation::Failpoint::RecoveryManifestStageObservedBeforePersistence,
                )
                .map_err(RecoveryError::Journal)?;
            sync_directory(&layout.staging_directory()).map_err(|source| RecoveryError::Io {
                action: "persist staging directory after observed manifest replacement",
                path: layout.staging_directory(),
                source,
            })?;
            journal
                .check_failpoint(
                    crate::operation::Failpoint::RecoveryManifestDestinationObservedBeforePersistence,
                )
                .map_err(RecoveryError::Journal)?;
            sync_file(&manifest.directory).map_err(|source| RecoveryError::Io {
                action: "persist observed manifest directory",
                path: manifest.directory_path.clone(),
                source,
            })?;
            journal
                .record_transition(DurableTransition::ManifestReplaced {
                    step_id: state.step_id,
                    manifest_path,
                    resulting_sha256: state.resulting_sha256,
                })
                .map_err(RecoveryError::Journal)?;
            continue;
        }
        if let Some(request) = requests.get(&manifest_path) {
            if ensure_manifest_request_matches(&state, request).is_err()
                && mode == RecoveryMode::CancelRemainingCopies
            {
                if staged_digest.is_some() {
                    remove_owned_stage(layout, lock, journal, &state.step_id, &state.staged_path)?;
                }
                layout
                    .replace_manifest(
                        lock,
                        journal,
                        ManifestReplacementRequest {
                            path: &request.path,
                            previous_sha256: request.previous_sha256.as_deref(),
                            resulting_sha256: &digest_bytes(&request.contents),
                            step_id: &request.step_id,
                        },
                        &request.contents,
                    )
                    .map_err(RecoveryError::Manifest)?;
                continue;
            }
        }
        if !state.staged && staged_digest.is_some() {
            remove_owned_stage(layout, lock, journal, &state.step_id, &state.staged_path)?;
            staged_digest = None;
        } else if staged_digest.as_deref() != Some(state.resulting_sha256.as_str())
            && staged_digest.is_some()
        {
            return Err(RecoveryError::ManifestConflict(manifest_path));
        }
        if staged_digest.as_deref() != Some(state.resulting_sha256.as_str()) {
            let request = requests
                .get(&manifest_path)
                .ok_or_else(|| RecoveryError::MissingManifestContents(manifest_path.clone()))?;
            ensure_manifest_request_matches(&state, request)?;
            write_stage(layout, lock, journal, &staged_path, &request.contents)?;
            journal
                .ensure_active(layout, lock)
                .map_err(RecoveryError::Journal)?;
            journal
                .record_transition(DurableTransition::ManifestStaged {
                    step_id: state.step_id.clone(),
                    staged_path: state.staged_path.clone(),
                    manifest_path: manifest_path.clone(),
                    resulting_sha256: state.resulting_sha256.clone(),
                })
                .map_err(RecoveryError::Journal)?;
        }
        verify_previous_manifest(&manifest.resolved_path, state.previous_sha256.as_deref())
            .map_err(RecoveryError::Manifest)?;
        journal
            .ensure_active(layout, lock)
            .map_err(RecoveryError::Journal)?;
        journal
            .record_transition(DurableTransition::ManifestPublicationStarted {
                step_id: state.step_id.clone(),
                staged_path: state.staged_path,
                manifest_path: manifest_path.clone(),
                resulting_sha256: state.resulting_sha256.clone(),
            })
            .map_err(RecoveryError::Journal)?;
        fs::rename(&staged_path, &manifest.resolved_path).map_err(|source| RecoveryError::Io {
            action: "publish recovered manifest",
            path: manifest.resolved_path.clone(),
            source,
        })?;
        sync_directory(&layout.staging_directory()).map_err(|source| RecoveryError::Io {
            action: "persist staging directory after recovered manifest",
            path: layout.staging_directory(),
            source,
        })?;
        sync_file(&manifest.directory).map_err(|source| RecoveryError::Io {
            action: "persist recovered manifest directory",
            path: manifest.directory_path,
            source,
        })?;
        journal
            .record_transition(DurableTransition::ManifestReplaced {
                step_id: state.step_id,
                manifest_path,
                resulting_sha256: state.resulting_sha256,
            })
            .map_err(RecoveryError::Journal)?;
    }
    Ok(interrupt(RecoveryCheckpoint::DurableBoundary))
}

#[derive(Clone)]
struct ManifestState {
    step_id: String,
    staged_path: PathBuf,
    previous_sha256: Option<String>,
    resulting_sha256: String,
    staged: bool,
    replaced: bool,
}

fn recover_missing_manifests(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    records: &[OperationRecord],
    requests: &[RecoveryManifest],
    interrupt: &mut impl FnMut(RecoveryCheckpoint) -> bool,
) -> Result<bool, RecoveryError> {
    let recorded: BTreeSet<_> = transitions(records)
        .filter_map(|transition| match transition {
            DurableTransition::ManifestReplacementStarted { manifest_path, .. } => {
                Some(manifest_path.clone())
            }
            _ => None,
        })
        .collect();
    for request in requests {
        if interrupt(RecoveryCheckpoint::DurableBoundary) {
            return Ok(true);
        }
        if recorded.contains(&request.path) {
            continue;
        }
        let resulting_sha256 = digest_bytes(&request.contents);
        let manifest = crate::catalog_path::resolve_catalog_entry(layout, &request.path)
            .map_err(RecoveryError::Path)?;
        if digest_if_file(&manifest.resolved_path)?.as_deref() == Some(resulting_sha256.as_str()) {
            continue;
        }
        layout
            .replace_manifest(
                lock,
                journal,
                ManifestReplacementRequest {
                    path: &request.path,
                    previous_sha256: request.previous_sha256.as_deref(),
                    resulting_sha256: &resulting_sha256,
                    step_id: &request.step_id,
                },
                &request.contents,
            )
            .map_err(RecoveryError::Manifest)?;
    }
    Ok(interrupt(RecoveryCheckpoint::DurableBoundary))
}

fn cleanup_owned_stages(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    records: &[OperationRecord],
) -> Result<(), RecoveryError> {
    #[derive(Default)]
    struct CleanupState {
        step_id: String,
        removal_started: bool,
        removed: bool,
    }

    let mut paths = BTreeMap::<PathBuf, CleanupState>::new();
    for transition in transitions(records) {
        match transition {
            DurableTransition::CopyStarted { staged_path, .. }
            | DurableTransition::CopyVerified { staged_path, .. }
            | DurableTransition::CopyPublicationStarted { staged_path, .. }
            | DurableTransition::ManifestReplacementStarted { staged_path, .. }
            | DurableTransition::ManifestStaged { staged_path, .. }
            | DurableTransition::ManifestPublicationStarted { staged_path, .. } => {
                let step_id = match transition {
                    DurableTransition::CopyStarted { step_id, .. }
                    | DurableTransition::CopyVerified { step_id, .. }
                    | DurableTransition::CopyPublicationStarted { step_id, .. }
                    | DurableTransition::ManifestReplacementStarted { step_id, .. }
                    | DurableTransition::ManifestStaged { step_id, .. }
                    | DurableTransition::ManifestPublicationStarted { step_id, .. } => step_id,
                    _ => unreachable!(),
                };
                validate_owned_stage(layout, journal.operation_id(), staged_path)?;
                let state = paths.entry(staged_path.clone()).or_default();
                if !state.step_id.is_empty() && state.step_id != *step_id {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.step_id = step_id.clone();
                if state.removed {
                    state.removal_started = false;
                    state.removed = false;
                }
            }
            DurableTransition::StageRemovalStarted {
                step_id,
                staged_path,
            } => {
                validate_owned_stage(layout, journal.operation_id(), staged_path)?;
                let state = paths
                    .get_mut(staged_path)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.step_id != *step_id || state.removed {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.removal_started = true;
            }
            DurableTransition::StageRemoved {
                step_id,
                staged_path,
            } => {
                validate_owned_stage(layout, journal.operation_id(), staged_path)?;
                let state = paths
                    .get_mut(staged_path)
                    .ok_or_else(|| RecoveryError::InconsistentStep(step_id.clone()))?;
                if state.step_id != *step_id || !state.removal_started {
                    return Err(RecoveryError::InconsistentStep(step_id.clone()));
                }
                state.removed = true;
            }
            _ => {}
        }
    }
    for (path, state) in paths {
        if state.removed {
            continue;
        }
        if state.removal_started {
            finish_owned_stage_removal(layout, lock, journal, &state.step_id, &path)?;
        } else if digest_if_file(&layout.root().join(&path))?.is_some() {
            remove_owned_stage(layout, lock, journal, &state.step_id, &path)?;
        }
    }
    Ok(())
}

fn validate_manifest_requests(
    requests: &[RecoveryManifest],
) -> Result<BTreeMap<PathBuf, &RecoveryManifest>, RecoveryError> {
    let mut by_path = BTreeMap::new();
    for request in requests {
        validate_identifier("step", &request.step_id).map_err(RecoveryError::Journal)?;
        if request.path.file_name() != Some(std::ffi::OsStr::new("SHA256SUMS")) {
            return Err(RecoveryError::InvalidManifestPath(request.path.clone()));
        }
        if by_path.insert(request.path.clone(), request).is_some() {
            return Err(RecoveryError::DuplicateManifest(request.path.clone()));
        }
    }
    Ok(by_path)
}

fn validate_requests(request: &RecoveryRequest) -> Result<(), RecoveryError> {
    let mut step_ids = BTreeSet::new();
    for directory in &request.directories {
        validate_identifier("step", &directory.step_id).map_err(RecoveryError::Journal)?;
        crate::catalog_path::validate_catalog_relative(&directory.path)
            .map_err(RecoveryError::Path)?;
        if !step_ids.insert(directory.step_id.clone()) {
            return Err(RecoveryError::DuplicateStep(directory.step_id.clone()));
        }
    }
    for copy in &request.copies {
        validate_identifier("step", &copy.step_id).map_err(RecoveryError::Journal)?;
        crate::catalog_path::validate_catalog_relative(&copy.destination)
            .map_err(RecoveryError::Path)?;
        if !step_ids.insert(copy.step_id.clone()) {
            return Err(RecoveryError::DuplicateStep(copy.step_id.clone()));
        }
    }
    for manifest in &request.manifests {
        if !step_ids.insert(manifest.step_id.clone()) {
            return Err(RecoveryError::DuplicateStep(manifest.step_id.clone()));
        }
    }
    Ok(())
}

fn ensure_manifest_request_matches(
    state: &ManifestState,
    request: &RecoveryManifest,
) -> Result<(), RecoveryError> {
    if state.step_id != request.step_id
        || state.previous_sha256 != request.previous_sha256
        || state.resulting_sha256 != digest_bytes(&request.contents)
    {
        Err(RecoveryError::ManifestRequestMismatch(request.path.clone()))
    } else {
        Ok(())
    }
}

fn write_stage(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &OperationJournal,
    path: &Path,
    contents: &[u8],
) -> Result<(), RecoveryError> {
    journal
        .ensure_active(layout, lock)
        .map_err(RecoveryError::Journal)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| RecoveryError::Io {
            action: "create recovered manifest stage",
            path: path.to_path_buf(),
            source,
        })?;
    sync_directory(&layout.staging_directory()).map_err(|source| RecoveryError::Io {
        action: "persist recovered manifest stage entry",
        path: layout.staging_directory(),
        source,
    })?;
    file.write_all(contents)
        .and_then(|()| sync_file(&file))
        .map_err(|source| RecoveryError::Io {
            action: "write recovered manifest stage",
            path: path.to_path_buf(),
            source,
        })
}

fn remove_owned_stage(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    step_id: &str,
    relative_path: &Path,
) -> Result<(), RecoveryError> {
    validate_owned_stage(layout, journal.operation_id(), relative_path)?;
    journal
        .ensure_active(layout, lock)
        .map_err(RecoveryError::Journal)?;
    journal
        .record_transition(DurableTransition::StageRemovalStarted {
            step_id: step_id.to_owned(),
            staged_path: relative_path.to_path_buf(),
        })
        .map_err(RecoveryError::Journal)?;
    journal
        .check_failpoint(crate::operation::Failpoint::StageRemovalIntentPersisted)
        .map_err(RecoveryError::Journal)?;
    finish_owned_stage_removal(layout, lock, journal, step_id, relative_path)
}

fn finish_owned_stage_removal(
    layout: &CatalogLayout,
    lock: &CatalogLock,
    journal: &mut OperationJournal,
    step_id: &str,
    relative_path: &Path,
) -> Result<(), RecoveryError> {
    validate_owned_stage(layout, journal.operation_id(), relative_path)?;
    journal
        .ensure_active(layout, lock)
        .map_err(RecoveryError::Journal)?;
    let path = layout.root().join(relative_path);
    let observed_absent = match fs::remove_file(&path) {
        Ok(()) => false,
        Err(source) if source.kind() == io::ErrorKind::NotFound => true,
        Err(source) => Err(RecoveryError::Io {
            action: "remove journal-owned staging file",
            path: path.clone(),
            source,
        })?,
    };
    remove_apple_double_sidecar(&path).map_err(|source| RecoveryError::Io {
        action: "remove recovered staged AppleDouble sidecar",
        path: path.clone(),
        source,
    })?;
    if observed_absent {
        journal
            .check_failpoint(
                crate::operation::Failpoint::RecoveryStageRemovalObservedBeforePersistence,
            )
            .map_err(RecoveryError::Journal)?;
    }
    sync_directory(&layout.staging_directory()).map_err(|source| RecoveryError::Io {
        action: "persist recovered staging cleanup",
        path: layout.staging_directory(),
        source,
    })?;
    journal
        .record_transition(DurableTransition::StageRemoved {
            step_id: step_id.to_owned(),
            staged_path: relative_path.to_path_buf(),
        })
        .map_err(RecoveryError::Journal)?;
    journal
        .check_failpoint(crate::operation::Failpoint::StageRemovalPersisted)
        .map_err(RecoveryError::Journal)
}

fn validate_owned_stage(
    layout: &CatalogLayout,
    operation_id: &str,
    relative_path: &Path,
) -> Result<(), RecoveryError> {
    let valid = !relative_path.is_absolute()
        && relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        && relative_path
            .strip_prefix(Path::new(".souvenirs/staging"))
            .is_ok_and(|suffix| {
                suffix.components().count() == 1
                    && suffix
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with(&format!("{}-{operation_id}-", operation_id.len()))
                        })
            });
    if !valid {
        return Err(RecoveryError::InvalidStagingPath(
            relative_path.to_path_buf(),
        ));
    }
    let parent = layout
        .root()
        .join(relative_path)
        .parent()
        .expect("owned stage has a parent")
        .to_path_buf();
    let canonical_parent = fs::canonicalize(&parent).map_err(|source| RecoveryError::Io {
        action: "resolve journal-owned staging directory",
        path: parent,
        source,
    })?;
    let canonical_staging =
        fs::canonicalize(layout.staging_directory()).map_err(|source| RecoveryError::Io {
            action: "resolve catalog staging directory",
            path: layout.staging_directory(),
            source,
        })?;
    if canonical_parent != canonical_staging {
        return Err(RecoveryError::InvalidStagingPath(
            relative_path.to_path_buf(),
        ));
    }
    Ok(())
}

fn resolve_directory(
    layout: &CatalogLayout,
    relative_path: &Path,
) -> Result<PathBuf, RecoveryError> {
    crate::catalog_path::validate_catalog_relative(relative_path).map_err(RecoveryError::Path)?;
    let path = layout.root().join(relative_path);
    let parent = path.parent().expect("catalog directory has a parent");
    let canonical_parent = fs::canonicalize(parent).map_err(|source| RecoveryError::Io {
        action: "resolve recovered destination parent",
        path: parent.to_path_buf(),
        source,
    })?;
    let canonical_root = fs::canonicalize(layout.root()).map_err(|source| RecoveryError::Io {
        action: "resolve recovery catalog root",
        path: layout.root().to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(canonical_root) {
        return Err(RecoveryError::PathConflict(path));
    }
    Ok(path)
}

fn ensure_directory(path: &Path) -> Result<(), RecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(RecoveryError::PathConflict(path.to_path_buf())),
        Err(source) => Err(RecoveryError::Io {
            action: "inspect recovered destination directory",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_digest(
    path: &Path,
    actual: Option<String>,
    expected: Option<&str>,
) -> Result<(), RecoveryError> {
    if actual.as_deref() == expected {
        Ok(())
    } else {
        Err(RecoveryError::DigestConflict {
            path: path.to_path_buf(),
            expected: expected.map(str::to_owned),
            actual,
        })
    }
}

fn digest_if_file(path: &Path) -> Result<Option<String>, RecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let mut file = File::open(path).map_err(|source| RecoveryError::Io {
                action: "open recovery file for hashing",
                path: path.to_path_buf(),
                source,
            })?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0_u8; 128 * 1024];
            loop {
                let count = file.read(&mut buffer).map_err(|source| RecoveryError::Io {
                    action: "hash recovery file",
                    path: path.to_path_buf(),
                    source,
                })?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(Some(format!("{:x}", hasher.finalize())))
        }
        Ok(_) => Err(RecoveryError::PathConflict(path.to_path_buf())),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RecoveryError::Io {
            action: "inspect recovery file",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn transitions(records: &[OperationRecord]) -> impl Iterator<Item = &DurableTransition> {
    records.iter().filter_map(|record| match record {
        OperationRecord::Transition { transition } => Some(transition),
        _ => None,
    })
}

#[derive(Debug)]
pub enum RecoveryError {
    Copy(crate::VerifiedCopyError),
    CopyConflict(PathBuf),
    DigestConflict {
        path: PathBuf,
        expected: Option<String>,
        actual: Option<String>,
    },
    DuplicateManifest(PathBuf),
    DuplicateStep(String),
    ExpectedContentMismatch {
        step: String,
        expected_sha256: String,
        actual_sha256: String,
        expected_size_bytes: u64,
        actual_size_bytes: Option<u64>,
    },
    InconsistentStep(String),
    InvalidManifestPath(PathBuf),
    InvalidStagingPath(PathBuf),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Journal(OperationJournalError),
    Manifest(crate::ManifestReplacementError),
    ManifestConflict(PathBuf),
    ManifestRequestMismatch(PathBuf),
    MissingManifestContents(PathBuf),
    Path(crate::CatalogPathError),
    PathConflict(PathBuf),
    RequestMismatch(String),
    SourceUnavailable(PathBuf),
    UnverifiedDestination(PathBuf),
    VerifiedStageMissing(PathBuf),
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Copy(source) => source.fmt(formatter),
            Self::CopyConflict(path) => write!(
                formatter,
                "interrupted copy state conflicts at {}",
                path.display()
            ),
            Self::DigestConflict {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "recovery digest conflict at {}: expected {expected:?}, found {actual:?}",
                path.display()
            ),
            Self::DuplicateManifest(path) => {
                write!(formatter, "duplicate recovery manifest: {}", path.display())
            }
            Self::DuplicateStep(step) => {
                write!(formatter, "duplicate recovery step identifier: {step:?}")
            }
            Self::ExpectedContentMismatch {
                step,
                expected_sha256,
                actual_sha256,
                expected_size_bytes,
                actual_size_bytes,
            } => {
                let actual_size_bytes = actual_size_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_owned());
                write!(
                    formatter,
                    "verified recovery copy {step:?} does not match the confirmed plan: expected SHA-256 {expected_sha256} and {expected_size_bytes} bytes, found SHA-256 {actual_sha256} and {actual_size_bytes} bytes"
                )
            }
            Self::InconsistentStep(step) => {
                write!(
                    formatter,
                    "operation journal has inconsistent step {step:?}"
                )
            }
            Self::InvalidManifestPath(path) => {
                write!(
                    formatter,
                    "invalid recovery manifest path: {}",
                    path.display()
                )
            }
            Self::InvalidStagingPath(path) => write!(
                formatter,
                "journal staging path is not owned by the catalog staging directory: {}",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Journal(source) => source.fmt(formatter),
            Self::Manifest(source) => source.fmt(formatter),
            Self::ManifestConflict(path) => {
                write!(
                    formatter,
                    "interrupted manifest state conflicts at {}",
                    path.display()
                )
            }
            Self::ManifestRequestMismatch(path) => write!(
                formatter,
                "recovery manifest does not match the journaled replacement at {}",
                path.display()
            ),
            Self::MissingManifestContents(path) => write!(
                formatter,
                "recovery requires planned manifest contents for {}",
                path.display()
            ),
            Self::Path(source) => source.fmt(formatter),
            Self::PathConflict(path) => {
                write!(
                    formatter,
                    "recovery refuses conflicting path {}",
                    path.display()
                )
            }
            Self::RequestMismatch(step) => write!(
                formatter,
                "recovery request does not match journaled step {step:?}"
            ),
            Self::SourceUnavailable(path) => {
                write!(formatter, "copy source is unavailable: {}", path.display())
            }
            Self::UnverifiedDestination(path) => write!(
                formatter,
                "unverified bytes appeared at copy destination {}",
                path.display()
            ),
            Self::VerifiedStageMissing(path) => write!(
                formatter,
                "verified staging file is missing before publication: {}",
                path.display()
            ),
        }
    }
}

impl Error for RecoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Copy(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Journal(source) => Some(source),
            Self::Manifest(source) => Some(source),
            Self::Path(source) => Some(source),
            _ => None,
        }
    }
}
