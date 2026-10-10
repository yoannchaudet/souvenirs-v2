//! Newline-delimited JSON process lifecycle for the supervised engine.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};
use souvenirs_application::{CatalogIntent as ApplicationIntent, CatalogService};
use souvenirs_protocol::{
    CatalogIntent, CatalogResult, EngineMessage, HandshakeResult, ProtocolError, Request,
    RequestPayload, ResponseResult, CANCEL_OPERATION_METHOD, CATALOG_CREATE_METHOD,
    CATALOG_OPEN_METHOD, CATALOG_VALIDATE_METHOD, COMPATIBLE_CLIENT_VERSION, PROTOCOL_VERSION,
};

pub fn run_engine(mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut connection = Connection::default();
    let mut line = Vec::new();

    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        trim_line_ending(&mut line);
        let response = connection.handle_line(&line);
        serde_json::to_writer(&mut output, &response).map_err(io::Error::other)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}

#[derive(Default)]
struct Connection {
    handshake_complete: bool,
}

impl Connection {
    fn handle_line(&mut self, line: &[u8]) -> EngineMessage {
        let (request_id, protocol_version) = match request_envelope(line) {
            Ok(envelope) => envelope,
            Err(error) => return EngineMessage::error(request_id(line), error),
        };
        if protocol_version != PROTOCOL_VERSION {
            return incompatible_protocol(request_id, protocol_version);
        }

        let request = match serde_json::from_slice::<Request>(line) {
            Ok(request) => request,
            Err(error) => {
                return EngineMessage::error(
                    request_id,
                    ProtocolError::new(
                        "invalid_request",
                        "request does not match protocol version 1",
                    )
                    .with_data(json!({"detail": error.to_string()})),
                );
            }
        };

        if !self.handshake_complete && !matches!(request.payload, RequestPayload::Handshake(_)) {
            return EngineMessage::error(
                Some(request.id),
                ProtocolError::new(
                    "handshake_required",
                    "the first request must be a handshake",
                ),
            );
        }

        match request.payload {
            RequestPayload::Handshake(parameters) => {
                if self.handshake_complete {
                    return EngineMessage::error(
                        Some(request.id),
                        ProtocolError::new(
                            "handshake_already_completed",
                            "the connection handshake has already completed",
                        ),
                    );
                }
                if parameters.client_version.trim().is_empty() {
                    return EngineMessage::error(
                        Some(request.id),
                        ProtocolError::new(
                            "invalid_client_version",
                            "client version must not be empty",
                        ),
                    );
                }
                self.handshake_complete = true;
                EngineMessage::success(
                    request.id,
                    ResponseResult::Handshake(HandshakeResult {
                        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
                        protocol_version: PROTOCOL_VERSION,
                        compatible_client_version: COMPATIBLE_CLIENT_VERSION.to_owned(),
                        capabilities: vec![
                            CANCEL_OPERATION_METHOD.to_owned(),
                            CATALOG_VALIDATE_METHOD.to_owned(),
                            CATALOG_CREATE_METHOD.to_owned(),
                            CATALOG_OPEN_METHOD.to_owned(),
                        ],
                    }),
                )
            }
            RequestPayload::StartOperation(parameters) => EngineMessage::error(
                Some(request.id),
                ProtocolError::new(
                    "unsupported_operation",
                    format!("operation {:?} is not implemented", parameters.name),
                ),
            ),

            RequestPayload::CancelOperation(parameters) => EngineMessage::error(
                Some(request.id),
                ProtocolError::new(
                    "operation_not_found",
                    format!("operation {:?} was not found", parameters.operation_id),
                )
                .with_data(json!({"operation_id": parameters.operation_id})),
            ),
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
        }
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
    use std::io::Cursor;

    use serde_json::{json, Value};
    use souvenirs_protocol::PROTOCOL_VERSION;

    use super::run_engine;

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
                "compatible_client_version": "0.1.0",
                "capabilities": [
                    "operation.cancel",
                    "catalog.validate",
                    "catalog.create",
                    "catalog.open"
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
    fn incompatible_protocol_is_rejected_with_version_data() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION + 1,
                "method": "handshake",
                "params": {"client_version": "0.1.0"}
            })
        );

        let response = run(input);

        assert_eq!(response["id"], "request-1");
        assert_eq!(response["error"]["code"], "incompatible_protocol");
        assert_eq!(response["error"]["data"]["expected"], PROTOCOL_VERSION);
        assert_eq!(response["error"]["data"]["actual"], PROTOCOL_VERSION + 1);
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
            "request does not match protocol version 1"
        );
    }

    #[test]
    fn empty_client_version_is_rejected() {
        let input = request("request-1", "handshake", json!({"client_version": ""}));

        let response = run(input);

        assert_eq!(response["error"]["code"], "invalid_client_version");
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
    fn unsupported_operation_is_structured() {
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

        assert_eq!(responses[1]["error"]["code"], "unsupported_operation");
        assert_eq!(responses[1]["id"], "request-2");
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
