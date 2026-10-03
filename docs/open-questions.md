# Open questions

**Status:** Decisions that the proposed specifications need before implementation.
Each entry states the gap, why it matters, and a recommendation.
Companion specifications cite entries as `OQ-n` where they adopt a recommendation provisionally.
A resolved entry moves its decision into the owning specification and records the outcome here.

## Media roles and compound items

### OQ-1: Media kinds without routing groups

[Local relationships](catalog.md#local-relationships) require RAW, JPEG, still, and motion roles.
The configuration has only a flat `media_extensions` list.
That list cannot decide whether `.dng` is RAW or whether `.mp4` is motion.

**Recommendation:** Define a built-in role table in core.
Map each known extension to one kind: `raw`, `jpeg`, `still`, or `video`.
`jpeg` is also a `still`.
Configuration controls recognition only.
A recognized extension without a built-in kind forms only solo logical items.
Do not add a role map to `.souvenirs.yml` in format version 2.

### OQ-2: Compound role combinations

The catalog spec allows RAW+JPEG and still/motion compounds with two members.
It does not say whether HEIC+MOV, JPEG+MP4, or DNG+MOV are valid still/motion pairs.

**Recommendation:** A RAW+JPEG compound has exactly one `raw` and one `jpeg` member.
A still/motion compound has exactly one `still` (including `jpeg`) and one `video` member.
A `raw` member is never a still/motion still.
Three-member compounds, such as RAW+JPEG+MOV, are unsupported.
Catalog reading reports any other shared-stem combination as a counter collision finding.

## Names and parsing

### OQ-3: Year and datetime bounds

"An arbitrary valid datetime" has no bound.
The `{yyyy}` directory token implies four digits.

**Recommendation:** Accepted dates range from `0001-01-01` to `9999-12-31` in the proleptic Gregorian calendar.
Clock time ranges from `00-00-00` to `23-59-59`.
A leap-second value `60` is invalid.

### OQ-4: Canonical duration token

The spec does not say whether `+02d` or `+0d` is canonical.

**Recommendation:** `N` is a positive decimal integer without leading zeros.
`+0d` is invalid; a single-day collection omits the token.
Reading reports a noncanonical duration spelling as a finding.
It still parses the value.
Repair does not normalize it automatically (see [OQ-9](#oq-9-what-repair-mutates)).

### OQ-5: Label equality for canonical validity

Canonical validity requires a "matching collection label".
Namespace collisions use normalization and case-fold equivalence.
The spec does not say which equality decides "matching".
macOS HFS+ stores names in a decomposed form, so exact bytes can differ from the accepted NFC label.

**Recommendation:** A filename label matches when its NFC form equals the folder label's NFC form, code point for code point.
A case-only difference is a finding, not a match.
Namespace collision checks use NFC plus Unicode default full case folding.
Neither check rewrites native names.

### OQ-6: Counter allocation and noncanonical names

"Allocates above the highest existing counter for that date" does not say which names count.
A noncanonical name, such as one with a stale label, can carry a counter-shaped token.

**Recommendation:** The highest counter includes every direct child whose name parses with this collection's date and any label.
This prevents a later relabel of a stale file from colliding.
Complete-name collision checks still apply to every allocation.

### OQ-7: Tie-break order for source paths

"Source paths break ties" does not define the path order.

**Recommendation:** Compare catalog-independent source paths component by component.
Compare each component by its native byte or UTF-16 unit order.
The order must be deterministic on one platform.
It is not required to match across platforms.

### OQ-8: Operating-system noise

The old guide ignored OS noise files.
The new spec does not mention them.
Files such as `.DS_Store`, `Thumbs.db`, `desktop.ini`, and AppleDouble `._*` files appear on external drives.

**Recommendation:** Core keeps one fixed OS-noise list.
Reading reports noise as preserved noise, not as unknown content.
Noise does not block plans.
Collection-directory moves carry noise.
Import does not transfer source noise.
No operation deletes noise implicitly.
AppleDouble files that shadow an original stay beside it and are never treated as sidecars.

## Operations

### OQ-9: What repair mutates

The operations table says repair "reports inconsistencies".
The spec mentions quarantine but no quarantine operation.
The spec does not define what repair changes.

**Recommendation:** Split repair into two parts.
A read-only check produces findings.
Explicit resolutions are separate confirmed plans, one finding type per resolution kind:

| Finding                                 | Resolution                                                           |
| --------------------------------------- | -------------------------------------------------------------------- |
| Noncanonical original                   | Adopt: user accepts or assigns a datetime; plan allocates a counter. |
| Stale label in a canonical-shaped name  | Relabel the item to the collection label, keeping its datetime.      |
| Counter collision between logical items | Reallocate one selected item above the date's highest counter.       |
| Item date outside the dated range       | Extend the range, change the item datetime, or relocate the item.    |
| Item in the wrong ongoing year          | Move the item to the matching annual part.                           |
| Orphan or ambiguous sidecar             | Attach to a selected owner, or leave in place.                       |
| Unknown content                         | Quarantine to a location outside the catalog, or leave in place.     |

Quarantine is an explicit resolution, never a side effect.
Repair never closes gaps, shrinks ranges, or normalizes names.

### OQ-10: Moving items between collections

Culling and correction need "move these items to another collection".
The spec lists reassignment and merging as unsupported.
It does not say whether single-item reassignment is planned.

**Recommendation:** Add an explicit "reassign items" operation after the first release.
It relocates selected logical items with sidecars into a named target collection.
It preserves datetimes, allocates new counters in the target, and leaves exports in the source collection.
Collection merge, split, and dated/ongoing conversion remain unsupported.

### OQ-11: Add exports transfer mode

The operations table says add exports "copies".
Package transfer supports moves.
The spec does not say whether add exports supports move.

**Recommendation:** Add exports supports copy and move from unmanaged sources.
Move uses the same cleanup barrier as other copy-and-delete transfers.
Exports from another managed catalog stay copy-only.

### OQ-12: Same-filesystem move import

The spec defines staged copies and organization renames.
It does not define a move import from an unmanaged source on the same filesystem.
A native rename would avoid copying large videos.

**Recommendation:** Allow a move import to rename a source file directly to its final destination when three conditions hold:
the source and destination share a filesystem,
the plan names the rename strategy,
and the user confirms it.
Record intent before each rename, as for organization renames.
No digest is computed.
The source entry disappears at the same moment as publication, so no cleanup barrier applies.
Cross-filesystem moves always use staged copies.

### OQ-13: Canonical-looking source names

"Transferring an existing canonical item preserves that value."
The spec does not define when a source item is an existing canonical item.
A canonical-looking name in an arbitrary folder can be a coincidence or a stale value.

**Recommendation:** An item is existing-canonical only in two cases.
It arrives through a package transfer or a managed-catalog transfer.
Or the source path is inside a recognized format-version-2 collection directory.
In an arbitrary source, a canonical-looking name yields a proposal with origin `filename`.
The plan shows that origin, and the user confirms it like a metadata proposal.

### OQ-14: Annual part transfer into an existing ongoing label

Equal ongoing labels across years identify one collection.
A package transfer of `2024/z Everyday` into a catalog that has `2025/z Everyday` creates a semantic merge.
The spec says package transfer must not merge into another collection.

**Recommendation:** Reject the transfer when the destination already has an ongoing collection with an equal label in any year.
The user can transfer with an explicit new label instead.
A later explicit "join annual part" operation can lift this restriction.

### OQ-15: Empty ongoing collections

An empty dated collection needs an explicit range.
An empty ongoing collection has no year to create.

**Recommendation:** Creating an empty ongoing collection requires an explicit year.
Souvenirs creates one empty annual directory for that year.

### OQ-16: Rename with noncanonical members

The spec says noncanonical names "require resolution".
It does not say whether rename blocks on them.

**Recommendation:** Rename does not block.
The plan lists noncanonical direct children.
They move with the directory and keep their names.
After rename, they remain findings.

## Audit log and storage

### OQ-17: Missing audit log

A corrupt log must not be replaced silently.
The spec does not address a catalog whose `.souvenirs/` directory is missing.
This happens after a copy tool skips hidden directories.

**Recommendation:** A missing log blocks mutation.
An explicit "reestablish history" operation creates a new log.
Its first entry records a history discontinuity, the reason, and the catalog root identity.
That operation requires confirmation and must refuse when staging contains artifacts.

### OQ-18: Writer lock mechanism

"Only one Souvenirs writer" has no mechanism.
SQLite locks alone do not cover long filesystem work between transactions.

**Recommendation:** Hold an exclusive OS advisory lock on `.souvenirs/lock` for the full operation.
Use `flock` on Unix and `LockFileEx` on Windows.
Treat lock failure as busy.
Readers take no lock.

### OQ-19: Staging and mount points

Staging lives in `.souvenirs/staging/`.
A mount point inside the catalog puts some destinations on another filesystem.
Staged publication then cannot use an atomic rename.

**Recommendation:** Every destination directory must share the device identity of `.souvenirs/staging/`.
A plan that targets another device fails validation with a storage error.

### OQ-20: Supported storage list

The spec requires documented tested storage.
External drives commonly use exFAT.

**Recommendation:** The first tested list covers APFS and HFS+ on macOS, ext4 and Btrfs on Linux, and NTFS on Windows.
exFAT needs a decision.
It is common for archive drives, but it has weaker crash guarantees and no inode identity.
A proposed answer is read and copy-source support with an explicit opt-in for writes.
SMB, NFS, and synchronized folders stay unsupported for writes.

### OQ-21: Clone and restore detection

The spec requires revalidation after relocation or log restore.
It does not define the evidence.

**Recommendation:** The log stores a random catalog identifier, the last root path, and the root directory's filesystem identity.
It also stores a monotonically increasing operation sequence.
A mismatch in root identity or path requires explicit revalidation.
A copy of `.souvenirs.yml` without the log carries no identity.

## Datetimes

### OQ-22: Metadata field precedence

"Usable capture metadata" and "supported companion metadata" have no field list.

**Recommendation:** Adopt the precedence table in the [datetimes specification](datetimes.md).
Companion metadata means the other member of a compound item.
XMP sidecar values are proposals of last resort.
They are shown with origin `sidecar`.

### OQ-23: Unusable metadata values

The spec does not define "usable".

**Recommendation:** A value is unusable when it fails to parse, is all zeros, or is before `1900-01-01`.
A QuickTime zero timestamp decodes to `1904-01-01 00:00:00` and is also unusable.
A value later than the planning time plus one day is suspicious.
A suspicious value is still proposed, with a plan warning.

## Interfaces

### OQ-24: Catalog registry

The spec does not mention the per-user registry.

**Recommendation:** Keep a per-user registry of names and roots as a convenience.
It is not authoritative.
Operations accept a root path directly.

### OQ-25: Command surface

The catalog spec defines no commands.

**Recommendation:** Specify the CLI after the core operation contracts in [operations](operations.md) are stable.
Keep the CLI a thin layer over typed plans.
