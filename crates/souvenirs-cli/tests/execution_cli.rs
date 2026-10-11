use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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

fn sha256_hex(contents: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};

    format!("{:x}", Sha256::digest(contents))
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

#[test]
fn execution_rejects_missing_changed_and_unexpected_manifest_namespace_entries() {
    for (case, expected_message) in [
        ("missing", "catalog manifest is missing after planning"),
        ("changed", "catalog manifest changed after planning"),
        (
            "unexpected",
            "unexpected catalog manifest appeared after planning",
        ),
    ] {
        let temporary = TempDir::new().unwrap();
        let catalog = temporary.path().join("catalog");
        let source = temporary.path().join("source");
        let snapshot = temporary.path().join("scan.json");
        let plan = temporary.path().join("plan.json");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("photo.jpg"), b"photo").unwrap();
        json_stdout(&run(&[
            "catalog",
            "initialize",
            "--catalog",
            catalog.to_str().unwrap(),
        ]));
        let existing = catalog.join("2025/2025-01-01 Existing");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("old.jpg"), b"old").unwrap();
        fs::write(
            existing.join("SHA256SUMS"),
            format!("{}  old.jpg\n", sha256_hex(b"old")),
        )
        .unwrap();
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
            "Vacation",
            "--destination-mode",
            "create",
            "--accepted-at",
            "2026-10-10T12:34:56",
            "--confirm",
        ]));

        match case {
            "missing" => fs::remove_file(existing.join("SHA256SUMS")).unwrap(),
            "changed" => fs::write(
                existing.join("SHA256SUMS"),
                format!("{}  old.jpg\n", sha256_hex(b"changed")),
            )
            .unwrap(),
            "unexpected" => {
                let added = catalog.join("2025/2025-02-02 Added");
                fs::create_dir_all(&added).unwrap();
                fs::write(added.join("SHA256SUMS"), "").unwrap();
            }
            _ => unreachable!(),
        }

        let output = run(&[
            "import",
            "execute",
            "--catalog",
            catalog.to_str().unwrap(),
            "--plan",
            plan.to_str().unwrap(),
        ]);

        assert_eq!(output.status.code(), Some(4), "{case}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected_message),
            "{case}: {error}"
        );
    }
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

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                snapshot.insert(relative, None);
                collect(root, &path, snapshot);
            } else {
                snapshot.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    collect(root, root, &mut snapshot);
    snapshot
}

