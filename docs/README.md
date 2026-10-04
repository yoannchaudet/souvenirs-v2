# Souvenirs specification

**Status: draft, not implemented behavior.** `Must` identifies a requirement of this proposal.
This specification develops the supplied *Catalog collections* draft and approved design changes.

Souvenirs organizes long-term photo and video collections as ordinary, readable files.
Folders and filenames define organization.
SQLite tracks immutable originals and supports integrity checks and recovery.
Sidecars and exports remain mutable.

## Review order

| Read | Document | Scope |
| --- | --- | --- |
| 1 | [Catalog](catalog.md) | Layout, collections, datetimes, relationships, filenames, exports, and portable format. |
| 2 | [Operations](operations.md) | Operation inventory, scope boundaries, plans, content changes, and recovery actions. |
| 3 | [Storage and recovery](storage.md) | Original records, integrity, audit history, verified copies, interruption, and database loss. |

This is a reading order, not a release plan.
Each rule has one primary home.
Links connect shared requirements.
The storage contract applies to every mutating operation.

## Scope

Imports are copy-only.
Initial writes require the [local macOS/APFS profile](storage.md#write-profile).
Collection-package transfers, direct transfers between catalogs, and historical undo are deferred.
Editing, culling decisions, tags, and backups remain external tasks.
Souvenirs does not rewrite embedded metadata, sidecar contents, or external editor references.
Commands, app interfaces, and the legacy conversion algorithm are unspecified.
