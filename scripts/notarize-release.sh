#!/bin/bash
set -euo pipefail

usage() {
    echo "usage: SOUVENIRS_NOTARY_PROFILE=profile $0 APP [OUTPUT_DIRECTORY]" >&2
    exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
[[ -n "${SOUVENIRS_NOTARY_PROFILE:-}" ]] || usage

repo_root=$(cd "$(dirname "$0")/.." && pwd)
app=$1
output_directory=${2:-"$(dirname "$app")"}
archive="$output_directory/Souvenirs-notarization.zip"
distribution_archive="$output_directory/Souvenirs-notarized.zip"

"$repo_root/scripts/verify-release.sh" "$app"
rm -f "$archive" "$distribution_archive"
ditto -c -k --keepParent "$app" "$archive"
submission=$(xcrun notarytool submit "$archive" \
    --keychain-profile "$SOUVENIRS_NOTARY_PROFILE" \
    --wait \
    --output-format json)
printf '%s\n' "$submission"
status=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' <<<"$submission")
[[ "$status" == Accepted ]] || {
    echo "notarization failed with status: $status" >&2
    exit 1
}
xcrun stapler staple "$app"
"$repo_root/scripts/verify-release.sh" --notarized "$app"
rm -f "$archive"
ditto -c -k --keepParent "$app" "$distribution_archive"

echo "notarized release archive: $distribution_archive"
