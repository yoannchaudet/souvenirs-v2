# Datetime proposals

**Status:** Proposed behavior contract.
This document does not describe implemented behavior.
It refines [accepted datetimes](catalog.md#accepted-datetimes) in the catalog specification.
The catalog specification wins when the two documents disagree.

## Purpose

Import must propose an accepted datetime for every original logical item.
This document defines how Souvenirs reads metadata and classifies each proposal.
It also defines the interpretations that a user can apply to a proposal.
Souvenirs never writes metadata.

## Proposal kinds

Every metadata value that Souvenirs reads has one kind:

| Kind        | Meaning                                                        | Plan effect                                |
| ----------- | -------------------------------------------------------------- | ------------------------------------------ |
| `local`     | A local date and clock, with or without a recorded offset.     | Proposed as recorded.                      |
| `instant`   | A known UTC instant without local context.                     | Requires an interpretation.                |
| `ambiguous` | A date and clock whose UTC-versus-local convention is unknown. | Requires an interpretation.                |
| `date-only` | A calendar date without a usable clock time.                   | Requires explicit assignment.              |
| `none`      | No usable value.                                               | Requires explicit assignment or exclusion. |

A `local` value keeps its recorded clock.
A recorded offset does not change the clock.
The offset stays in the original metadata.
Souvenirs records it in the audit log as evidence.

## Field precedence

Souvenirs reads the fields below in order.
The first usable field of each asset determines that asset's value.
Later fields are not consulted for that asset.

### Still images

This table applies to JPEG, HEIC, TIFF-based RAW, and other EXIF-bearing formats.

| Order | Field                                                             | Kind        |
| ----- | ----------------------------------------------------------------- | ----------- |
| 1     | EXIF `DateTimeOriginal`, with `OffsetTimeOriginal` when present   | `local`     |
| 2     | EXIF `DateTimeDigitized`, with `OffsetTimeDigitized` when present | `local`     |
| 3     | EXIF GPS `GPSDateStamp` with `GPSTimeStamp`                       | `instant`   |
| 4     | EXIF GPS `GPSDateStamp` alone                                     | `date-only` |

EXIF `DateTime` (IFD0) is not used.
It records the last modification, not capture.

Sub-second fields are read and then discarded without rounding.

### Videos

This table applies to QuickTime and ISO base media files, including MOV and MP4.

| Order | Field                                                      | Kind        |
| ----- | ---------------------------------------------------------- | ----------- |
| 1     | `com.apple.quicktime.creationdate` with an ISO 8601 offset | `local`     |
| 2     | `com.apple.quicktime.creationdate` without an offset       | `local`     |
| 3     | `moov/mvhd` `creation_time`                                | `ambiguous` |
| 4     | `moov/trak/tkhd` `creation_time` of the first video track  | `ambiguous` |

The ISO base media format defines `creation_time` as UTC.
Many cameras write local clock values to that field.
Souvenirs therefore treats it as `ambiguous`.

Other container formats have no supported fields in the first version.
Their items have kind `none`.

## Usable values

A field value is unusable when one of these conditions is true:

- The value does not parse as a valid date, or as a valid date and time.
- All date and time digits are zero.
- The value decodes to the QuickTime epoch `1904-01-01 00:00:00`.
- The date is before `1900-01-01` (see [OQ-23](open-questions.md#oq-23-unusable-metadata-values)).

A value later than the planning time plus one day is usable but suspicious.
The plan shows a warning for each suspicious value.

## Logical item proposals

The primary asset's value comes first:

| Compound item | Primary member |
| ------------- | -------------- |
| Solo asset    | The asset.     |
| RAW+JPEG      | The RAW asset. |
| Still/motion  | The still.     |

When the primary has kind `none`, the other member's value becomes the proposal.
Its origin is `companion`.

When both members have usable values, Souvenirs compares them at second precision after any interpretation.
Different values are a conflict.
A conflict requires explicit resolution.
The plan can offer "use the primary value" as a batch resolution.

When no member has a usable value, Souvenirs can read an attached XMP sidecar.
It reads `exif:DateTimeOriginal`, then `xmp:CreateDate`, then `photoshop:DateCreated`.
These values have origin `sidecar` and the kind that their text implies.
A sidecar value is always shown for confirmation (see [OQ-22](open-questions.md#oq-22-metadata-field-precedence)).

Filesystem times are never read as proposals.

## Interpretations

An interpretation turns an `instant` or `ambiguous` value into a local clock value.
The user chooses one interpretation for one item or for a selected batch.

| Interpretation | Result                                                                         |
| -------------- | ------------------------------------------------------------------------------ |
| Keep clock     | Use the recorded date and clock unchanged.                                     |
| Fixed offset   | Treat the value as UTC and add the selected offset.                            |
| Named timezone | Treat the value as UTC and convert it with the selected IANA timezone's rules. |

A named timezone applies the offset in effect at that instant.
Daylight-saving transitions therefore produce different offsets within one batch.
The plan shows each resulting datetime.

The importing machine's timezone is never a default.
No interpretation is preselected.

## Explicit assignment

The user can assign any valid datetime to any item.
An assignment overrides every proposal for that item.
A date-only value can become a proposal only after the user supplies a clock time.
The plan never fills a clock time with midnight or another placeholder.

## Recorded origin

The audit log records one origin for every admitted logical item:

| Origin               | Source of the accepted datetime                                             |
| -------------------- | --------------------------------------------------------------------------- |
| `metadata`           | The primary asset's field.                                                  |
| `companion`          | The other compound member's field.                                          |
| `sidecar`            | An XMP sidecar field, confirmed by the user.                                |
| `filename`           | A canonical-looking source name, confirmed by the user (see [OQ-13][oq13]). |
| `user`               | An explicit assignment.                                                     |
| `existing-canonical` | An admitted canonical name carried by a package or managed transfer.        |

The log also records the field name, raw value, recorded offset, and any interpretation.
Reading the catalog does not need these records.

## Repair findings

Catalog reading can compare an admitted filename datetime with current metadata.
A difference is a `MetadataDisagreement` finding.
The finding never changes the accepted value.
Metadata comparison is optional because it reads file contents.

[oq13]: open-questions.md#oq-13-canonical-looking-source-names
