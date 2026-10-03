# Architecture

**Status:** Proposed architecture.
Nothing in this document is implemented.
It describes how an implementation can meet the [catalog](catalog.md), [operations](operations.md), and [reading](reading.md) contracts.

## Principles

- The file system is the catalog.
  Reading never needs a database.
- The audit log coordinates mutation and recovery.
  It never defines membership, labels, or accepted datetimes.
- One crate owns each concern.
  No layer duplicates domain rules.
- Core stays cross-platform and free of image decoders and platform UI frameworks.

## Workspace

| Crate                | Responsibility                                                                                   | Database |
| -------------------- | ------------------------------------------------------------------------------------------------ | -------- |
| `souvenirs-format`   | Names, parsing, formatting, validation, portable comparison, role table, and configuration.      | No       |
| `souvenirs-core`     | Catalog reading, source scanning, metadata proposals, planning, execution, and recovery logic.   | No       |
| `souvenirs-journal`  | The SQLite audit log, schema migrations, writer lock, and durable records.                       | Yes      |
| `souvenirs-platform` | No-replace rename, persistence, file identity, locking, and storage detection for each platform. | No       |
| `svs`                | The command-line interface. A thin layer that presents typed plans and outcomes.                 | No       |

`souvenirs-core` defines a `Journal` trait for the records that execution needs.
`souvenirs-journal` implements that trait.
Reading code does not depend on the trait.
Tests can supply an in-memory journal.

`souvenirs-format` has no I/O.
Every structural rule lives there once.
Other crates import its constants and helpers.

A native app and its service can follow the original Souvenirs pattern later.
They are outside the first scope.

## Platform primitives

Each primitive must exist on every supported platform.
A missing primitive blocks mutation on that platform.
It never falls back to a weaker operation.

| Primitive             | Linux                               | macOS                                                | Windows                                                                 |
| --------------------- | ----------------------------------- | ---------------------------------------------------- | ----------------------------------------------------------------------- |
| No-replace rename     | `renameat2` with `RENAME_NOREPLACE` | `renamex_np` with `RENAME_EXCL`                      | `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`                       |
| File persistence      | `fsync`                             | `fcntl` with `F_FULLFSYNC`                           | `FlushFileBuffers`                                                      |
| Directory persistence | `fsync` on a directory descriptor   | `fcntl` with `F_FULLFSYNC` on a directory descriptor | `FlushFileBuffers` on a directory handle, to be verified per filesystem |
| File identity         | Device and inode                    | Device and inode                                     | Volume serial number and 128-bit file ID                                |
| Writer lock           | `flock` exclusive, nonblocking      | `flock` exclusive, nonblocking                       | `LockFileEx` exclusive, fail immediately                                |
| Filesystem type       | `statfs` magic number               | `statfs` type name                                   | `GetVolumeInformationW`                                                 |

`renameat2` with `RENAME_NOREPLACE` is not available on every Linux filesystem.
An `EINVAL` result means unsupported storage.

## Audit log

### SQLite settings

| Setting                | Value                   | Reason                                             |
| ---------------------- | ----------------------- | -------------------------------------------------- |
| `journal_mode`         | `WAL`                   | Readers do not block the single writer.            |
| `synchronous`          | `FULL`                  | Each commit is durable before execution continues. |
| `fullfsync`            | `ON` on macOS           | SQLite uses `F_FULLFSYNC` for commits.             |
| `checkpoint_fullfsync` | `ON` on macOS           | Checkpoints are durable.                           |
| `application_id`       | A fixed Souvenirs value | Tools can identify the file.                       |
| `user_version`         | The schema version      | An unknown version blocks mutation.                |
| `foreign_keys`         | `ON`                    | Step records cannot outlive their operation.       |

A backup uses the SQLite online backup API or `VACUUM INTO`.
A plain file copy of a live WAL database is not a consistent backup.

### Schema sketch

The schema is illustrative.
It shows the records that the catalog contract requires.

| Table              | Contents                                                                                            |
| ------------------ | --------------------------------------------------------------------------------------------------- |
| `catalog`          | Catalog identifier, root path, root identity, and history discontinuities.                          |
| `operations`       | Identity, kind, sequence, start and end time, confirmed plan digest, plan body, state, and outcome. |
| `steps`            | Operation, ordinal, action, source, destination, precondition evidence, and state.                  |
| `staged_files`     | Step, staged path, intended final path, SHA-256 digest, and verification state.                     |
| `source_removals`  | Step, source path, authority state, and result.                                                     |
| `datetime_origins` | Step, item stem, origin, field name, raw value, offset, and interpretation.                         |
| `links`            | References from resolutions and reverts to earlier operations.                                      |

Paths are stored as native bytes with a platform tag.
Unix paths store their raw bytes.
Windows paths store their UTF-16 units.
A path never passes through a lossy string conversion.

### Step states

Each step moves forward through a fixed state sequence.
Each transition commits before the next filesystem action.

```text
copy:          intended -> staged -> verified -> published
rename:        intended -> renamed
create-dir:    intended -> created
remove-source: pending -> authorized -> removed
                                     -> revoked
```

Recovery reads the last committed state for every step.
It then inspects the disk:

| Recorded state | Disk observation                      | Recovery classification  |
| -------------- | ------------------------------------- | ------------------------ |
| `intended`     | Destination absent                    | Not started              |
| `intended`     | Destination present and source absent | Renamed, record lagged   |
| `staged`       | Staged file present                   | Verify again             |
| `verified`     | Final path present, digest matches    | Published, record lagged |
| `verified`     | Final path present, digest differs    | Uncertain                |
| `published`    | Final path absent                     | Uncertain                |

An uncertain step requires an explicit user decision.
Recovery never deletes a final path.

## Execution order

A copy-based transfer runs in three phases:

1. Publish every destination through staged publication.
2. Persist every affected destination directory and commit all `published` states.
3. Authorize and run source removals, one committed record per removal.

A move never enters phase 3 before phase 2 completes.
A crash during phase 3 leaves pending removals for recovery.
Recovery after reopening asks for fresh confirmation before phase 3 continues.

## Testing strategy

- Format crate: round-trip tests for every name pattern and every rejection rule.
- Core: behavior tests over temporary directories, including cross-year collections, empty collections, collisions, and noncanonical names.
- Crash tests: a fault-injection journal and platform layer that stops after each step state.
  Recovery must reach a consistent state from every stop point.
- Platform tests: run on each supported filesystem in the tested-storage list.
- Portability tests: Unicode normalization, case folding, reserved Windows names, and 255-unit limits.

## Dependencies

Dependencies must be MIT-compatible.
LGPL and AGPL dependencies are not permitted.
An MPL-2.0 dependency needs an explicit reviewed exception.
