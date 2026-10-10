//! Line-delimited JSON process lifecycle for the supervised engine.

use std::io::{self, BufRead, Write};

use serde_json::json;
use souvenirs_protocol::{Handshake, Request, Response, HANDSHAKE_METHOD, PROTOCOL_VERSION};

pub fn run_engine(mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut line = Vec::new();

    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        trim_line_ending(&mut line);
        let response = handle_line(&line);
        serde_json::to_writer(&mut output, &response).map_err(io::Error::other)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}

fn trim_line_ending(line: &mut Vec<u8>) {
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
}

fn handle_line(line: &[u8]) -> Response {
    let request = match serde_json::from_slice::<Request>(line) {
        Ok(request) => request,
        Err(error) => {
            return Response::error(None, "invalid_request", error.to_string());
        }
    };

    if request.protocol_version != PROTOCOL_VERSION {
        return Response::error(
            Some(request.id),
            "incompatible_protocol",
            format!(
                "protocol version {} is unsupported; expected {PROTOCOL_VERSION}",
                request.protocol_version
            ),
        );
    }

    if request.method != HANDSHAKE_METHOD {
        return Response::error(
            Some(request.id),
            "unsupported_method",
            format!("method {:?} is not implemented", request.method),
        );
    }

    let handshake = Handshake {
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        compatible_client_version: PROTOCOL_VERSION.to_string(),
        capabilities: Vec::new(),
    };
    let result = serde_json::to_value(handshake).expect("handshake should serialize");

    Response::success(request.id, json!(result))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::{json, Value};
    use souvenirs_protocol::PROTOCOL_VERSION;

    use super::run_engine;

    #[test]
    fn handshake_returns_process_contract() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION,
                "method": "handshake",
                "params": {}
            })
        );
        let mut output = Vec::new();

        run_engine(Cursor::new(input), &mut output).expect("engine should run");

        let response: Value =
            serde_json::from_slice(&output).expect("engine should write valid JSON");
        assert_eq!(response["id"], "request-1");
        assert_eq!(response["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(
            response["result"],
            json!({
                "engine_version": env!("CARGO_PKG_VERSION"),
                "protocol_version": PROTOCOL_VERSION,
                "compatible_client_version": PROTOCOL_VERSION.to_string(),
                "capabilities": []
            })
        );
    }

    #[test]
    fn engine_processes_requests_until_end_of_input() {
        let input = [
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION,
                "method": "unknown"
            })
            .to_string(),
            "{not-json}".to_owned(),
        ]
        .join("\n");
        let mut output = Vec::new();

        run_engine(Cursor::new(input), &mut output).expect("engine should run");

        let lines = String::from_utf8(output).expect("output should be UTF-8");
        let responses: Vec<Value> = lines
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line should be valid JSON"))
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], "unsupported_method");
        assert_eq!(responses[1]["error"]["code"], "invalid_request");
    }

    #[test]
    fn incompatible_protocol_is_rejected() {
        let input = format!(
            "{}\n",
            json!({
                "id": "request-1",
                "protocol_version": PROTOCOL_VERSION + 1,
                "method": "handshake"
            })
        );
        let mut output = Vec::new();

        run_engine(Cursor::new(input), &mut output).expect("engine should run");

        let response: Value =
            serde_json::from_slice(&output).expect("engine should write valid JSON");
        assert_eq!(response["id"], "request-1");
        assert_eq!(response["error"]["code"], "incompatible_protocol");
    }

    #[test]
    fn invalid_utf8_is_rejected_without_stopping_the_engine() {
        let mut input = b"\xff\n".to_vec();
        input.extend_from_slice(
            format!(
                "{}\n",
                json!({
                    "id": "request-2",
                    "protocol_version": PROTOCOL_VERSION,
                    "method": "handshake"
                })
            )
            .as_bytes(),
        );
        let mut output = Vec::new();

        run_engine(Cursor::new(input), &mut output).expect("engine should keep running");

        let lines = String::from_utf8(output).expect("output should be UTF-8");
        let responses: Vec<Value> = lines
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line should be valid JSON"))
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], "invalid_request");
        assert!(responses[1]["result"].is_object());
    }
}
