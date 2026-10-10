use std::env;
use std::io::{self, BufRead, Write};

use serde_json::{json, Value};
use souvenirs_protocol::{
    CancellationAccepted, CatalogResult, EngineMessage, HandshakeResult, ImportDateSource,
    ImportDateSourceSummary, ImportDateState, ImportReviewResult, ImportScanResult,
    OperationAccepted, OperationEvent, OperationEventPayload, OperationInspectionResult,
    ProtocolError, Request, RequestPayload, ResponseResult, UnfinishedOperationResult,
    CANCEL_OPERATION_METHOD, CATALOG_CREATE_METHOD, CATALOG_OPEN_METHOD, CATALOG_VALIDATE_METHOD,
    COMPATIBLE_CLIENT_VERSION, IMPORT_REVIEW_METHOD, IMPORT_SCAN_METHOD, OPERATION_INSPECT_METHOD,
    PROTOCOL_VERSION, START_OPERATION_METHOD,
};

fn main() {
    let scenario = match parse_scenario() {
        Ok(scenario) => scenario,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    if let Err(error) = run(scenario, io::stdin().lock(), io::stdout().lock()) {
        eprintln!("souvenirs-fake-engine: {error}");
        std::process::exit(1);
    }
}

#[derive(Clone, Copy)]
enum Scenario {
    Ready,
    SuccessfulOperation,
    FailedOperation,
    CancellationRace,
    UnexpectedTermination,
    MalformedResponse,
    CatalogFlow,
    CatalogError,
    ImportFlow,
    ImportError,
    ImportExecution,
    ImportSourceUnavailable,
    ImportRecovery,
    ImportRecoveryMissing,
}

fn parse_scenario() -> Result<Scenario, String> {
    let mut arguments = env::args().skip(1);
    let Some(flag) = arguments.next() else {
        return Err(usage());
    };
    if flag == "--help" {
        println!("{}", usage());
        std::process::exit(0);
    }
    if flag != "--scenario" {
        return Err(usage());
    }
    let scenario = match arguments.next().as_deref() {
        Some("ready") => Scenario::Ready,
        Some("successful-operation") => Scenario::SuccessfulOperation,
        Some("failed-operation") => Scenario::FailedOperation,
        Some("cancellation-race") => Scenario::CancellationRace,
        Some("unexpected-termination") => Scenario::UnexpectedTermination,
        Some("malformed-response") => Scenario::MalformedResponse,
        Some("catalog-flow") => Scenario::CatalogFlow,
        Some("catalog-error") => Scenario::CatalogError,
        Some("import-flow") => Scenario::ImportFlow,
        Some("import-error") => Scenario::ImportError,
        Some("import-execution") => Scenario::ImportExecution,
        Some("import-source-unavailable") => Scenario::ImportSourceUnavailable,
        Some("import-recovery") => Scenario::ImportRecovery,
        Some("import-recovery-missing") => Scenario::ImportRecoveryMissing,
        _ => return Err(usage()),
    };
    if arguments.next().is_some() {
        return Err(usage());
    }
    Ok(scenario)
}

fn usage() -> String {
    [
        "Usage: souvenirs-fake-engine --scenario <name>",
        "",
        "Scenarios:",
        "  ready",
        "  successful-operation",
        "  failed-operation",
        "  cancellation-race",
        "  unexpected-termination",
        "  malformed-response",
        "  catalog-flow",
        "  catalog-error",
        "  import-flow",
        "  import-error",
        "  import-execution",
        "  import-source-unavailable",
        "  import-recovery",
        "  import-recovery-missing",
    ]
    .join("\n")
}

fn run(scenario: Scenario, mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let Some(handshake) = next_request(&mut input, &mut output)? else {
        return Ok(());
    };
    let parameters = match handshake.payload {
        RequestPayload::Handshake(parameters) => parameters,
        _ => {
            return write_protocol_error(
                &mut output,
                handshake.id,
                "handshake_required",
                "The first request must be a handshake.",
            );
        }
    };
    if parameters.client_version.trim().is_empty() {
        return write_protocol_error(
            &mut output,
            handshake.id,
            "invalid_client_version",
            "Client version must not be empty.",
        );
    }
    write_message(
        &mut output,
        &EngineMessage::success(
            handshake.id,
            ResponseResult::Handshake(HandshakeResult {
                engine_version: env!("CARGO_PKG_VERSION").to_owned(),
                protocol_version: PROTOCOL_VERSION,
                compatible_client_version: COMPATIBLE_CLIENT_VERSION.to_owned(),
                capabilities: vec![
                    START_OPERATION_METHOD.to_owned(),
                    CANCEL_OPERATION_METHOD.to_owned(),
                    CATALOG_VALIDATE_METHOD.to_owned(),
                    CATALOG_CREATE_METHOD.to_owned(),
                    CATALOG_OPEN_METHOD.to_owned(),
                    IMPORT_SCAN_METHOD.to_owned(),
                    IMPORT_REVIEW_METHOD.to_owned(),
                    OPERATION_INSPECT_METHOD.to_owned(),
                ],
            }),
        ),
    )?;

    match scenario {
        Scenario::Ready => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                reject_request(&mut output, request)?;
            }
        }
        Scenario::SuccessfulOperation => {
            let Some(request) = expect_start(&mut input, &mut output)? else {
                return Ok(());
            };
            accept_operation(&mut output, request.id)?;
            write_event(
                &mut output,
                0,
                OperationEventPayload::Started {
                    name: "import.scan".to_owned(),
                },
            )?;
            write_event(
                &mut output,
                1,
                OperationEventPayload::Progress {
                    completed_units: 1,
                    total_units: Some(2),
                    message: Some("Scanning".to_owned()),
                },
            )?;
            write_event(
                &mut output,
                2,
                OperationEventPayload::Completed {
                    result: json!({"item_count": 1}),
                },
            )?;
        }
        Scenario::FailedOperation => {
            let Some(request) = expect_start(&mut input, &mut output)? else {
                return Ok(());
            };
            accept_operation(&mut output, request.id)?;
            write_event(
                &mut output,
                0,
                OperationEventPayload::Failed {
                    error: ProtocolError::new("source_unavailable", "The source is unavailable."),
                },
            )?;
        }
        Scenario::CancellationRace => {
            let Some(request) = expect_start(&mut input, &mut output)? else {
                return Ok(());
            };
            accept_operation(&mut output, request.id)?;
            write_event(
                &mut output,
                0,
                OperationEventPayload::Started {
                    name: "import.scan".to_owned(),
                },
            )?;
            let Some(cancellation) = next_request(&mut input, &mut output)? else {
                return Ok(());
            };
            match cancellation.payload {
                RequestPayload::CancelOperation(parameters)
                    if parameters.operation_id == "operation-1" =>
                {
                    write_event(
                        &mut output,
                        1,
                        OperationEventPayload::Completed {
                            result: json!({"item_count": 1}),
                        },
                    )?;
                    write_message(
                        &mut output,
                        &EngineMessage::success(
                            cancellation.id,
                            ResponseResult::CancellationAccepted(CancellationAccepted {
                                operation_id: "operation-1".to_owned(),
                            }),
                        ),
                    )?;
                }
                _ => {
                    reject_request(&mut output, cancellation)?;
                }
            }
        }
        Scenario::UnexpectedTermination => {
            let Some(_) = expect_start(&mut input, &mut output)? else {
                return Ok(());
            };
            std::process::exit(42);
        }
        Scenario::MalformedResponse => {
            let Some(_) = expect_start(&mut input, &mut output)? else {
                return Ok(());
            };
            output.write_all(b"{not-json}\n")?;
            output.flush()?;
        }
        Scenario::CatalogFlow => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                match request.payload {
                    RequestPayload::ValidateCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::CreateCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::OpenCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::InspectOperation(_) => {
                        operation_inspection(&mut output, request.id, None)?;
                    }
                    _ => reject_request(&mut output, request)?,
                }
            }
        }
        Scenario::CatalogError => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                match request.payload {
                    RequestPayload::ValidateCatalog(_) => write_protocol_error(
                        &mut output,
                        request.id,
                        "synchronized_folder",
                        "This synchronized folder cannot hold a catalog because it may rename files on its own.",
                    )?,
                    _ => reject_request(&mut output, request)?,
                }
            }
        }
        Scenario::ImportFlow => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                match request.payload {
                    RequestPayload::OpenCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::InspectOperation(_) => {
                        operation_inspection(&mut output, request.id, None)?;
                    }
                    RequestPayload::ScanImport(parameters) => {
                        import_scan_result(&mut output, request.id, parameters.source_path)?;
                    }
                    RequestPayload::ReviewImport(parameters) => {
                        let scan = fake_scan(parameters.source_path);
                        let can_start = parameters.date_resolution.is_some();
                        write_message(
                            &mut output,
                            &EngineMessage::success(
                                request.id,
                                ResponseResult::ImportReview(Box::new(ImportReviewResult {
                                    scan,
                                    collection_name: parameters.collection_name.clone(),
                                    destination_mode: parameters.destination_mode,
                                    destination_path: format!(
                                        "{}/2026/{} {}",
                                        parameters.catalog_path,
                                        parameters.accepted_date,
                                        parameters.collection_name
                                    ),
                                    accepted_date: parameters.accepted_date,
                                    date_resolution: parameters.date_resolution,
                                    required_decision_count: usize::from(!can_start),
                                    transfer_count: 12,
                                    skipped_count: 3,
                                    left_on_source_count: 2,
                                    operation_count: 14,
                                    required_space_bytes: 41_000_000_000,
                                    can_start,
                                })),
                            ),
                        )?;
                    }
                    _ => reject_request(&mut output, request)?,
                }
            }
        }
        Scenario::ImportError => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                match request.payload {
                    RequestPayload::OpenCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::InspectOperation(_) => {
                        operation_inspection(&mut output, request.id, None)?;
                    }
                    RequestPayload::ScanImport(_) => write_protocol_error(
                        &mut output,
                        request.id,
                        "source_unavailable",
                        "The source is no longer available. Reconnect it and try again.",
                    )?,
                    _ => reject_request(&mut output, request)?,
                }
            }
        }
        Scenario::ImportExecution | Scenario::ImportSourceUnavailable => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                match request.payload {
                    RequestPayload::OpenCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::InspectOperation(_) => {
                        operation_inspection(&mut output, request.id, None)?;
                    }
                    RequestPayload::ScanImport(parameters) => {
                        import_scan_result(&mut output, request.id, parameters.source_path)?;
                    }
                    RequestPayload::ReviewImport(parameters) => {
                        import_review(&mut output, request.id, parameters)?;
                    }
                    RequestPayload::StartOperation(
                        souvenirs_protocol::StartOperationParameters::ImportExecute { .. },
                    ) => {
                        accept_operation(&mut output, request.id)?;
                        write_event(
                            &mut output,
                            0,
                            OperationEventPayload::Started {
                                name: "import.execute".to_owned(),
                            },
                        )?;
                        write_event(
                            &mut output,
                            1,
                            OperationEventPayload::Progress {
                                completed_units: 7,
                                total_units: Some(12),
                                message: Some("Reading back and checking each copy…".to_owned()),
                            },
                        )?;
                        let terminal = if matches!(scenario, Scenario::ImportSourceUnavailable) {
                            OperationEventPayload::Failed {
                                error: ProtocolError::new(
                                    "source_unavailable",
                                    "The source was disconnected.",
                                ),
                            }
                        } else {
                            OperationEventPayload::Completed {
                                result: json!({
                                    "copied_count": 12,
                                    "skipped_count": 3,
                                    "left_on_source_count": 2
                                }),
                            }
                        };
                        write_event(&mut output, 2, terminal)?;
                    }
                    _ => reject_request(&mut output, request)?,
                }
            }
        }
        Scenario::ImportRecovery | Scenario::ImportRecoveryMissing => {
            while let Some(request) = next_request(&mut input, &mut output)? {
                match request.payload {
                    RequestPayload::OpenCatalog(parameters) => {
                        catalog_result(&mut output, request.id, parameters.path)?;
                    }
                    RequestPayload::InspectOperation(_) => operation_inspection(
                        &mut output,
                        request.id,
                        Some(UnfinishedOperationResult {
                            operation_id: "operation-1".to_owned(),
                            collection_name: "Dolomites".to_owned(),
                            source_name: "EOS_DIGITAL".to_owned(),
                            copied_count: 7,
                            total_count: 12,
                            source_available: matches!(scenario, Scenario::ImportRecovery),
                        }),
                    )?,
                    RequestPayload::StartOperation(
                        souvenirs_protocol::StartOperationParameters::Recover { .. },
                    ) => {
                        accept_operation(&mut output, request.id)?;
                        write_event(
                            &mut output,
                            0,
                            OperationEventPayload::Started {
                                name: "operation.recover".to_owned(),
                            },
                        )?;
                        write_event(
                            &mut output,
                            1,
                            OperationEventPayload::Progress {
                                completed_units: 10,
                                total_units: Some(12),
                                message: Some("Finishing verified copies…".to_owned()),
                            },
                        )?;
                        write_event(
                            &mut output,
                            2,
                            OperationEventPayload::Completed {
                                result: json!({
                                    "copied_count": 12,
                                    "skipped_count": 3,
                                    "left_on_source_count": 2
                                }),
                            },
                        )?;
                    }
                    RequestPayload::CancelOperation(_) => {
                        write_message(
                            &mut output,
                            &EngineMessage::success(
                                request.id,
                                ResponseResult::CancellationAccepted(CancellationAccepted {
                                    operation_id: "operation-1".to_owned(),
                                }),
                            ),
                        )?;
                    }
                    _ => reject_request(&mut output, request)?,
                }
            }
        }
    }

    Ok(())
}

