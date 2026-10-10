#!/bin/bash
set -euo pipefail

usage() {
    echo "usage: $0 [--allow-adhoc] [--allow-no-timestamp] [--notarized] APP" >&2
    exit 2
}

allow_adhoc=false
allow_no_timestamp=false
require_notarized=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --allow-adhoc)
            allow_adhoc=true
            shift
            ;;
        --allow-no-timestamp)
            allow_no_timestamp=true
            shift
            ;;
        --notarized)
            require_notarized=true
            shift
            ;;
        -*)
            usage
            ;;
        *)
            break
            ;;
    esac
done
[[ $# -eq 1 ]] || usage

app=$1
engine="$app/Contents/MacOS/souvenirs-engine"
[[ -d "$app" ]] || { echo "app bundle not found: $app" >&2; exit 1; }
[[ -x "$engine" ]] || { echo "embedded engine not found: $engine" >&2; exit 1; }

codesign --verify --strict --verbose=2 "$engine"
codesign --verify --deep --strict --verbose=2 "$app"

app_details=$(codesign -dvvv "$app" 2>&1)
engine_details=$(codesign -dvvv "$engine" 2>&1)
grep -Eq 'flags=.*runtime' <<<"$app_details" || {
    echo "app signature does not enable Hardened Runtime" >&2
    exit 1
}
grep -Eq 'flags=.*runtime' <<<"$engine_details" || {
    echo "engine signature does not enable Hardened Runtime" >&2
    exit 1
}

app_team=$(sed -n 's/^TeamIdentifier=//p' <<<"$app_details")
engine_team=$(sed -n 's/^TeamIdentifier=//p' <<<"$engine_details")
if [[ "$allow_adhoc" == false ]]; then
    [[ -n "$app_team" && "$app_team" != "not set" ]] || {
        echo "app is not signed with a team identity" >&2
        exit 1
    }
    [[ -n "$engine_team" && "$engine_team" != "not set" ]] || {
        echo "engine is not signed with a team identity" >&2
        exit 1
    }
    ! grep -Eq '(^Signature=adhoc|flags=.*adhoc)' <<<"$app_details"$'\n'"$engine_details" || {
        echo "release contains an ad hoc signature" >&2
        exit 1
    }
    [[ "$engine_team" == "$app_team" ]] || {
        echo "app and embedded engine use different signing teams" >&2
        exit 1
    }
    if [[ "$allow_no_timestamp" == false ]]; then
        grep -q '^Timestamp=' <<<"$app_details" || {
            echo "app signature does not contain a secure timestamp" >&2
            exit 1
        }
        grep -q '^Timestamp=' <<<"$engine_details" || {
            echo "engine signature does not contain a secure timestamp" >&2
            exit 1
        }
    fi
fi

if [[ "$require_notarized" == true ]]; then
    xcrun stapler validate "$app"
    spctl --assess --type execute --verbose=2 "$app"
fi

echo "verified release app: $app"
