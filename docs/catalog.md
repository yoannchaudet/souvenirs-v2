# Catalog collections

**Status:** Proposed catalog format and behavior contract.
This document does not describe implemented behavior.
The [legacy comparison](legacy.md) relates it to the original Souvenirs guide at commit `5d8202a`.
Unresolved decisions are listed in [open questions](open-questions.md).

## Purpose and boundaries

A catalog organizes a long-term photo and video collection as ordinary files.
The folder structure must remain understandable without Souvenirs.
Photos and videos share the same collection folders.
Media type does not select a folder.

The file system defines collection membership, labels, and accepted datetimes.
A SQLite audit log records operations and coordinates recovery.
The log does not replace the catalog with a database.

Editing, culling decisions, tags, and backups remain external tasks.
This specification does not define an app interface or commands.
The [operations](operations.md), [reading](reading.md), and [datetimes](datetimes.md) specifications refine its contracts.
Exported media stays in an opaque directory inside its collection.
No operation requires permanent media snapshots or a hidden asset database.

## Concepts

| Concept            | Meaning                                                                                   |
| ------------------ | ----------------------------------------------------------------------------------------- |
| Catalog            | A root directory containing collections, format configuration, and operation state.       |
| Collection         | A named set of media that belongs together, independent of import batches and media type. |
| Dated collection   | An occasion with an accepted start date and optional end date, stored in one directory.   |
| Ongoing collection | A continuing collection, stored in one annual directory for each represented year.        |
| Asset              | An original photo or video with a recognized media extension.                             |
| Logical item       | One asset or a supported compound item, with its associated sidecars.                     |
| Sidecar            | A supported metadata file associated with a local asset or pair.                          |
| Accepted datetime  | A selected calendar date and clock time used for organization, not proof of capture time. |
| Export             | Opaque edited media or supporting content inside a collection's `exports/` directory.     |
| Canonical name     | A name that follows this specification.                                                   |

A collection is not a permanent asset identifier.
Paths can change through explicit rename or date decisions.
No hidden database record defines an asset's membership.

## On-disk structure

```text
{catalog}/
  .souvenirs.yml
  .souvenirs/
    operations.sqlite
    staging/
  {yyyy}/
    {start-yyyy-MM-dd} [+Nd] {label}/
      {item-yyyy-MM-dd} {HH-mm-ss} {label} {NNN}.{ext}
      exports/
        arbitrary-files-and-folders
    z {label}/
      {item-yyyy-MM-dd} {HH-mm-ss} {label} {NNN}.{ext}
      exports/
        arbitrary-files-and-folders
```

Square brackets mark an optional token.
They are not literal filename characters.
There are no `photos/` or `videos/` routing directories.
Originals and their sidecars live directly inside their collection directory.
Recognized media extensions remain configurable.
Extension configuration does not define routing groups.

`.souvenirs/` contains reserved operational data.
It can also contain SQLite auxiliary files and recovery artifacts.
Its contents are not media or collection members.
Repair must not quarantine this directory.

For example:

```text
Family/
  .souvenirs.yml
  .souvenirs/
    operations.sqlite
  2024/
    2024-12-31 +1d New Year/
      2024-12-31 23-30-00 New Year 001.cr3
      2024-12-31 23-30-00 New Year 001.jpg
      2024-12-31 23-30-00 New Year 001.xmp
      2025-01-01 00-15-00 New Year 001.mov
      exports/
        highlights.mp4
        web/
          invitation.jpg
    z Everyday/
      2024-06-02 09-00-00 Everyday 001.jpg
      2024-06-02 17-45-00 Everyday 003.mp4
  2025/
    z Everyday/
      2025-02-09 10-00-00 Everyday 001.heic
```

The RAW+JPEG pair shares one datetime and counter.
The video is another logical item in the same dated collection.
The gap before counter `003` is valid.

### Dated collections

A dated collection stays under its accepted start year.
It remains in one directory when it crosses New Year.
Original filenames use item datetimes, not the collection start date.

`+Nd` means that the end date is `N` days after the start.
For example, `+2d` spans three calendar dates.
A single-day collection omits the duration token.