fn expect_start(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<Option<Request>> {
    while let Some(request) = next_request(input, output)? {
        if matches!(request.payload, RequestPayload::StartOperation(_)) {
            return Ok(Some(request));
        }
        reject_request(output, request)?;
    }
    Ok(None)
}

fn accept_operation(output: &mut impl Write, request_id: String) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::success(
            request_id,
            ResponseResult::OperationAccepted(OperationAccepted {
                operation_id: "operation-1".to_owned(),
            }),
        ),
    )
}

fn write_event(
    output: &mut impl Write,
    sequence: u64,
    payload: OperationEventPayload,
) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::event(OperationEvent {
            protocol_version: PROTOCOL_VERSION,
            operation_id: "operation-1".to_owned(),
            sequence,
            payload,
        }),
    )
}

fn reject_request(output: &mut impl Write, request: Request) -> io::Result<()> {
    match request.payload {
        RequestPayload::Handshake(_) => write_protocol_error(
            output,
            request.id,
            "handshake_already_completed",
            "The connection handshake has already completed.",
        ),
        RequestPayload::StartOperation(parameters) => write_protocol_error(
            output,
            request.id,
            "unsupported_operation",
            &format!(
                "Operation {:?} is not supported in this scenario.",
                match parameters {
                    souvenirs_protocol::StartOperationParameters::ImportExecute { .. } =>
                        "import.execute",
                    souvenirs_protocol::StartOperationParameters::Recover { .. } =>
                        "operation.recover",
                }
            ),
        ),
        RequestPayload::CancelOperation(parameters) => write_protocol_error(
            output,
            request.id,
            "operation_not_found",
            &format!("Operation {:?} was not found.", parameters.operation_id),
        ),
        RequestPayload::ValidateCatalog(_)
        | RequestPayload::CreateCatalog(_)
        | RequestPayload::OpenCatalog(_)
        | RequestPayload::ScanImport(_)
        | RequestPayload::ReviewImport(_)
        | RequestPayload::InspectOperation(_) => write_protocol_error(
            output,
            request.id,
            "unsupported_catalog_request",
            "Catalog requests are not supported in this scenario.",
        ),
    }
}

