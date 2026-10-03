# Operations

**Status:** Proposed behavior contract.
This document does not describe implemented behavior.
It refines [catalog operations](catalog.md#catalog-operations) and the [audit log](catalog.md#audit-log-and-recovery).
The catalog specification wins when the two documents disagree.

## Purpose

This document defines the lifecycle that every mutation follows.
It defines the inputs, plan contents, and outcomes of each operation.
It does not define commands, flags, or screens.
A CLI or app presents these typed plans and outcomes.

## Lifecycle

Every mutation follows five stages:

1. **Prepare.** Souvenirs reads the catalog and any sources. It does not write.
2. **Draft.** Souvenirs builds a plan. The plan can contain required decisions.
3. **Decide.** The user resolves every required decision. Souvenirs rebuilds the plan after each change.
4. **Confirm.** The user confirms one executable plan. The plan has no open decisions.
5. **Execute.** Souvenirs records intent, mutates the filesystem, and records the outcome.

Preparing and drafting are read-only.
A dry run stops after the draft or executable plan.
A plan is an immutable value.
Each executable plan has a digest over its complete contents.
Confirmation binds to that digest.
Execution refuses a plan whose digest differs from the confirmed digest.

## Write eligibility

Execution requires every condition below.
Drafting checks them too and reports failures as blocking findings.

| Condition     | Requirement                                                                                      |
| ------------- | ------------------------------------------------------------------------------------------------ |
| Format        | `.souvenirs.yml` declares `format_version: 2`.                                                   |
| Audit log     | `.souvenirs/operations.sqlite` opens, passes an integrity check, and has a supported schema.     |
| Writer lock   | The operation holds the catalog's exclusive writer lock (see [OQ-18][oq18]).                     |
| Recovery gate | No operation is in recovery-required state, except the operation that recovery is resolving.     |
| Root identity | The catalog root identity matches the log, or the user revalidated it (see [OQ-21][oq21]).       |
| Storage       | Every destination is on supported storage and shares the staging filesystem (see [OQ-19][oq19]). |

A plan can be drafted without the writer lock.
Execution acquires the lock and then rechecks all preconditions.

## Plan contents

Every plan states:

- The operation kind and the catalog root.
- The selected scope, such as a collection, an annual part, or source paths.
- Each entry, with its source, destination, and action.
- Each source removal, when the operation moves files.
- Each explicit exclusion and its reason.
- Each untouched source entry that the user can mistake for selected content, such as a source `exports` tree.
- Each warning, including external relinking warnings for changed paths.
- The precondition evidence for each source and destination.

Entry actions are:

| Action          | Meaning                                                 |
| --------------- | ------------------------------------------------------- |
| `create-dir`    | Create a real directory at an absent path.              |
| `rename`        | Move a directory entry with a native no-replace rename. |
| `copy`          | Copy a regular file through staged publication.         |
| `remove-source` | Remove a transfer source after the cleanup barrier.     |

A plan never contains an overwrite, a delete of catalog content, or a metadata write.

## Required decisions

A draft plan lists each decision that blocks execution.
Each decision names its affected items and its permitted resolutions.

| Decision                      | Permitted resolutions                                                |
| ----------------------------- | -------------------------------------------------------------------- |
| Missing datetime              | Assign a datetime, or exclude the item.                              |
| Datetime needs interpretation | Choose an [interpretation](datetimes.md#interpretations), or assign. |
| Datetime conflict             | Choose one member's value, or assign.                                |
| Date outside dated range      | Extend the range, assign another datetime, or exclude the item.      |
| Ambiguous pair                | Choose members, or exclude.                                          |
| Ambiguous or orphan sidecar   | Choose an owner, or exclude.                                         |
| Name collision                | Choose another label or target, or exclude the colliding entry.      |
| Invalid name for the target   | Choose another label, or exclude.                                    |
| Unsupported source entry      | Exclude.                                                             |

A resolution applies to the selected items only.
A batch resolution applies one choice to a named set of items.
The audit log records every resolution with the confirmed plan.

## Outcomes

| Outcome                      | Meaning                                                                  |
| ---------------------------- | ------------------------------------------------------------------------ |
| `completed`                  | Every planned change and source removal finished.                        |
| `completed-sources-retained` | Every destination finished; the user canceled remaining source removals. |
| `no-op`                      | The plan had no entries. Nothing changed.                                |
| `refused`                    | A precondition failed before the first mutation. Nothing changed.        |
| `interrupted`                | Execution stopped after the first mutation. Recovery is required.        |

Every outcome names the operation identity.
An `interrupted` outcome also names completed entries, remaining entries, staged files, and remaining sources.

Cancellation is cooperative.
Cancellation before the first mutation produces `refused` with reason `cancelled`.
Cancellation after the first mutation stops at the next step boundary.
It produces `interrupted`.
It never rolls back completed steps.

## Catalog operations

### Initialize catalog

**Input:** a root path that is absent or empty.

Initialization creates `.souvenirs/`, the audit log, and `.souvenirs/staging/`.
It records an initialization entry with the root identity.
It then publishes `.souvenirs.yml` with `format_version: 2` and the configured extensions.
A root that contains `.souvenirs.yml` is never initialized again.

### Reestablish history

**Input:** a format-version-2 catalog whose audit log is missing.

This operation creates a new log (see [OQ-17][oq17]).
Its first entry records a history discontinuity.
It refuses when `.souvenirs/staging/` contains entries.
It refuses when a log file exists, even a corrupt one.
A corrupt log requires manual recovery or archival outside Souvenirs.

### Create collection

**Input:** a collection kind and a label.
A dated collection also needs an explicit range.
An ongoing collection also needs an explicit year (see [OQ-15][oq15]).

The plan contains one `create-dir` entry.
Validation applies the [portable name rules](catalog.md#portable-names).
An ongoing label must be free in every year.
Year scanning must be complete.

### Add originals

**Input:** source paths, a target, and a transfer mode.
The target is a new collection or an explicitly selected existing collection.
The mode is `copy` or `move`.

Preparation scans each source directory without following symlinks.
It applies the [local relationship rules](catalog.md#local-relationships) in each directory.
It skips source `exports` subtrees and lists them as untouched.
It reads metadata and builds [datetime proposals](datetimes.md).

The draft plan:

- Proposes a range for a new dated collection from every resolved datetime.
- Routes ongoing originals to annual parts by accepted year.
- Allocates counters per date above the highest existing counter, in accepted-datetime order, with source paths as tie-breaks.
- Gives compound members and sidecars one shared stem.
- Keeps each sidecar's suffix and each extension's spelling.

A cross-filesystem `move` copies through staged publication and then removes sources after the cleanup barrier.
A same-filesystem `move` can use direct renames when the plan says so (see [OQ-12][oq12]).
A `copy` never removes sources.

The user can include a complete source `exports` subtree.
That inclusion requires an absent destination `exports` directory.

### Rename collection

**Input:** a collection and a new label.

For a dated collection, the plan renames the directory and every canonical original and sidecar.
For an ongoing collection, the plan renames every annual part and its canonical contents.
It preserves datetimes, counters, counter widths, extension spellings, and sidecar suffixes.
It moves `exports` and unknown contents with each directory unchanged.
Noncanonical direct children keep their names (see [OQ-16][oq16]).
A namespace conflict blocks the plan.

### Change item datetime

**Input:** selected logical items and a new datetime for each.

The plan renames each member and sidecar.
A clock-only change keeps the counter.
A date change keeps the counter when it is free on the new date.
Otherwise, it allocates above that date's highest counter.
A new date outside a dated range requires a range decision.
A new year for an ongoing item moves it to that year's annual part.
The plan creates a missing annual part.
Other items are never renumbered.

### Change dated range

**Input:** a dated collection and a new range.

Every current original date must fit the new range.
The plan renames the collection directory.
A new start year moves the directory under another year directory.
Item names do not change.

### Attach companion

**Input:** an existing logical item and incoming files.

Each incoming file must fill a [supported, vacant role](open-questions.md#oq-2-compound-role-combinations).
Each incoming file takes the item's stem.
Sidecars keep their suffixes.
The plan uses `copy`, or `copy` and `remove-source` for a move.

### Add exports

**Input:** source files or directories, a collection directory, and a destination path inside its `exports` directory.

Each selected entry keeps its name.
An occupied entry name is a conflict.
A selected directory requires an absent destination directory.
Transfer mode follows [OQ-11][oq11].

### Transfer collection package

**Input:** one physical collection directory and a destination catalog.

The plan enumerates the complete directory before mutation, including `exports` and unknown content.
The destination collection directory must be absent.
For an annual part, the plan names the year.
An equal ongoing label in the destination blocks the plan (see [OQ-14][oq14]).
Canonical originals keep their names and accepted datetimes.
Between managed catalogs, the operation is copy-only.
It holds both writer locks without waiting.

### Repair

**Input:** a catalog.

Repair starts with a read-only check.
The check produces the findings defined in [reading](reading.md#findings).
The check never mutates the catalog.
Each actionable finding offers a separate resolution plan (see [OQ-9][oq9]).
Each resolution plan follows this lifecycle and is one audited operation.

### Recover

**Input:** an operation in recovery-required state.

Recovery compares the disk with recorded intent, progress, and digests.
It classifies each step as done, not started, or uncertain.
An uncertain step requires an explicit decision.
Recovery can complete remaining publications from verified staged files.
It can resume source reads when the source is eligible.
Source removal requires fresh confirmation.
Recovery never allocates new counters for already-published entries.

### Cancel remaining cleanup

**Input:** an operation with verified destinations and pending source removals.

The operation durably revokes deletion authority.
It records the outcome `completed-sources-retained`.
It then releases the recovery gate.

### Revert organization

**Input:** a completed rename, datetime change, or range change.

The operation drafts a new plan from the recorded before-value and the current disk.
It refuses when the current value differs from the recorded after-value.
It refuses when affected objects are no longer identifiable.
It follows the rules of the corresponding forward operation.

[oq9]: open-questions.md#oq-9-what-repair-mutates
[oq11]: open-questions.md#oq-11-add-exports-transfer-mode
[oq12]: open-questions.md#oq-12-same-filesystem-move-import
[oq14]: open-questions.md#oq-14-annual-part-transfer-into-an-existing-ongoing-label
[oq15]: open-questions.md#oq-15-empty-ongoing-collections
[oq16]: open-questions.md#oq-16-rename-with-noncanonical-members
[oq17]: open-questions.md#oq-17-missing-audit-log
[oq18]: open-questions.md#oq-18-writer-lock-mechanism
[oq19]: open-questions.md#oq-19-staging-and-mount-points
[oq21]: open-questions.md#oq-21-clone-and-restore-detection
