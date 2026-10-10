#!/bin/bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
image=${SOUVENIRS_SMB_IMAGE:-dockurr/samba:4.23.10}
container="souvenirs-smb-$RANDOM-$$"
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/souvenirs-smb.XXXXXX")
share_directory="$work_directory/share"
mount_point="$work_directory/mount"
username=souvenirs
password="souvenirs-$RANDOM-$$"

cleanup() {
    umount "$mount_point" >/dev/null 2>&1 || true
    docker rm -f "$container" >/dev/null 2>&1 || true
    rm -rf "$work_directory"
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "Docker is required for the disposable SMB harness" >&2; exit 1; }
docker info >/dev/null
mkdir "$share_directory" "$mount_point"
chmod 777 "$share_directory"

docker run --detach --rm \
    --name "$container" \
    --publish 127.0.0.1:445:445 \
    --env "NAME=Souvenirs" \
    --env "USER=$username" \
    --env "PASS=$password" \
    --env RW=true \
    --volume "$share_directory:/shared" \
    "$image" >/dev/null

for _ in {1..30}; do
    if smbutil view "//$username:$password@127.0.0.1" >/dev/null 2>&1; then
        break
    fi
    sleep 1
done
smbutil view "//$username:$password@127.0.0.1" >/dev/null
mount_smbfs "//$username:$password@127.0.0.1/Souvenirs" "$mount_point"

SOUVENIRS_STORAGE_TEST_ROOT="$mount_point" \
    cargo test --manifest-path "$repo_root/Cargo.toml" \
    -p souvenirs-storage \
    --features test-failpoints \
    --test representative_storage \
    -- --nocapture
