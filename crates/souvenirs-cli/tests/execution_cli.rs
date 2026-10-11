use std::fs;
use std::path::{Path, PathBuf};
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

fn prepare_plan(temporary: &TempDir, confirmed: bool) -> (PathBuf, PathBuf, PathBuf) {
    prepare_plan_with_existing_year(temporary, confirmed, false)
}

fn prepare_plan_with_existing_year(
    temporary: &TempDir,
    confirmed: bool,
    existing_year: bool,
) -> (PathBuf, PathBuf, PathBuf) {
    let catalog = temporary.path().join("catalog");
    let source = temporary.path().join("source");
    let snapshot = temporary.path().join("scan.json");
    let plan = temporary.path().join("plan.json");
    fs::create_dir(&catalog).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("photo-a.jpg"), b"photo a bytes").unwrap();
    fs::write(source.join("photo-b.jpg"), b"photo b bytes").unwrap();

    json_stdout(&run(&[
        "catalog",
        "initialize",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    if existing_year {
        fs::create_dir(catalog.join("2026")).unwrap();
    }
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));
    let mut arguments = vec![
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
    ];
    if confirmed {
        arguments.push("--confirm");
    }
    json_stdout(&run(&arguments));
    (catalog, source, plan)
}

fn operation_journal(catalog: &Path, operation_id: &str) -> PathBuf {
    fs::read_dir(catalog.join(".souvenirs").join("journal"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .next()
                .is_some_and(|line| line.contains(operation_id))
        })
        .expect("operation journal should exist")
}

fn operation_journal_count(catalog: &Path, operation_id: &str) -> usize {
    fs::read_dir(catalog.join(".souvenirs").join("journal"))
        .unwrap()
        .flatten()
        .filter(|entry| {
            fs::read_to_string(entry.path())
                .unwrap()
                .lines()
                .next()
                .is_some_and(|line| line.contains(operation_id))
        })
        .count()
}

fn simulate_interruption_after_first_copy(catalog: &Path, path: &Path) {
    let contents = fs::read_to_string(path).unwrap();
    let lines = contents.lines().collect::<Vec<_>>();
    let metadata = lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["kind"] == "metadata")
        .expect("journal should contain recovery metadata");
    assert_eq!(metadata["collection_name"], "Vacation");
    let published_index = lines
        .iter()
        .position(|line| {
            let record: Value = serde_json::from_str(line).unwrap();
            record["transition"]["kind"] == "copy_published"
        })
        .expect("journal should contain a published copy");
    let published: Value = serde_json::from_str(lines[published_index]).unwrap();
    let published_path = catalog.join(published["transition"]["destination"].as_str().unwrap());
    fs::write(path, format!("{}\n", lines[..=published_index].join("\n"))).unwrap();

    for entry in fs::read_dir(published_path.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        if entry.path() != published_path {
            fs::remove_file(entry.path()).unwrap();
        }
    }
}

fn simulate_interruption_after_first_verification(
    catalog: &Path,
    path: &Path,
    preserve_stage: bool,
) -> PathBuf {
    let contents = fs::read_to_string(path).unwrap();
    let lines = contents.lines().collect::<Vec<_>>();
    let verified_index = lines
        .iter()
        .position(|line| {
            let record: Value = serde_json::from_str(line).unwrap();
            record["transition"]["kind"] == "copy_verified"
        })
        .expect("journal should contain a verified copy");
    let verified: Value = serde_json::from_str(lines[verified_index]).unwrap();
    let step_id = verified["transition"]["step_id"].as_str().unwrap();
    let started: Value = lines[..verified_index]
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| {
            record["transition"]["kind"] == "copy_started"
                && record["transition"]["step_id"] == step_id
        })
        .expect("verified copy should have a start record");
    let source = PathBuf::from(started["transition"]["source"].as_str().unwrap());
    let destination = catalog.join(verified["transition"]["destination"].as_str().unwrap());
    let staged = catalog.join(verified["transition"]["staged_path"].as_str().unwrap());
    if preserve_stage {
        fs::copy(&destination, &staged).unwrap();
    }
    fs::write(path, format!("{}\n", lines[..=verified_index].join("\n"))).unwrap();
    for entry in fs::read_dir(destination.parent().unwrap()).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    source
}

