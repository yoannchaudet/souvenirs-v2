# Catalog reading

**Status:** Proposed behavior contract.
This document does not describe implemented behavior.
It defines how Souvenirs interprets a format-version-2 catalog without the audit log.
The [catalog specification](catalog.md) wins when the two documents disagree.

## Purpose

Reading turns the directory tree into collections, logical items, and findings.
Every operation plan starts from a reading of the affected scope.
Apps and caches use the same reading through a streaming inventory.
Reading never mutates the catalog and never requires the audit log.

## Opening a catalog

Opening reads `.souvenirs.yml` from the selected root.

| Configuration state            | Result                                                           |
| ------------------------------ | ---------------------------------------------------------------- |
| `format_version: 2`            | The catalog opens.                                               |
| No `format_version` key        | Legacy catalog. Reading refuses, except for conversion planning. |
| Another `format_version` value | Unsupported catalog. Reading refuses.                            |
| Missing or unparsable file     | Not a catalog. Reading refuses.                                  |

Opening also reports the audit log status: `healthy`, `missing`, `corrupt`, `unsupported-schema`, or `recovery-required`.
This status affects write eligibility only.
Reading continues for every log status.

## Traversal

Reading walks a fixed depth.
It never follows symlinks.
It compares names by their native form.
It uses normalized comparisons only for the equality rules below.

### Root level

| Entry                            | Classification                          |
| -------------------------------- | --------------------------------------- |
| `.souvenirs.yml`                 | Configuration.                          |
| `.souvenirs/` directory          | Reserved. Not traversed, not a finding. |
| Directory named with four digits | Year directory.                         |
| OS noise                         | Noise (see [OQ-8][oq8]).                |
| Anything else                    | `UnknownRootEntry` finding.             |

### Year level

| Entry                                  | Classification              |
| -------------------------------------- | --------------------------- |
| Directory `{yyyy-MM-dd} [+Nd] {label}` | Dated collection directory. |
| Directory `z {label}`                  | Ongoing annual part.        |
| OS noise                               | Noise.                      |
| Anything else                          | `UnknownYearEntry` finding. |

A dated collection directory under the wrong year produces a `WrongStartYear` finding.
An invalid label or duration produces a `MalformedCollectionName` finding.
That directory is still traversed as an unparsed collection.
A valid but noncanonical duration spelling, such as `+02d`, produces a `NoncanonicalDuration` finding (see [OQ-4][oq4]).

At every level, a name that is not valid Unicode produces a `NonUnicodeName` finding.
Reading does not traverse or interpret that entry.

### Collection level

| Entry                                          | Classification                             |
| ---------------------------------------------- | ------------------------------------------ |
| Regular file with a recognized media extension | Asset candidate.                           |
| Regular file matching a sidecar pattern        | Sidecar candidate.                         |
| Real directory named exactly `exports`         | Export subtree. Excluded from originals.   |
| OS noise                                       | Noise.                                     |
| Other regular file                             | `UnknownFile` finding.                     |
| Other directory                                | `UnknownDirectory` finding. Not traversed. |
| Symlink or special entry                       | `UnsupportedEntry` finding.                |

A directory that differs from `exports` only by case is an `UnknownDirectory`.

## Logical items

Reading assembles logical items inside each collection directory only.

1. Parse each asset candidate as a canonical name.
2. Group canonical assets by their complete stem.
3. A stem group with one asset is a solo item.
4. A stem group with two assets in a [supported role combination][oq2] is a compound item.
5. Any other stem group is a `CounterCollision` finding. Its assets remain separate items.
6. Attach sidecars to the unique item whose stem and extension match a supported sidecar form.

A sidecar that matches no item is an `OrphanSidecar` finding.
A sidecar that matches more than one item is an `AmbiguousSidecar` finding.
Reading never attaches it to the first match.

A noncanonical asset is a `NoncanonicalName` finding.
It is still an item with no parsed fields.
Noncanonical RAW+JPEG basenames in one directory can form a proposed pair for adoption.
That proposal is not membership.

## Canonical validity

A canonical asset name meets every condition below:

- The date and time are valid (see [OQ-3][oq3]).
- The label matches the collection label (see [OQ-5][oq5]).
- The counter is a positive decimal with at least three digits.
- The date fits the dated range, or the year equals the annual part's year.
- The extension is recognized.

Each failed condition has its own finding:

| Condition failure                  | Finding             |
| ---------------------------------- | ------------------- |
| Unparseable name                   | `NoncanonicalName`  |
| Label mismatch                     | `LabelMismatch`     |
| Case-only label mismatch           | `LabelCaseMismatch` |
| Date outside the dated range       | `DateOutsideRange`  |
| Date in another year than the part | `WrongOngoingYear`  |

## Collections

A dated collection is one directory.
Its identity is its complete folder name in its year directory.

An ongoing collection is the set of annual parts whose labels are equal under the portable comparison.
Two parts in one year with equal labels produce an `OngoingLabelCollision` finding.
Parts whose labels differ only by spelling produce an `OngoingLabelSpelling` finding.
They still belong to one collection.

Reading reports the full ongoing collection only after a complete year scan.
An incomplete scan reports the parts it saw and marks the collection incomplete.

## Findings

Every finding has a path, a kind, and a coverage effect.

| Finding                   | Coverage effect | Actionable |
| ------------------------- | --------------- | ---------- |
| `UnknownRootEntry`        | Preserves       | Yes        |
| `UnknownYearEntry`        | Preserves       | Yes        |
| `WrongStartYear`          | Preserves       | Yes        |
| `MalformedCollectionName` | Preserves       | Yes        |
| `UnknownFile`             | Preserves       | Yes        |
| `UnknownDirectory`        | Defeats         | Yes        |
| `UnsupportedEntry`        | Defeats         | Yes        |
| `NoncanonicalName`        | Preserves       | Yes        |
| `LabelMismatch`           | Preserves       | Yes        |
| `LabelCaseMismatch`       | Preserves       | Yes        |
| `DateOutsideRange`        | Preserves       | Yes        |
| `WrongOngoingYear`        | Preserves       | Yes        |
| `CounterCollision`        | Preserves       | Yes        |
| `OrphanSidecar`           | Preserves       | Yes        |
| `AmbiguousSidecar`        | Preserves       | Yes        |
| `OngoingLabelCollision`   | Preserves       | Yes        |
| `OngoingLabelSpelling`    | Preserves       | Yes        |
| `NoncanonicalDuration`    | Preserves       | Yes        |
| `NonUnicodeName`          | Defeats         | Yes        |
| `ReadFailure`             | Defeats         | No         |
| `MetadataDisagreement`    | Preserves       | No         |

`MetadataDisagreement` appears only when the caller requests [metadata comparison](datetimes.md#repair-findings).
Noise and export subtrees are not findings.

## Coverage

Reading reports two separate coverage values.

**Original coverage** is `complete` when no emitted finding defeats coverage.
It is the only basis for removing unseen records from a cache.
It never authorizes deleting catalog files.

**Export coverage** is always `not-enumerated` for an ordinary reading.
Export subtrees are an intentional exclusion.
They do not make original coverage incomplete.
A caller that enumerates exports receives a separate export inventory.

Cancellation or a sink failure returns no coverage.
Partial results are never complete.

## Inventory interface

The streaming inventory keeps the properties of the original Souvenirs inventory:

- It emits batches of complete logical items, bounded by a nonzero item limit.
- A batch never spans collection directories or splits a compound item.
- Findings stream beside batches.
- Paths are native, lossless, and relative to the catalog root.
- Memory stays bounded by the largest collection directory.
- Cancellation is cooperative and returns no summary.

Each item exposes its parsed datetime, label, counter, members with roles, sidecars, size, modification time, and file identity where available.
Reading does not read media contents unless the caller requests metadata comparison.

[oq2]: open-questions.md#oq-2-compound-role-combinations
[oq3]: open-questions.md#oq-3-year-and-datetime-bounds
[oq4]: open-questions.md#oq-4-canonical-duration-token
[oq5]: open-questions.md#oq-5-label-equality-for-canonical-validity
[oq8]: open-questions.md#oq-8-operating-system-noise