The accepted range describes the occasion.
It must contain the calendar date of every original logical item.
Exports do not contribute dates to this range.
It does not continuously recompute from the remaining files.
Removing an item must not shrink the range automatically.
An empty collection remains valid.
Creating an empty dated collection requires an explicit range.

Dated ranges may overlap.
Dated collections may share a start date when their complete folder names remain distinct.
An item date does not select between them; continuation always names the target explicitly.
A dated and an ongoing collection may share a label.
They occupy separate namespaces distinguished by their folder forms.

### Ongoing collections

An ongoing collection uses `z {label}` under each represented year.
Each annual directory contains originals with accepted datetimes in that year.
The `z` prefix keeps these directories after dated collections in lexical order.

Matching labels across years identify annual parts of the same ongoing collection.
Label equality follows the portable comparison rules below.
Creation and rename must check the ongoing-label namespace across all years.
A label already used by another ongoing collection is a semantic conflict.
Free physical destination paths do not override that conflict.
Incomplete year scanning blocks these operations.
An empty annual directory remains valid.

Annual parts bound directory size and preserve year-based browsing.
The tradeoff is that an ongoing collection spans several physical directories.
Ordinary rename changes every annual part of the selected ongoing collection.
It must not silently combine collections or rename only selected years.
Partial-year reassignment and collection merging are separate, unsupported operations.

## Creation and continuation

An import must either create a collection or target an existing collection explicitly.
A matching label or derived range must not silently select an existing collection.
A conflicting creation request must not merge directories.

A new dated collection proposes one range from all resolved item datetimes.
Photos and videos contribute to the same range.
The user accepts that range or supplies a wider range.
The accepted range must contain the calendar date of every selected original.

Continuation targets the selected collection, not a range derived from the new batch.
A shorter later batch must not create another collection automatically.
Calendar dates inside the accepted range leave the collection directory unchanged.

Dates outside the range require a user decision.
The user can extend the range, correct item datetimes, or exclude those items.
Extension includes the old range and the newly accepted calendar dates.
An earlier start can move the complete directory to another year.
Extension must not rename existing asset basenames or change their counters.
An occupied destination requires conflict resolution, not an implicit merge.

Continuation of an ongoing collection routes originals by their accepted year.
It creates missing annual parts of the selected collection.
No import or repair may split, merge, or reassign collections automatically.

## Accepted datetimes

Every original logical item requires a calendar date and a clock time.
Accepted datetimes use second precision and the 24-hour clock.
They are organizational wall-clock values, not universal instants or verified historical facts.
The user can explicitly assign an arbitrary valid datetime.

Souvenirs proposes a datetime from usable capture metadata.
It uses the primary asset's metadata first.
It can use supported companion metadata when the primary has no usable value.
Conflicting usable values require explicit resolution.

When metadata records a local date and clock, the proposal preserves those fields.
The importing machine's timezone must not change those values.
A missing timezone must not cause a guessed timezone conversion.

A known UTC instant without capture-local context requires an explicit organizational interpretation.
The user can keep UTC clock values or select an offset or timezone for conversion.
Ambiguous UTC-versus-local fields also require explicit interpretation.
This includes MP4/MOV timestamp fields whose actual timezone convention is unknown.
The same interpretation can apply to a selected batch.
The plan shows the resulting datetimes, and the audit log records the interpretation.

Source offsets and fractions remain in original metadata.
They are not additional filename fields.
Fractional seconds are discarded without rounding when proposing second-precision values.
Wall-clock sorting does not guarantee chronological order across travel or daylight-saving transitions.

Filesystem modification time is not a capture-datetime substitute.
Souvenirs must not silently assign epoch values, midnight, or another placeholder.
A collection range does not assign datetimes to its originals.
A date without a clock time requires explicit assignment.
There is no automatically populated undated collection.

An unresolved item blocks an executable import plan.
The user must correct metadata, assign a datetime, or exclude the item explicitly.
The same rule applies to ongoing collections.
Exclusion must be reported and must leave the source intact.
An import must not silently skip unresolved items.

After admission, the canonical filename carries the complete accepted datetime.
That value remains authoritative during repair.
New metadata or metadata disagreement must not overwrite it automatically.
The audit log records metadata-derived, user-assigned, or existing-canonical origin.
Reading the accepted value does not require the log.
Transferring an existing canonical item preserves that value instead of deriving it again from metadata.

