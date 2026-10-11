//! Newline-delimited JSON process lifecycle for the supervised engine.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ScopedJoinHandle};
use std::time::Duration;

use serde_json::{json, Value};
use souvenirs_application::{
    CatalogIntent as ApplicationIntent, CatalogService, DateResolution, DestinationMode,
    ImportCancellation, ImportDateSource as ApplicationDateSource,
    ImportDateState as ApplicationDateState, ImportExecutionRequest, ImportExecutionResult,
    ImportExecutionService, ImportPlanningService, ImportRecoveryMode, ImportRecoveryService,
    ImportReviewRequest, OperationInspectionService, OperationProgress, PreparedImport,
    RecoveryOutcome,
};
use souvenirs_protocol::{
    CatalogIntent, CatalogResult, ClientVersion, EngineMessage, HandshakeResult,
    ImportDateResolution, ImportDateSource, ImportDateSourceSummary, ImportDateState,
    ImportDestinationMode, ImportReviewResult, ImportScanResult, OperationAccepted, OperationEvent,
    OperationEventPayload, OperationInspectionResult, ProtocolError, Request, RequestPayload,
    ResponseResult, StartOperationParameters, UnfinishedOperationResult, CANCEL_OPERATION_METHOD,
    CATALOG_CREATE_METHOD, CATALOG_OPEN_METHOD, CATALOG_VALIDATE_METHOD, IMPORT_REVIEW_METHOD,
    IMPORT_SCAN_METHOD, MINIMUM_CLIENT_VERSION, OPERATION_INSPECT_METHOD, PROTOCOL_VERSION,
    START_OPERATION_METHOD,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct EngineOptions {
    pub progress_delay: Duration,
}

pub fn run_engine(input: impl BufRead, output: impl Write + Send) -> io::Result<()> {
    run_engine_with_options(input, output, EngineOptions::default())
}

pub fn run_engine_with_options(
    mut input: impl BufRead,
    output: impl Write + Send,
    options: EngineOptions,
) -> io::Result<()> {
    let output = Arc::new(Mutex::new(OutputSink::new(output)));
    let result: io::Result<()> = thread::scope(|scope| {
        let mut connection = Connection::default();
        let mut active: Option<ActiveOperation<'_>> = None;
        let mut line = Vec::new();

        let loop_result = (|| {
            loop {
                line.clear();
                if input.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                reap_active(&mut active, &mut connection)?;
                trim_line_ending(&mut line);
                let active_operation_id = active
                    .as_ref()
                    .map(|operation| operation.operation_id.as_str());
                match connection.handle_line(&line, active_operation_id) {
                    EngineAction::Message(message) => write_message(&output, &message)?,
                    EngineAction::Cancel {
                        request_id,
                        operation_id,
                    } => {
                        let message = if let Some(operation) = active
                            .as_ref()
                            .filter(|operation| operation.operation_id == operation_id)
                        {
                            if operation.cancellation.request() {
                                cancellation_accepted(request_id, operation_id)
                            } else {
                                operation_finished(request_id, operation_id)
                            }
                        } else {
                            connection.cancel_inactive(request_id, operation_id)
                        };
                        write_message(&output, &message)?;
                    }
                    EngineAction::Execute {
                        request_id,
                        prepared,
                    } => {
                        if let Some(operation) = active.as_ref() {
                            write_message(
                                &output,
                                &operation_in_progress(request_id, &operation.operation_id),
                            )?;
                            continue;
                        }
                        let operation_id = prepared.operation_id().to_owned();
                        connection
                            .operation_catalogs
                            .insert(operation_id.clone(), prepared_catalog(&prepared));
                        write_message(
                            &output,
                            &EngineMessage::success(
                                request_id,
                                ResponseResult::OperationAccepted(OperationAccepted {
                                    operation_id: operation_id.clone(),
                                }),
                            ),
                        )?;
                        write_operation_event(
                            &output,
                            &operation_id,
                            0,
                            OperationEventPayload::Started {
                                name: "import.execute".to_owned(),
                            },
                        )?;
                        let cancellation = ImportCancellation::default();
                        let worker_cancellation = cancellation.clone();
                        let worker_output = Arc::clone(&output);
                        let worker_operation_id = operation_id.clone();
                        let terminal_ready = Arc::new(AtomicBool::new(false));
                        let worker_terminal_ready = Arc::clone(&terminal_ready);
                        let handle = scope.spawn(move || {
                            let mut sequence = 1_u64;
                            let result = ImportExecutionService::execute(
                                *prepared,
                                &worker_cancellation,
                                |progress| {
                                    let _ = write_progress(
                                        &worker_output,
                                        &worker_operation_id,
                                        &mut sequence,
                                        progress,
                                    );
                                    if !options.progress_delay.is_zero() {
                                        thread::sleep(options.progress_delay);
                                    }
                                },
                            );
                            worker_cancellation.finish();
                            worker_terminal_ready.store(true, Ordering::Release);
                            let _ = write_execution_result(
                                &worker_output,
                                &worker_operation_id,
                                sequence,
                                result,
                            );
                        });
                        active = Some(ActiveOperation {
                            operation_id,
                            cancellation,
                            terminal_ready,
                            handle,
                        });
                    }
                    EngineAction::Recover {
                        request_id,
                        catalog_path,
                        operation_id,
                    } => {
                        if let Some(operation) = active.as_ref() {
                            write_message(
                                &output,
                                &operation_in_progress(request_id, &operation.operation_id),
                            )?;
                            continue;
                        }
                        connection
                            .operation_catalogs
                            .insert(operation_id.clone(), catalog_path.clone());
                        write_message(
                            &output,
                            &EngineMessage::success(
                                request_id,
                                ResponseResult::OperationAccepted(OperationAccepted {
                                    operation_id: operation_id.clone(),
                                }),
                            ),
                        )?;
                        write_operation_event(
                            &output,
                            &operation_id,
                            0,
                            OperationEventPayload::Started {
                                name: "operation.recover".to_owned(),
                            },
                        )?;
                        let mut sequence = 1_u64;
                        let result = ImportRecoveryService::recover(
                            catalog_path,
                            &operation_id,
                            ImportRecoveryMode::Resume,
                            |progress| {
                                let _ =
                                    write_progress(&output, &operation_id, &mut sequence, progress);
                            },
                        );
                        write_recovery_result(&output, &operation_id, sequence, result)?;
                    }
                }
            }
            Ok(())
        })();

        if let Some(operation) = active.take() {
            let _ = operation.cancellation.request();
            finish_active(operation, &mut connection)?;
        }
        loop_result
    });
    result?;
    let result = output
        .lock()
        .expect("engine output mutex should not be poisoned")
        .finish();
    result
}

struct ActiveOperation<'scope> {
    operation_id: String,
    cancellation: ImportCancellation,
    terminal_ready: Arc<AtomicBool>,
    handle: ScopedJoinHandle<'scope, ()>,
}

