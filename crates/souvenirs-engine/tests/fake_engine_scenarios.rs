use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use souvenirs_protocol::PROTOCOL_VERSION;

#[test]
fn ready_scenario_rejects_requests_and_exits_cleanly_on_eof() {
    let output = run("ready", &[handshake(), start_request()]);

    assert!(output.status.success());
    let messages = messages(&output);
    assert_eq!(messages[1]["error"]["code"], "unsupported_operation");
}

#[test]
fn successful_operation_emits_ordered_terminal_events() {
    let output = run("successful-operation", &[handshake(), start_request()]);

    assert!(output.status.success());
    let messages = messages(&output);
    assert_eq!(messages[1]["result"]["type"], "operation_accepted");
    assert_eq!(
        messages[2..]
            .iter()
            .map(|message| message["sequence"].as_u64().expect("event sequence"))
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(messages[4]["event"], "completed");
}

#[test]
fn failed_operation_is_deterministic() {
    let output = run("failed-operation", &[handshake(), start_request()]);

    assert!(output.status.success());
    let messages = messages(&output);
    assert_eq!(messages[2]["event"], "failed");
    assert_eq!(messages[2]["error"]["code"], "source_unavailable");
}

#[test]
fn cancellation_race_always_completes_before_acknowledgement() {
    let output = run(
        "cancellation-race",
        &[
            handshake(),
            start_request(),
            request(
                "request-3",
                "operation.cancel",
                json!({"operation_id": "operation-1"}),
            ),
        ],
    );

    assert!(output.status.success());
    let messages = messages(&output);
    assert_eq!(messages[3]["event"], "completed");
    assert_eq!(messages[4]["result"]["type"], "cancellation_accepted");
}

#[test]
fn unexpected_termination_uses_stable_exit_status() {
    let output = run("unexpected-termination", &[handshake(), start_request()]);

    assert_eq!(output.status.code(), Some(42));
    assert_eq!(messages(&output).len(), 1);
}

#[test]
fn malformed_response_scenario_is_reproducible() {
    let output = run("malformed-response", &[handshake(), start_request()]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout
        .lines()
        .last()
        .is_some_and(|line| line == "{not-json}"));
}

#[test]
fn normal_shutdown_before_operation_exits_cleanly() {
    let output = run("successful-operation", &[handshake()]);

    assert!(output.status.success());
    assert_eq!(messages(&output).len(), 1);
}

#[test]
fn unknown_scenario_returns_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_souvenirs-fake-engine"))
        .args(["--scenario", "typo"])
        .output()
        .expect("fake engine should start");

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .expect("stderr should be UTF-8")
        .contains("Usage: souvenirs-fake-engine"));
}

#[test]
fn incompatible_protocol_returns_structured_error() {
    let output = run(
        "ready",
        &[json!({
            "id": "request-1",
            "protocol_version": PROTOCOL_VERSION + 1,
            "method": "handshake",
            "params": {"client_version": "0.1.0"}
        })],
    );

    assert!(output.status.success());
    let messages = messages(&output);
    assert_eq!(messages[0]["error"]["code"], "incompatible_protocol");
}

#[test]
fn invalid_utf8_is_rejected_without_stopping_the_fake() {
    let mut input = b"\xff\n".to_vec();
    writeln!(&mut input, "{}", handshake()).expect("handshake should be written");

    let output = run_input("ready", &input);

    assert!(output.status.success());
    let messages = messages(&output);
    assert_eq!(messages[0]["error"]["code"], "invalid_request");
    assert_eq!(messages[1]["result"]["type"], "handshake");
}

fn run(scenario: &str, requests: &[Value]) -> Output {
    let mut input = Vec::new();
    for request in requests {
        writeln!(&mut input, "{request}").expect("request should be written");
    }
    run_input(scenario, &input)
}

fn run_input(scenario: &str, input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_souvenirs-fake-engine"))
        .args(["--scenario", scenario])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fake engine should start");

    child
        .stdin
        .as_mut()
        .expect("stdin should be available")
        .write_all(input)
        .expect("input should be written");
    drop(child.stdin.take());
    child.wait_with_output().expect("fake engine should exit")
}

fn messages(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .expect("stdout should be UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("line should be valid JSON"))
        .collect()
}

fn handshake() -> Value {
    request(
        "macos-handshake-1",
        "handshake",
        json!({"client_version": "0.1.0"}),
    )
}

fn start_request() -> Value {
    request(
        "request-2",
        "operation.start",
        json!({
            "name": "import.execute",
            "catalog_path": "/Volumes/photos/Family Catalog",
            "source_path": "/Volumes/camera/EOS_DIGITAL",
            "collection_name": "Dolomites",
            "destination_mode": "create",
            "accepted_date": "2026-10-10",
            "date_resolution": "as_shown"
        }),
    )
}

fn request(id: &str, method: &str, params: Value) -> Value {
    json!({
        "id": id,
        "protocol_version": PROTOCOL_VERSION,
        "method": method,
        "params": params
    })
}
