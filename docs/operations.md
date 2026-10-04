# Operations

Every mutation requires a concrete plan, confirmation, and [write eligibility](storage.md#audit-and-write-eligibility).
No operation may overwrite an occupied destination.
Destination directories below the selected root must be real directories, not symlinks.

## Operation inventory

These are proposed operations, not command names or implemented features.
The linked contracts define their behavior and restrictions.
Workflows with unresolved scope appear separately below.

### Read and plan

| Operation | Purpose and contract |
| --- | --- |
| Read catalog | Inventory folders, logical items, and findings from the [filesystem](catalog.md), including unregistered originals. |
| Inspect original records | Read recorded IDs, roles, paths, and content baselines from the [registry](storage.md#original-registry). |
| Inspect operation history | Read recorded intent, progress, outcomes, and pending recovery from the [audit](storage.md#audit-and-write-eligibility). |
| Prepare a plan | Scan selected sources and resolve dates, relationships, targets, and exclusions before proposing [changes](#plans-and-selection). |
| Check original integrity | Compare selected original bytes with recorded baselines and report [coverage and discrepancies](storage.md#integrity-and-reconciliation). |

These operations do not change media, organization, or accepted baselines.
Confirmation and execution are shared stages of the mutating operations below.

### Create and change content

| Operation | Purpose and contract |
| --- | --- |
| Initialize catalog | Create configuration, registry, and audit state in the required [initialization order](storage.md#audit-and-write-eligibility). |
| Create dated collection | Declare a named occasion and its accepted [date range](catalog.md#dated-collections). |
| Create ongoing collection | Declare a continuing group organized into [annual folders](catalog.md#ongoing-collections). |
| Import originals | Copy selected originals into an explicitly [new or existing collection](#creation-and-continuation), leaving sources unchanged. |
| Admit originals in place | Explicitly register existing originals and establish their [content baselines](storage.md#original-registry) without transferring them. |
| Rename collection | Change the label of a dated collection or every annual part of an [ongoing collection](#organization-and-repair). |
| Change accepted datetime | Change an admitted item's organizational date or time under the [placement and counter rules](#organization-and-repair). |
| Change dated range | Change a dated collection's accepted range and affected folder placement through an [explicit plan](#organization-and-repair). |
| Attach companion or sidecar | Add an incoming file to a selected item's [vacant supported role](#late-companions). |
| Add exports | Copy a selected export file or subtree into an [explicit export destination](#exports). |

Organization changes use same-filesystem renames under the [write profile](storage.md#write-profile).
They do not authorize arbitrary collection reassignment.

### Maintain and recover

| Operation | Purpose and contract |
| --- | --- |
| Reconcile original records | Resolve identity, path, or grouping disagreement through an [audited plan](storage.md#integrity-and-reconciliation). |
| Recover or resume operation | Reconcile interrupted work and finish the matching operation without [reimporting completed transfers](storage.md#interruption-and-recovery). |
| Abandon a copy operation | Retain completed files and stop unfinished work through [explicit abandonment](storage.md#abandon-a-copy-operation). |
| Reverse an unfinished rename | Reverse completed steps under [identity and destination guards](storage.md#rename-recovery), not historical undo. |
| Revalidate catalog | Revalidate identity and pending work after catalog [relocation, cloning, or database restoration](storage.md#relocation-backup-and-database-loss). |
| Rebuild database | Explicitly establish new IDs and baselines while preserving [old state and recovery evidence](storage.md#relocation-backup-and-database-loss). |
| Archive completed history | Archive eligible history without discarding current original records, baselines, or [pending recovery evidence](storage.md#audit-and-write-eligibility). |

### Workflows needing further definition

| Operation | Unresolved scope |
| --- | --- |
| Repair organization | The allowed corrections need definition. Read-only findings do not authorize automatic changes. |
| Quarantine content | No explicit quarantine operation is defined yet. |
| Admit a changed original | The workflow must distinguish a new occurrence from restoration of expected bytes while preserving earlier evidence. |
| Convert legacy catalog | [Migration constraints](#legacy-conversion) exist, but the conversion algorithm is unspecified. |

Unspecified recognition, metadata, or placement rules must not become silent defaults.
Editing, culling, tags, and backups remain external tasks.
General deletion, automatic deduplication, collection merging, and partial-year reassignment are not defined operations in this version.
Counter resequencing is deferred rather than part of repair.
Initial sidecar support excludes [proxy associations and directory-package interpretation](catalog.md#relationships-and-sidecars).
Import moves, collection-package transfers, direct transfers between catalogs, and historical undo are outside the initial scope.

Users can copy collection folders externally while catalogs are idle, then scan and explicitly admit valid originals.
External copies have no Souvenirs verification, audit, or resume guarantee.
Admission establishes new baselines; it does not prove that copied bytes match an earlier source.

## Plans and selection

A plan must identify selected sources, destinations, and intended filesystem and registry changes.
For tracked originals, it must identify original IDs, logical-item IDs, and expected registry state.
It must distinguish selected transfers, explicit exclusions, and untouched source content.
Excluded content must stay at source and must not appear as transferred.
Unknown content must not be copied, renamed, or deleted implicitly.

Unresolved dates or relationships block an executable import plan.
The user must resolve them or explicitly exclude the affected items.
Import must not silently skip unresolved items, including in ongoing collections.
Plans must warn when path changes require external relinking.

### Unhandled content

Source scans, catalog inventory, and repair must distinguish these findings:

| Finding | Meaning |
| --- | --- |
| Unknown content | An entry without a recognized media, sidecar, catalog-structure, or allowed-payload role. |
| Orphan or unmatched sidecar | A recognized candidate has no matching owner or no supported naming rule. Report which condition applies. |
| Ambiguous sidecar | More than one owner remains possible. Traversal order must not select one. |

Rename and transfer plans must identify unhandled neighboring content and its disposition.
Distinguish files retained at source from unchanged payload carried by an explicitly selected whole-folder operation.
The plan must not imply that unhandled files follow an original's basename changes.
These findings do not authorize automatic attachment, deletion, or quarantine.

## Preconditions

Planning must validate:

- Object types, filesystem identity where available, size, and modification time.
- Item roles, recognition settings, relationship rules, collection ranges, counters, and affected registry state.
- Complete destination namespaces, including unmanaged content.
- [Portable names and complete target paths](catalog.md#portable-names).
- The [write profile](storage.md#write-profile), including mount boundaries between staging and every managed destination.

Execution must recheck relevant preconditions before mutation.
Drift requires a new plan or explicit reconciliation, not silent replanning.
Identity and content conflicts block mutation of the affected scope.
These checks detect ordinary drift, not every same-size, same-mtime edit.
They provide neither a byte snapshot nor isolation from external writers.
Pure naming and folder operations must not copy or hash the whole media collection.

## Creation and continuation

Creation must declare a dated or ongoing collection and validate its names before mutation.
Every import must explicitly create a collection or continue a selected existing collection.
Matching labels, dates, or derived ranges must not select an existing target or merge directories automatically.

A new dated collection proposes one range from all resolved original datetimes across media types.
The user accepts it or supplies a wider range containing every selected original date.
Continuation targets the existing collection rather than deriving another collection from the incoming batch.
For example, a one-day contribution can join an existing week-long trip.

Dates inside the accepted range leave the directory unchanged.
Outside dates require explicit range extension, datetime correction, or exclusion.
An extension must contain the old range and newly accepted dates.
An earlier start can move the entire directory to another year without changing existing original basenames or counters.
An occupied destination requires conflict resolution, not merging.

Ongoing continuation routes by accepted year and creates missing annual parts of the selected collection.
Ongoing creation and rename must scan the label namespace across all years.
An incomplete scan blocks those operations.
A label used by another ongoing collection is a semantic conflict even when destination paths are free.

## Organization and repair

Rename changes the selected collection label while preserving item datetimes and counters.
An ongoing rename must change all annual parts, not only selected years.
Partial-year reassignment and collection merging are unsupported.
Import and repair must not split, merge, or reassign collections automatically.

Datetime and range changes require explicit plans addressing names, counters, placement, and registry paths.
They must preserve unaffected names and reject namespace conflicts.
Placement choices for admitted dates outside their existing collection remain unspecified.
[Counter rules](catalog.md#names-and-counters) determine whether a changed date retains its counter.
[Registry rules](storage.md#original-registry) preserve IDs and baselines through organization changes.

Whole-folder organization renames carry current exports and unknown contents unchanged.
They must not reinterpret this payload or silently omit it.
Interrupted renames require [completion or guarded reversal](storage.md#rename-recovery).

Repair reports inconsistencies while preserving accepted datetimes, ranges, gaps, exports, and empty collections.
It must not silently normalize names, adopt external originals, replace baselines, or reconcile ambiguous identities.
Permitted corrections and any explicit quarantine operation remain unspecified.
Read-only [integrity checks](storage.md#integrity-and-reconciliation) compare bytes without changing accepted organization or baselines.

## Late companions

Attachment requires explicit selection of an existing owner and every incoming companion or sidecar.
The role must be supported and vacant.
The plan must validate conflicts and retain the owner's datetime, label, and counter.
Attachment must not replace an occupied role or rewrite existing bytes.

A late original receives a new original ID and baseline under the existing logical-item ID.
Existing original IDs and baselines remain unchanged.
A sidecar receives no original record.
This is declared ownership, not cross-directory inference.

## Exports

Adding originals does not silently include a source export tree.
The plan must identify any such tree left behind.
Explicit inclusion of a complete source export subtree requires an absent destination `exports` directory.
An existing target requires separate explicit placement, not automatic merging.

Further exports require an explicit destination inside `exports`.
Existing directories can contain that destination, but selected directory payloads require absent destination directories.
Occupied entry names remain conflicts.

The operation must enumerate the complete export payload before mutation.
Unsupported entries and target-invalid names require resolution.
It must not follow symlink targets or silently omit unsupported entries.
Transfers preserve bytes and relative paths, not universal cross-platform filesystem attributes.
Export copies must preserve empty directories and must not register payload as originals.

Every copied regular file follows the [staged-copy protocol](storage.md#staged-copies).

## Legacy conversion

Conversion from the original Souvenirs requires a separate approved plan, not implicit repair.
It must resolve conflicting counters from former media groups and account for legacy date uncertainty.
Date-only names require explicit clock assignment when metadata provides no usable time.
Conversion must preserve unknown content and establish records and baselines for admitted originals.
Those baselines cannot prove earlier bytes.
The conversion algorithm and command compatibility remain unspecified.
