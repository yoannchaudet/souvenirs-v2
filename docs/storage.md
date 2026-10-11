# Storage and recovery

Souvenirs keeps tool state in `.souvenirs/`.
The durable record is the [catalog format](catalog.md): folders, filenames, and manifests.
Tool state is either rebuildable or limited to unfinished work and optional history.

| Path | Role | If lost |
| --- | --- | --- |
| `lock` | Exclusive writer lock. | Recreated on next write. |
| `journal/` | One plain-text journal per operation. | Unfinished operations lose guided recovery. History is lost. |
| `index.sqlite` | Rebuildable cache for search and duplicate lookup. Kept on the local computer for a NAS catalog. | Rebuilt from manifests. |
| `staging/` | Unverified copies in progress. | Nothing. Staged files are never authoritative. |

The safety model relies on one fact: imports copy and never delete sources.
Until an import finishes, its source is the backup.
The worst outcome of an interruption is therefore a repeated or cancelled import.

## Supported storage

| Storage | Writes | Notes |
| --- | --- | --- |
| Internal or attached drive | Yes | APFS, HFS+, or exFAT. |
| NAS over SMB | Yes | Persistence is confirmed by the NAS, then checked by [verification](#verified-copies). |
| Synchronized folders, such as iCloud Drive or Dropbox | No | They evict and rename files on their own. Reading is allowed. |

The catalog, staging area, and destinations must share one volume.
Sources can live anywhere readable.
Reading and integrity checks work on any storage, including backups.

Every published file, manifest, journal append, and directory change must be persisted before the next step.
Souvenirs uses `F_FULLFSYNC` where the volume supports it, and `fsync` otherwise.
A persistence failure stops the operation with a contextual error.

## Catalog initialization

Initialization creates a transient lock and journal in the catalog root before it creates catalog state.
A retry verifies the requested configuration and resumes valid state under the bootstrap lock.

## Lock

Every mutation holds `.souvenirs/lock`, a file created exclusively at the start and removed at the end.
It records the host, machine, boot, process ID, process start identity, and lock start time.
Only one mutating operation per catalog runs at a time, even across computers sharing a NAS.
Another writer must report busy and show the recorded holder.

A lock left by a crashed process on the same host is removed automatically.
Souvenirs compares the recorded boot and process start identity before it removes the lock.
Missing or unavailable identity evidence must not cause automatic removal.
Any other existing lock requires explicit user confirmation before removal.

## Verified copies

Every copied file, including sidecars and exports, follows these steps:

1. Create a new file exclusively in `staging/`, named after the operation and step.
2. Copy source bytes while computing SHA-256.
3. Persist the staged file, then read it back without the local cache (`F_NOCACHE`) and hash it again.
4. Compare the two digests. A mismatch stops the step.
5. Publish by rename without overwriting. Use `renamex_np(..., RENAME_EXCL)` where supported. Otherwise, confirm the target is free under the [lock](#lock), then rename.
6. Persist the destination directory.

Unverified bytes must never appear at a final path.
Originals publish before their sidecars.
After an item's originals publish, Souvenirs records them in the [manifest](#manifest-updates).

## Manifest updates

Souvenirs replaces a manifest atomically:

1. Read the current manifest and confirm it matches the version the plan read.
2. Write the new manifest to `staging/`, then persist it.
3. Rename it over `SHA256SUMS` and persist the directory.

A changed manifest at step 1 stops the operation for a new plan.
New entries use the digest computed during verified copy, without another full read.
Admitting an existing original in place requires reading the complete file once.

A datetime change renames an original and then updates its manifest line.
A folder rename changes no manifest, because manifests list only filenames.

## Journal

Each mutation writes `.souvenirs/journal/{yyyyMMdd-HHmmss}-{operation}.jsonl` before changing any file.
The first line records the confirmed plan.
Each completed step appends one persisted line.
The last line records the outcome: `completed` or `cancelled`.

The plan line records:

- Source paths and catalog-relative destinations.
- Accepted datetimes with their source, interpretation, and any batch correction.
- Explicit exclusions and reported duplicates.
- Expected digests where known.

A journal without an outcome is unfinished.
An unfinished journal blocks other mutations until recovery records an outcome.
Completed journals are optional history and can be archived or deleted.

## Interruption and recovery

Recovery moves forward.
It reads the unfinished journal and checks each unfinished step against the files:

| Observation | Action |
| --- | --- |
| A staged file exists without a recorded verification. | Delete it and copy again from the source. |
| A staged file has a recorded verification and matches its digest. | Publish it. |
| A published original is missing from its manifest. | Hash it, compare with the journal digest, then add it. |
| A rename's source path exists and its target is free. | Perform the rename. |
| A rename's target exists with the expected digest. | Mark the step done. |
| Anything else. | Stop and report. Never overwrite, delete, or guess. |

Recovery deletes only staging files that the journal names.
If a source is unavailable, the user can cancel the remaining copy steps.
Cancelling keeps published files, records them in manifests, removes owned staging files, and records `cancelled`.
A later import of the remaining files needs a new plan.

Renames cannot be cancelled halfway.
Recovery must finish them or stop and report the conflict.

## Index

`index.sqlite` caches manifest digests, paths, items, and extracted metadata.
It speeds up browsing, search, and [duplicate reporting](operations.md#duplicates).
It holds no information that the catalog format cannot reproduce.

For a catalog on a NAS, the index lives in the local user cache instead of `.souvenirs/`.
SQLite file locking is unreliable over network shares.
Each computer keeps its own index and rebuilds it when it is missing or outdated.

A missing, outdated, or corrupt index is deleted and rebuilt.
Rebuilding reads manifests and filenames.
It does not rehash media.
Mutations must update the index after the matching manifest change.

## Integrity checks

An integrity check verifies selected collection directories against their manifests.
It reports checked scope, missing originals, changed originals, unadmitted originals, and unreadable files.
Partial coverage must not appear as a full check.
A check changes nothing.

Integrity checks work on any copy of the catalog, including backups and network storage.
Recovering a changed original means restoring it from a backup that passes its manifest.

## Moving and backing up

A catalog is self-contained.
Copying the whole folder while no operation runs produces a complete, verifiable catalog.
Journals store catalog-relative destinations, so a moved catalog keeps its history.
The index can be rebuilt at the new location.

Before enabling writes, the implementation must test crashes around staging, publication, manifest replacement, and renames.