Changing an accepted datetime requires an explicit plan.
The plan must address affected names, counters, and collection placement.
Souvenirs does not rewrite embedded metadata or XMP contents.

## Local relationships

Relationship inference requires files in the same direct directory.
Recursive scanning discovers directories but keeps their relationship namespaces separate.
The same locality rule applies during catalog scanning.

Souvenirs must not infer ownership across directories.
Matching basenames, timestamps, or content do not authorize such a relationship.
Later imports must not infer ownership from existing destination files.

A filename-based pair requires an unambiguous supported RAW+JPEG combination.
Its members must share a basename in the same directory.
A matching photo and video basename must not automatically create a pair.
An arbitrary non-JPEG extension does not establish a RAW role.
Multiple possible pairs require explicit resolution.
Automatic phone still/motion pairing is not required.
An explicit still/motion association is a supported two-member compound item.
Its members share one canonical stem, accepted datetime, and counter.
This does not promise vendor-specific Live Photo playback or metadata rewriting.
Catalog reading recognizes declared compound membership from a shared complete canonical stem and unambiguous supported roles.
It requires no attachment history.
This is recognition of existing organization, not new source-basename inference.

Discovery examines all recognized direct children when evaluating ambiguity.
It is not restricted to the selected transfer subset.
Every attached member must appear in the confirmed transfer set.
Unselected files remain untouched.

XMP sidecars retain the three supported forms:

```text
{basename}.xmp
{basename}.{asset-ext}.xmp
{basename}_{nn}.{asset-ext}.xmp
```

Sidecars must attach only to an unambiguous local owner.
An ambiguous sidecar must not attach to the first traversal or sorted match.
An executable plan requires explicit resolution or exclusion of unresolved associations.
Excluded or orphan sidecars remain at source unless a separate action handles them.

Recognized sidecars follow their logical item through approved changes.
Their naming suffixes and bytes remain unchanged.

### Late companions

The user can explicitly select an existing item and an incoming companion or sidecar.
The selected role must be supported and vacant.
The attachment uses the existing item's datetime, label, and counter.
The plan names every incoming file and validates conflicts.
This is declared ownership, not inferred cross-directory matching.
It must not replace an occupied role or rewrite the existing media bytes.

## Filenames and counter stability

Canonical asset names use this pattern:

```text
{item-yyyy-MM-dd} {HH-mm-ss} {label} {NNN}.{ext}
```

Time uses hyphens instead of Windows-forbidden colons.
The complete prefix preserves accepted datetime without another metadata file.
Canonical validity requires a valid datetime, matching collection label, and a positive counter.
The date must fit the dated range or the ongoing part's year.
Supported compound members share one stem and have unambiguous roles.
Noncanonical or conflicting names require resolution, not silent acceptance.

One counter namespace applies to each date inside each physical collection directory.
Photos and videos share that namespace.
A compound item and its sidecars share one counter.
Separate logical items must not share the same date and counter.

Counters start at `001` and widen beyond three digits.
An addition allocates above the highest existing counter for that date.
It must not fill lower counter gaps.
Existing names, counter values, and counter widths remain unchanged.

New counters represent allocation order, not permanent shooting order.
A later import can add an earlier photograph without renumbering existing items.
Accepted datetimes order new allocations.
Source paths break ties.
Changing only clock time preserves the counter.
A calendar-date change preserves the counter when it is free in the new namespace.
Otherwise, the plan allocates above that date's highest existing counter.
Other logical items must not be renumbered.

Gaps are legitimate.
Repair must never close them automatically.
Resequencing is a possible later explicit operation.
It is not part of routine import, repair, or collection rename.

Counters are not permanent global identifiers.
This contract does not reserve counters belonging to deleted items.
Deleted names can therefore identify different media later.
Approved label or datetime changes can invalidate external pathname references.

## Catalog operations