fn reap_active(
    active: &mut Option<ActiveOperation<'_>>,
    connection: &mut Connection,
) -> io::Result<()> {
    if active.as_ref().is_some_and(|operation| {
        operation.handle.is_finished() || operation.terminal_ready.load(Ordering::Acquire)
    }) {
        finish_active(
            active.take().expect("active operation should exist"),
            connection,
        )?;
    }
    Ok(())
}

fn finish_active(operation: ActiveOperation<'_>, connection: &mut Connection) -> io::Result<()> {
    let operation_id = operation.operation_id;
    operation
        .handle
        .join()
        .map_err(|_| io::Error::other("import execution worker panicked"))?;
    connection.operation_catalogs.remove(&operation_id);
    Ok(())
}

#[derive(Default)]
struct Connection {
    handshake_complete: bool,
    operation_catalogs: HashMap<String, String>,
}

impl Connection {
    fn handle_line(&mut self, line: &[u8], active_operation_id: Option<&str>) -> EngineAction {
        let (request_id, protocol_version) = match request_envelope(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                return EngineAction::Message(EngineMessage::error(request_id(line), error))
            }
        };
        if protocol_version != PROTOCOL_VERSION {
            return EngineAction::Message(incompatible_protocol(request_id, protocol_version));
        }

        let request = match serde_json::from_slice::<Request>(line) {
            Ok(request) => request,
            Err(error) => {
                return EngineAction::Message(EngineMessage::error(
                    request_id,
                    ProtocolError::new(
                        "invalid_request",
                        format!("request does not match protocol version {PROTOCOL_VERSION}"),
                    )
                    .with_data(json!({"detail": error.to_string()})),
                ));
            }
        };

        if !self.handshake_complete && !matches!(request.payload, RequestPayload::Handshake(_)) {
            return EngineAction::Message(EngineMessage::error(
                Some(request.id),
                ProtocolError::new(
                    "handshake_required",
                    "the first request must be a handshake",
                ),
            ));
        }