fn operation_id_from_plan(plan: &Path) -> String {
    let plan: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    plan["import_plan"]["document"]["plan_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn first_planned_destination(catalog: &Path, plan: &Path) -> PathBuf {
    let plan: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    let components = plan["import_plan"]["document"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|step| step["action"]["path"].as_array())
        .unwrap();
    components.iter().fold(catalog.to_path_buf(), |path, part| {
        path.join(part.as_str().unwrap())
    })
}

fn assert_conflict(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
    error
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
    assert_eq!(integrity["result"]["coverage_complete"], true);
    assert_eq!(integrity["result"]["checked_files"], 2);
    assert_eq!(integrity["result"]["findings"], serde_json::json!([]));
}

#[cfg(unix)]
#[test]
fn unreadable_manifested_original_preserves_findings_and_continues_checking() {
    let temporary = TempDir::new().unwrap();
    let (catalog, _source, plan) = prepare_plan(&temporary, true);
    json_stdout(&run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]));

    let year = catalog.join("2026");
    fs::create_dir(year.join("2026-10-09 Earlier")).unwrap();
    let collection = year.join("2026-10-10 Vacation");
    let later = year.join("2026-10-11 Later");
    fs::create_dir(&later).unwrap();
    for filename in [
        "2026-10-10 12-34-56 001.jpg",
        "2026-10-10 12-34-56 002.jpg",
        "SHA256SUMS",
    ] {
        fs::copy(collection.join(filename), later.join(filename)).unwrap();
    }
    let unreadable_manifest = later.join("SHA256SUMS");
    fs::set_permissions(&unreadable_manifest, fs::Permissions::from_mode(0o000)).unwrap();
    let unlistable = year.join("2026-10-12 Unlistable");
    fs::create_dir(&unlistable).unwrap();
    for filename in [
        "2026-10-10 12-34-56 001.jpg",
        "2026-10-10 12-34-56 002.jpg",
        "SHA256SUMS",
    ] {
        fs::copy(collection.join(filename), unlistable.join(filename)).unwrap();
    }
    fs::set_permissions(&unlistable, fs::Permissions::from_mode(0o100)).unwrap();
    fs::create_dir(year.join("2026-10-13 Final")).unwrap();
    let unreadable = collection.join("2026-10-10 12-34-56 001.jpg");
    let changed = collection.join("2026-10-10 12-34-56 002.jpg");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    fs::write(&changed, b"changed after import").unwrap();

    let integrity = json_stdout(&run(&[
        "integrity",
        "check",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&unreadable_manifest, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&unlistable, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(integrity["result"]["valid"], false);
    assert_eq!(integrity["result"]["coverage_complete"], false);
    assert_eq!(integrity["result"]["checked_files"], 3);
    assert_eq!(integrity["result"]["checked_manifests"], 2);
    assert_eq!(
        integrity["result"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|finding| finding["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "missing_manifest",
            "unreadable_file",
            "digest_mismatch",
            "unreadable_file",
            "unreadable_collection",
            "missing_manifest"
        ]
    );
    assert_eq!(
        integrity["result"]["findings"][1]["path"],
        "2026/2026-10-10 Vacation/2026-10-10 12-34-56 001.jpg"
    );
    assert!(integrity["result"]["findings"][1]["error"]
        .as_str()
        .unwrap()
        .contains("Permission denied"));
    assert_eq!(
        integrity["result"]["findings"][3]["path"],
        "2026/2026-10-11 Later/SHA256SUMS"
    );
    assert_eq!(
        integrity["result"]["findings"][4]["collection"],
        "2026/2026-10-12 Unlistable"
    );
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
fn rejects_portable_directory_drift_before_creating_progress_or_mutating_inputs() {
    let temporary = TempDir::new().unwrap();
    let (catalog, source, plan) = prepare_plan_with_existing_year(&temporary, true, true);
    let plan_json: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let operation_id = plan_json["import_plan"]["document"]["plan_id"]
        .as_str()
        .unwrap();
    let occupied = catalog.join("2026").join("2026-10-10 VACATION");
    fs::create_dir(&occupied).unwrap();
    let progress = temporary.path().join("progress.jsonl");
    let catalog_before = tree_snapshot(&catalog);
    let source_before = tree_snapshot(&source);
    let plan_before = fs::read(&plan).unwrap();

    let output = run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
        "--progress",
        progress.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("planned destination is occupied"));
    assert!(!progress.exists());
    assert!(!catalog.join(".souvenirs").join("lock").exists());
    assert_eq!(operation_journal_count(&catalog, operation_id), 0);
    assert_eq!(tree_snapshot(&catalog), catalog_before);
    assert_eq!(tree_snapshot(&source), source_before);
    assert_eq!(fs::read(&plan).unwrap(), plan_before);
}

#[test]
fn rejects_import_progress_paths_without_mutating_inputs_or_catalog_state() {
    for case in ["catalog_lock", "planned_destination", "plan", "source"] {
        let temporary = TempDir::new().unwrap();
        let (catalog, source, plan) = prepare_plan(&temporary, true);
        let operation_id = operation_id_from_plan(&plan);
        let progress = match case {
            "catalog_lock" => catalog.join(".souvenirs").join("lock"),
            "planned_destination" => first_planned_destination(&catalog, &plan),
            "plan" => plan.clone(),
            "source" => source.join("progress.jsonl"),
            _ => unreachable!(),
        };
        let progress_existed = progress.exists();
        let catalog_before = tree_snapshot(&catalog);
        let source_before = tree_snapshot(&source);
        let plan_before = fs::read(&plan).unwrap();

        let output = run(&[
            "import",
            "execute",
            "--catalog",
            catalog.to_str().unwrap(),
            "--plan",
            plan.to_str().unwrap(),
            "--progress",
            progress.to_str().unwrap(),
        ]);

        let error = assert_conflict(&output);
        let expected_message = match case {
            "catalog_lock" => "progress path is inside the catalog",
            "planned_destination" => "progress path overlaps a planned destination",
            "plan" => "progress path overlaps the saved plan",
            "source" => "progress path overlaps the source",
            _ => unreachable!(),
        };
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected_message),
            "case: {case}"
        );
        assert_eq!(tree_snapshot(&catalog), catalog_before, "case: {case}");
        assert_eq!(tree_snapshot(&source), source_before, "case: {case}");
        assert_eq!(fs::read(&plan).unwrap(), plan_before, "case: {case}");
        assert_eq!(progress.exists(), progress_existed, "case: {case}");
        assert_eq!(operation_journal_count(&catalog, &operation_id), 0);
        assert!(!catalog.join(".souvenirs").join("lock").exists());
    }
}

#[cfg(unix)]
#[test]
fn rejects_an_existing_progress_symlink_to_the_plan_without_truncating_it() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let (catalog, source, plan) = prepare_plan(&temporary, true);
    let progress = temporary.path().join("progress.jsonl");
    symlink(&plan, &progress).unwrap();
    let catalog_before = tree_snapshot(&catalog);
    let source_before = tree_snapshot(&source);
    let plan_before = fs::read(&plan).unwrap();

    let output = run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
        "--progress",
        progress.to_str().unwrap(),
    ]);

    let error = assert_conflict(&output);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("progress path overlaps the saved plan"));
    assert_eq!(tree_snapshot(&catalog), catalog_before);
    assert_eq!(tree_snapshot(&source), source_before);
    assert_eq!(fs::read(&plan).unwrap(), plan_before);
    assert_eq!(fs::read(&progress).unwrap(), plan_before);
}

