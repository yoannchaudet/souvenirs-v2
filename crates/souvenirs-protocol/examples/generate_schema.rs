use std::fs;
use std::path::PathBuf;

fn main() {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/v1.json");
    let schema = serde_json::to_string_pretty(&souvenirs_protocol::protocol_schema())
        .expect("protocol schema should serialize");
    fs::create_dir_all(output.parent().expect("schema path should have a parent"))
        .expect("schema directory should be created");
    fs::write(&output, schema + "\n").expect("protocol schema should be written");
    println!("{}", output.display());
}