        EngineAction::Message(match request.payload {
            RequestPayload::Handshake(parameters) => {
                if self.handshake_complete {
                    return EngineAction::Message(EngineMessage::error(
                        Some(request.id),
                        ProtocolError::new(
                            "handshake_already_completed",
                            "the connection handshake has already completed",
                        ),
                    ));
                }
                if parameters.client_version.parse::<ClientVersion>().is_err() {
                    return EngineAction::Message(EngineMessage::error(
                        Some(request.id),
                        ProtocolError::new(
                            "invalid_client_version",
                            "client version must use major.minor.patch unsigned decimal syntax",
                        ),
                    ));
                }
                self.handshake_complete = true;
                EngineMessage::success(
                    request.id,
                    ResponseResult::Handshake(HandshakeResult {
                        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
                        protocol_version: PROTOCOL_VERSION,
                        compatible_client_version: MINIMUM_CLIENT_VERSION.to_owned(),
                        capabilities: vec![
                            CANCEL_OPERATION_METHOD.to_owned(),
                            CATALOG_VALIDATE_METHOD.to_owned(),
                            CATALOG_CREATE_METHOD.to_owned(),
                            CATALOG_OPEN_METHOD.to_owned(),
                            IMPORT_SCAN_METHOD.to_owned(),
                            IMPORT_REVIEW_METHOD.to_owned(),
                            OPERATION_INSPECT_METHOD.to_owned(),
                            START_OPERATION_METHOD.to_owned(),
                        ],
                    }),
                )
            }
            RequestPayload::StartOperation(parameters) => {
                return self.start_operation(request.id, parameters);
            }
            RequestPayload::CancelOperation(parameters) => {
                return EngineAction::Cancel {
                    request_id: request.id,
                    operation_id: parameters.operation_id,
                };
            }
            RequestPayload::ValidateCatalog(parameters) => catalog_response(
                request.id,
                CatalogService::validate(
                    parameters.path,
                    match parameters.intent {
                        CatalogIntent::Create => ApplicationIntent::Create,
                        CatalogIntent::Open => ApplicationIntent::Open,
                    },
                ),
            ),
            RequestPayload::CreateCatalog(parameters) => catalog_response(
                request.id,
                CatalogService::create(parameters.path, parameters.display_name),
            ),
            RequestPayload::OpenCatalog(parameters) => {
                catalog_response(request.id, CatalogService::open(parameters.path))
            }
            RequestPayload::ScanImport(parameters) => import_scan_response(
                request.id,
                ImportPlanningService::scan(parameters.source_path),
            ),
            RequestPayload::ReviewImport(parameters) => import_review_response(
                request.id,
                ImportPlanningService::review(ImportReviewRequest {
                    catalog_path: parameters.catalog_path.into(),
                    source_path: parameters.source_path.into(),
                    collection_name: parameters.collection_name,
                    destination_mode: match parameters.destination_mode {
                        ImportDestinationMode::Create => DestinationMode::Create,
                        ImportDestinationMode::Existing => DestinationMode::Existing,
                    },
                    accepted_date: parameters.accepted_date,
                    date_resolution: parameters.date_resolution.map(
                        |resolution| match resolution {
                            ImportDateResolution::AsShown => DateResolution::AsShown,
                            ImportDateResolution::ConvertFromUtc => DateResolution::ConvertFromUtc,
                        },
                    ),
                }),
            ),
            RequestPayload::InspectOperation(parameters) => {
                match OperationInspectionService::unfinished(&parameters.path) {
                    Ok(operation) => {
                        let operation = operation.filter(|operation| {
                            active_operation_id != Some(operation.operation_id.as_str())
                        });
                        if let Some(operation) = &operation {
                            self.operation_catalogs
                                .insert(operation.operation_id.clone(), parameters.path.clone());
                        }
                        EngineMessage::success(
                            request.id,
                            ResponseResult::OperationInspection(OperationInspectionResult {
                                operation: operation.map(|operation| UnfinishedOperationResult {
                                    operation_id: operation.operation_id,
                                    collection_name: operation.collection_name,
                                    source_name: operation.source_name,
                                    copied_count: operation.copied_count,
                                    total_count: operation.total_count,
                                    source_available: operation.source_available,
                                }),
                            }),
                        )
                    }
                    Err(error) => EngineMessage::error(
                        Some(request.id),
                        ProtocolError::new(error.code(), error.to_string()),
                    ),
                }
            }
        })
    }

    fn start_operation(
        &self,
        request_id: String,
        parameters: StartOperationParameters,
    ) -> EngineAction {
        match parameters {
            StartOperationParameters::ImportExecute { reviewed_plan } => {
                match ImportExecutionService::prepare(ImportExecutionRequest { reviewed_plan }) {
                    Ok(prepared) => EngineAction::Execute {
                        request_id,
                        prepared: Box::new(prepared),
                    },
                    Err(error) => EngineAction::Message(EngineMessage::error(
                        Some(request_id),
                        ProtocolError::new(error.code(), error.to_string()),
                    )),
                }
            }
            StartOperationParameters::Recover {
                catalog_path,
                operation_id,
            } => EngineAction::Recover {
                request_id,
                catalog_path,
                operation_id,
            },
        }
    }

    fn cancel_inactive(&self, request_id: String, operation_id: String) -> EngineMessage {
        let Some(catalog_path) = self.operation_catalogs.get(&operation_id) else {
            return operation_not_found(request_id, operation_id);
        };
        match ImportRecoveryService::recover(
            catalog_path,
            &operation_id,
            ImportRecoveryMode::Cancel,
            |_| {},
        ) {
            Ok(_) => cancellation_accepted(request_id, operation_id),
            Err(error) => EngineMessage::error(
                Some(request_id),
                ProtocolError::new(error.code(), error.to_string()),
            ),
        }
    }
}

