#!/bin/bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
. "$repo_root/scripts/release-signing.sh"
output_root=${1:-"$repo_root/target/release-validation"}
archive_path="$output_root/Souvenirs.xcarchive"
derived_data="$output_root/DerivedData"
timestamp_mode=${SOUVENIRS_TIMESTAMP:-auto}

identity=${SOUVENIRS_SIGNING_IDENTITY:-}
if [[ -z "$identity" ]]; then
    identities=$(
        security find-identity -v -p codesigning |
            sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p'
    )
    if [[ $(printf '%s\n' "$identities" | sed '/^$/d' | wc -l | tr -d ' ') -ne 1 ]]; then
        echo "set SOUVENIRS_SIGNING_IDENTITY to one Developer ID Application identity" >&2
        exit 1
    fi
    identity=$identities
fi

is_developer_id_application_identity "$identity" || {
    echo "SOUVENIRS_SIGNING_IDENTITY must name a Developer ID Application identity" >&2
    exit 1
}

if [[ "$identity" =~ \(([A-Z0-9]{10})\)$ ]]; then
    team_id=${BASH_REMATCH[1]}
else
    echo "cannot read the team identifier from the signing identity" >&2
    exit 1
fi

rm -rf "$archive_path" "$derived_data"
mkdir -p "$output_root"

case "$timestamp_mode" in
    auto)
        timestamp_flag=--timestamp
        ;;
    none)
        timestamp_flag=--timestamp=none
        ;;
    *)
        echo "SOUVENIRS_TIMESTAMP must be auto or none" >&2
        exit 1
        ;;
esac

xcodebuild archive \
    -project "$repo_root/macos/Souvenirs.xcodeproj" \
    -scheme Souvenirs \
    -configuration Release \
    -destination 'generic/platform=macOS' \
    -archivePath "$archive_path" \
    -derivedDataPath "$derived_data" \
    CODE_SIGN_STYLE=Manual \
    CODE_SIGN_IDENTITY="$identity" \
    DEVELOPMENT_TEAM="$team_id" \
    OTHER_CODE_SIGN_FLAGS="$timestamp_flag"

app="$archive_path/Products/Applications/Souvenirs.app"
if [[ "$timestamp_mode" == none ]]; then
    "$repo_root/scripts/verify-release.sh" --allow-no-timestamp "$app"
else
    "$repo_root/scripts/verify-release.sh" "$app"
fi
echo "$app"
