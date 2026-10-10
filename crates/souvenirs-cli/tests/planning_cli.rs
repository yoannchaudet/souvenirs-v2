use std::fs;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_souvenirs-cli"))
        .args(args)
        .output()
        .expect("CLI should run")
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("stdout should be one JSON result")
}

#[test]
fn initializes_inspects_scans_and_writes_a_confirmed_plan() {
    let temporary = TempDir::new().unwrap();
    let catalog = temporary.path().join("catalog");
    let source = temporary.path().join("source");
    fs::create_dir(&catalog).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("photo.jpg"), b"photo bytes").unwrap();
    fs::write(source.join("notes.txt"), b"ignored bytes").unwrap();
    let snapshot = temporary.path().join("scan.json");
    let plan = temporary.path().join("plan.json");

    let initialized = json_stdout(&run(&[
        "catalog",
        "initialize",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(initialized["command"], "catalog.initialize");
    assert_eq!(initialized["result"]["format_version"], 2);

    let inspected = json_stdout(&run(&[
        "catalog",
        "inspect",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(inspected["command"], "catalog.inspect");
    assert_eq!(inspected["result"]["managed_file_count"], 0);

    let scanned = json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));
    assert_eq!(scanned["result"]["file_count"], 2);
    assert_eq!(scanned["result"]["media_count"], 1);
    assert_eq!(scanned["result"]["unknown_count"], 1);

    let planned = json_stdout(&run(&[
        "import",
        "plan",
        "--catalog",
        catalog.to_str().unwrap(),
        "--snapshot",
        snapshot.to_str().unwrap(),
        "--output",
        plan.to_str().unwrap(),
        "--collection",
        "Vacation",
        "--destination-mode",
        "create",
        "--accepted-at",
        "2026-10-10T12:34:56",
        "--confirm",
    ]));
    assert_eq!(planned["command"], "import.plan");
    assert_eq!(planned["result"]["confirmed"], true);
    assert_eq!(planned["result"]["transfer_count"], 1);
    assert_eq!(planned["result"]["unhandled_count"], 1);

    let plan_value: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    assert_eq!(plan_value["confirmed"], true);
    assert_eq!(
        plan_value["import_plan"]["document"]["items"][0]["disposition"]["kind"],
        "transfer"
    );
}

#[test]
fn help_is_successful_and_human_readable() {
    let output = run(&["--help"]);

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn create_destination_refuses_an_existing_collection() {
    let temporary = TempDir::new().unwrap();
    let catalog = temporary.path().join("catalog");
    let source = temporary.path().join("source");
    fs::create_dir(&catalog).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("photo.jpg"), b"photo bytes").unwrap();
    let snapshot = temporary.path().join("scan.json");

    json_stdout(&run(&[
        "catalog",
        "initialize",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));
    fs::create_dir_all(catalog.join("2026").join("2026-10-10 Vacation")).unwrap();

    let output = run(&[
        "import",
        "plan",
        "--catalog",
        catalog.to_str().unwrap(),
        "--snapshot",
        snapshot.to_str().unwrap(),
        "--output",
        temporary.path().join("plan.json").to_str().unwrap(),
        "--collection",
        "Vacation",
        "--destination-mode",
        "create",
        "--accepted-at",
        "2026-10-10T12:34:56",
    ]);

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
}

#[test]
fn reports_structured_errors_only_on_stderr() {
    let temporary = TempDir::new().unwrap();
    let missing = temporary.path().join("missing");

    let output = run(&["catalog", "inspect", "--catalog", missing.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "not_found");
}