#[cfg(unix)]
#[test]
fn rejects_a_dangling_progress_symlink_as_a_conflict() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let (catalog, source, plan) = prepare_plan(&temporary, true);
    let progress = temporary.path().join("progress.jsonl");
    symlink(temporary.path().join("missing.jsonl"), &progress).unwrap();
    let catalog_before = tree_snapshot(&catalog);
    let source_before = tree_snapshot(&source);
    let plan_before = fs::read(&plan).unwrap();

    let output = run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
        "--progress",
        progress.to_str().unwrap(),
    ]);

    let error = assert_conflict(&output);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("existing output path cannot be resolved"));
    assert_eq!(tree_snapshot(&catalog), catalog_before);
    assert_eq!(tree_snapshot(&source), source_before);
    assert_eq!(fs::read(&plan).unwrap(), plan_before);
    assert!(progress.is_symlink());
}

#[test]
fn preserves_not_found_for_a_missing_progress_directory() {
    let temporary = TempDir::new().unwrap();
    let (catalog, source, plan) = prepare_plan(&temporary, true);
    let progress = temporary.path().join("missing").join("progress.jsonl");
    let catalog_before = tree_snapshot(&catalog);
    let source_before = tree_snapshot(&source);
    let plan_before = fs::read(&plan).unwrap();

    let output = run(&[
        "import",
        "execute",
        "--catalog",
        catalog.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
        "--progress",
        progress.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "not_found");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("output directory does not exist"));
    assert_eq!(tree_snapshot(&catalog), catalog_before);
    assert_eq!(tree_snapshot(&source), source_before);
    assert_eq!(fs::read(&plan).unwrap(), plan_before);
    assert!(!catalog.join(".souvenirs").join("lock").exists());
}

#[test]
fn recovery_commands_reject_catalog_progress_before_acquiring_the_lock() {
    for command in ["recover", "cancel"] {
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
        simulate_interruption_after_first_copy(
            &catalog,
            &operation_journal(&catalog, operation_id),
        );
        let progress = catalog.join(".souvenirs").join("lock");
        let catalog_before = tree_snapshot(&catalog);
        let source_before = tree_snapshot(&source);
        let plan_before = fs::read(&plan).unwrap();

        let output = run(&[
            "operation",
            command,
            "--catalog",
            catalog.to_str().unwrap(),
            "--operation-id",
            operation_id,
            "--plan",
            plan.to_str().unwrap(),
            "--progress",
            progress.to_str().unwrap(),
        ]);

        let error = assert_conflict(&output);
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("progress path is inside the catalog"));
        assert_eq!(
            tree_snapshot(&catalog),
            catalog_before,
            "command: {command}"
        );
        assert_eq!(tree_snapshot(&source), source_before, "command: {command}");
        assert_eq!(fs::read(&plan).unwrap(), plan_before, "command: {command}");
        assert!(!progress.exists());
    }
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
        let progress = temporary.path().join(format!("{command}-progress.jsonl"));

        let result = json_stdout(&run(&[
            "operation",
            command,
            "--catalog",
            catalog.to_str().unwrap(),
            "--operation-id",
            operation_id,
            "--plan",
            plan.to_str().unwrap(),
            "--progress",
            progress.to_str().unwrap(),
        ]));
        assert_eq!(result["result"]["outcome"], expected);
        let events = fs::read_to_string(progress).unwrap();
        assert!(events.contains("\"event\":\"recovery_started\""));
        assert!(events.contains(&format!("\"event\":\"{expected}\"")));

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
fn cancellation_accepts_a_safe_progress_path_when_the_source_root_is_missing() {
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
    for entry in fs::read_dir(&source).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    fs::remove_dir(&source).unwrap();
    let progress = temporary.path().join("cancel-progress.jsonl");

    let result = json_stdout(&run(&[
        "operation",
        "cancel",
        "--catalog",
        catalog.to_str().unwrap(),
        "--operation-id",
        operation_id,
        "--plan",
        plan.to_str().unwrap(),
        "--progress",
        progress.to_str().unwrap(),
    ]));

    assert_eq!(result["result"]["outcome"], "cancelled");
    assert!(progress.is_file());
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
