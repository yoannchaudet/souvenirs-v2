#!/bin/bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/souvenirs-storage.XXXXXX")
attached_mount=
attached_device=

cleanup() {
    if [[ -n "$attached_mount" && -d "$attached_mount" ]]; then
        hdiutil detach "$attached_mount" -quiet ||
            hdiutil detach "$attached_mount" -force -quiet ||
            true
    fi
    if [[ -n "$attached_device" ]]; then
        hdiutil detach "$attached_device" -quiet ||
            hdiutil detach "$attached_device" -force -quiet ||
            true
    fi
    if [[ -n "$attached_mount" && -d "$attached_mount" ]]; then
        local local_mount
        local_mount=$(cd "$attached_mount" && pwd -P)
        if mount | awk -v target="$local_mount" '$3 == target {found = 1} END {exit !found}'; then
            echo "refusing to remove a work directory that still contains a mounted image" >&2
            return
        fi
    fi
    rm -rf "$work_directory"
}
trap cleanup EXIT

validate_image() {
    local filesystem=$1
    local label=$2
    local image="$work_directory/$label.dmg"
    local mount_point="$work_directory/$label"

    mkdir "$mount_point"
    if [[ "$filesystem" == ExFAT ]]; then
        local attachment="$work_directory/exfat-attachment.txt"
        hdiutil create -quiet -size 256m -layout MBRSPUD -fs 'MS-DOS FAT32' \
            -volname Souvenirs-base "$image"
        hdiutil attach -nomount "$image" >"$attachment"
        local disk
        local partition
        disk=$(awk 'NR == 1 {print $1}' "$attachment")
        partition=$(awk 'END {print $1}' "$attachment")
        attached_device=$disk
        diskutil eraseVolume ExFAT SOUVENIRS "$partition" >/dev/null
        hdiutil detach "$disk" -quiet
        attached_device=
    else
        hdiutil create -quiet -size 256m -fs "$filesystem" -volname "Souvenirs-$label" "$image"
    fi
    hdiutil attach "$image" -quiet -nobrowse -mountpoint "$mount_point"
    attached_mount=$mount_point

    SOUVENIRS_STORAGE_TEST_ROOT="$mount_point" \
        cargo test --manifest-path "$repo_root/Cargo.toml" \
        -p souvenirs-storage \
        --features test-failpoints \
        --test representative_storage \
        -- --nocapture

    hdiutil detach "$mount_point" -quiet
    attached_mount=
}

validate_image APFS apfs
validate_image ExFAT exfat
