# Operations

Every mutation follows the same pattern: plan, review, confirm, execute.
Execution writes a [journal](storage.md#journal) before changing files.
No operation overwrites an occupied destination or deletes a source.
Decisions apply to batches wherever possible, so large imports stay practical.

## Operation inventory

These are proposed operations, not command names.

### Read

| Operation | Purpose |
| --- | --- |
| Browse catalog | List collections, items, and findings from the [filesystem](catalog.md). |
| Check integrity | Verify collection directories against their [manifests](storage.md#integrity-checks). |
| Report duplicates | List byte-identical originals inside the catalog or between a source and the catalog. |
| Read history | Read completed and unfinished [journals](storage.md#journal). |

### Change

| Operation | Purpose |
| --- | --- |
| Initialize catalog | Create `.souvenirs.yml` and `.souvenirs/`. |
| Create collection | Declare a dated collection with its range, or an ongoing collection. |
| Import | Copy originals from a source into one selected collection. |
| Organize backlog | Copy a large source tree into proposed collections. See [Organize backlog](#organize-backlog). |
| Admit in place | Add existing canonical originals to their directory's manifest. |
| Rename collection | Rename one dated folder, or every annual folder of an ongoing collection. |
| Change dated range | Rename a dated folder, moving it to another year when the start year changes. |
| Change datetime | Rename an item's originals and sidecars and update the manifest. |
| Attach companion or sidecar | Add a file to a selected item's vacant supported role. |
| Add exports | Copy a file or subtree into a collection's `exports/`. |
| Convert legacy catalog | Organize a legacy Souvenirs catalog as a backlog source. See [Legacy conversion](#legacy-conversion). |

### Maintain

| Operation | Purpose |
| --- | --- |
| Recover | Finish an unfinished operation from its [journal](storage.md#interruption-and-recovery). |
| Cancel a copy | Keep published files and stop unfinished copy steps. |
| Rebuild index | Recreate `index.sqlite` from manifests and filenames. |

## Plans

A plan lists sources, destinations, accepted datetimes, and every planned file and manifest change.
It distinguishes transferred items, excluded items, and untouched source content.
Excluded content stays at the source.
Unknown content is never copied, renamed, or deleted implicitly.

Items with unresolved dates or relationships do not block the rest of the plan.
The plan groups them, and the user resolves or excludes each group.
Excluded groups stay at the source for a later import.

Planning validates names, counters, ranges, destination namespaces, and the [write profile](storage.md#write-profile).
Execution rechecks that destinations are still free and manifests are unchanged.
Drift requires a new plan, never silent replanning.
Naming and folder operations must not hash the media collection.

A saved source scan uses a versioned snapshot.
Version 1 records file identity, classification, size, and digest.
Version 2 also records normalized relationship metadata.
Planning rescans the source with the saved snapshot version.
This rule preserves version 1 fingerprint semantics.

A saved import plan uses one versioned envelope.
The envelope contains its version, confirmation state, source location metadata, and the digested import plan.
The import plan digest protects every field in the normalized plan document.
The digest does not protect confirmation state or source location metadata.
Version 2 records `ambiguous_sidecar` when sidecar ownership blocks a candidate item.
Readers continue to accept valid version 1 plans.

Initial import execution validates confirmation state before it mutates the catalog.
Initial import execution uses source location metadata to find each planned item file.
It compares each item file with the size and digest in the plan document.
Changing the source location does not change the planned catalog operation.

### Unhandled content

Scans and plans report these findings without acting on them:

| Finding | Meaning |
| --- | --- |
| Unknown content | A file with no recognized media, sidecar, or catalog role. |
| Orphan sidecar | A sidecar with no matching owner. |
| Ambiguous sidecar | A sidecar with more than one possible owner. |

## Duplicates

Souvenirs never removes duplicates automatically.
It reports them so the user can decide in bulk.

A plan reports two kinds:

| Kind | Default | Batch choice |
| --- | --- | --- |
| Source original already in the catalog | Import. | Skip all of them. |
| Several identical originals within the source | Import all. | Import one copy per digest. |

Skipped files stay at the source.
"One copy per digest" keeps the copy with the earliest accepted datetime, then the first component-wise source path.
The journal records every skipped file and the catalog path it duplicates.
Duplicate detection uses the [index](storage.md#index) for catalog content and the copy digest for sources.

## Organize backlog

This workflow turns a large, messy source tree into collections with few decisions.
It is the primary path for the existing 10-year archive.

1. Select a source tree. Souvenirs only reads it.
2. Scan every directory. Each direct directory is its own [relationship](catalog.md#relationships-and-sidecars) namespace.
3. Resolve datetimes by [source](catalog.md#sources). Group proposals and ambiguous values for batch confirmation.
4. Report [duplicates](#duplicates) and accept batch choices.
5. Propose collections, described below.
6. Let the user adjust proposals: rename, merge, split, retarget to an existing collection, or exclude.
7. Confirm and execute as one journaled operation.

Souvenirs proposes collections by sorting resolved items by accepted datetime.
A gap longer than the configured threshold starts a new proposed dated collection.
The default threshold is 24 hours.

A proposal takes its label from the source folder name when all its items share one folder.
Otherwise the label is empty and the user must supply one.
A proposal that overlaps an existing collection's range is flagged, never merged automatically.
The user can assign any proposal to an ongoing collection instead.

Unresolved items form their own group.
They stay at the source until the user resolves or excludes them.

## Import

Import copies selected originals into one explicitly selected collection.
Matching labels or dates never select or merge a collection automatically.

For a new dated collection, the plan proposes a range covering every resolved datetime.
The user accepts it or supplies a wider range.

For an existing dated collection, dates inside the range need no change.
Outside dates require a range extension, a datetime correction, or exclusion.
An earlier start date can move the folder to another year without renaming any file.

For an ongoing collection, items route to annual parts by accepted year.
Missing annual parts are created.
Creating or renaming an ongoing collection checks its label across all years.

## Organization changes

Renaming a collection renames folders only.
Files, manifests, and exports are unchanged.
An ongoing rename covers every annual part.

Changing a datetime renames the item's originals and sidecars, then updates the manifest.
[Counter rules](catalog.md#names-and-counters) decide whether the counter is kept.
Placement of a new date outside the collection's range remains an open question.

Folder renames carry exports and unknown content unchanged.
Plans warn that renames can break external editor references.

## Late companions

Attachment requires explicit selection of an existing item and every incoming file.
The role must be supported and vacant.
The incoming file takes the item's stem.
A late original is copied, verified, and added to the manifest.
Existing files are never replaced or rewritten.

## Exports

Import never includes a source export tree implicitly.
Adding exports requires an explicit destination inside `exports/`.
Occupied names are conflicts, never merged.
Export copies preserve bytes, relative paths, and empty directories.
They do not follow symlinks.
Every copied file follows the [verified-copy protocol](storage.md#verified-copies).

## Legacy conversion

A legacy Souvenirs catalog is a backlog source.
Conversion uses the [Organize backlog](#organize-backlog) workflow with two additions:

- Legacy folder names propose collection labels and ranges.
- Legacy filenames propose datetimes, confirmed as a batch.

Conversion copies into a new catalog and leaves the legacy catalog unchanged.
Legacy-specific name parsing remains an open question until the legacy format is documented here.

## Release validation

The Release configuration enables Hardened Runtime for the application and embedded Rust engine.
The build signs both executables with the same identity.

Run `just build-release` to create and verify a Developer ID signed archive.
The command requires one local Developer ID Application identity.
The verification checks both signatures, Hardened Runtime flags, and signing teams.
The verification also confirms that the app does not contain development engine overrides.
The command requests a secure timestamp by default.
Set `SOUVENIRS_TIMESTAMP=none` only for an offline local test build.
An artifact without a secure timestamp must not be notarized or released.

Set `SOUVENIRS_NOTARY_PROFILE` and run `scripts/notarize-release.sh APP`.
The profile must refer to credentials stored by `notarytool store-credentials`.
The command submits a ZIP archive, staples the accepted ticket, and validates the result.
The final validation includes Gatekeeper assessment.
The command does not print credentials.

Run `just test-representative-storage` to test APFS and exFAT.
The command creates disposable disk images and removes them after the test.
The test interrupts a verified copy before publication, resumes it, and checks bytes, the manifest, staging cleanup, and the terminal journal record.

Run `just test-smb-storage` to test SMB.
The command starts a disposable local Samba container and mounts its share.
The command uses temporary credentials and removes the container, mount, and files after the test.
The default image is `dockurr/samba:4.23.10`.
Set `SOUVENIRS_SMB_IMAGE` to test another fixed image.

The release operator must complete these environment-dependent checks:

- Run the signed archive on a separate supported Mac that does not contain the development certificate.
- Notarize and staple the final distribution artifact with the release Apple account.
- Repeat the SMB harness against the supported NAS model and production SMB configuration.
- Disconnect the NAS during copy, reconnect it, resume recovery, and run an integrity check.
- Connect each supported physical APFS and exFAT device and repeat interruption recovery.
- Confirm that Gatekeeper opens the downloaded artifact after quarantine is applied.

## Open questions

- Should conversion also support in-place renaming, to avoid doubling disk usage for a large archive?
- Should a move-items operation exist for moving originals between collections on the same volume?
- Where does a changed datetime go when it falls outside its collection's range?
- Should import optionally copy to a second destination, such as a backup drive?
- Which repair corrections, if any, should be automated after explicit confirmation?
- How does a user re-admit an intentionally changed original?
- Should counter resequencing ever exist as an explicit operation?
