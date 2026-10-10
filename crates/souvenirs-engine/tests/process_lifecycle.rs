use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use souvenirs_protocol::PROTOCOL_VERSION;

#[test]
fn process_serves_requests_and_exits_cleanly_on_eof() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_souvenirs-engine"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("engine process should start");

    let request = json!({
        "id": "macos-1",
        "protocol_version": PROTOCOL_VERSION,
        "method": "handshake",
        "params": {}
    });
    writeln!(
        child.stdin.as_mut().expect("stdin should be available"),
        "{request}"
    )
    .expect("request should be written");
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .expect("engine process should finish");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert_eq!(stdout.lines().count(), 1);
    let response: Value = serde_json::from_str(&stdout).expect("stdout should contain JSON");
    assert_eq!(response["id"], "macos-1");
    assert_eq!(response["result"]["protocol_version"], PROTOCOL_VERSION);
}
