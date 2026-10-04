# Souvenirs specification

**Status: draft, not implemented behavior.** `Must` identifies a requirement of this proposal.

Souvenirs organizes long-term photo and video collections as ordinary, readable files.
It must help organize a 10-year backlog of past photos and the next 40 years of new ones.
Simplicity is the primary design constraint.

## The 40-year contract

Souvenirs separates a durable **format** from a replaceable **tool**.

The format is the folder layout, filenames, and per-folder checksum manifests in [Catalog](catalog.md).
It must remain readable and verifiable without Souvenirs, using only a file browser and standard checksum tools.
The format must survive disk migrations, backups, cloud copies, and operating-system changes.

The tool is everything that reads, plans, and writes that format.
It will likely be rewritten several times in 40 years.
Tool state must be rebuildable from the format, except for unfinished-operation journals and optional history.

Every requirement must serve one of these goals:

| Goal | Meaning |
| --- | --- |
| Never lose an original | Imports copy and verify. Sources are never deleted. Originals are never modified. |
| Never invent a date | Every accepted datetime comes from metadata, a confirmed proposal, or explicit user assignment. |
| Readable without the tool | A person in 2066 can browse, understand, and verify the archive with stock tools. |
| Cheap to organize | Bulk decisions apply to batches, not individual files. A 10-year backlog must be practical. |

## Non-goals

- Editing, culling, ratings, tags, albums, and face recognition. External tools handle them.
- Backups. Souvenirs produces files that ordinary backup tools copy well.
- Automatic deduplication or deletion. Souvenirs reports duplicates but never removes them on its own.
- Rewriting embedded metadata, sidecar contents, or external editor references.
- Simultaneous writes from several computers, and historical undo.

## Review order

| Read | Document | Scope |
| --- | --- | --- |
| 1 | [Catalog](catalog.md) | The durable format: layout, collections, datetimes, relationships, filenames, manifests, and exports. |
| 2 | [Operations](operations.md) | Imports, backlog organization, duplicates, organization changes, and legacy conversion. |
| 3 | [Storage and recovery](storage.md) | Verified copies, the operation journal, the rebuildable index, and interruption recovery. |

Each rule has one primary home.
Links connect shared requirements.

## First milestone

The specification must not grow far ahead of a working implementation.
The first milestone is one thin, end-to-end slice:

1. Import one memory card into a new dated collection on an attached drive and on a NAS.
2. Write canonical names and the folder manifest.
3. Verify the collection with `shasum -a 256 -c SHA256SUMS`.
4. Interrupt an import on the NAS, by disconnecting the network, and recover it.
5. Run the slice on one real year of the backlog.

Findings from that run must update this specification before further features are specified.
