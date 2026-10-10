set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

# Run the fast, process-free core suite.
test-core:
    cargo test -p souvenirs-domain -p souvenirs-application

# Run every Rust test target.
test-rust:
    cargo test --workspace --all-targets

# Check formatting, static analysis, and tests.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    just test-rust
