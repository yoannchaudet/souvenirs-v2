use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use jsonschema::JSONSchema;
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
    let schema: Value = serde_json::from_slice(
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/import-plan/v1.json"))
            .unwrap(),
    )
    .unwrap();
    let validator = JSONSchema::compile(&schema).expect("committed schema should compile");
    assert!(
        validator.is_valid(&plan_value),
        "CLI-produced plan should match the committed schema"
    );
    assert_eq!(plan_value["confirmed"], true);
    assert_eq!(
        plan_value["import_plan"]["document"]["items"][0]["disposition"]["kind"],
        "transfer"
    );
}

#[test]
fn produces_canonical_scan_and_plan_across_equivalent_sources() {
    let temporary = TempDir::new().unwrap();
    let first_source = temporary.path().join("source-a");
    let second_source = temporary.path().join("source-b");
    fs::create_dir(&first_source).unwrap();
    fs::create_dir(&second_source).unwrap();
    let files = [
        ("nested/IMG_0001.CR3", b"raw bytes".as_slice()),
        ("nested/IMG_0001.JPG", b"jpeg bytes".as_slice()),
        ("nested/IMG_0001.xmp", b"xmp bytes".as_slice()),
        ("clip.MOV", b"video bytes".as_slice()),
        ("notes.pdf", b"unsupported bytes".as_slice()),
    ];
    for (path, contents) in files {
        let destination = first_source.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, contents).unwrap();
    }
    for (path, contents) in files.into_iter().rev() {
        let destination = second_source.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, contents).unwrap();
    }

    let first_scan = temporary.path().join("scan-a.json");
    let second_scan = temporary.path().join("scan-b.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        first_source.to_str().unwrap(),
        "--output",
        first_scan.to_str().unwrap(),
    ]));
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        second_source.to_str().unwrap(),
        "--output",
        second_scan.to_str().unwrap(),
    ]));

    let first_snapshot: Value = serde_json::from_slice(&fs::read(&first_scan).unwrap()).unwrap();
    let second_snapshot: Value = serde_json::from_slice(&fs::read(&second_scan).unwrap()).unwrap();
    assert_eq!(first_snapshot["files"], second_snapshot["files"]);
    assert_eq!(
        first_snapshot["fingerprint"],
        second_snapshot["fingerprint"]
    );

    let first_plan = create_plan(&temporary, "a", &first_scan);
    let second_plan = create_plan(&temporary, "b", &second_scan);
    assert_eq!(
        first_plan["import_plan"]["document"]["items"],
        second_plan["import_plan"]["document"]["items"]
    );
    assert_eq!(
        first_plan["import_plan"]["document"]["unhandled_content"],
        second_plan["import_plan"]["document"]["unhandled_content"]
    );
    assert_eq!(
        first_plan["import_plan"]["document"]["steps"],
        second_plan["import_plan"]["document"]["steps"]
    );
    assert_eq!(
        first_plan["import_plan"]["document"]["items"][0]["files"][0]["source_path"],
        serde_json::json!(["clip.MOV"])
    );
    let copy_destinations: Vec<_> = first_plan["import_plan"]["document"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["action"]["destination"].as_array())
        .cloned()
        .collect();
    let expected_destination =
        serde_json::json!(["2026", "2026-10-10 Corpus", "2026-10-10 12-34-56 001.MOV"]);
    assert!(copy_destinations.contains(expected_destination.as_array().unwrap()));
}

fn create_plan(temporary: &TempDir, suffix: &str, snapshot: &std::path::Path) -> Value {
    let catalog = temporary.path().join(format!("catalog-{suffix}"));
    fs::create_dir(&catalog).unwrap();
    json_stdout(&run(&[
        "catalog",
        "initialize",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    let plan = temporary.path().join(format!("plan-{suffix}.json"));
    json_stdout(&run(&[
        "import",
        "plan",
        "--catalog",
        catalog.to_str().unwrap(),
        "--snapshot",
        snapshot.to_str().unwrap(),
        "--output",
        plan.to_str().unwrap(),
        "--collection",
        "Corpus",
        "--destination-mode",
        "create",
        "--accepted-at",
        "2026-10-10T12:34:56",
        "--confirm",
    ]));
    serde_json::from_slice(&fs::read(plan).unwrap()).unwrap()
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
