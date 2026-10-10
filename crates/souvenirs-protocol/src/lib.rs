//! Versioned, newline-delimited JSON messages exchanged with the engine process.

use schemars::{schema::RootSchema, schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
// Advance only when the oldest compatible macOS client changes.
pub const COMPATIBLE_CLIENT_VERSION: &str = "0.1.0";
pub const HANDSHAKE_METHOD: &str = "handshake";
pub const START_OPERATION_METHOD: &str = "operation.start";
pub const CANCEL_OPERATION_METHOD: &str = "operation.cancel";
pub const CATALOG_VALIDATE_METHOD: &str = "catalog.validate";
pub const CATALOG_CREATE_METHOD: &str = "catalog.create";
pub const CATALOG_OPEN_METHOD: &str = "catalog.open";

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Request {
    pub id: String,
    pub protocol_version: u32,
    #[serde(flatten)]
    pub payload: RequestPayload,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "method", content = "params")]
pub enum RequestPayload {
    #[serde(rename = "handshake")]
    Handshake(HandshakeParameters),
    #[serde(rename = "operation.start")]
    StartOperation(StartOperationParameters),
    #[serde(rename = "operation.cancel")]
    CancelOperation(CancelOperationParameters),
    #[serde(rename = "catalog.validate")]
    ValidateCatalog(CatalogParameters),
    #[serde(rename = "catalog.create")]
    CreateCatalog(CreateCatalogParameters),
    #[serde(rename = "catalog.open")]
    OpenCatalog(CatalogPathParameters),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeParameters {
    pub client_version: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOperationParameters {
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartOperationParameters {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogParameters {
    pub path: String,
    pub intent: CatalogIntent,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogIntent {
    Create,
    Open,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPathParameters {
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCatalogParameters {
    pub path: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EngineMessage {
    Response {
        #[serde(flatten)]
        response: Response,
    },
    Event {
        #[serde(flatten)]
        event: OperationEvent,
    },
}

impl EngineMessage {
    #[must_use]
    pub fn success(id: String, result: ResponseResult) -> Self {
        Self::Response {
            response: Response {
                id: Some(id),
                protocol_version: PROTOCOL_VERSION,
                payload: ResponsePayload::Success { result },
            },
        }
    }

    #[must_use]
    pub fn error(id: Option<String>, error: ProtocolError) -> Self {
        Self::Response {
            response: Response {
                id,
                protocol_version: PROTOCOL_VERSION,
                payload: ResponsePayload::Error { error },
            },
        }
    }

    #[must_use]
    pub fn event(event: OperationEvent) -> Self {
        Self::Event { event }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Response {
    pub id: Option<String>,
    pub protocol_version: u32,
    #[serde(flatten)]
    pub payload: ResponsePayload,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ResponsePayload {
    Success { result: ResponseResult },
    Error { error: ProtocolError },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ResponseResult {
    Handshake(HandshakeResult),
    OperationAccepted(OperationAccepted),
    CancellationAccepted(CancellationAccepted),
    Catalog(CatalogResult),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct HandshakeResult {
    pub engine_version: String,
    pub protocol_version: u32,
    pub compatible_client_version: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct OperationAccepted {
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CancellationAccepted {
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CatalogResult {
    pub path: String,
    pub volume_name: String,
    pub display_name: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ProtocolError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct OperationEvent {
    pub protocol_version: u32,
    pub operation_id: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub payload: OperationEventPayload,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum OperationEventPayload {
    Started {
        name: String,
    },
    Progress {
        completed_units: u64,
        total_units: Option<u64>,
        message: Option<String>,
    },
    Completed {
        result: Value,
    },
    Failed {
        error: ProtocolError,
    },
    Cancelled,
}

#[must_use]
pub fn protocol_schema() -> RootSchema {
    schema_for!(WireMessage)
}

#[derive(JsonSchema)]
#[allow(dead_code)]
#[schemars(untagged)]
enum WireMessage {
    Request(Request),
    EngineMessage(EngineMessage),
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use jsonschema::JSONSchema;
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use serde_json::{json, Value};

    use super::{
        protocol_schema, EngineMessage, HandshakeResult, OperationEvent, OperationEventPayload,
        ProtocolError, Request, RequestPayload, ResponseResult, PROTOCOL_VERSION,
    };

    #[test]
    fn success_response_has_a_typed_result() {
        let response = EngineMessage::success(
            "request-1".to_owned(),
            ResponseResult::Handshake(HandshakeResult {
                engine_version: "0.1.0".to_owned(),
                protocol_version: PROTOCOL_VERSION,
                compatible_client_version: "0.1.0".to_owned(),
                capabilities: vec!["operation.cancel".to_owned()],
            }),
        );

        assert_eq!(
            serde_json::to_value(response).expect("response should serialize"),
            json!({
                "kind": "response",
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION,
                "status": "success",
                "result": {
                    "type": "handshake",
                    "engine_version": "0.1.0",
                    "protocol_version": PROTOCOL_VERSION,
                    "compatible_client_version": "0.1.0",
                    "capabilities": ["operation.cancel"]
                }
            })
        );
    }

    #[test]
    fn structured_error_can_include_machine_readable_data() {
        let response = EngineMessage::error(
            Some("request-2".to_owned()),
            ProtocolError::new("incompatible_protocol", "unsupported protocol version")
                .with_data(json!({"expected": 1, "actual": 2})),
        );

        assert_eq!(
            serde_json::to_value(response).expect("response should serialize"),
            json!({
                "kind": "response",
                "id": "request-2",
                "protocol_version": PROTOCOL_VERSION,
                "status": "error",
                "error": {
                    "code": "incompatible_protocol",
                    "message": "unsupported protocol version",
                    "data": {"expected": 1, "actual": 2}
                }
            })
        );
    }

    #[test]
    fn ordered_event_round_trips() {
        let message = EngineMessage::event(OperationEvent {
            protocol_version: PROTOCOL_VERSION,
            operation_id: "operation-1".to_owned(),
            sequence: 2,
            payload: OperationEventPayload::Progress {
                completed_units: 3,
                total_units: Some(10),
                message: Some("Scanning".to_owned()),
            },
        });

        let encoded = serde_json::to_string(&message).expect("event should serialize");
        let decoded: EngineMessage =
            serde_json::from_str(&encoded).expect("event should deserialize");
        assert_eq!(decoded, message);
    }

    #[test]
    fn unknown_parameter_fields_are_rejected() {
        let error = serde_json::from_value::<Request>(json!({
            "id": "request-1",
            "protocol_version": PROTOCOL_VERSION,
            "method": "handshake",
            "params": {
                "client_version": "0.1.0",
                "unexpected": true
            }
        }))
        .expect_err("unknown fields must be rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn golden_fixture_matches_rust_types() {
        let fixture = fixture_path("protocol-v1.jsonl");
        let contents = fs::read_to_string(fixture).expect("fixture should be readable");
        let values: Vec<Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).expect("fixture line should be valid JSON"))
            .collect();

        let mut request_count = 0;
        let mut engine_message_count = 0;
        for value in &values {
            if value.get("kind").is_some() {
                decode_canonically::<EngineMessage>(value);
                engine_message_count += 1;
            } else {
                let request = decode_canonically::<Request>(value);
                assert!(matches!(
                    request.payload,
                    RequestPayload::Handshake(_)
                        | RequestPayload::StartOperation(_)
                        | RequestPayload::CancelOperation(_)
                        | RequestPayload::ValidateCatalog(_)
                        | RequestPayload::CreateCatalog(_)
                        | RequestPayload::OpenCatalog(_)
                ));
                request_count += 1;
            }
        }
        assert_eq!(request_count, 3);
        assert_eq!(engine_message_count, 9);

        let schema_value =
            serde_json::to_value(protocol_schema()).expect("schema should serialize");
        let validator = JSONSchema::compile(&schema_value).expect("schema should compile");
        for value in &values {
            if let Err(errors) = validator.validate(value) {
                let errors = errors.map(|error| error.to_string()).collect::<Vec<_>>();
                panic!("fixture value does not match the committed schema: {errors:?}");
            }
        }
    }

    #[test]
    fn committed_schema_matches_generated_schema() {
        let expected = serde_json::to_string_pretty(&protocol_schema())
            .expect("protocol schema should serialize")
            + "\n";
        let actual = fs::read_to_string(fixture_path("../schema/v1.json"))
            .expect("committed schema should be readable");

        assert_eq!(actual, expected, "regenerate the committed protocol schema");
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    fn decode_canonically<T>(value: &Value) -> T
    where
        T: DeserializeOwned + Serialize,
    {
        let decoded: T =
            serde_json::from_value(value.clone()).expect("fixture value should decode");
        assert_eq!(
            serde_json::to_value(&decoded).expect("fixture value should re-encode"),
            *value,
            "fixture value should use the canonical wire representation"
        );
        decoded
    }
}
