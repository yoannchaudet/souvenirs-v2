#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional


MEDIA_COUNT = 512
SIDECAR_COUNT = 128
UNKNOWN_COUNT = 64
CATALOG_MANIFEST_COUNT = 8
CATALOG_FILE_COUNT = 32
CATALOG_DUPLICATE_COUNT = 32
SOURCE_DUPLICATE_GROUP_COUNT = 32
FILE_SIZE_BYTES = 4096
WORKLOAD_VERSION = 1
DEFAULT_SAMPLES = 5


def deterministic_bytes(key: str, size: int = FILE_SIZE_BYTES) -> bytes:
    seed = hashlib.sha256(key.encode("utf-8")).digest()
    repeats = (size + len(seed) - 1) // len(seed)
    return (seed * repeats)[:size]


def write_file(path: Path, contents: bytes) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents)
    return len(contents)


def run_json(command: list[str]) -> dict:
    completed = subprocess.run(command, check=False, capture_output=True, text=True)
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n"
            f"{completed.stderr}"
        )
    if completed.stderr:
        raise RuntimeError(f"command wrote diagnostics: {' '.join(command)}\n{completed.stderr}")
    return json.loads(completed.stdout)


def timed_json(command: list[str]) -> tuple[float, dict]:
    started_at = time.monotonic_ns()
    result = run_json(command)
    finished_at = time.monotonic_ns()
    return (finished_at - started_at) / 1_000_000, result


def media_contents(index: int) -> bytes:
    if index < CATALOG_DUPLICATE_COUNT:
        return deterministic_bytes(f"catalog-duplicate-{index:04d}")
    if index < CATALOG_DUPLICATE_COUNT + SOURCE_DUPLICATE_GROUP_COUNT * 2:
        pair = (index - CATALOG_DUPLICATE_COUNT) // 2
        return deterministic_bytes(f"source-duplicate-{pair:04d}")
    return deterministic_bytes(f"unique-media-{index:04d}")


def create_source(source: Path) -> dict:
    extensions = ["jpg", "heic", "dng", "cr3", "nef", "arw", "mov", "mp4"]
    base_time = datetime(2026, 1, 1, 12, 0, 0)
    total_bytes = 0
    media_paths = []

    for index in range(MEDIA_COUNT):
        timestamp = (base_time + timedelta(seconds=index)).strftime("%Y%m%d_%H%M%S")
        extension = extensions[index % len(extensions)]
        relative = Path(f"batch-{index % 16:02d}") / f"IMG_{timestamp}.{extension}"
        total_bytes += write_file(source / relative, media_contents(index))
        media_paths.append(relative)

    for index, media_path in enumerate(media_paths[:SIDECAR_COUNT]):
        sidecar = source / f"{media_path}.xmp"
        timestamp = (base_time + timedelta(seconds=index)).isoformat()
        contents = (
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
            f"<x:xmpmeta><exif:DateTimeOriginal>{timestamp}</exif:DateTimeOriginal>"
            "</x:xmpmeta>\n"
        ).encode("utf-8")
        total_bytes += write_file(sidecar, contents)

    for index in range(UNKNOWN_COUNT):
        relative = Path(f"batch-{index % 16:02d}") / f"notes-{index:04d}.txt"
        total_bytes += write_file(
            source / relative, deterministic_bytes(f"unknown-{index:04d}", 512)
        )

    return {
        "files": MEDIA_COUNT + SIDECAR_COUNT + UNKNOWN_COUNT,
        "media": MEDIA_COUNT,
        "sidecars": SIDECAR_COUNT,
        "unknown": UNKNOWN_COUNT,
        "bytes": total_bytes,
        "catalog_duplicates": CATALOG_DUPLICATE_COUNT,
        "source_duplicate_groups": SOURCE_DUPLICATE_GROUP_COUNT,
    }


