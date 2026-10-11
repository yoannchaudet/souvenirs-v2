//! Versioned, newline-delimited JSON messages exchanged with the engine process.

use std::fmt;
use std::str::FromStr;

use schemars::{schema::RootSchema, schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 5;
// This is the oldest macOS client version supported by the engine.
pub const MINIMUM_CLIENT_VERSION: &str = "0.2.0";
pub const HANDSHAKE_METHOD: &str = "handshake";
pub const START_OPERATION_METHOD: &str = "operation.start";
pub const CANCEL_OPERATION_METHOD: &str = "operation.cancel";
pub const CATALOG_VALIDATE_METHOD: &str = "catalog.validate";
pub const CATALOG_CREATE_METHOD: &str = "catalog.create";
pub const CATALOG_OPEN_METHOD: &str = "catalog.open";
pub const CATALOG_COLLECTIONS_METHOD: &str = "catalog.collections";
pub const IMPORT_SCAN_METHOD: &str = "import.scan";
pub const IMPORT_REVIEW_METHOD: &str = "import.review";
pub const OPERATION_INSPECT_METHOD: &str = "operation.inspect";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ClientVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl FromStr for ClientVersion {
    type Err = InvalidClientVersion;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut components = value.split('.');
        let major = parse_version_component(components.next())?;
        let minor = parse_version_component(components.next())?;
        let patch = parse_version_component(components.next())?;
        if components.next().is_some() {
            return Err(InvalidClientVersion);
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidClientVersion;

impl fmt::Display for InvalidClientVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "client version must contain three unsigned decimal components without leading zeros",
        )
    }
}

impl std::error::Error for InvalidClientVersion {}

