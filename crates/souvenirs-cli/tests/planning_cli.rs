use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use jsonschema::JSONSchema;
use serde_json::Value;
use souvenirs_cli::{ScannedMetadata, SourceScan};
use souvenirs_domain::plan::ContentDigest;
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
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/import-plan/v2.json"))
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
fn plans_a_version_one_scan_without_reinterpreting_metadata() {
    const IDENTIFIER: &str = "11111111-2222-3333-4444-555555555555";
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("still.jpg"), live_photo_jpeg(IDENTIFIER)).unwrap();
    fs::write(source.join("different.mov"), live_photo_video(IDENTIFIER)).unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));
    let mut scan: SourceScan = serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
    scan.schema_version = 1;
    for file in &mut scan.files {
        file.metadata = ScannedMetadata::default();
    }
    scan.fingerprint = ContentDigest::from_bytes(&serde_json::to_vec(&scan.files).unwrap());
    fs::write(&snapshot, serde_json::to_vec_pretty(&scan).unwrap()).unwrap();

    let plan = create_plan(&temporary, "legacy-scan", &snapshot);

    assert_eq!(
        plan["import_plan"]["document"]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
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

fn apple_maker_note(identifier: &str) -> Vec<u8> {
    let mut value = identifier.as_bytes().to_vec();
    value.push(0);
    let value_offset = 14 + 2 + 12 + 4;
    let mut output = b"Apple iOS\0\0\x01MM".to_vec();
    output.extend_from_slice(&1_u16.to_be_bytes());
    output.extend_from_slice(&0x0011_u16.to_be_bytes());
    output.extend_from_slice(&2_u16.to_be_bytes());
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(&(value_offset as u32).to_be_bytes());
    output.extend_from_slice(&0_u32.to_be_bytes());
    output.extend_from_slice(&value);
    output
}

fn live_photo_jpeg(identifier: &str) -> Vec<u8> {
    let maker_note = apple_maker_note(identifier);
    let mut tiff = b"MM\0\x2a".to_vec();
    tiff.extend_from_slice(&8_u32.to_be_bytes());
    tiff.extend_from_slice(&1_u16.to_be_bytes());
    tiff.extend_from_slice(&0x8769_u16.to_be_bytes());
    tiff.extend_from_slice(&4_u16.to_be_bytes());
    tiff.extend_from_slice(&1_u32.to_be_bytes());
    tiff.extend_from_slice(&26_u32.to_be_bytes());
    tiff.extend_from_slice(&0_u32.to_be_bytes());
    tiff.extend_from_slice(&1_u16.to_be_bytes());
    tiff.extend_from_slice(&0x927c_u16.to_be_bytes());
    tiff.extend_from_slice(&7_u16.to_be_bytes());
    tiff.extend_from_slice(&(maker_note.len() as u32).to_be_bytes());
    tiff.extend_from_slice(&44_u32.to_be_bytes());
    tiff.extend_from_slice(&0_u32.to_be_bytes());
    tiff.extend_from_slice(&maker_note);

    let mut segment = b"Exif\0\0".to_vec();
    segment.extend_from_slice(&tiff);
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend_from_slice(&u16::try_from(segment.len() + 2).unwrap().to_be_bytes());
    jpeg.extend_from_slice(&segment);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    jpeg
}

fn atom(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut output = u32::try_from(body.len() + 8)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    output.extend_from_slice(&kind);
    output.extend_from_slice(body);
    output
}

fn live_photo_video(identifier: &str) -> Vec<u8> {
    let mut keys = vec![0, 0, 0, 0];
    keys.extend_from_slice(&1_u32.to_be_bytes());
    keys.extend_from_slice(&atom(*b"mdta", b"com.apple.quicktime.content.identifier"));

    let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
    data.extend_from_slice(identifier.as_bytes());
    let ilst = atom(*b"ilst", &atom(1_u32.to_be_bytes(), &atom(*b"data", &data)));
    [
        atom(*b"ftyp", b"qt  "),
        atom(
            *b"moov",
            &atom(*b"meta", &[atom(*b"keys", &keys), ilst].concat()),
        ),
    ]
    .concat()
}

fn bigtiff_dng(little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        b"II".to_vec()
    } else {
        b"MM".to_vec()
    };
    let u16_bytes = |value: u16| {
        if little_endian {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    };
    let u64_bytes = |value: u64| {
        if little_endian {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    };
    bytes.extend_from_slice(&u16_bytes(43));
    bytes.extend_from_slice(&u16_bytes(8));
    bytes.extend_from_slice(&u16_bytes(0));
    bytes.extend_from_slice(&u64_bytes(16));
    bytes.extend_from_slice(&u64_bytes(0));
    bytes
}

#[test]
fn plans_differing_basename_live_photo_from_scanned_metadata() {
    const IDENTIFIER: &str = "11111111-2222-3333-4444-555555555555";
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("STILL.JPG"), live_photo_jpeg(IDENTIFIER)).unwrap();
    fs::write(source.join("DIFFERENT.MOV"), live_photo_video(IDENTIFIER)).unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let snapshot_value: Value = serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
    assert_eq!(snapshot_value["schema_version"], 2);
    assert!(snapshot_value["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| {
            file["metadata"]["apple_content_identifier"] == serde_json::json!(IDENTIFIER)
        }));

    let plan = create_plan(&temporary, "live-photo", &snapshot);
    let items = plan["import_plan"]["document"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["companion", "primary"]
    );
    let destinations = plan["import_plan"]["document"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["action"]["destination"].as_array())
        .map(|path| path.last().unwrap().as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(destinations.contains(&"2026-10-10 12-34-56 001.JPG"));
    assert!(destinations.contains(&"2026-10-10 12-34-56 001.MOV"));
}

#[test]
fn missing_and_duplicate_live_photo_identifiers_do_not_guess_by_basename() {
    const IDENTIFIER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("same.JPG"), b"not a jpeg").unwrap();
    fs::write(source.join("same.MOV"), b"not a movie").unwrap();
    fs::write(source.join("first.JPG"), live_photo_jpeg(IDENTIFIER)).unwrap();
    fs::write(source.join("second.JPG"), live_photo_jpeg(IDENTIFIER)).unwrap();
    fs::write(source.join("clip.MOV"), live_photo_video(IDENTIFIER)).unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let plan = create_plan(&temporary, "live-findings", &snapshot);
    let items = plan["import_plan"]["document"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(
        items
            .iter()
            .filter(|item| item["disposition"]["finding"] == "ambiguous_live_photo")
            .count(),
        3
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["disposition"]["kind"] == "transfer")
            .count(),
        2
    );
}

