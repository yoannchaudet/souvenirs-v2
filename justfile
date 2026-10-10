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

# Regenerate the committed engine protocol schema.
protocol-schema:
    cargo run -p souvenirs-protocol --example generate_schema

# Check formatting, static analysis, and tests.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    if [[ "$(uname -s)" == "Darwin" ]]; then just test-core-performance; fi
    just test-rust