#[test]
fn executes_inspects_and_verifies_without_parsing_human_output() {
    let temporary = TempDir::new().unwrap();
    let (catalog, _source, plan) = prepare_plan(&temporary, true);
    let progress = temporary.path().join("progress.jsonl");

    let executed = json_stdout(&run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
        "--progress",
        progress.to_str().unwrap(),
    ]));
    let operation_id = executed["result"]["operation_id"].as_str().unwrap();
    assert_eq!(executed["result"]["outcome"], "completed");
    assert!(progress.is_file());
    for line in fs::read_to_string(progress).unwrap().lines() {
        let event: Value = serde_json::from_str(line).unwrap();
        assert_eq!(event["operation_id"], operation_id);
        assert!(event["sequence"].is_u64());
    }

    let status = json_stdout(&run(&[
        "operation",
        "status",
        "--catalog",
        catalog.to_str().unwrap(),
        "--operation-id",
        operation_id,
    ]));
    assert_eq!(status["result"]["state"], "completed");

    let inspected = json_stdout(&run(&[
        "catalog",
        "inspect",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(inspected["result"]["managed_file_count"], 2);
    assert_eq!(inspected["result"]["manifest_count"], 1);

    let integrity = json_stdout(&run(&[
        "integrity",
        "check",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(integrity["result"]["valid"], true);
    assert_eq!(integrity["result"]["checked_files"], 2);
    assert_eq!(integrity["result"]["findings"], serde_json::json!([]));
}

#[test]
fn executes_compound_item_with_sidecar_and_manifests_only_originals() {
    let temporary = TempDir::new().unwrap();
    let catalog = temporary.path().join("catalog");
    let source = temporary.path().join("source");
    let snapshot = temporary.path().join("scan.json");
    let plan = temporary.path().join("plan.json");
    fs::create_dir(&catalog).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("IMG_0001.CR3"), b"raw").unwrap();
    fs::write(source.join("IMG_0001.JPG"), b"jpeg").unwrap();
    fs::write(source.join("IMG_0001.JPG.xmp"), b"xmp").unwrap();
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
        "Compound",
        "--destination-mode",
        "create",
        "--accepted-at",
        "2026-10-10T12:34:56",
        "--confirm",
    ]));

    let executed = json_stdout(&run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]));
    assert_eq!(executed["result"]["outcome"], "completed");

    let collection = catalog.join("2026").join("2026-10-10 Compound");
    assert!(collection.join("2026-10-10 12-34-56 001.CR3").is_file());
    assert!(collection.join("2026-10-10 12-34-56 001.JPG").is_file());
    assert!(collection.join("2026-10-10 12-34-56 001.JPG.xmp").is_file());
    let manifest = fs::read_to_string(collection.join("SHA256SUMS")).unwrap();
    assert_eq!(manifest.lines().count(), 2);
    assert!(!manifest.contains(".xmp"));

    let integrity = json_stdout(&run(&[
        "integrity",
        "check",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(integrity["result"]["valid"], true);
    assert_eq!(integrity["result"]["checked_files"], 2);
}

#[test]
fn refuses_to_execute_an_unconfirmed_plan_without_mutating_the_catalog() {
    let temporary = TempDir::new().unwrap();
    let (catalog, _source, plan) = prepare_plan(&temporary, false);

    let output = run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
    assert!(!catalog.join("2026").exists());
}

#[test]
fn rejects_portable_directory_drift_without_creating_a_journal() {
    let temporary = TempDir::new().unwrap();
    let (catalog, _source, plan) = prepare_plan_with_existing_year(&temporary, true, true);
    let plan_json: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let operation_id = plan_json["import_plan"]["document"]["plan_id"]
        .as_str()
        .unwrap();
    let occupied = catalog.join("2026").join("2026-10-10 VACATION");
    fs::create_dir(&occupied).unwrap();

    let output = run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("planned destination is occupied"));
    assert_eq!(operation_journal_count(&catalog, operation_id), 0);
    assert!(occupied.is_dir());
}

#[test]
fn recovers_and_cancels_interrupted_operation_journals() {
    for (command, expected) in [("recover", "completed"), ("cancel", "cancelled")] {
        let temporary = TempDir::new().unwrap();
        let (catalog, _source, plan) = prepare_plan(&temporary, true);
        let executed = json_stdout(&run(&[
            "import",
            "execute",
            "--catalog",
            catalog.to_str().unwrap(),
            "--plan",
            plan.to_str().unwrap(),
        ]));
        let operation_id = executed["result"]["operation_id"].as_str().unwrap();
        simulate_interruption_after_first_copy(
            &catalog,
            &operation_journal(&catalog, operation_id),
        );

        let result = json_stdout(&run(&[
            "operation",
            command,
            "--catalog",
            catalog.to_str().unwrap(),
            "--operation-id",
            operation_id,
            "--plan",
            plan.to_str().unwrap(),
        ]));
        assert_eq!(result["result"]["outcome"], expected);

        let status = json_stdout(&run(&[
            "operation",
            "status",
            "--catalog",
            catalog.to_str().unwrap(),
            "--operation-id",
            operation_id,
        ]));
        assert_eq!(status["result"]["state"], expected);

        let integrity = json_stdout(&run(&[
            "integrity",
            "check",
            "--catalog",
            catalog.to_str().unwrap(),
        ]));
        assert_eq!(integrity["result"]["valid"], true);
        assert_eq!(
            integrity["result"]["checked_files"],
            if command == "recover" { 2 } else { 1 }
        );
    }
}

#[test]
fn recovery_refuses_a_changed_remaining_source() {
    let temporary = TempDir::new().unwrap();
    let (catalog, source, plan) = prepare_plan(&temporary, true);
    let executed = json_stdout(&run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]));
    let operation_id = executed["result"]["operation_id"].as_str().unwrap();
    simulate_interruption_after_first_copy(&catalog, &operation_journal(&catalog, operation_id));
    fs::write(source.join("photo-a.jpg"), b"changed a").unwrap();
    fs::write(source.join("photo-b.jpg"), b"changed b").unwrap();

    let output = run(&[
        "operation",
        "recover",
        "--catalog",
        catalog.to_str().unwrap(),
        "--operation-id",
        operation_id,
        "--plan",
        plan.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(4));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
}

