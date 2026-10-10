set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

# Run the fast, process-free core suite.
test-core:
    cargo test -p souvenirs-domain -p souvenirs-metadata -p souvenirs-application

# Build once, measure the warmed core suite, and enforce the macOS development target.
test-core-performance:
    ./scripts/check-core-runtime.sh

# Run every Rust test target.
test-rust:
    cargo test --workspace --all-targets

# Validate interruption recovery on disposable APFS and exFAT disk images.
test-representative-storage:
    ./scripts/validate-representative-storage.sh

# Validate interruption recovery on a disposable local SMB share.
test-smb-storage:
    ./scripts/validate-smb-storage.sh

# Build and verify a Developer ID signed Release archive.
build-release:
    ./scripts/build-release.sh

# Regenerate the committed engine protocol schema.
protocol-schema:
    cargo run -p souvenirs-protocol --example generate_schema

# Regenerate the committed import plan schema.
import-plan-schema:
    cargo run -p souvenirs-cli --example generate_import_plan_schema

# Check formatting, static analysis, and tests.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    if [[ "$(uname -s)" == "Darwin" ]]; then just test-core-performance; fi
    just test-rust
