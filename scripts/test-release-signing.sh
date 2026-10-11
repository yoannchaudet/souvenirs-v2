#!/bin/bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
. "$repo_root/scripts/release-signing.sh"
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/souvenirs-release-signing.XXXXXX")
trap 'rm -rf "$work_directory"' EXIT

app="$work_directory/Souvenirs.app"
app_executable="$app/Contents/MacOS/Souvenirs"
engine="$app/Contents/MacOS/souvenirs-engine"
mkdir -p "$(dirname "$engine")" "$work_directory/bin"
touch "$app_executable" "$engine"
chmod +x "$app_executable" "$engine"

cat >"$work_directory/bin/codesign" <<'EOF'
#!/bin/bash
set -euo pipefail

if [[ "$1" == --verify ]]; then
    exit 0
fi
[[ "$1" == -dvvv ]] || exit 2

target=${!#}
kind=${APP_SIGNING_KIND:-developer-id}
if [[ "$target" == */souvenirs-engine ]]; then
    kind=${ENGINE_SIGNING_KIND:-developer-id}
fi

case "$kind" in
    developer-id)
        cat <<'DETAILS'
Executable=/tmp/Souvenirs
Identifier=com.example.Souvenirs
CodeDirectory v=20500 size=780 flags=0x10000(runtime) hashes=14+7 location=embedded
Signature size=9055
Authority=Developer ID Application: Example Developer (ABCDE12345)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Oct 10, 2026 at 10:00:00
TeamIdentifier=ABCDE12345
DETAILS
        ;;
    apple-development)
        cat <<'DETAILS'
Executable=/tmp/Souvenirs
Identifier=com.example.Souvenirs
CodeDirectory v=20500 size=780 flags=0x10000(runtime) hashes=14+7 location=embedded
Signature size=4788
Authority=Apple Development: Example Developer (ABCDE12345)
Authority=Apple Worldwide Developer Relations Certification Authority
Authority=Apple Root CA
Timestamp=Oct 10, 2026 at 10:00:00
TeamIdentifier=ABCDE12345
DETAILS
        ;;
    adhoc)
        cat <<'DETAILS'
Executable=/tmp/Souvenirs
Identifier=com.example.Souvenirs
CodeDirectory v=20500 size=780 flags=0x10002(adhoc,runtime) hashes=14+7 location=embedded
Signature=adhoc
TeamIdentifier=not set
DETAILS
        ;;
    *)
        exit 2
        ;;
esac
EOF
chmod +x "$work_directory/bin/codesign"

verify="$repo_root/scripts/verify-release.sh"
test_path="$work_directory/bin:$PATH"

expect_success() {
    local name=$1
    shift
    if ! output=$("$@" 2>&1); then
        printf 'FAIL: %s\n%s\n' "$name" "$output" >&2
        exit 1
    fi
    printf 'PASS: %s\n' "$name"
}

expect_failure() {
    local name=$1
    local expected=$2
    shift 2
    if output=$("$@" 2>&1); then
        printf 'FAIL: %s unexpectedly succeeded\n%s\n' "$name" "$output" >&2
        exit 1
    fi
    grep -Fq "$expected" <<<"$output" || {
        printf 'FAIL: %s did not report %q\n%s\n' "$name" "$expected" "$output" >&2
        exit 1
    }
    printf 'PASS: %s\n' "$name"
}

is_developer_id_application_identity \
    "Developer ID Application: Example Developer (ABCDE12345)" || {
    echo "FAIL: Developer ID identity was rejected" >&2
    exit 1
}
if is_developer_id_application_identity \
    "Apple Development: Example Developer (ABCDE12345)"; then
    echo "FAIL: Apple Development identity was accepted" >&2
    exit 1
fi
echo "PASS: explicit signing identity policy"

expect_success \
    "Developer ID app and engine" \
    env PATH="$test_path" APP_SIGNING_KIND=developer-id ENGINE_SIGNING_KIND=developer-id \
    "$verify" "$app"

expect_failure \
    "Apple Development app" \
    "app signature leaf authority is not Developer ID Application" \
    env PATH="$test_path" APP_SIGNING_KIND=apple-development ENGINE_SIGNING_KIND=developer-id \
    "$verify" "$app"

expect_failure \
    "Apple Development engine" \
    "engine signature leaf authority is not Developer ID Application" \
    env PATH="$test_path" APP_SIGNING_KIND=developer-id ENGINE_SIGNING_KIND=apple-development \
    "$verify" "$app"

expect_failure \
    "ad hoc release" \
    "release contains an ad hoc signature" \
    env PATH="$test_path" APP_SIGNING_KIND=adhoc ENGINE_SIGNING_KIND=adhoc \
    "$verify" "$app"

expect_success \
    "ad hoc CI build" \
    env PATH="$test_path" APP_SIGNING_KIND=adhoc ENGINE_SIGNING_KIND=adhoc \
    "$verify" --allow-adhoc "$app"

override_fixture="$app/Contents/Frameworks/EngineSupport"
mkdir -p "$(dirname "$override_fixture")"
printf '%s\n' "SOUVENIRS_ENGINE_EXECUTABLE" >"$override_fixture"
expect_failure \
    "development engine override" \
    "release app contains development engine override: SOUVENIRS_ENGINE_EXECUTABLE" \
    env PATH="$test_path" APP_SIGNING_KIND=adhoc ENGINE_SIGNING_KIND=adhoc \
    "$verify" --allow-adhoc "$app"
rm "$override_fixture"

expect_failure \
    "explicit Apple Development identity" \
    "SOUVENIRS_SIGNING_IDENTITY must name a Developer ID Application identity" \
    env SOUVENIRS_SIGNING_IDENTITY="Apple Development: Example Developer (ABCDE12345)" \
    "$repo_root/scripts/build-release.sh" "$work_directory/release"
