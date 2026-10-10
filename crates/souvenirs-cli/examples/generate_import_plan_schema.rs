use std::fs;
use std::path::PathBuf;

fn main() {
    for version in 1..=souvenirs_domain::plan::IMPORT_PLAN_SCHEMA_VERSION {
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("schema/import-plan/v{version}.json"));
        let schema = serde_json::to_string_pretty(
            &souvenirs_cli::stored_plan_schema_for_version(version)
                .expect("import plan schema version should be supported"),
        )
        .expect("import plan schema should serialize");
        fs::create_dir_all(output.parent().expect("schema path should have a parent"))
            .expect("schema directory should be created");
        fs::write(&output, schema + "\n").expect("import plan schema should be written");
        println!("{}", output.display());
    }
}
