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
