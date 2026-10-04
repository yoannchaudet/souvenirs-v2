# Storage and recovery

`.souvenirs/operations.sqlite` holds the original registry and audit log.
The registry records identity, expected content, and last confirmed location.
The audit records operations and recovery evidence.
Neither replaces [filesystem authority](catalog.md).

## Write profile

The first version supports copy-only imports, in-place admission, integrity checks, and same-filesystem organization renames.
Souvenirs must not delete import sources.
Collection-package transfers, direct transfers between catalogs, and historical undo are deferred.

Writes require local APFS on macOS.
The catalog, staging area, and managed destinations must share one volume and a rename-capable mount.
Nested mounts, network shares, synchronized folders, and unvalidated storage configurations must be rejected before mutation.
Read sources need not use APFS, but direct sources inside another managed catalog are unsupported.

Every writer connection must set and verify:

```sql
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = EXTRA;
PRAGMA fullfsync = ON;
```

These settings protect SQLite writes, not media files.
Copied files require their own persistence operations, including macOS `F_FULLFSYNC`.
Creation, publication, rename, and temporary-file removal must persist affected directory updates.
Unsupported operations or persistence failures must stop mutation with a contextual error.
The implementation must validate this complete storage profile before enabling writes.

## Original registry

Every admitted original must have one durable record with these conceptual fields:

| Field | Meaning |
| --- | --- |
| Original ID | Stable catalog-local identity for one original file occurrence. |
| Logical-item ID | Stable catalog-local identity shared by originals in one logical item. |
| Role | The original's supported role in that item. |
| SHA-256 and byte size | The accepted content baseline for the complete file. |
| Relative path | The last confirmed catalog-relative path, stored without lossy conversion. |
| Filesystem identity | Last-confirmed volume and file identifiers, including available creation or generation evidence. |
| Observed size and modification time | Last-confirmed filesystem observations used to detect ordinary changes between operations. |

A RAW+JPEG pair has two original IDs and two digests under one logical-item ID.
A single original also has both an original ID and logical-item ID.
Sidecars and opaque export contents have no original records or permanent baselines.

IDs must not derive from paths or hashes.
Organization renames and their guarded recovery reversal must preserve original IDs, logical-item IDs, and content baselines.
Committed steps update affected paths and filesystem observations together.
New occurrences receive new original IDs, even when reusing deleted paths.
Deleted IDs must not be reassigned.
IDs are not portable global identifiers.

Byte-identical originals may belong to different items or collections.
The digest is not a uniqueness constraint.
Hash equality must not automatically skip imports, merge records, infer ownership, or authorize deletion.
This design adds no automatic deduplication.

Copy admission uses the verified transfer digest without an additional full-file pass.
Explicit admission of in-place originals requires reading each complete file to establish its baseline.
Externally added originals remain unregistered until explicit admission.
Inventory must report them, not hide or silently register them.

## Integrity and reconciliation

Planning must compare current files with retained identity and stat evidence, not merely snapshot whatever occupies each recorded path.
Execution must recheck that evidence before mutation.
A discrepancy requires verification or explicit reconciliation.
Insufficient identity evidence must not silently identify a replacement as the old original.

An integrity check reads selected originals and compares their digest and size with recorded baselines.
It must report checked scope, mismatches, missing files, unreadable files, and absent baselines.
Incomplete coverage must not appear as a successful full check.
The check does not change media, organization, or accepted baselines.
It detects changed bytes, not their cause, and cannot restore damaged content.