#[test]
fn cancellation_does_not_manifest_a_missing_verified_stage() {
    let temporary = TempDir::new().unwrap();
    let (catalog, _source, plan) = prepare_plan(&temporary, true);
    let executed = json_stdout(&run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]));
    let operation_id = executed["result"]["operation_id"].as_str().unwrap();
    simulate_interruption_after_first_verification(
        &catalog,
        &operation_journal(&catalog, operation_id),
        false,
    );

    let result = json_stdout(&run(&[
        "operation",
        "cancel",
        "--catalog",
        catalog.to_str().unwrap(),
        "--operation-id",
        operation_id,
        "--plan",
        plan.to_str().unwrap(),
    ]));

    assert_eq!(result["result"]["outcome"], "cancelled");
    let integrity = json_stdout(&run(&[
        "integrity",
        "check",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(integrity["result"]["valid"], true);
    assert_eq!(integrity["result"]["checked_files"], 0);
}

#[test]
fn recovery_publishes_a_verified_stage_without_the_source() {
    let temporary = TempDir::new().unwrap();
    let (catalog, _source, plan) = prepare_plan(&temporary, true);
    let executed = json_stdout(&run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]));
    let operation_id = executed["result"]["operation_id"].as_str().unwrap();
    let verified_source = simulate_interruption_after_first_verification(
        &catalog,
        &operation_journal(&catalog, operation_id),
        true,
    );
    fs::remove_file(verified_source).unwrap();

    let result = json_stdout(&run(&[
        "operation",
        "recover",
        "--catalog",
        catalog.to_str().unwrap(),
        "--operation-id",
        operation_id,
        "--plan",
        plan.to_str().unwrap(),
    ]));

    assert_eq!(result["result"]["outcome"], "completed");
    let integrity = json_stdout(&run(&[
        "integrity",
        "check",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    assert_eq!(integrity["result"]["valid"], true);
    assert_eq!(integrity["result"]["checked_files"], 2);
}