enum EngineAction {
    Message(EngineMessage),
    Cancel {
        request_id: String,
        operation_id: String,
    },
    Execute {
        request_id: String,
        prepared: Box<PreparedImport>,
    },
    Recover {
        request_id: String,
        catalog_path: String,
        operation_id: String,
    },
}

fn prepared_catalog(prepared: &PreparedImport) -> String {
    prepared.catalog_path().to_string_lossy().into_owned()
}

fn cancellation_accepted(request_id: String, operation_id: String) -> EngineMessage {
    EngineMessage::success(
        request_id,
        ResponseResult::CancellationAccepted(souvenirs_protocol::CancellationAccepted {
            operation_id,
        }),
    )
}

fn operation_finished(request_id: String, operation_id: String) -> EngineMessage {
    EngineMessage::error(
        Some(request_id),
        ProtocolError::new(
            "operation_finished",
            format!("operation {operation_id:?} already reached its terminal boundary"),
        )
        .with_data(json!({"operation_id": operation_id})),
    )
}

fn operation_not_found(request_id: String, operation_id: String) -> EngineMessage {
    EngineMessage::error(
        Some(request_id),
        ProtocolError::new(
            "operation_not_found",
            format!("operation {operation_id:?} was not found"),
        )
        .with_data(json!({"operation_id": operation_id})),
    )
}

fn operation_in_progress(request_id: String, operation_id: &str) -> EngineMessage {
    EngineMessage::error(
        Some(request_id),
        ProtocolError::new(
            "operation_in_progress",
            format!("operation {operation_id:?} is already running"),
        )
        .with_data(json!({"operation_id": operation_id})),
    )
}

struct OutputSink<W> {
    writer: W,
    error: Option<(io::ErrorKind, String)>,
}

impl<W: Write> OutputSink<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            error: None,
        }
    }

    fn write(&mut self, message: &EngineMessage) -> io::Result<()> {
        if let Some((kind, message)) = &self.error {
            return Err(io::Error::new(*kind, message.clone()));
        }
        let result = (|| {
            serde_json::to_writer(&mut self.writer, message).map_err(io::Error::other)?;
            self.writer.write_all(b"\n")?;
            self.writer.flush()
        })();
        if let Err(error) = &result {
            self.error = Some((error.kind(), error.to_string()));
        }
        result
    }

    fn finish(&mut self) -> io::Result<()> {
        match &self.error {
            Some((kind, message)) => Err(io::Error::new(*kind, message.clone())),
            None => Ok(()),
        }
    }
}

fn write_message(
    output: &Arc<Mutex<OutputSink<impl Write>>>,
    message: &EngineMessage,
) -> io::Result<()> {
    output
        .lock()
        .expect("engine output mutex should not be poisoned")
        .write(message)
}

fn write_operation_event(
    output: &Arc<Mutex<OutputSink<impl Write>>>,
    operation_id: &str,
    sequence: u64,
    payload: OperationEventPayload,
) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::event(OperationEvent {
            protocol_version: PROTOCOL_VERSION,
            operation_id: operation_id.to_owned(),
            sequence,
            payload,
        }),
    )
}

