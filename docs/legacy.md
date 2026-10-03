# Legacy comparison

**Status:** Reference comparison for the proposed format.
The original behavior is the implemented [Souvenirs guide][guide] at commit `5d8202a`.
The proposed behavior is the [catalog specification](catalog.md).
It applies only to the new format.

## Comparison

| Topic               | Original Souvenirs                                                                                           | Proposed contract                                                                                                |
| ------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------- |
| Media folders       | Separate configurable-extension [Photos and Videos groups][layout].                                          | Mixed media in one collection directory; extensions control recognition only.                                    |
| Organizational unit | [Dated and global events][events], represented independently in each media group.                            | Dated and ongoing collections independent of media type and import batches.                                      |
| Dated range         | [Separate ranges per group][routing], derived during import and repair.                                      | One accepted range across all media; changes require explicit approval.                                          |
| Later contributions | Import reuses only a complete matching derived folder name.                                                  | Explicit continuation targets the existing collection regardless of the new batch's range.                       |
| Ongoing layout      | One `z {event}` directory per year and media group.                                                          | One mixed-media annual directory per ongoing collection.                                                         |
| RAW+JPEG inference  | [Scan-wide basename matching][pairing]; non-JPEG extensions need not be RAW.                                 | Local, unambiguous supported RAW+JPEG combinations only.                                                         |
| Sidecar ambiguity   | Scan-wide matching can attach an ambiguous XMP to the first stable match.                                    | Local matching; unresolved ownership requires action, never first-match assignment.                              |
| Accepted datetime   | [Date-only filenames with metadata, mtime, and epoch fallbacks][dates].                                      | Required second-precision datetime in every original filename; arbitrary user assignment is explicit.            |
| Timestamp context   | Metadata and local-time fallback rules determine dates.                                                      | Recorded local clocks are preserved; instant-only or ambiguous values require an explicit interpretation.        |
| Datetime authority  | Repair derives canonical dates using import conventions.                                                     | Valid filename datetimes remain authoritative; metadata disagreement is a finding.                               |
| Counters            | [Per-day counters][counters] within each media-group event.                                                  | One per-day namespace across all media in each physical collection directory.                                    |
| Repair and gaps     | [Repair closes gaps][repairs] and can change dated ranges.                                                   | Gaps and accepted ranges remain valid; no automatic resequencing or range shrinkage.                             |
| Failure and retry   | [Partial progress remains; reimport can duplicate transfers][transfers].                                     | Durable audit and recovery, guarded resume, and a terminal outcome for canceled source cleanup.                  |
| Copy publication    | Exclusive destinations and attempted incomplete-file cleanup; no staged-publication guarantee in the guide.  | Same-filesystem staging, SHA-256 destination read-back, and per-file atomic no-replace publication.              |
| History and revert  | Recovery information accompanies partial failures; no durable catalog operation history is specified.        | Append-only completed audit history and guarded organization reverts without permanent media snapshots.          |
| Atomicity           | [No whole-operation rollback or multi-file isolation][safety].                                               | Recoverable operations, still without atomic visibility or unconditional rollback.                               |
| Configuration       | [Unversioned extension assignments][config] for fixed groups.                                                | Explicit incompatible format version and a flat media-extension list.                                            |
| Name portability    | Accepted labels can include [platform-specific characters][safety].                                          | Conservative Unicode labels, portable collision checks, and component plus target-path validation.               |
| Companions          | RAW+JPEG pairing and XMP association during scanning.                                                        | Local inference plus explicit late-member and still/motion attachment; no automatic arbitrary photo/video pairs. |
| Exports             | [Dated-event preservation and name-based selective ownership][exports].                                      | Opaque collection payload in dated and ongoing folders; complete explicit transfer and no inferred owner links.  |
| Retained principles | Ordinary files, readable dates and labels, byte-preserving sidecars, plans, confirmation, and no overwrites. | Retained; audit history supports recovery and guarded reverts, not a separate authoritative catalog.             |

## Conversion obligations

A legacy catalog has no `format_version` key in `.souvenirs.yml`.
Souvenirs2 must not mutate a legacy catalog.
It can read a legacy catalog only to prepare a conversion plan.

The conversion algorithm is not specified.
A conversion plan must address these differences:

- Merge `photos/` and `videos/` parts of the same event into one collection directory.
- Resolve counter collisions between former media groups on the same date.
- Union per-group dated ranges into one accepted range.
- Assign a clock time to every date-only filename.
  Usable metadata can propose the time.
  Otherwise, the user must assign it explicitly.
- Report `1970-01-01` names as probable placeholders, not accepted dates.
- Keep legacy exports as an opaque subtree.
  Legacy owner-stem associations are not maintained after conversion.
- Preserve unknown contents and report them.
- Create the audit log before the converted configuration is published.

Conversion is one confirmed operation in the new catalog's audit history.
It must not run as a side effect of repair.

[guide]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md
[layout]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#catalog-layout
[events]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#events
[routing]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#event-routing
[pairing]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#pairing-and-sidecar-attachment
[dates]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#file-dates
[counters]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#filenames-and-counters
[repairs]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#repairs-and-flags
[transfers]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#transfers-failures-and-retries
[safety]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#mutation-safety-and-platform-limits
[config]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#configuration-and-registry
[exports]: https://github.com/yoannchaudet/souvenirs/blob/5d8202a/docs/guide.md#edited-exports
