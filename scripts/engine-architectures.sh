#!/bin/bash
set -euo pipefail

usage() {
    echo "usage: $0 build PROFILE OUTPUT ARCH..." >&2
    echo "       $0 verify APPLICATION_EXECUTABLE ENGINE_EXECUTABLE" >&2
    echo "       $0 rust-target XCODE_ARCHITECTURE" >&2
    exit 2
}

lipo_command=${SOUVENIRS_LIPO:-lipo}

rust_target() {
    case "$1" in
        arm64)
            echo aarch64-apple-darwin
            ;;
        x86_64)
            echo x86_64-apple-darwin
            ;;
        *)
            echo "unsupported Xcode architecture for the Rust engine: $1" >&2
            return 1
            ;;
    esac
}

normalized_architectures() {
    printf '%s\n' "$@" | sed '/^$/d' | LC_ALL=C sort -u | paste -sd ' ' -
}

binary_architectures() {
    local binary=$1
    local architectures
    local normalized
    architectures=$("$lipo_command" -archs "$binary") || {
        echo "cannot inspect Mach-O architectures: $binary" >&2
        return 1
    }
    # lipo separates architecture names with spaces.
    # shellcheck disable=SC2086
    normalized=$(normalized_architectures $architectures)
    [[ -n "$normalized" ]] || {
        echo "Mach-O binary does not contain any architectures: $binary" >&2
        return 1
    }
    echo "$normalized"
}

verify_architectures() {
    local application=$1
    local engine=$2
    local application_architectures
    local engine_architectures

    application_architectures=$(binary_architectures "$application")
    engine_architectures=$(binary_architectures "$engine")
    if [[ "$application_architectures" != "$engine_architectures" ]]; then
        echo "application and embedded engine architectures differ" >&2
        echo "application: $application_architectures" >&2
        echo "engine: $engine_architectures" >&2
        return 1
    fi
}

build_engine() {
    local profile=$1
    local output=$2
    shift 2
    [[ $# -gt 0 ]] || {
        echo "Xcode did not select any architectures for the Rust engine" >&2
        return 1
    }
    [[ "$profile" == debug || "$profile" == release ]] || {
        echo "unsupported Cargo profile for the Rust engine: $profile" >&2
        return 1
    }

    local script_dir
    local repo_root
    local cargo_target_dir
    local architecture
    local target
    local slice
    local selected_architectures
    local -a architectures=()
    local -a slices=()
    local -a cargo_arguments=(build --locked --package souvenirs-engine --bin souvenirs-engine)

    selected_architectures=$(normalized_architectures "$@")
    read -r -a architectures <<<"$selected_architectures"
    script_dir=$(cd "$(dirname "$0")" && pwd)
    repo_root=$(cd "$script_dir/.." && pwd)
    cargo_target_dir=${CARGO_TARGET_DIR:-"$repo_root/target"}
    if [[ "$cargo_target_dir" != /* ]]; then
        cargo_target_dir="$repo_root/$cargo_target_dir"
    fi
    if [[ "$profile" == release ]]; then
        cargo_arguments+=(--release)
    fi

    cd "$repo_root"
    for architecture in "${architectures[@]}"; do
        target=$(rust_target "$architecture")
        cargo "${cargo_arguments[@]}" --target "$target"
        slice="$cargo_target_dir/$target/$profile/souvenirs-engine"
        [[ -x "$slice" ]] || {
            echo "Cargo did not produce the expected engine: $slice" >&2
            return 1
        }
        slices+=("$slice")
    done

    if [[ "$profile" == debug ]]; then
        cargo build --locked --package souvenirs-engine --bin souvenirs-fake-engine
    fi

    mkdir -p "$(dirname "$output")"
    if [[ ${#slices[@]} -eq 1 ]]; then
        install -m 755 "${slices[0]}" "$output"
    else
        "$lipo_command" -create "${slices[@]}" -output "$output"
        chmod 755 "$output"
    fi

    local expected_architectures
    local built_architectures
    expected_architectures=$selected_architectures
    built_architectures=$(binary_architectures "$output")
    if [[ "$expected_architectures" != "$built_architectures" ]]; then
        echo "Rust engine architectures do not match the Xcode selection" >&2
        echo "Xcode: $expected_architectures" >&2
        echo "engine: $built_architectures" >&2
        return 1
    fi
}

[[ $# -gt 0 ]] || usage
command=$1
shift
case "$command" in
    build)
        [[ $# -ge 3 ]] || usage
        build_engine "$@"
        ;;
    verify)
        [[ $# -eq 2 ]] || usage
        verify_architectures "$@"
        ;;
    rust-target)
        [[ $# -eq 1 ]] || usage
        rust_target "$1"
        ;;
    *)
        usage
        ;;
esac