fn write_progress(
    output: &Arc<Mutex<OutputSink<impl Write>>>,
    operation_id: &str,
    sequence: &mut u64,
    progress: OperationProgress,
) -> io::Result<()> {
    let result = write_operation_event(
        output,
        operation_id,
        *sequence,
        OperationEventPayload::Progress {
            completed_units: progress.completed_units,
            total_units: Some(progress.total_units),
            message: Some(progress.message),
        },
    );
    *sequence += 1;
    result
}

fn write_execution_result(
    output: &Arc<Mutex<OutputSink<impl Write>>>,
    operation_id: &str,
    sequence: u64,
    result: Result<
        souvenirs_application::ImportExecutionResult,
        souvenirs_application::ImportExecutionError,
    >,
) -> io::Result<()> {
    match result {
        Ok(ImportExecutionResult::Completed(outcome)) => write_operation_event(
            output,
            operation_id,
            sequence,
            OperationEventPayload::Completed {
                result: serde_json::to_value(outcome).map_err(io::Error::other)?,
            },
        ),
        Ok(ImportExecutionResult::Cancelled(outcome)) => write_operation_event(
            output,
            operation_id,
            sequence,
            OperationEventPayload::Cancelled {
                copied_count: outcome.copied_count,
            },
        ),
        Err(error) => write_operation_event(
            output,
            operation_id,
            sequence,
            OperationEventPayload::Failed {
                error: ProtocolError::new(error.code(), error.to_string()),
            },
        ),
    }
}

fn write_recovery_result(
    output: &Arc<Mutex<OutputSink<impl Write>>>,
    operation_id: &str,
    sequence: u64,
    result: Result<
        (
            RecoveryOutcome,
            souvenirs_application::ImportExecutionOutcome,
        ),
        souvenirs_application::ImportExecutionError,
    >,
) -> io::Result<()> {
    match result {
        Ok((RecoveryOutcome::Completed, outcome)) => write_operation_event(
            output,
            operation_id,
            sequence,
            OperationEventPayload::Completed {
                result: serde_json::to_value(outcome).map_err(io::Error::other)?,
            },
        ),
        Ok((RecoveryOutcome::Cancelled, outcome)) => write_operation_event(
            output,
            operation_id,
            sequence,
            OperationEventPayload::Cancelled {
                copied_count: outcome.copied_count,
            },
        ),
        Err(error) => write_operation_event(
            output,
            operation_id,
            sequence,
            OperationEventPayload::Failed {
                error: ProtocolError::new(error.code(), error.to_string()),
            },
        ),
    }
}

fn catalog_response(
    request_id: String,
    result: Result<
        souvenirs_application::CatalogLocation,
        souvenirs_application::CatalogServiceError,
    >,
) -> EngineMessage {
    match result {
        Ok(location) => EngineMessage::success(
            request_id,
            ResponseResult::Catalog(CatalogResult {
                path: location.path.to_string_lossy().into_owned(),
                volume_name: location.volume_name,
                display_name: location.display_name,
                detail: location.detail,
            }),
        ),
        Err(error) => EngineMessage::error(
            Some(request_id),
            ProtocolError::new(error.code(), error.to_string()),
        ),
    }
}

fn import_scan_result(scan: souvenirs_application::ImportScanSummary) -> ImportScanResult {
    use ApplicationDateSource as A;
    use ImportDateSource as P;

    ImportScanResult {
        source_path: scan.source_path.to_string_lossy().into_owned(),
        source_name: scan.source_name,
        file_count: scan.file_count,
        media_count: scan.media_count,
        photo_count: scan.photo_count,
        video_count: scan.video_count,
        unsupported_count: scan.unsupported_count,
        uncertain_time_count: scan.uncertain_time_count,
        total_bytes: scan.total_bytes,
        suggested_collection_name: scan.suggested_collection_name,
        suggested_accepted_date: scan.suggested_accepted_date,
        suggested_range_end: scan.suggested_range_end,
        pending_item_count: scan.pending_item_count,
        unresolved_item_count: scan.unresolved_item_count,
        datetime_sources: scan
            .datetime_sources
            .into_iter()
            .map(|summary| ImportDateSourceSummary {
                source: match summary.source {
                    A::ExifDateTimeOriginal => P::ExifDateTimeOriginal,
                    A::ExifDateTimeDigitized => P::ExifDateTimeDigitized,
                    A::QuickTimeCreationDate => P::QuickTimeCreationDate,
                    A::XmpDateCreated => P::XmpDateCreated,
                    A::XmpExifDateTimeOriginal => P::XmpExifDateTimeOriginal,
                    A::ContainerCreationTime => P::ContainerCreationTime,
                    A::GoogleTakeoutPhotoTakenTime => P::GoogleTakeoutPhotoTakenTime,
                    A::FilenamePattern => P::FilenamePattern,
                },
                state: match summary.state {
                    ApplicationDateState::Accepted => ImportDateState::Accepted,
                    ApplicationDateState::NeedsConfirmation => ImportDateState::NeedsConfirmation,
                },
                item_count: summary.item_count,
            })
            .collect(),
    }
}