def create_catalog(binary: Path, catalog: Path) -> dict:
    catalog.mkdir()
    run_json([str(binary), "catalog", "initialize", "--catalog", str(catalog)])
    total_bytes = 0

    for collection_index in range(CATALOG_MANIFEST_COUNT):
        collection = catalog / "2025" / f"2025-01-{collection_index + 1:02d} Baseline"
        collection.mkdir(parents=True)
        manifest_lines = []
        for file_offset in range(4):
            index = collection_index * 4 + file_offset
            filename = f"catalog-{index:04d}.jpg"
            contents = deterministic_bytes(f"catalog-duplicate-{index:04d}")
            total_bytes += write_file(collection / filename, contents)
            digest = hashlib.sha256(contents).hexdigest()
            manifest_lines.append(f"{digest}  {filename}\n")
        manifest = "".join(manifest_lines).encode("utf-8")
        total_bytes += write_file(collection / "SHA256SUMS", manifest)

    return {
        "manifests": CATALOG_MANIFEST_COUNT,
        "files": CATALOG_FILE_COUNT,
        "bytes": total_bytes,
    }


def tree_digest(roots: list[Path]) -> str:
    digest = hashlib.sha256()
    for root in roots:
        for path in sorted(path for path in root.rglob("*") if path.is_file()):
            relative = path.relative_to(root)
            digest.update(root.name.encode("utf-8"))
            digest.update(b"\0")
            digest.update(relative.as_posix().encode("utf-8"))
            digest.update(b"\0")
            digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def workload_identity(source: dict, catalog: dict, contents_digest: str) -> str:
    encoded = json.dumps(
        {
            "version": WORKLOAD_VERSION,
            "source": source,
            "catalog": catalog,
            "file_size_bytes": FILE_SIZE_BYTES,
            "contents_digest": contents_digest,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def command_output(command: list[str]) -> Optional[str]:
    completed = subprocess.run(command, check=False, capture_output=True, text=True)
    if completed.returncode != 0:
        return None
    return completed.stdout.strip()


def filesystem_type(temporary_root: Path) -> str:
    if platform.system() == "Darwin":
        disk_usage = command_output(["df", "-P", str(temporary_root)])
        if disk_usage:
            mount_point = disk_usage.splitlines()[-1].split()[-1]
            disk_info = command_output(["diskutil", "info", mount_point])
            if disk_info:
                for line in disk_info.splitlines():
                    if "File System Personality:" in line:
                        return line.split(":", 1)[1].strip()
    else:
        detected = command_output(["stat", "-f", "-c", "%T", str(temporary_root)])
        if detected:
            return detected
    return "unknown"


def host_conditions(temporary_root: Path) -> dict:
    if platform.system() == "Darwin":
        machine_model = command_output(["sysctl", "-n", "hw.model"])
    else:
        machine_model = None
    return {
        "os": platform.platform(),
        "machine": platform.machine(),
        "machine_model": machine_model or "unknown",
        "python": platform.python_version(),
        "rust_toolchain": command_output(["rustc", "--version"]) or "unknown",
        "filesystem": filesystem_type(temporary_root),
        "storage": "temporary directory",
        "profile": "release",
        "warmed": True,
    }


def summarize(
    operation: str,
    scope: str,
    durations_ms: list[float],
    workload: dict,
    conditions: dict,
    operation_result: dict,
) -> dict:
    return {
        "operation": operation,
        "scope": scope,
        "duration_ms": {
            "median": round(statistics.median(durations_ms), 3),
            "minimum": round(min(durations_ms), 3),
            "maximum": round(max(durations_ms), 3),
            "samples": len(durations_ms),
        },
        "workload": workload,
        "conditions": conditions,
        "result": operation_result,
        "informational": True,
    }


def main() -> int:
    samples = int(os.environ.get("IMPORT_PERF_SAMPLES", DEFAULT_SAMPLES))
    if samples < 1:
        raise ValueError("IMPORT_PERF_SAMPLES must be at least 1")

    repository = Path(__file__).resolve().parent.parent
    binary_override = os.environ.get("SOUVENIRS_CLI_BIN")
    if binary_override:
        binary = Path(binary_override).resolve()
    else:
        subprocess.run(
            ["cargo", "build", "-p", "souvenirs-cli", "--release", "--quiet"],
            cwd=repository,
            check=True,
        )
        binary = repository / "target" / "release" / "souvenirs-cli"

    with tempfile.TemporaryDirectory(prefix="souvenirs-import-performance-") as directory:
        root = Path(directory)
        source = root / "source"
        source.mkdir()
        source_size = create_source(source)
        catalog_size = create_catalog(binary, root / "catalog")
        contents_digest = tree_digest([source, root / "catalog" / "2025"])
        workload = {
            "version": WORKLOAD_VERSION,
            "id": workload_identity(source_size, catalog_size, contents_digest),
            "contents_digest": contents_digest,
            "source": source_size,
            "catalog": catalog_size,
        }
        conditions = host_conditions(root)
        scan_durations = []
        plan_durations = []
        scan_result = None
        plan_result = None

        for sample in range(samples + 1):
            snapshot = root / f"scan-{sample}.json"
            plan = root / f"plan-{sample}.json"
            scan_duration, scan_envelope = timed_json(
                [
                    str(binary),
                    "import",
                    "scan",
                    "--source",
                    str(source),
                    "--output",
                    str(snapshot),
                ]
            )
            scan_result = scan_envelope["result"]
            observed_scan = {
                "files": scan_result["file_count"],
                "media": scan_result["media_count"],
                "sidecars": scan_result["sidecar_count"],
                "unknown": scan_result["unknown_count"],
            }
            expected_scan = {
                "files": source_size["files"],
                "media": source_size["media"],
                "sidecars": source_size["sidecars"],
                "unknown": source_size["unknown"],
            }
            if observed_scan != expected_scan:
                raise RuntimeError("scan result does not match the deterministic workload")

            plan_duration, plan_envelope = timed_json(
                [
                    str(binary),
                    "import",
                    "plan",
                    "--catalog",
                    str(root / "catalog"),
                    "--snapshot",
                    str(snapshot),
                    "--output",
                    str(plan),
                    "--collection",
                    "Performance",
                    "--destination-mode",
                    "create",
                    "--accepted-at",
                    "2026-01-01T12:00:00",
                    "--catalog-duplicates",
                    "skip",
                    "--source-duplicates",
                    "one-per-digest",
                ]
            )
            plan_result = plan_envelope["result"]
            observed_plan = {
                "items": plan_result["item_count"],
                "transfers": plan_result["transfer_count"],
                "unhandled": plan_result["unhandled_count"],
            }
            expected_plan = {
                "items": MEDIA_COUNT,
                "transfers": 448,
                "unhandled": UNKNOWN_COUNT,
            }
            if observed_plan != expected_plan:
                raise RuntimeError("plan result does not match the duplicate-aware workload")

            if sample > 0:
                scan_durations.append(scan_duration)
                plan_durations.append(plan_duration)

        print(
            json.dumps(
                summarize(
                    "import.scan",
                    "end-to-end scan command",
                    scan_durations,
                    workload,
                    conditions,
                    {
                        "files": scan_result["file_count"],
                        "media": scan_result["media_count"],
                        "sidecars": scan_result["sidecar_count"],
                        "unknown": scan_result["unknown_count"],
                    },
                ),
                sort_keys=True,
            )
        )
        print(
            json.dumps(
                summarize(
                    "import.plan",
                    "end-to-end plan command, including required source revalidation",
                    plan_durations,
                    workload,
                    conditions,
                    {
                        "items": plan_result["item_count"],
                        "transfers": plan_result["transfer_count"],
                        "unhandled": plan_result["unhandled_count"],
                        "required_space_bytes": plan_result["required_space_bytes"],
                    },
                ),
                sort_keys=True,
            )
        )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError, ValueError) as error:
        print(f"import performance measurement failed: {error}", file=sys.stderr)
        sys.exit(1)
