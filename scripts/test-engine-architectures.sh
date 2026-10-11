#!/bin/bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/souvenirs-engine-architectures.XXXXXX")
trap 'rm -rf "$fixture_root"' EXIT

application="$fixture_root/Souvenirs"
engine="$fixture_root/souvenirs-engine"
touch "$application" "$engine"

cat >"$fixture_root/lipo" <<'EOF'
#!/bin/bash
set -euo pipefail
[[ "$1" == -archs && $# -eq 2 ]] || exit 2
case "$(basename "$2")" in
    Souvenirs)
        echo "${FIXTURE_APPLICATION_ARCHITECTURES:?}"
        ;;
    souvenirs-engine)
        echo "${FIXTURE_ENGINE_ARCHITECTURES:?}"
        ;;
    *)
        exit 2
        ;;
esac
EOF
chmod +x "$fixture_root/lipo"

SOUVENIRS_LIPO="$fixture_root/lipo" \
    FIXTURE_APPLICATION_ARCHITECTURES="x86_64 arm64" \
    FIXTURE_ENGINE_ARCHITECTURES="arm64 x86_64" \
    "$repo_root/scripts/engine-architectures.sh" verify "$application" "$engine"

if SOUVENIRS_LIPO="$fixture_root/lipo" \
    FIXTURE_APPLICATION_ARCHITECTURES="x86_64" \
    FIXTURE_ENGINE_ARCHITECTURES="arm64" \
    "$repo_root/scripts/engine-architectures.sh" verify "$application" "$engine" \
    >"$fixture_root/mismatch.stdout" 2>"$fixture_root/mismatch.stderr"; then
    echo "architecture mismatch unexpectedly passed verification" >&2
    exit 1
fi
grep -q '^application: x86_64$' "$fixture_root/mismatch.stderr"
grep -q '^engine: arm64$' "$fixture_root/mismatch.stderr"

[[ $("$repo_root/scripts/engine-architectures.sh" rust-target arm64) == aarch64-apple-darwin ]]
[[ $("$repo_root/scripts/engine-architectures.sh" rust-target x86_64) == x86_64-apple-darwin ]]
if "$repo_root/scripts/engine-architectures.sh" rust-target powerpc \
    >"$fixture_root/unsupported.stdout" 2>"$fixture_root/unsupported.stderr"; then
    echo "unsupported architecture unexpectedly mapped to a Rust target" >&2
    exit 1
fi
grep -q 'unsupported Xcode architecture.*powerpc' "$fixture_root/unsupported.stderr"

echo "engine architecture tests passed"