fn import_scan_response(
    request_id: String,
    result: Result<
        souvenirs_application::ImportScanSummary,
        souvenirs_application::ImportPlanningError,
    >,
) -> EngineMessage {
    match result {
        Ok(scan) => EngineMessage::success(
            request_id,
            ResponseResult::ImportScan(Box::new(import_scan_result(scan))),
        ),
        Err(error) => EngineMessage::error(
            Some(request_id),
            ProtocolError::new(error.code(), error.to_string()),
        ),
    }
}

fn import_review_response(
    request_id: String,
    result: Result<souvenirs_application::ImportReview, souvenirs_application::ImportPlanningError>,
) -> EngineMessage {
    match result {
        Ok(review) => EngineMessage::success(
            request_id,
            ResponseResult::ImportReview(Box::new(ImportReviewResult {
                scan: import_scan_result(review.scan),
                collection_name: review.collection_name,
                destination_mode: match review.destination_mode {
                    DestinationMode::Create => ImportDestinationMode::Create,
                    DestinationMode::Existing => ImportDestinationMode::Existing,
                },
                destination_path: review.destination_path.to_string_lossy().into_owned(),
                accepted_date: review.accepted_date,
                date_resolution: review.date_resolution.map(|resolution| match resolution {
                    DateResolution::AsShown => ImportDateResolution::AsShown,
                    DateResolution::ConvertFromUtc => ImportDateResolution::ConvertFromUtc,
                }),
                required_decision_count: review.required_decision_count,
                transfer_count: review.transfer_count,
                skipped_count: review.skipped_count,
                left_on_source_count: review.left_on_source_count,
                operation_count: review.operation_count,
                required_space_bytes: review.required_space_bytes,
                can_start: review.can_start,
                reviewed_plan: review.reviewed_plan,
            })),
        ),
        Err(error) => EngineMessage::error(
            Some(request_id),
            ProtocolError::new(error.code(), error.to_string()),
        ),
    }
}

fn request_envelope(line: &[u8]) -> Result<(Option<String>, u32), ProtocolError> {
    let value: Value = serde_json::from_slice(line).map_err(|error| {
        ProtocolError::new("invalid_request", "request is not valid JSON")
            .with_data(json!({"detail": error.to_string()}))
    })?;
    let object = value
        .as_object()
        .ok_or_else(|| ProtocolError::new("invalid_request", "request must be a JSON object"))?;
    let id = object.get("id").and_then(Value::as_str).map(str::to_owned);
    let version = object
        .get("protocol_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            ProtocolError::new(
                "invalid_request",
                "request protocol_version must be an unsigned 32-bit integer",
            )
        })?;
    Ok((id, version))
}

fn incompatible_protocol(id: Option<String>, actual: u32) -> EngineMessage {
    EngineMessage::error(
        id,
        ProtocolError::new(
            "incompatible_protocol",
            format!("protocol version {actual} is unsupported; expected {PROTOCOL_VERSION}"),
        )
        .with_data(json!({
            "expected": PROTOCOL_VERSION,
            "actual": actual
        })),
    )
}

fn trim_line_ending(line: &mut Vec<u8>) {
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
}