Ordinary browsing and naming operations use [inexpensive preconditions](operations.md#preconditions), not full-file hashing.
A stored digest does not verify current bytes without a new read.
A detected content mismatch must block mutations affecting that original until explicit reconciliation.
Souvenirs must not silently overwrite its bytes, replace its baseline, or treat it as a new admission.
Edits belong in sidecars or exports.
Intentionally changed originals need an [admission workflow that remains unspecified](operations.md#workflows-needing-further-definition).

A missing path can mean relocation, deletion, or unavailable storage.
It must not automatically remove the record.
Hashes help reconcile displaced originals only when evidence identifies the occurrence unambiguously.
Multiple identical candidates require explicit resolution, not first-match selection.
Stable IDs and hashes do not replace filesystem identity checks.
Registry identity, path, or grouping changes require a confirmed, audited reconciliation plan.

## Audit and write eligibility

The database contains three tables:

| Table | Responsibility |
| --- | --- |
| `originals` | Original identity, grouping, baselines, paths, and retained filesystem evidence. |
| `operations` | Confirmed plans, scope, progress, errors, and completed or abandoned outcomes. |
| `steps` | Concrete actions, before/after paths, staging names, expected identity and digest evidence, and per-step state. |

Ordinary mutations require a healthy registry and audit log.
Initialization and explicitly approved rebuilding are controlled setup procedures, not permission to replace unknown state.
Initialization creates the tables and durable setup intent before publishing catalog configuration.
Interrupted initialization must recover or reconcile existing state rather than silently create another database.
Ordinary catalog reading requires neither registry nor audit state.
Persistent identity and integrity checks depend on registry records.

Hold an operating-system-managed exclusive lock on `.souvenirs/lock` throughout mutation or recovery.
The lock must not depend on the SQLite file being replaced during rebuilding.
Only one operation per catalog can be active.
Another writer must report busy; a stale marker file alone must not act as the lock.

The audit must record confirmed intent before filesystem mutation.
Over the operation's lifetime, it records:

- Operation identity, time, scope, and outcome.
- Original source names, old and new paths, affected original and logical-item IDs, and intended registry changes.
- Datetime origin: metadata-derived, user-assigned, or existing-canonical, including explicit timezone interpretation.
- Step progress, recovery identity evidence, staging ownership, verified digests, and copied, skipped, or unfinished files.

Completed history is append-only through Souvenirs.
Resolution and rename reversal append references to earlier steps.
They must not erase failures or rewrite completed history.
Registry updates describe current confirmed state without erasing earlier audit transitions.
External filesystem edits are not automatically audited.
The log is not tamper-proof.

The database is durable state, not a disposable cache.
Unresolved records and recovery artifacts must not be discarded.
Completed history can be archived only when no recovery remains pending.
Archiving must retain current original records and baselines.
A corrupt database must not be silently replaced.

An interrupted operation remains active and blocks unrelated mutations until recovery records a terminal outcome.
`completed` means every planned change is durable.
`abandoned` means unfinished work is explicitly stopped and retained results are recorded.
Neither outcome may conceal unresolved publication or rename conflicts.

## Staged copies

Execute one filesystem step at a time.
Imports complete target organization before publishing incoming files.
Every copied regular file, including sidecars and export payload, uses these states:

| State | Durable meaning |
| --- | --- |
| `planned` | Source, destination, unique staging path, and intended IDs are recorded before creating the temporary file. |
| `ready` | The staged copy is persisted and read-back verified. Its digest, size, and filesystem identity are recorded. |
| `done` | Publication and directory updates are persisted. Registration and completed progress commit together in SQLite. |

Create staging files exclusively and retain evidence of their ownership.
Compute SHA-256 while copying source bytes, persist the staged file and directory, then reopen and hash the complete staged file.
The source and staged digests must match before recording `ready`.
Any known source baseline must also match.

Publish with `renamex_np(..., RENAME_EXCL)` or its directory-relative equivalent.
It must not overwrite an occupied entry or fall back to another copy.
Unverified files must not appear at final paths.
The temporary file becomes the final file without another media copy.
Read, write, identity, verification, or persistence failures leave the step unfinished and report a contextual error.

For originals, the verified digest and size become the accepted baseline.
Sidecars and exports receive transfer verification, not permanent baselines or original records.
Process each item's originals before its sidecars.
A sidecar must not publish before its required owners are present.

Directory creation also requires recorded intent and persisted progress.
In-place admission hashes stable files and commits their records without staging or rewriting media.
Registry-only steps use `planned` and `done`.

## Interruption and recovery

Recovery first reconciles the unfinished step against actual files and durable evidence:

| Observation | Allowed action |
| --- | --- |
| Only a temporary file exists | Resume an identified, owned file, or abandon. Retain artifacts with uncertain ownership. |
| Neither temporary nor final file exists | Retry from the source or abandon the step. |
| A final file matches a `ready` step's recorded identity and digest | Persist directory updates and complete registration under the same IDs. Do not copy it again. |
| Paths or identities conflict with the record | Stop for explicit reconciliation. Do not overwrite, delete, or guess ownership. |

A recorded `done` step is not reimported.
A `ready` file can publish after its current identity and digest match the record, without reading the source again.
Resume must retain planned IDs and counters.
Missing or changed committed files are integrity findings, not permission to replay completed steps.
An interrupted operation is not successful completion.
Errors must identify completed files, unfinished work, and recovery paths.

### Abandon a copy operation

Abandonment requires explicit confirmation but does not require every planned destination to exist.
First reconcile any publication that occurs before its database update.
Any unfinished target-organization change must finish or reverse before abandonment releases the gate.
Keep completed files and stop unfinished work, even when the source drive is unavailable.
Remove only identified, owned temporary files.
Retain and report artifacts whose ownership is uncertain.

Record retained files, missing companions, skipped work, and residual artifacts before releasing the recovery gate.
Abandonment must revoke future execution of unfinished steps, including publication of retained staging files.
A new import requires a new plan.
Abandoned operations must not resume.

A partly imported RAW+JPEG item can remain a valid single-original item under its existing logical-item ID.
Missing companions must be reported rather than counted as transferred.
Created empty collections and partial export subtrees may remain, but the outcome must not claim a complete transfer.
Sources, original bytes already published, and accepted baselines remain unchanged.

## Rename recovery

Same-filesystem organization changes use native no-replace renames, not media copies.
Each step records before/after paths and expected object identity before execution.
After directory persistence, commit the changed registry paths, observations, and `done` progress together.
A folder rename must update every affected original path.

Recovery checks whether the expected object remains at the before-path or has reached the after-path.
Unexpected or ambiguous locations require explicit reconciliation.
The user can finish the plan or approve guarded reversal of its completed steps.
The recovery choice must be durable before executing it.

Reversal appends inverse rename steps in reverse order without erasing forward history.
Every inverse step requires matching identity and an unoccupied original path.
Inverse steps use the same intent, persistence, and registry-update protocol.
Reversal must preserve current bytes, original IDs, and baselines.
Conflicts block reversal rather than permitting overwrites.

Unfinished organization changes must not be silently abandoned with mixed old and new names.
Successful reversal records the original operation as abandoned after all inverse steps are durable.
This is recovery of unfinished work, not historical undo or restoration of deleted files or edits.

## Relocation, backup, and database loss

The audit must store catalog-relative destinations separately from external source locations, preserving native path identity.
Pending operations bind to the selected catalog root and its filesystem identity.
Relocation, cloning, or detected database restoration requires explicit revalidation before mutation.
Recovery must not silently use an earlier absolute root or resume old operations without revalidation.

Catalog backup requires quiescent files and a closed, consistent backup of the complete SQLite database.
A consistent rollback of both files and database cannot always be detected locally.

A missing or corrupt database permits filesystem browsing but blocks ordinary mutations.
Recovery restores a consistent backup or performs an explicitly approved fresh rebuild.
Rebuilding must preserve prior database files, SQLite auxiliary files, and staging artifacts as recovery evidence.
It must validate visible layout and resolve incomplete organization before restoring ordinary write eligibility.
It hashes current originals and assigns new IDs and baselines without claiming earlier identity, contents, or history.

Build the replacement database separately and persist it before installation.
Preserve prior database state at exclusive recovery locations rather than overwrite it.
An interrupted rebuild must not expose a partially populated replacement as ready for ordinary use.
Old staging files must not be published automatically or treated as originals.
Initialization and rebuilding use the same catalog lock and validated persistence profile.

SQLite transactions do not make filesystem changes atomic or hide intermediate states from external tools.
External tools must not edit or relocate affected files during mutation or recovery.
Read-back can use caches and does not independently prove physical-media persistence.
Before enabling writes, the implementation must exercise crashes around staging creation, publication, database commits, rename reversal, and rebuild installation.
The database is not a backup.
It stores no permanent media snapshots or hidden retention copies.
This contract promises neither unconditional rollback nor recovery from lost media, destroyed journal state, or concurrent external writes.