| Operation                   | Required behavior                                                                                  |
| --------------------------- | -------------------------------------------------------------------------------------------------- |
| Create                      | Declare a dated or ongoing collection and validate its names before mutation.                      |
| Add                         | Target creation or continuation explicitly; resolve dates and relationships before execution.      |
| Rename                      | Change the selected label; preserve item dates and counters; identify every affected path.         |
| Change datetime or range    | Require explicit approval; preserve unaffected names and reject namespace conflicts.               |
| Attach companion            | Select an existing owner explicitly; validate a vacant supported role and retain its stem.         |
| Add exports                 | Copy selected payload into an explicit export destination without automatic merging.               |
| Transfer collection package | Carry one entire physical folder, including its opaque exports and unknown contents.               |
| Repair                      | Report inconsistencies; preserve accepted datetimes, ranges, gaps, exports, and empty collections. |
| Revert organization         | Prepare a new guarded operation using recorded earlier values and the current filesystem.          |

Every mutation requires a concrete plan and confirmation.
The plan must identify its selected sources, destinations, and source removals.
It must report excluded content without implying that the content transferred.
It must distinguish selected transfers, explicit exclusions, and untouched source content.
Unknown files and directories must not be deleted or moved implicitly.
No operation may overwrite an occupied destination.
Destination directories below the selected root must be real directories, not symlinks.
Execution must reject relevant changes that invalidate the confirmed plan.
External editor projects and their internal references are not rewritten.
Plans must warn when path changes can require external relinking.

### Inexpensive preconditions

Plans validate object types, filesystem identity where available, size, and modification time.
They also validate item roles, collection ranges, counters, and complete destination namespaces.
Execution rechecks relevant preconditions before mutation and source removal.
Mismatch requires a new plan or explicit reconciliation, not silent replanning.
These checks detect ordinary drift, not every same-size, same-mtime in-place edit.
They do not establish a byte snapshot or isolation from external writers.
Pure naming and folder operations must not copy or hash the whole media collection.
Cross-filesystem relocation requires an explicit transfer plan, not an implicit rename fallback.

## Audit log and recovery

### History

Every catalog-content mutation requires a healthy, durable SQLite audit log.
The log lives in `.souvenirs/operations.sqlite`.
Only one Souvenirs writer may operate on a catalog at a time.
Initialization creates operational state before publishing the catalog configuration.

The log records confirmed intent before filesystem mutation.
It records operation identity, time, scope, and outcome.
It retains original source names, old and new paths, and datetime assignment origin.
It records progress and the identity evidence needed for that operation's recovery.
It records staged locations and completed or remaining source removals.
Paths retain their native identity without lossy conversion.

Completed history is append-only through Souvenirs.
Resolution and revert append new entries referencing the earlier operation.
They must not erase a failure or rewrite history as if it never happened.
External filesystem edits are not automatically audited.
The log is not tamper-proof.

The log is operational history, not a disposable index.
It must not define current collection membership, labels, or accepted datetimes.
Ordinary catalog reading does not require it.
It stores no permanent media snapshots or hidden retention copies.
Its loss removes audit and revert evidence without erasing the catalog's meaning.
Unresolved records and recovery artifacts must not be discarded.
Completed history can be archived only when no recovery remains pending.
A corrupt log must not be silently replaced with an empty log.

### Same-filesystem organization

Label, datetime, and range changes use native no-replace renames.
Each step records durable intent before moving its directory entry.
It persists affected directory updates before recording completed progress.
Recovery reconciles a rename that happens before its progress record persists.
An old pathname can disappear before other rename steps complete.
These operations relocate existing objects; they do not perform transfer-source cleanup.

### Copy-based transfers and interruption

Copy-based transfers create destinations before removing source entries.
Every copied regular file must use staged publication, including originals, companions, sidecars, and export payloads.
Incomplete copies must never be written directly to their final paths.

For each copied regular file, the operation must:

1. Create an exclusive temporary file in reserved staging on the destination filesystem.
2. Compute SHA-256 over the source bytes while copying them into the staged file.
3. Persist the staged file and its directory using the supported platform's persistence operations.
4. Reopen and read the entire staged file; compare its SHA-256 digest with the source digest.
5. Durably record the verified digest, staged location, and intended final path before publication.
6. Publish through an atomic no-replace rename, persist affected directories, and durably record publication.

A read, write, persistence, verification, or publication failure stops the operation with a contextual error.
Staging and the final file must share a filesystem; publication must not fall back to another copy.
The expected digest remains available in the audit log for operation recovery.
Staged files are operational artifacts, not catalog media.
Unverified staged files must not be published or treated as completed transfers.