fn parse_version_component(component: Option<&str>) -> Result<u64, InvalidClientVersion> {
    let component = component.ok_or(InvalidClientVersion)?;
    if component.is_empty()
        || (component.len() > 1 && component.starts_with('0'))
        || !component.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(InvalidClientVersion);
    }
    component.parse().map_err(|_| InvalidClientVersion)
}

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
    #[serde(rename = "catalog.collections")]
    ListCatalogCollections(CatalogPathParameters),
    #[serde(rename = "import.scan")]
    ScanImport(ImportScanParameters),
    #[serde(rename = "import.review")]
    ReviewImport(ImportReviewParameters),
    #[serde(rename = "operation.inspect")]
    InspectOperation(CatalogPathParameters),
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
#[serde(deny_unknown_fields, tag = "name")]
pub enum StartOperationParameters {
    #[serde(rename = "import.execute")]
    ImportExecute { reviewed_plan: String },
    #[serde(rename = "operation.recover")]
    Recover {
        catalog_path: String,
        operation_id: String,
    },
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
#[serde(deny_unknown_fields)]
pub struct ImportScanParameters {
    pub source_path: String,
    #[serde(default)]
    pub utc_offset_seconds: i32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportReviewParameters {
    pub catalog_path: String,
    pub source_path: String,
    pub collection_name: String,
    pub destination_mode: ImportDestinationMode,
    pub accepted_date: String,
    pub date_resolution: Option<ImportDateResolution>,
    #[serde(default)]
    pub utc_offset_seconds: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDestinationMode {
    Create,
    Existing,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDateResolution {
    AsShown,
    ConvertFromUtc,
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
    CatalogCollections(CatalogCollectionsResult),
    ImportScan(Box<ImportScanResult>),
    ImportReview(Box<ImportReviewResult>),
    OperationInspection(OperationInspectionResult),
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
pub struct CatalogCollectionsResult {
    pub collections: Vec<CatalogCollection>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CatalogCollection {
    Dated {
        identity: CatalogCollectionIdentity,
        label: String,
        start_date: String,
        end_date: String,
    },
    Ongoing {
        identity: CatalogCollectionIdentity,
        label: String,
        parts: Vec<CatalogCollectionPart>,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CatalogCollectionIdentity {
    Dated { year: String, directory: String },
    Ongoing { key: String },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CatalogCollectionPart {
    pub year: i32,
    pub year_directory: String,
    pub collection_directory: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ImportScanResult {
    pub source_path: String,
    pub source_name: String,
    pub file_count: usize,
    pub media_count: usize,
    pub photo_count: usize,
    pub video_count: usize,
    pub unsupported_count: usize,
    pub uncertain_time_count: usize,
    pub total_bytes: u64,
    pub suggested_collection_name: String,
    pub suggested_accepted_date: Option<String>,
    pub suggested_range_end: Option<String>,
    pub pending_item_count: usize,
    pub unresolved_item_count: usize,
    pub datetime_sources: Vec<ImportDateSourceSummary>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ImportDateSourceSummary {
    pub source: ImportDateSource,
    pub state: ImportDateState,
    pub item_count: usize,
    #[serde(default)]
    pub requires_clock: bool,
    #[serde(default)]
    pub supports_utc_conversion: bool,
    pub as_shown_time: Option<String>,
    pub converted_from_utc_time: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDateSource {
    ExifDateTimeOriginal,
    ExifDateTimeDigitized,
    QuickTimeCreationDate,
    XmpDateCreated,
    XmpExifDateTimeOriginal,
    ContainerCreationTime,
    GoogleTakeoutPhotoTakenTime,
    FilenamePattern,
}
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDateState {
    Accepted,
    NeedsConfirmation,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ImportReviewResult {
    pub scan: ImportScanResult,
    pub collection_name: String,
    pub destination_mode: ImportDestinationMode,
    pub destination_path: String,
    pub accepted_date: String,
    pub date_resolution: Option<ImportDateResolution>,
    pub required_decision_count: usize,
    pub transfer_count: usize,
    pub skipped_count: usize,
    pub left_on_source_count: usize,
    pub operation_count: usize,
    pub preflight: ImportPreflightEvidence,
    pub can_start: bool,
    pub reviewed_plan: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ImportPreflightEvidence {
    pub representative_file_names: Vec<String>,
    pub required_bytes: u64,
    pub available_bytes: Option<u64>,
    pub available_space: ImportPreflightCheck,
    pub write_probe: ImportPreflightCheck,
    pub portable_names: ImportPreflightCheck,
    pub destination_conflicts: ImportPreflightCheck,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportPreflightCheck {
    NotRun,
    Passed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct OperationInspectionResult {
    pub operation: Option<UnfinishedOperationResult>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct UnfinishedOperationResult {
    pub operation_id: String,
    pub collection_name: String,
    pub source_name: String,
    pub copied_count: usize,
    pub total_count: usize,
    pub remaining_count: usize,
    pub source_available: bool,
    pub interruption: UnfinishedOperationInterruption,
    pub activity_time: UnfinishedOperationActivityTime,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnfinishedOperationInterruption {
    SourceUnavailable,
    IncompleteWithoutTerminalRecord,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnfinishedOperationActivityTimeKind {
    InterruptedAt,
    LastRecordedActivity,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct UnfinishedOperationActivityTime {
    pub kind: UnfinishedOperationActivityTimeKind,
    pub unix_seconds: i64,
    pub nanosecond: u32,
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
    Cancelled {
        copied_count: usize,
    },
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
        protocol_schema, ClientVersion, EngineMessage, HandshakeResult, OperationEvent,
        OperationEventPayload, ProtocolError, Request, RequestPayload, ResponseResult,
        PROTOCOL_VERSION,
    };

    #[test]
    fn client_versions_compare_numeric_components() {
        let older: ClientVersion = "0.9.10".parse().expect("version should parse");
        let newer: ClientVersion = "0.10.0".parse().expect("version should parse");

        assert!(newer > older);
        assert_eq!(
            "1.2.3".parse::<ClientVersion>(),
            "1.2.3".parse::<ClientVersion>()
        );
    }

    #[test]
    fn malformed_client_versions_are_rejected() {
        for version in ["", "1", "1.2", "1.2.3.4", "1.02.3", "1.a.3", "-1.2.3"] {
            assert!(
                version.parse::<ClientVersion>().is_err(),
                "{version:?} should be rejected"
            );
        }
    }

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
                .with_data(json!({"expected": 2, "actual": 1})),
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
                    "data": {"expected": 2, "actual": 1}
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
        let fixture = fixture_path("protocol-v5.jsonl");
        let contents = fs::read_to_string(fixture).expect("fixture should be readable");
        let values: Vec<Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).expect("fixture line should be valid JSON"))
            .collect();

        let mut request_count = 0;
        let mut engine_message_count = 0;
        for value in &values {
            assert_eq!(
                value.get("protocol_version"),
                Some(&json!(PROTOCOL_VERSION)),
                "fixture line should use the current protocol version"
            );
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
                        | RequestPayload::ListCatalogCollections(_)
                        | RequestPayload::InspectOperation(_)
                ));
                request_count += 1;
            }
            if value.pointer("/result/type") == Some(&json!("handshake")) {
                assert_eq!(
                    value.pointer("/result/protocol_version"),
                    Some(&json!(PROTOCOL_VERSION)),
                    "handshake result should report the current protocol version"
                );
            }
        }
        assert_eq!(request_count, 6);
        assert_eq!(engine_message_count, 11);

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
        let actual = fs::read_to_string(fixture_path("../schema/v5.json"))
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
