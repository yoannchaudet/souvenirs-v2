# Catalog

Folders and filenames define collection membership, labels, accepted datetimes, and supported compound relationships.
Reading that organization does not require SQLite.
The [original registry](storage.md#original-registry) records identity and expected content without overriding the filesystem.

| Term | Meaning |
| --- | --- |
| Catalog | A root directory containing collections, configuration, and operational state. |
| Collection | A named group of media stored in one dated folder or in annual folders for an ongoing activity. Year folders organize browsing, not collection membership. |
| Original, or asset | A recognized original photo or video. Its complete bytes, including embedded metadata, are immutable after admission. |
| Logical item | One original or a supported two-original compound item, with its associated sidecars. |
| Sidecar | A supported mutable metadata or edit-data file associated with an original or compound item. |
| Export | Mutable content inside a collection's opaque `exports/` subtree. |
| Canonical name | A name that follows this format. |

Immutability is a content contract, not a filesystem permission guarantee.
A file's role determines this contract: a JPEG in `exports/` is mutable export content.

## Layout

```text
{catalog}/
  .souvenirs.yml
  .souvenirs/
    lock
    operations.sqlite
    staging/
  {yyyy}/
    {start-yyyy-MM-dd} [+Nd] {label}/
      {item-yyyy-MM-dd} {HH-mm-ss} {label} {NNN}.{ext}
      exports/
    z {label}/
      {item-yyyy-MM-dd} {HH-mm-ss} {label} {NNN}.{ext}
      exports/
```

Square brackets mark an optional token, not literal characters.
Originals and sidecars live directly in collection directories.
Photos and videos share folders.
Extensions control recognition, not routing.

`.souvenirs/` contains the registry, audit history, staging, SQLite auxiliary files, and recovery artifacts.
These are not media or collection members.
Repair must not quarantine this directory.

### Dated collections

A dated collection stays in one directory under its accepted start year, even across New Year.
Its accepted range must contain every original item's calendar date.
`+Nd` adds `N` days to the start.
For example, `+2d` spans three calendar dates.
A single-day collection omits the token.
Original filenames use item datetimes, not the collection start date.

Exports do not contribute dates.
Removing items must not shrink the accepted range automatically.
Empty collections remain valid.
Creating an empty dated collection requires an explicit range.
Ranges may overlap, and start dates may match if complete folder names remain distinct.

### Ongoing collections

An ongoing collection uses `z {label}` under each represented year.
Each annual part contains originals accepted in that year.
The `z` prefix sorts ongoing parts after dated collections.
Annual parts limit directory size.
The collection spans multiple physical folders.
Empty annual parts remain valid.

Matching labels across years identify the same ongoing collection under the [portable comparison rules](#portable-names).
Dated and ongoing collections occupy separate namespaces and may share a label.
[Creation and rename](operations.md#creation-and-continuation) must check ongoing labels across all years.

## Accepted datetimes

Every original item requires a valid calendar date and 24-hour clock time at second precision.
This is an organizational wall-clock value, not a universal instant or proof of capture time.
The user can explicitly assign any valid datetime.

Souvenirs proposes usable capture metadata from the primary original.
Supported companion metadata can supply a value when the primary has none.
Conflicting usable values require explicit resolution.
Exact primary roles, usable capture fields, and metadata precedence remain unspecified.
Unresolved choices must not become silent defaults.

| Source value | Required interpretation |
| --- | --- |
| Recorded local date and clock | Preserve those fields regardless of the importing machine's timezone. |
| No timezone | Do not guess a conversion. |
| Known UTC instant without capture-local context | Require explicit use of UTC clock values or conversion through a selected offset or timezone. |
| Ambiguous UTC-versus-local timestamp | Require explicit interpretation, including MP4/MOV fields with an unknown convention. |
| Fractional seconds | Discard the fraction without rounding. |
| Date without a clock, or no usable value | Require explicit assignment, metadata correction, or exclusion before admission. |

One interpretation can apply to a selected batch.
The plan must show resulting datetimes.
The audit must record their origin and interpretation.
Source offsets and fractions remain in metadata, not additional filename fields.
Wall-clock sorting does not guarantee chronological order across travel or daylight-saving transitions.

Filesystem modification time is not a capture-datetime substitute.
Souvenirs must not silently assign epoch values, midnight, or other placeholders.
Collection ranges do not assign item datetimes.
There is no automatically populated undated collection.

After admission, the canonical filename remains authoritative during repair or admission of externally copied canonical files.
Metadata disagreement is a finding, not permission to replace the accepted value.
Changing embedded metadata also changes immutable bytes and is subject to [integrity checks](storage.md#integrity-and-reconciliation).

## Relationships and sidecars

Inferred relationships require files in the same direct directory, during both source discovery and catalog scanning.
Recursive scans keep those relationship namespaces separate.
Basenames, timestamps, and hashes must not infer cross-directory ownership.
Later imports must not infer ownership from existing destination files.

Discovery must consider all recognized direct children, not only selected files, when evaluating ambiguity.
Every attached member must appear in the confirmed transfer set.
An unresolved relationship blocks execution unless the user resolves or explicitly excludes it.
Excluded or orphan sidecars stay at source unless a separate action handles them.

| Relationship | Rule |
| --- | --- |
| Inferred RAW+JPEG | Require the same basename and an unambiguous supported combination. An arbitrary non-JPEG extension does not establish a RAW role. |
| Still/motion compound | Require explicit association. Matching photo/video basenames must not pair automatically. |
| Existing canonical compound | Recognize a shared complete canonical stem and unambiguous supported roles without database or attachment history. |
| Sidecar | Require one unambiguous local owner. Traversal or sorted order must not choose among candidates. |

A basename or canonical stem is the filename without its asset extension.
Compound originals share one stem, accepted datetime, and counter.
Registry grouping must agree with these relationships.
Automatic phone still/motion pairing is not required.
Explicit pairing does not promise vendor-specific Live Photo playback or metadata rewriting.

Sidecar recognition uses the [configured extensions](#configuration).
An extension identifies a candidate, not its owner.
Automatic association requires a supported filename rule for that format.
Explicit owner selection must still produce a supported, unambiguous catalog-sidecar name.

Sidecar naming forms are:

```text
{basename}.{sidecar-ext}
{basename}.{asset-ext}.{sidecar-ext}
{basename}_{nn}.{asset-ext}.{sidecar-ext}
```

XMP supports all three forms.
Other formats use only their declared naming forms, not every XMP rule automatically.
Adding `xml` must not enable generic XML ownership inference.
XML association requires a specific supported rule or explicit owner selection.
One item can have multiple sidecars in different formats or supported versions.

Sidecars follow their item through approved organization changes.
Souvenirs must preserve their naming suffixes and current bytes.
Souvenirs treats sidecar contents as opaque data without interpreting or rewriting them.
External tools can edit them between operations.
Byte preservation and filename association do not guarantee external editor compatibility.

Initial sidecar support covers per-file metadata and edit data.
Proxy/thumbnail associations (`.lrv`, `.thm`) and AVCHD directory interpretation are outside this scope.
Such content can remain opaque payload in an explicitly selected export subtree.
Adding extensions does not enable these vendor-specific relationships or package handling.
Scans and plans must report [unhandled content](operations.md#unhandled-content), not silently imply ownership or successful attachment.

## Names and counters

The asset pattern is `{item-yyyy-MM-dd} {HH-mm-ss} {label} {NNN}.{ext}`.
Hyphens replace Windows-forbidden clock colons.
Canonical validity requires a valid datetime, matching collection label, and positive counter.
The date must fit the dated range or ongoing part's year.
Noncanonical names, conflicting counters, or ambiguous compound roles require resolution.

Each date in each physical collection directory has one counter namespace across all media.
Compound members and sidecars share a counter.
Separate logical items must not share the same date and counter.
New counters start at `001` and widen beyond three digits.
Allocation sorts new items by accepted datetime, then source path.
It must allocate above that date's highest existing counter without changing existing names, counter values, or widths.

Gaps are valid and must not be filled automatically.
For example, if `002` disappears while `001` and `003` remain, the next addition uses `004`.
Counters represent allocation order, not shooting order.
Later additions can therefore have earlier datetimes without renumbering existing items.

A clock-only change preserves the counter.
A calendar-date change preserves it if free in the new namespace.
Otherwise, allocation continues above that date's highest counter.
Other items must not be renumbered.
Resequencing is a possible later explicit operation, not routine import, repair, or rename behavior.

Paths and counters are not permanent identifiers.
Deleted counters are not reserved, so a deleted name can later identify different media.
Approved path changes can break external references.

## Exports

Every dated collection or ongoing annual part can contain one real directory named exactly `exports`.
It can hold arbitrary edited media, sidecars, documents, nested folders, unknown files, and empty directories.
It requires no canonical names or accepted datetimes.

Exports belong to the physical collection, not inferred original owners.
Souvenirs must not rewrite their names, flatten their hierarchy, or automatically delete or separate them.
They do not affect original pairing, counters, ranges, or year routing.
They are not repair or quarantine candidates.
Changing an individual original's name or datetime leaves exports in place.
[Whole-folder organization renames](operations.md#organization-and-repair) carry them unchanged.

Original scanning and integrity checks exclude export subtrees, even when the user selects the subtree itself.
Inventory must distinguish intentional exclusion from unreadable original directories.
This exclusion does not establish complete export coverage.

## Portable names

The format targets Windows, Linux, and macOS.
These naming rules do not extend initial [write support](storage.md#write-profile) beyond macOS/APFS.
New labels use Unicode NFC normalization while retaining accepted spelling.
Souvenirs must reject:

- Empty labels, leading or trailing whitespace, trailing dots, control characters, NUL, and whitespace other than ordinary spaces.
- `<`, `>`, `:`, `"`, `/`, `\`, `|`, `?`, and `*`.
- `.` and `..`, plus Windows device names, including reserved forms with extensions.
- A first space-delimited token matching `+[0-9]+d`, including when it is the entire label.

Thus `+2d` and `+2d Reunion` are invalid labels.
Otherwise-valid date-looking labels remain permitted.
Extension recognition is ASCII-case-insensitive.
Import and rename must preserve extension spelling, including case.

Each complete generated component must fit both 255 UTF-8 bytes and 255 UTF-16 code units.
This includes asset names and sidecar suffixes, so permitted label length depends on the generated names.
Souvenirs must not truncate or silently substitute names.

Collision checks must compare Unicode normalization and case-fold equivalents across planned, existing, and unmanaged entries.
Case-only differences are conflicts.
No entry wins by traversal order.
Comparisons must preserve native path identity without lossy conversion.
Repair must not silently normalize external names.

Validation must also include the selected root and complete target path.
Component limits do not guarantee full-path validity or Windows long-path support.
An externally copied catalog requires path validation at its new location before managed mutation.
The host can impose stricter limits.

## Configuration

```yaml
format_version: 2
media_extensions: [jpg, jpeg, heic, cr3, dng, mp4, mov]
sidecar_extensions: [xmp, aae, dop, pp3, acr]
```

The extension lists are illustrative, not a finalized support matrix.
Configuration must declare original-media and candidate-sidecar extensions separately.
The lists must not overlap under ASCII-case-insensitive comparison.
An overlap is a configuration error that blocks managed mutation.
Sidecars are not original assets.
Recognition does not establish an ownership rule or enable sidecar-content parsing.
Additional format-specific naming rules and owner slots remain unspecified.
The configuration has no routing groups.
Original IDs and digests are not filename fields.

An absent version denotes the legacy layout.
Unsupported versions must refuse managed mutation.
Repair must not adopt or convert catalogs implicitly.
Legacy binaries must not mutate this format.
The version marker cannot make them safe.
Version 2 writes require the [registry and audit contract](storage.md#audit-and-write-eligibility).
Legacy conversion follows the separate [migration constraints](operations.md#legacy-conversion).