fn import_review(
    output: &mut impl Write,
    request_id: String,
    parameters: souvenirs_protocol::ImportReviewParameters,
) -> io::Result<()> {
    let scan = fake_scan(parameters.source_path);
    let can_start = parameters.date_resolution.is_some();
    write_message(
        output,
        &EngineMessage::success(
            request_id,
            ResponseResult::ImportReview(Box::new(ImportReviewResult {
                scan,
                collection_name: parameters.collection_name.clone(),
                destination_mode: parameters.destination_mode,
                destination_path: format!(
                    "{}/2026/{} {}",
                    parameters.catalog_path, parameters.accepted_date, parameters.collection_name
                ),
                accepted_date: parameters.accepted_date,
                date_resolution: parameters.date_resolution,
                required_decision_count: usize::from(!can_start),
                transfer_count: 12,
                skipped_count: 3,
                left_on_source_count: 2,
                operation_count: 14,
                required_space_bytes: 41_000_000_000,
                can_start,
            })),
        ),
    )
}

fn operation_inspection(
    output: &mut impl Write,
    request_id: String,
    operation: Option<UnfinishedOperationResult>,
) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::success(
            request_id,
            ResponseResult::OperationInspection(OperationInspectionResult { operation }),
        ),
    )
}

fn fake_scan(source_path: String) -> ImportScanResult {
    ImportScanResult {
        source_path,
        source_name: "EOS_DIGITAL".to_owned(),
        file_count: 17,
        media_count: 12,
        photo_count: 8,
        video_count: 4,
        unsupported_count: 2,
        uncertain_time_count: 4,
        total_bytes: 41_000_000_000,
        suggested_collection_name: "Dolomites".to_owned(),
        suggested_accepted_date: Some("2026-10-10".to_owned()),
        suggested_range_end: Some("2026-10-12".to_owned()),
        pending_item_count: 4,
        unresolved_item_count: 0,
        datetime_sources: vec![
            ImportDateSourceSummary {
                source: ImportDateSource::ExifDateTimeOriginal,
                state: ImportDateState::Accepted,
                item_count: 8,
            },
            ImportDateSourceSummary {
                source: ImportDateSource::ContainerCreationTime,
                state: ImportDateState::NeedsConfirmation,
                item_count: 4,
            },
        ],
    }
}