fn request_id(line: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(line)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor};
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        sync::Arc,
        time::Duration,
    };

    use serde_json::{json, Value};
    use souvenirs_application::{
        CatalogService, DateResolution, DestinationMode, ImportCancellation,
        ImportExecutionService, ImportPlanningService, ImportReviewRequest,
    };
    use souvenirs_protocol::PROTOCOL_VERSION;

    use super::run_engine;

    #[test]
    fn reaps_worker_when_terminal_is_ready_before_thread_exit() {
        std::thread::scope(|scope| {
            let terminal_ready = Arc::new(AtomicBool::new(false));
            let worker_terminal_ready = Arc::clone(&terminal_ready);
            let handle = scope.spawn(move || {
                worker_terminal_ready.store(true, Ordering::Release);
                std::thread::sleep(Duration::from_millis(20));
            });
            let mut active = Some(super::ActiveOperation {
                operation_id: "operation-1".to_owned(),
                cancellation: ImportCancellation::default(),
                terminal_ready,
                handle,
            });
            let mut connection = super::Connection::default();
            connection
                .operation_catalogs
                .insert("operation-1".to_owned(), "/catalog".to_owned());
            while !active
                .as_ref()
                .unwrap()
                .terminal_ready
                .load(Ordering::Acquire)
            {
                std::thread::yield_now();
            }

            super::reap_active(&mut active, &mut connection).unwrap();

            assert!(active.is_none());
            assert!(!connection.operation_catalogs.contains_key("operation-1"));
        });
    }

    #[test]
    fn handshake_returns_process_contract() {
        let input = request("request-1", "handshake", json!({"client_version": "0.1.0"}));

        let response = run(input);

        assert_eq!(response["kind"], "response");
        assert_eq!(response["id"], "request-1");
        assert_eq!(response["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(response["status"], "success");
        assert_eq!(
            response["result"],
            json!({
                "type": "handshake",
                "engine_version": env!("CARGO_PKG_VERSION"),
                "protocol_version": PROTOCOL_VERSION,
                "compatible_client_version": "0.2.0",
                "capabilities": [
                    "operation.cancel",
                    "catalog.validate",
                    "catalog.create",
                    "catalog.open",
                    "import.scan",
                    "import.review",
                    "operation.inspect",
                    "operation.start"
                ]
            })
        );
    }

    #[test]
    fn first_request_must_be_handshake() {
        let input = request(
            "request-1",
            "operation.cancel",
            json!({"operation_id": "operation-1"}),
        );

        let response = run(input);

        assert_eq!(response["error"]["code"], "handshake_required");
    }

    #[test]
    fn duplicate_handshake_is_rejected() {
        let handshake = request("request-1", "handshake", json!({"client_version": "0.1.0"}));
        let input = format!("{handshake}{handshake}");

        let responses = run_lines(input);

        assert!(responses[0]["result"].is_object());
        assert_eq!(responses[1]["error"]["code"], "handshake_already_completed");
    }

    #[test]
    fn previous_protocol_is_rejected_with_version_data() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION - 1,
                "method": "handshake",
                "params": {"client_version": "0.1.0"}
            })
        );

        let response = run(input);

        assert_eq!(response["id"], "request-1");
        assert_eq!(response["error"]["code"], "incompatible_protocol");
        assert_eq!(response["error"]["data"]["expected"], PROTOCOL_VERSION);
        assert_eq!(response["error"]["data"]["actual"], PROTOCOL_VERSION - 1);
    }

    #[test]
    fn future_method_is_rejected_as_incompatible_before_typed_decode() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION + 1,
                "method": "future.method",
                "params": {"future_field": true}
            })
        );

        let response = run(input);

        assert_eq!(response["error"]["code"], "incompatible_protocol");
    }

    #[test]
    fn future_handshake_shape_is_rejected_as_incompatible() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION + 1,
                "method": "handshake",
                "params": {
                    "client_version": "0.2.0",
                    "future_field": true
                }
            })
        );

        let response = run(input);

        assert_eq!(response["error"]["code"], "incompatible_protocol");
    }

    #[test]
    fn version_only_future_message_is_rejected_as_incompatible() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION + 1
            })
        );

        let response = run(input);

        assert_eq!(response["error"]["code"], "incompatible_protocol");
    }

    #[test]
    fn malformed_message_preserves_valid_request_identifier() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION,
                "method": "handshake",
                "params": {}
            })
        );

        let response = run(input);

        assert_eq!(response["id"], "request-1");
        assert_eq!(response["error"]["code"], "invalid_request");
        assert_eq!(
            response["error"]["message"],
            format!("request does not match protocol version {PROTOCOL_VERSION}")
        );
    }

    #[test]
    fn malformed_client_version_is_rejected() {
        for client_version in ["", "0.1", "0.01.0", "0.a.0"] {
            let input = request(
                "request-1",
                "handshake",
                json!({"client_version": client_version}),
            );

            let response = run(input);

            assert_eq!(response["error"]["code"], "invalid_client_version");
        }
    }

    #[test]
    fn invalid_utf8_is_rejected_without_stopping_the_engine() {
        let mut input = b"\xff\n".to_vec();
        input.extend_from_slice(
            request("request-2", "handshake", json!({"client_version": "0.1.0"})).as_bytes(),
        );

        let responses = run_lines(input);

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], "invalid_request");
        assert!(responses[1]["result"].is_object());
    }

    #[test]
    fn cancellation_of_unknown_operation_is_structured() {
        let input = format!(
            "{}{}",
            request("request-1", "handshake", json!({"client_version": "0.1.0"})),
            request(
                "request-2",
                "operation.cancel",
                json!({"operation_id": "operation-missing"})
            )
        );

        let responses = run_lines(input);

        assert_eq!(responses[1]["error"]["code"], "operation_not_found");
        assert_eq!(
            responses[1]["error"]["data"]["operation_id"],
            "operation-missing"
        );
    }

    #[test]
    fn operation_start_requires_typed_parameters() {
        let input = format!(
            "{}{}",
            request("request-1", "handshake", json!({"client_version": "0.1.0"})),
            request(
                "request-2",
                "operation.start",
                json!({"name": "import.scan"})
            )
        );

        let responses = run_lines(input);

        assert_eq!(responses[1]["error"]["code"], "invalid_request");
        assert_eq!(responses[1]["id"], "request-2");
    }

    #[test]
    fn operation_start_rejects_a_removed_or_replaced_reviewed_existing_collection() {
        use std::os::unix::fs::symlink;
        for replacement in ["missing", "directory", "symlink"] {
            let temporary = tempfile::tempdir().unwrap();
            let [catalog, source] = ["catalog", "source"].map(|name| temporary.path().join(name));
            for path in [&catalog, &source] {
                fs::create_dir(path).unwrap();
            }
            fs::write(source.join("IMG_20261010_101112.jpg"), b"photo").unwrap();
            CatalogService::create(&catalog, "Catalog").unwrap();
            let collection = catalog.join("2026/2026-10-10 Import");
            fs::create_dir_all(&collection).unwrap();
            let journal = catalog.join(".souvenirs/journal");
            let review = ImportPlanningService::review(ImportReviewRequest {
                catalog_path: catalog,
                source_path: source.clone(),
                collection_name: "Import".to_owned(),
                destination_mode: DestinationMode::Existing,
                accepted_date: "2026-10-10".to_owned(),
                date_resolution: Some(DateResolution::AsShown),
            })
            .unwrap();
            let prepared = match super::Connection::default().start_operation(
                "request-2".to_owned(),
                super::StartOperationParameters::ImportExecute {
                    reviewed_plan: review.reviewed_plan.clone().unwrap(),
                },
            ) {
                super::EngineAction::Execute { prepared, .. } => prepared,
                _ => panic!("operation was not prepared"),
            };
            fs::remove_dir(&collection).unwrap();
            match replacement {
                "directory" => fs::create_dir(&collection).unwrap(),
                "symlink" => symlink(&source, &collection).unwrap(),
                _ => {}
            }
            let error =
                ImportExecutionService::execute(*prepared, &ImportCancellation::default(), |_| {})
                    .unwrap_err();
            assert_eq!(error.code(), "operation_conflict");
            assert_eq!(fs::read_dir(&journal).unwrap().count(), 1);
        }
    }

    #[test]
    fn catalog_create_and_open_use_application_service() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().to_string_lossy();
        let input = format!(
            "{}{}{}",
            request("request-1", "handshake", json!({"client_version": "0.1.0"})),
            request(
                "request-2",
                "catalog.create",
                json!({"path": path, "display_name": "Family Catalog"})
            ),
            request("request-3", "catalog.open", json!({"path": path}))
        );

        let responses = run_lines(input);

        assert_eq!(responses[1]["result"]["type"], "catalog");
        assert_eq!(responses[1]["result"]["display_name"], "Family Catalog");
        assert_eq!(responses[2]["result"]["display_name"], "Family Catalog");
    }

    #[test]
    fn opening_plain_folder_returns_structured_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let input = format!(
            "{}{}",
            request("request-1", "handshake", json!({"client_version": "0.1.0"})),
            request(
                "request-2",
                "catalog.open",
                json!({"path": directory.path().to_string_lossy()})
            )
        );

        let responses = run_lines(input);

        assert_eq!(responses[1]["error"]["code"], "not_a_catalog");
    }

    fn request(id: &str, method: &str, params: Value) -> String {
        format!(
            "{}\n",
            json!({
                "id": id,
                "protocol_version": PROTOCOL_VERSION,
                "method": method,
                "params": params
            })
        )
    }

    fn run(input: impl AsRef<[u8]>) -> Value {
        run_lines(input).remove(0)
    }

    fn run_lines(input: impl AsRef<[u8]>) -> Vec<Value> {
        let mut output = Vec::new();
        run_engine(Cursor::new(input), &mut output).expect("engine should run");
        String::from_utf8(output)
            .expect("output should be UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("line should be valid JSON"))
            .collect()
    }
}
