//! Versioned, line-delimited JSON messages exchanged with the engine process.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
pub const HANDSHAKE_METHOD: &str = "handshake";

#[derive(Debug, Deserialize, PartialEq)]
pub struct Request {
    pub id: String,
    pub protocol_version: u32,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Response {
    pub id: Option<String>,
    pub protocol_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl Response {
    #[must_use]
    pub fn success(id: String, result: Value) -> Self {
        Self {
            id: Some(id),
            protocol_version: PROTOCOL_VERSION,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn error(id: Option<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id,
            protocol_version: PROTOCOL_VERSION,
            result: None,
            error: Some(ProtocolError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Handshake {
    pub engine_version: String,
    pub protocol_version: u32,
    pub compatible_client_version: String,
    pub capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{Response, PROTOCOL_VERSION};
    use serde_json::json;

    #[test]
    fn success_response_omits_error() {
        let response = Response::success("request-1".to_owned(), json!({"ready": true}));

        assert_eq!(
            serde_json::to_value(response).expect("response should serialize"),
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION,
                "result": {"ready": true}
            })
        );
    }

    #[test]
    fn error_response_omits_result() {
        let response = Response::error(
            Some("request-2".to_owned()),
            "unsupported_method",
            "method is not implemented",
        );

        assert_eq!(
            serde_json::to_value(response).expect("response should serialize"),
            json!({
                "id": "request-2",
                "protocol_version": PROTOCOL_VERSION,
                "error": {
                    "code": "unsupported_method",
                    "message": "method is not implemented"
                }
            })
        );
    }
}
