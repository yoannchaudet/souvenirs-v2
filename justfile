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
    cargo test --workspace --all-targets --all-features

# Run macOS state and unit tests without launching UI automation.
test-swift:
    xcodebuild test \
        -project macos/Souvenirs.xcodeproj \
        -scheme SouvenirsStateTests \
        -configuration Debug \
        -destination 'platform=macOS' \
        -derivedDataPath "${SWIFT_DERIVED_DATA_PATH:-${TMPDIR:-/tmp}/SouvenirsStateDerivedData}" \
        -only-testing:SouvenirsTests \
        CODE_SIGN_IDENTITY=- \
        DEVELOPMENT_TEAM=

# Run macOS UI tests separately from the fast local gate.
test-ui:
    xcodebuild test \
        -project macos/Souvenirs.xcodeproj \
        -scheme Souvenirs \
        -configuration Debug \
        -destination 'platform=macOS' \
        -derivedDataPath "${SWIFT_DERIVED_DATA_PATH:-${TMPDIR:-/tmp}/SouvenirsUIDerivedData}" \
        -only-testing:SouvenirsUITests \
        CODE_SIGN_IDENTITY=- \
        DEVELOPMENT_TEAM=

# Validate Xcode-to-Rust architecture mapping and mismatch rejection.
test-engine-architectures:
    ./scripts/test-engine-architectures.sh

# Validate interruption recovery on disposable APFS and exFAT disk images.
test-representative-storage:
    ./scripts/validate-representative-storage.sh

# Validate interruption recovery on a disposable local SMB share.
test-smb-storage:
    ./scripts/validate-smb-storage.sh

# Build and verify a Developer ID signed Release archive.
build-release:
    ./scripts/build-release.sh

# Test release signing policy with deterministic codesign output.
test-release-signing:
    ./scripts/test-release-signing.sh

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
    just test-engine-architectures
    just test-release-signing
    if [[ "$(uname -s)" == "Darwin" ]]; then just test-core-performance; fi
    if [[ "$(uname -s)" == "Darwin" ]]; then just test-swift; fi
    just test-rust