fn import_scan_result(
    output: &mut impl Write,
    request_id: String,
    source_path: String,
) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::success(
            request_id,
            ResponseResult::ImportScan(Box::new(fake_scan(source_path))),
        ),
    )
}

fn catalog_result(output: &mut impl Write, request_id: String, path: String) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::success(
            request_id,
            ResponseResult::Catalog(CatalogResult {
                path,
                volume_name: "photos-nas.local".to_owned(),
                display_name: Some("Family Catalog".to_owned()),
                detail: "Catalog format 2".to_owned(),
            }),
        ),
    )
}

fn write_protocol_error(
    output: &mut impl Write,
    request_id: String,
    code: &str,
    message: &str,
) -> io::Result<()> {
    write_message(
        output,
        &EngineMessage::error(Some(request_id), ProtocolError::new(code, message)),
    )
}

fn next_request(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<Option<Request>> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Ok(None);
        }
        trim_line_ending(&mut line);
        match decode_request(&line) {
            Ok(request) => return Ok(Some(request)),
            Err((request_id, error)) => {
                write_message(output, &EngineMessage::error(request_id, error))?;
            }
        }
    }
}

fn decode_request(line: &[u8]) -> Result<Request, (Option<String>, ProtocolError)> {
    let value: Value = serde_json::from_slice(line).map_err(|error| {
        (
            None,
            ProtocolError::new("invalid_request", "Request is not valid JSON.")
                .with_data(json!({"detail": error.to_string()})),
        )
    })?;
    let request_id = value.get("id").and_then(Value::as_str).map(str::to_owned);
    let protocol_version = value
        .get("protocol_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            (
                request_id.clone(),
                ProtocolError::new(
                    "invalid_request",
                    "Request protocol_version must be an unsigned 32-bit integer.",
                ),
            )
        })?;
    if protocol_version != PROTOCOL_VERSION {
        return Err((
            request_id,
            ProtocolError::new(
                "incompatible_protocol",
                format!(
                    "Protocol version {protocol_version} is unsupported; expected {PROTOCOL_VERSION}."
                ),
            )
            .with_data(json!({
                "expected": PROTOCOL_VERSION,
                "actual": protocol_version
            })),
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        (
            request_id,
            ProtocolError::new(
                "invalid_request",
                "Request does not match protocol version 1.",
            )
            .with_data(json!({"detail": error.to_string()})),
        )
    })
}

fn trim_line_ending(line: &mut Vec<u8>) {
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
}

fn write_message(output: &mut impl Write, message: &EngineMessage) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}
