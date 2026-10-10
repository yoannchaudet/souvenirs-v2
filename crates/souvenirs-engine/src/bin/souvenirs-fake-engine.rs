use std::env;
use std::io::{self, BufRead, Write};

use serde_json::{json, Value};
use souvenirs_protocol::{
    CancellationAccepted, CatalogResult, EngineMessage, HandshakeResult, OperationAccepted,
    OperationEvent, OperationEventPayload, ProtocolError, Request, RequestPayload, ResponseResult,
    CANCEL_OPERATION_METHOD, CATALOG_CREATE_METHOD, CATALOG_OPEN_METHOD, CATALOG_VALIDATE_METHOD,
    COMPATIBLE_CLIENT_VERSION, PROTOCOL_VERSION, START_OPERATION_METHOD,
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
                parameters.name
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
        | RequestPayload::OpenCatalog(_) => write_protocol_error(
            output,
            request.id,
            "unsupported_catalog_request",
            "Catalog requests are not supported in this scenario.",
        ),
    }
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