Read-back verifies the copied content, not independent physical-media persistence.
Reads can use operating-system or device caches.
Durability depends on successful persistence operations and the documented storage guarantees.
Hashing while copying avoids a separate source pass; it does not replace destination read-back.

Publication is atomic for one file, not for the whole collection or a compound item.
A crash can leave some verified files published while others remain staged or absent.
Recovery must reconcile final and staged files with recorded intent and digest evidence.
Staging alone does not authorize recovery or source deletion when the audit log is unavailable.

The cleanup barrier applies to copy-and-delete transfers, not organization renames.
A move must publish every selected file and persist all destination directory updates before removing any selected source.
The log must durably record the completed destinations before source removal starts.
Unknown and excluded source content must remain intact.

Completion requires all planned changes and requested source removals to finish.
Pending source cleanup remains recovery work.
An interrupted operation puts the catalog into recovery-required state.
Further mutations must stop until recovery resolves that operation.
Recovery must compare actual files with recorded intent and progress.
It must not assume that the last recorded step exactly matches the disk.
After reopening, destructive cleanup requires fresh confirmation.

Resume must recognize completed transfers in that operation.
It must not import them again under new counters.
This guarantee does not deduplicate a separate, newly requested import.

Conflicting or uncertain state requires explicit reconciliation.
Failures must identify the operation, completed work, remaining sources, and recovery paths.

### Cancel remaining cleanup

This action applies only to pending transfer-source deletions.
Once every destination is verified, the user can retain those copies and cancel remaining source removals.
Cancellation durably revokes deletion authority before releasing the recovery gate.
The outcome records completion with sources retained, not completed cleanup.
Reopening or resume must not reactivate those deletions.
Later source cleanup requires a new confirmed operation.
Unverified destinations still require recovery.

### Revert organization

Revert is a new confirmed operation, not database rollback or a filesystem snapshot.
It initially supports label, accepted-datetime, and dated-range changes.
The affected objects must remain unambiguously identifiable.
The selected property must still match the recorded after-value.
Otherwise, revert is unavailable with an explicit reason.

A label revert applies the earlier label to the current collection.
It preserves later additions and carries current exports unchanged.
It does not replay an obsolete list of filenames.
A range revert requires every current original date to fit the earlier range.
A datetime revert requires a compatible collection range and an available counter namespace.
Revert must not overwrite unrelated paths.

Revert preserves present media bytes, including external edits.
It does not restore image edits, deleted files, imports, attachments, or completed source cleanup.
Those actions require an ordinary new transfer or organization plan where supported.
No dependency graph or permanent shadow archive is required.
The original operation and the revert both remain in history.

### Relocation and restore

Record catalog-relative destinations separately from external source locations.
Pending operations bind to the selected catalog root and its filesystem identity.
Relocation, cloning, or detected log restoration requires explicit revalidation before mutation.
Recovery must not silently fall back to an earlier absolute catalog root.
Old source-removal authority must not resume automatically.

Copying a catalog for backup requires quiescent files and a closed, consistent SQLite backup.
A collection package copied into another catalog starts new audit history there.
Its files remain meaningful without the source catalog's log.
Old reverts can be unavailable after identity evidence changes.
A consistent rollback of both files and log cannot always be detected locally.

### Atomicity and storage limits

The log provides audit history and recovery coordination, not atomic visibility.
A SQLite transaction does not make multiple filesystem changes atomic.
External tools can observe intermediate states.
They must not edit or relocate affected files during a mutation or recovery.

The implementation must document tested storage configurations.
Write support requires reliable locking, journal persistence, and filesystem persistence.
Unsupported storage or failed journal persistence must block mutation.
Network shares and synchronized folders are not automatically covered by this guarantee.

The log is not a backup.
This contract does not promise recovery from lost media, destroyed journal state, or concurrent external writes.
It does not promise unconditional rollback.

## Portable names

The format targets Windows, Linux, and macOS.
This is a required support contract, not a claim of existing verification.

New labels use Unicode NFC normalization.
Accepted spelling remains visible.
Souvenirs must reject:

- Empty labels, leading or trailing whitespace, and trailing dots.
- Control characters, NUL, and whitespace other than ordinary spaces.
- The characters `<`, `>`, `:`, `"`, `/`, `\`, `|`, `?`, and `*`.
- The names `.` and `..`.
- Windows device names, including reserved forms with an extension.
- Labels whose first space-delimited token matches `+[0-9]+d`.

The duration-shaped token is reserved even when it is the entire label.
For example, `+2d Reunion` is invalid as a label.
Date-looking labels remain permitted when they meet the other rules.

All generated components must fit 255 UTF-8 bytes and 255 UTF-16 code units.
Validation includes complete asset names and sidecar suffixes, not only labels.
Names must not be truncated or silently substituted.
The permitted label length therefore depends on the complete generated names.

Extension recognition is ASCII-case-insensitive.
Import and rename preserve extension spelling, including case.
Case-only destination differences are conflicts under the portable comparison rules.

Collision checks must compare Unicode normalization and case-fold equivalents.
They must include planned entries, existing entries, and unmanaged content.
Different physical spellings that collide require user resolution.
Comparison must not replace native paths with normalized identities.
Repair must not silently normalize externally supplied names.

Component limits do not establish a full-path guarantee.
Validation must include the selected root and complete destination path.
Windows long-path support must not be assumed.
A transfer to another platform must validate paths for the target location before mutation.
The host filesystem can impose stricter limits.

## Format compatibility

The proposed configuration declares the incompatible format explicitly:

```yaml
format_version: 2
media_extensions: [jpg, jpeg, heic, cr3, dng, mp4, mov]
```

The extension list is illustrative.
XMP remains a sidecar, not a media asset.
This configuration contains no routing groups.

An absent version denotes the legacy layout, not this format.
Unsupported versions must refuse managed mutation.
Repair must not act as implicit adoption or conversion.
A version marker does not make old binaries safe.
Legacy binaries must not mutate a new-format catalog.

Legacy conversion requires a separate approved plan.
It must resolve conflicting counters from former media groups.
It must account for legacy date uncertainty and preserve unknown contents.
Date-only names require explicit clock assignment when metadata provides no usable time.
This document does not specify the conversion algorithm.
The [legacy comparison](legacy.md) summarizes the differences that conversion must address.

## Exports

Every physical collection directory can contain one real directory named exactly `exports`.
This includes dated collections and ongoing annual parts.
Its contents belong to the collection, not an inferred original owner.

The subtree is opaque payload.
It can contain edited photos, videos, sidecars, documents, nested folders, and empty directories.
No accepted datetime or canonical filename is required inside it.
Souvenirs must not rewrite its names, parse owner IDs, or flatten its hierarchy.
Export contents do not affect original pairing, counters, ranges, or year routing.
They are allowed contents, not repair or quarantine candidates.

Renaming, relocating, copying, or moving a complete collection carries its entire export subtree.
Changing an individual original's name or datetime leaves exports where they are.
Unknown or unmatched exports remain with the collection.
No automatic operation deletes or separates them.
Souvenirs does not maintain references inside editor projects.

Original-media scanning excludes export subtrees.
Selecting an export subtree itself must not promote its contents to originals.
Original inventory must distinguish this intentional exclusion from an unreadable original directory.
It must not claim that excluding exports establishes complete export coverage.

### Transfer boundary

Adding originals does not silently import a source export tree.
The plan must identify any such tree that remains at source.
The user can explicitly include a complete source export subtree.
That inclusion requires an absent destination `exports` directory.
An existing target requires a separate explicit export placement, not an automatic merge.

Adding further exports selects an explicit destination within `exports`.
Existing directories may contain the selected destination, but occupied entry names are conflicts.
Selected directory payloads require an absent destination directory.
Transferring a collection package includes the entire physical folder, not only recognized media.
One ongoing annual part is one physical package.
It is not a transfer of the whole multi-year collection.
Plans and outcomes must name the selected year explicitly.
Its destination collection directory must be absent.
It must not merge into another collection.
The operation must enumerate the complete export payload before mutation.
Unsupported entries or target-invalid names require resolution.
It must not follow symlink targets or omit unsupported entries silently.
Export names and relative paths remain unchanged during package transfer.

Copies use the [staged publication and verification contract](#copy-based-transfers-and-interruption) for every transferred regular file.
A move removes no selected source before all package destinations are complete and durable.
A partial package transfer is a recoverable failure, not successful completion.
Preservation covers file bytes and relative paths, not universal cross-platform filesystem attributes.

### Between managed catalogs

Transfers between distinct managed catalogs are initially copy-only.
This includes original and export transfers, not only complete packages.
Source removal is never part of the destination catalog's operation.
It requires a separate confirmed operation owned and audited by the source catalog.
That operation must satisfy the source's write eligibility, recovery gate, and exclusive lock.
Automatic cross-catalog move and source-deletion orchestration are outside this version.

A new managed copy requires both catalogs to be idle and free of unresolved operations.
It holds both operation locks while copying to prevent another Souvenirs writer from changing either scope.
If either lock is unavailable, it releases acquired locks and reports busy.
It must not wait indefinitely while holding the other catalog's lock.
The destination history records the source location and complete copy outcome.
No distributed transaction or shared audit database is required.

Recovery may continue the destination's matching pending operation after revalidating identities and preconditions.
Destination recovery always holds its operation lock.
It must reject unrelated unresolved operations.
Resuming source reads requires an eligible source and both operation locks.
Recovery that only verifies completed destinations needs no source access or source-side lock.

## Contract examples

| Situation                                                      | Required result                                                                                                 |
| -------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| A one-day contribution arrives for an existing week-long trip. | Explicit continuation adds it to that collection without creating another trip.                                 |
| An earlier date extends a collection across New Year.          | An approved range change moves the directory; existing item basenames and counters remain unchanged.            |
| Item `002` is removed while `001` and `003` remain.            | Repair leaves both names unchanged; the next addition uses `004`.                                               |
| Two source directories contain matching camera basenames.      | No pair or sidecar relationship crosses the directory boundary.                                                 |
| A scan has no usable datetime.                                 | An explicit arbitrary datetime is valid; automatic mtime, epoch, or midnight assignment is not.                 |
| A video has only an ambiguous UTC-versus-local timestamp.      | An explicit interpretation determines the planned datetime; the importing machine's timezone is not a fallback. |
| Two dated collection ranges overlap.                           | Explicit target selection determines membership; dates do not select or merge collections.                      |
| A label's first token is `+2d`.                                | Validation rejects the reserved duration-shaped token rather than changing the parsed event meaning.            |
| An incoming original uses `.JPG`.                              | Recognition accepts the extension and preserves its spelling; case-only destination conflicts still fail.       |
| Metadata disagrees with an admitted filename datetime.         | Repair reports the disagreement without replacing the accepted value.                                           |
| A copy stops while writing a large video.                      | Its partial bytes remain in reserved staging, not under the final media filename.                               |
| Destination read-back produces a different digest.             | Publication and source cleanup stop; the error identifies the affected file.                                    |
| Publication succeeds just before its audit update fails.       | Recovery reconciles the final file against the earlier durable intent and verified digest.                      |
| A move stops after some destination files appear.              | Recovery retains available copies, blocks unrelated mutations, and recognizes completed transfers.              |
| A rename stops before its progress record persists.            | Recovery reconciles durable intent with the actual entry locations without creating media copies.               |
| Two labels differ only by case or Unicode normalization.       | The namespace conflict requires resolution; no file or collection wins by traversal order.                      |
| Ongoing labels occupy different years but would become equal.  | Creation or rename rejects the semantic conflict despite free destination directories.                          |
| A source drive disappears after destination verification.      | Audited cleanup cancellation retains verified copies and permanently cancels that operation's deletions.        |
| The user reverts a label after adding exports.                 | A new rename plan restores the label and carries all current contents without deleting the additions.           |
| A late RAW or XMP arrives for an admitted item.                | Explicit owner selection can attach a vacant supported role without guessing from destination names.            |
| An event moves to another year and contains nested exports.    | Its entire export subtree moves unchanged, including unknown files and empty folders.                           |
| One annual part is copied to another catalog.                  | The plan names that year, copies its complete package, and does not delete the source.                          |
| A managed copy resumes after interruption.                     | Its own pending record permits recovery, not new-copy admission; unrelated pending work remains blocked.        |