#[test]
fn malformed_relationship_metadata_blocks_scanning_instead_of_guessing() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("paired.jpg"),
        [0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff, 0, 0, 0, 0, 0, 0],
    )
    .unwrap();
    fs::write(
        source.join("paired.mov"),
        live_photo_video("different-identifier"),
    )
    .unwrap();
    let snapshot = temporary.path().join("scan.json");

    let output = run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]);

    assert!(!output.status.success());
    assert!(!snapshot.exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("JPEG segment exceeds the file"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn valid_unsupported_bigtiff_dng_plans_without_an_identifier() {
    const IDENTIFIER: &str = "11111111-2222-3333-4444-555555555555";
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("paired.dng"), bigtiff_dng(true)).unwrap();
    fs::write(source.join("paired.mov"), live_photo_video(IDENTIFIER)).unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let scan: Value = serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
    let dng = scan["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == serde_json::json!(["paired.dng"]))
        .unwrap();
    assert!(dng.get("metadata").is_none());

    let plan = create_plan(&temporary, "bigtiff", &snapshot);
    assert_eq!(
        plan["import_plan"]["document"]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn differing_basename_compound_preserves_multi_suffix_sidecar_ambiguity() {
    const IDENTIFIER: &str = "99999999-8888-7777-6666-555555555555";
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("image.jpg"), live_photo_jpeg(IDENTIFIER)).unwrap();
    fs::write(source.join("different.mov"), live_photo_video(IDENTIFIER)).unwrap();
    fs::write(source.join("image.jpg.mp4"), b"standalone video").unwrap();
    fs::write(source.join("image.jpg.xmp"), b"sidecar").unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let plan = create_plan(&temporary, "sidecar-live", &snapshot);
    let document = &plan["import_plan"]["document"];
    assert_eq!(document["items"].as_array().unwrap().len(), 2);
    let ambiguous = document["unhandled_content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["finding"]["kind"] == "ambiguous_sidecar")
        .unwrap();
    let candidates = ambiguous["finding"]["candidate_item_ids"]
        .as_array()
        .unwrap();
    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0], candidates[1]);
    for candidate in candidates {
        let item = document["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["item_id"] == *candidate)
            .unwrap();
        assert_eq!(item["disposition"]["finding"], "ambiguous_sidecar");
    }
}

#[test]
fn plans_raw_jpeg_and_owned_sidecar_as_one_logical_item() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("IMG_0001.CR3"), b"raw").unwrap();
    fs::write(source.join("IMG_0001.JPG"), b"jpeg").unwrap();
    fs::write(source.join("IMG_0001.JPG.xmp"), b"xmp").unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let plan = create_plan(&temporary, "compound", &snapshot);
    let document = &plan["import_plan"]["document"];
    let items = document["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["companion", "primary", "sidecar"]
    );
    assert!(document["unhandled_content"].as_array().unwrap().is_empty());

    let destinations = document["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["action"]["destination"].as_array())
        .map(|path| path.last().unwrap().as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(destinations.contains(&"2026-10-10 12-34-56 001.CR3"));
    assert!(destinations.contains(&"2026-10-10 12-34-56 001.JPG"));
    assert!(destinations.contains(&"2026-10-10 12-34-56 001.JPG.xmp"));

    let manifest_step = document["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["action"]["kind"] == "write_manifest")
        .unwrap();
    assert_eq!(
        manifest_step["action"]["resulting_size_bytes"]
            .as_u64()
            .unwrap(),
        2 * (64 + 2 + "2026-10-10 12-34-56 001.CR3".len() as u64 + 1)
    );
}

#[test]
fn reports_orphan_and_ambiguous_sidecars_without_cross_directory_pairing() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    for (path, contents) in [
        ("ambiguous/image.heic", b"still".as_slice()),
        ("ambiguous/image.mov", b"video".as_slice()),
        ("ambiguous/image.xmp", b"xmp".as_slice()),
        ("orphan/lost.xmp", b"orphan".as_slice()),
        ("raw/image.CR3", b"raw".as_slice()),
        ("jpeg/image.JPG", b"jpeg".as_slice()),
    ] {
        let destination = source.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, contents).unwrap();
    }
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let plan = create_plan(&temporary, "findings", &snapshot);
    let document = &plan["import_plan"]["document"];
    assert_eq!(document["items"].as_array().unwrap().len(), 4);
    let findings = document["unhandled_content"].as_array().unwrap();
    assert_eq!(findings.len(), 2);
    let ambiguous = findings
        .iter()
        .find(|finding| finding["finding"]["kind"] == "ambiguous_sidecar")
        .unwrap();
    assert_eq!(
        ambiguous["finding"]["candidate_item_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for candidate in ambiguous["finding"]["candidate_item_ids"]
        .as_array()
        .unwrap()
    {
        let item = document["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["item_id"] == *candidate)
            .unwrap();
        assert_eq!(item["disposition"]["kind"], "unresolved");
        assert_eq!(
            item["disposition"]["finding"],
            serde_json::json!("ambiguous_sidecar")
        );
    }
    assert!(findings
        .iter()
        .any(|finding| finding["finding"]["kind"] == "orphan_sidecar"));
}

#[test]
fn unresolved_only_plan_does_not_create_catalog_steps() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("image.JPG"), b"jpeg-one").unwrap();
    fs::write(source.join("image.jpeg"), b"jpeg-two").unwrap();
    fs::write(source.join("image.CR3"), b"raw").unwrap();
    fs::write(source.join("image.AAE"), b"sidecar").unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));

    let plan = create_plan(&temporary, "unresolved", &snapshot);
    let document = &plan["import_plan"]["document"];
    assert!(document["steps"].as_array().unwrap().is_empty());
    assert_eq!(document["required_space_bytes"], 0);
    assert!(document["items"].as_array().unwrap().iter().all(|item| {
        item["disposition"]["kind"] == "unresolved"
            && item["disposition"]["finding"] == "ambiguous_raw_jpeg"
    }));
}

#[test]
fn existing_collection_validation_allows_owned_sidecars_outside_manifest() {
    let temporary = TempDir::new().unwrap();
    let catalog = temporary.path().join("catalog");
    let source = temporary.path().join("source");
    fs::create_dir(&catalog).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("new.jpg"), b"new").unwrap();
    json_stdout(&run(&[
        "catalog",
        "initialize",
        "--catalog",
        catalog.to_str().unwrap(),
    ]));
    let collection = catalog.join("2026").join("2026-10-10 Existing");
    fs::create_dir_all(&collection).unwrap();
    let still = "2026-10-10 09-00-00 001.heic";
    let video = "2026-10-10 09-00-00 001.mov";
    fs::write(collection.join(still), b"still").unwrap();
    fs::write(collection.join(video), b"video").unwrap();
    fs::write(collection.join("2026-10-10 09-00-00 001.AAE"), b"aae").unwrap();
    fs::write(
        collection.join("SHA256SUMS"),
        format!(
            "{}  {still}\n{}  {video}\n",
            sha256_hex(b"still"),
            sha256_hex(b"video")
        ),
    )
    .unwrap();
    let snapshot = temporary.path().join("scan.json");
    json_stdout(&run(&[
        "import",
        "scan",
        "--source",
        source.to_str().unwrap(),
        "--output",
        snapshot.to_str().unwrap(),
    ]));
    let plan = temporary.path().join("plan.json");

    let output = run(&[
        "import",
        "plan",
        "--catalog",
        catalog.to_str().unwrap(),
        "--snapshot",
        snapshot.to_str().unwrap(),
        "--output",
        plan.to_str().unwrap(),
        "--collection",
        "Existing",
        "--destination-mode",
        "existing",
        "--accepted-at",
        "2026-10-10T12:34:56",
        "--confirm",
    ]);

    let planned = json_stdout(&output);
    assert_eq!(planned["result"]["transfer_count"], 1);
}

fn sha256_hex(contents: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};

    format!("{:x}", Sha256::digest(contents))
}

fn write_initialization_plan(catalog: &Path) {
    let config = "format_version: 2\nmedia_extensions: [jpg]\nsidecar_extensions: [xmp]\n";
    fs::write(
        catalog.join(".souvenirs-initialize.jsonl"),
        format!(
            "{{\"kind\":\"plan\",\"operation_id\":\"initialize\",\"plan\":{{\"format_version\":2,\"config_yaml\":{},\"config_sha256\":\"{}\",\"journal_file_name\":\"recovery-initialize.jsonl\",\"staged_config_path\":\".souvenirs/staging/initialize-config.yml.stage\"}}}}\n",
            serde_json::to_string(config).unwrap(),
            sha256_hex(config.as_bytes())
        ),
    )
    .unwrap();
}

#[test]
fn initialization_resumes_a_recorded_plan() {
    let temporary = TempDir::new().unwrap();
    write_initialization_plan(temporary.path());

    json_stdout(&run(&[
        "catalog",
        "initialize",
        "--catalog",
        temporary.path().to_str().unwrap(),
    ]));

    assert!(temporary.path().join(".souvenirs.yml").exists());
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
