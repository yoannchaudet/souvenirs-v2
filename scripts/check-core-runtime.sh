#!/usr/bin/env bash
set -euo pipefail

target_seconds="${CORE_SUITE_TARGET_SECONDS:-5}"

cargo test -p souvenirs-domain -p souvenirs-metadata -p souvenirs-application --no-run --quiet

started_at="$(python3 -c 'import time; print(time.monotonic_ns())')"
cargo test -p souvenirs-domain -p souvenirs-metadata -p souvenirs-application --quiet
finished_at="$(python3 -c 'import time; print(time.monotonic_ns())')"

elapsed_seconds="$(
  python3 - "$started_at" "$finished_at" <<'PY'
import sys

started_at = int(sys.argv[1])
finished_at = int(sys.argv[2])
print(f"{(finished_at - started_at) / 1_000_000_000:.3f}")
PY
)"

printf 'Core suite runtime: %ss (target: <%ss)\n' "$elapsed_seconds" "$target_seconds"

if [[ "$(uname -s)" == "Darwin" && "${CI:-false}" != "true" ]]; then
  python3 - "$elapsed_seconds" "$target_seconds" <<'PY'
import sys

elapsed = float(sys.argv[1])
target = float(sys.argv[2])
if elapsed >= target:
    raise SystemExit(
        f"core suite took {elapsed:.3f}s; expected less than {target:.3f}s on a development Mac"
    )
PY
else
  printf 'Runtime is informational on this host to avoid environment-sensitive CI failures.\n'
fi
