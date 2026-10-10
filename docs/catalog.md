# Catalog

The catalog format is the durable part of Souvenirs.
Folders, filenames, and checksum manifests define collections, accepted datetimes, relationships, and expected content.
Reading and verifying that organization requires neither Souvenirs nor its database.

| Term | Meaning |
| --- | --- |
| Catalog | A root directory containing collections, configuration, and tool state. |
| Collection | A named group of media in one dated folder, or in annual folders for an ongoing activity. |
| Collection directory | One dated collection folder or one annual part of an ongoing collection. |
| Original | A recognized original photo or video. Its complete bytes, including embedded metadata, are immutable after admission. |
| Logical item | One original, or a supported two-original compound item, with its sidecars. |
| Sidecar | A supported mutable metadata or edit-data file associated with an item. |
| Export | Mutable content inside a collection directory's `exports/` subtree. |
| Manifest | The `SHA256SUMS` file listing the expected digest of every original in a collection directory. |
| Admission | Recording an original in its directory's manifest. |

Immutability is a content contract, not a filesystem permission guarantee.
A file's location determines its role: a JPEG in `exports/` is mutable export content.

## Layout

```text
{catalog}/
  .souvenirs.yml
  .souvenirs/
    lock
    journal/
    index.sqlite
    staging/
  {yyyy}/
    {start-yyyy-MM-dd} [+Nd] {label}/
      SHA256SUMS
      {yyyy-MM-dd} {HH-mm-ss} {NNN}.{ext}
      exports/
    z {label}/
      SHA256SUMS
      {yyyy-MM-dd} {HH-mm-ss} {NNN}.{ext}
      exports/
```

For example:

```text
2019/
  2019-07-14 +6d Brittany/
    SHA256SUMS
    2019-07-14 15-30-12 001.cr3
    2019-07-14 15-30-12 001.jpg
    2019-07-14 15-30-12 001.jpg.xmp
    2019-07-15 09-02-47 002.heic
    2019-07-15 09-02-47 002.mov
    exports/
  z Kids/
    SHA256SUMS
    2019-03-02 10-11-05 001.heic
```

Square brackets mark an optional token, not literal characters.
Originals, sidecars, and the manifest live directly in collection directories.
Photos and videos share folders.
Extensions control recognition, not routing.

`.souvenirs/` holds [tool state](storage.md): the lock, operation journals, the rebuildable index, and staging.
It is not part of the durable format.
Deleting it must not lose any accepted organization or expected content.
It can lose unfinished-operation recovery and optional history.

### Dated collections

A dated collection stays in one directory under its accepted start year, even across New Year.
Its accepted range must contain every original item's calendar date.
`+Nd` adds `N` days to the start.
For example, `+2d` spans three calendar dates.
A single-day collection omits the token.

Exports do not contribute dates.
Removing items must not shrink the accepted range automatically.
Empty collections remain valid.
Creating an empty dated collection requires an explicit range.
Ranges may overlap, and start dates may match if complete folder names remain distinct.

### Ongoing collections

An ongoing collection uses `z {label}` under each represented year.
Each annual part contains originals accepted in that year.
The `z` prefix sorts ongoing parts after dated collections.
Empty annual parts remain valid.

Matching labels across years identify the same ongoing collection under the [portable comparison rules](#portable-names).
Dated and ongoing collections occupy separate namespaces and may share a label.

Each original belongs to exactly one collection directory.
Cross-collection albums are external tasks.

## Accepted datetimes

Every original item requires a calendar date and 24-hour clock time at second precision.
This is an organizational wall-clock value, not a universal instant or proof of capture time.
Wall-clock sorting does not guarantee chronological order across travel or daylight-saving transitions.

### Sources

Souvenirs reads candidate datetimes from these sources, in precedence order:

| Rank | Source | Kind |
| --- | --- | --- |
| 1 | EXIF `DateTimeOriginal` of the primary original. | Metadata |
| 2 | EXIF `DateTimeDigitized` of the primary original. | Metadata |
| 3 | QuickTime `com.apple.quicktime.creationdate`, which carries a local time and offset. | Metadata |
| 4 | XMP `photoshop:DateCreated` or `exif:DateTimeOriginal` in an owned sidecar. | Metadata |
| 5 | QuickTime or MP4 container creation time. | Ambiguous metadata |
| 6 | Google Takeout JSON `photoTakenTime` for a matching file. | Proposal |
| 7 | A recognized [filename pattern](#filename-patterns). | Proposal |

The highest-ranked usable source supplies the candidate.
Disagreement between sources is reported in the plan but does not block import.
For a compound item, the still image is the primary original.

Metadata candidates are accepted as part of the plan.
Ambiguous metadata and proposals require batch confirmation before they become accepted datetimes.
The plan groups items by source so one confirmation covers many files.

Filesystem modification and creation times are never sources.
Souvenirs must not silently assign epoch values, midnight, or other placeholders.

### Interpretation

| Source value | Required interpretation |
| --- | --- |
| Local date and clock | Preserve those fields regardless of the importing machine's timezone. |
| Local value with an offset | Use the local fields. Record the offset in the journal, not the filename. |
| UTC instant without local context | Require a batch choice: use UTC clock values or convert through a selected timezone. |
| Container time with unknown convention | Treat as ambiguous. Require a batch choice between UTC and local. |
| Fractional seconds | Discard the fraction without rounding. |
| Date without a clock | Require an explicit batch clock, such as `12-00-00`. |
| No usable value | Require explicit assignment or exclusion. |

### Batch corrections

The plan groups items by camera make, model, and serial number when metadata provides them.
The user can apply a clock offset to one group, such as `+1h 03m 20s` for a drifted camera.
The user can also assign one datetime to a selected batch, such as scanned prints from one summer.
Every correction must appear in the plan as resulting datetimes before confirmation.

Approximate dates are explicit assignments.
For a scan known only as "summer 1987", the user can assign `1987-07-01 12-00-00`.
The journal records that the value is user-assigned.

### Filename patterns

Filename proposals cover common sources without capture metadata:

| Pattern example | Proposal |
| --- | --- |
| `IMG_20190714_153012.jpg`, `VID_20190714_153012.mp4` | Date and clock. |
| `PXL_20190714_153012345.jpg` | Date and clock. Treat as UTC unless confirmed as local. |
| `IMG-20190714-WA0001.jpg` | Date only. Requires a batch clock. |
| `Screenshot 2019-07-14 at 15.30.12.png` | Date and clock. |

The exact pattern list is configurable and remains open for extension.
A pattern match is a proposal, never an accepted value without confirmation.

### After admission

The canonical filename is authoritative after admission.
Metadata disagreement is a finding, not permission to rename.

## Relationships and sidecars

Relationships require files in the same direct directory.
Basenames, timestamps, and hashes must not infer cross-directory ownership.
An unresolved relationship blocks only the affected items, which the user can resolve or exclude as a batch.

| Relationship | Rule |
| --- | --- |
| RAW+JPEG | Same basename and an unambiguous supported combination. |
| Live Photo | A still and a video sharing one Apple content identifier, with exactly one match in the directory. |
| Other still and video | Require explicit pairing. Matching basenames alone must not pair photos and videos. |
| Existing canonical compound | A shared canonical stem and unambiguous supported roles. |
| Sidecar | Exactly one possible local owner. Traversal or sorted order must not choose among candidates. |

The Apple content identifier is `ContentIdentifier` in the still's maker notes and `com.apple.quicktime.content.identifier` in the video.
Identifier matching applies across differing basenames.

A basename or canonical stem is the filename without its asset extension.
Compound originals share one stem, accepted datetime, and counter.
Pairing does not promise vendor-specific playback or metadata rewriting.

Sidecar naming forms are:

```text
{basename}.{sidecar-ext}
{basename}.{asset-ext}.{sidecar-ext}
{basename}_{nn}.{asset-ext}.{sidecar-ext}
```

XMP supports all three forms.
Other sidecar formats use only their declared naming forms.
One item can have multiple sidecars.

Sidecars follow their item through approved renames with their suffixes and bytes preserved.
Souvenirs never interprets or rewrites sidecar contents.
External tools can edit them freely.
Proxy files such as `.lrv` and `.thm`, and card structures such as AVCHD, are outside initial scope.

## Names and counters

The original filename pattern is `{yyyy-MM-dd} {HH-mm-ss} {NNN}.{ext}`.
The collection label lives only in the folder name.
Renaming a collection therefore renames one folder and leaves every file and manifest unchanged.
Hyphens replace Windows-forbidden clock colons.

A canonical name requires a valid datetime and a positive counter.
The date must fit the dated range or the ongoing part's year.

Each date in each collection directory has one counter namespace across all media.
Compound members and sidecars share a counter.
New counters start at `001` and widen beyond three digits.
New items are allocated in accepted-datetime order, then component-wise source path order.
Allocation continues above that date's highest existing counter.

Gaps are valid and must not be filled.
Counters represent allocation order, not shooting order.
A clock-only change preserves the counter.
A date change preserves it if free in the new namespace, and otherwise allocates above the highest.
Other items are never renumbered.

Paths are not permanent identifiers.
Approved renames can break external editor references.

## Manifests

Every collection directory containing originals has a `SHA256SUMS` manifest.
It lists each admitted original in that directory and nothing else.
Sidecars, exports, and the manifest itself are not listed.

The format is the standard checksum text format:

```text
{64 lowercase hex digits}  {filename}
```

Lines use UTF-8 and LF endings, sorted by filename bytes.
Filenames are direct children of the collection directory.
Anyone can verify a directory with `shasum -a 256 -c SHA256SUMS` or `sha256sum -c SHA256SUMS`.

The manifest is the durable integrity record.
It survives copies to new disks, backups, and cloud storage alongside the media.
A verified copy of a catalog therefore proves that its bytes still match their admission.

| Observation | Finding |
| --- | --- |
| A listed file is missing | Missing original. |
| A listed file's digest differs | Changed original. |
| A recognized original is not listed | Unadmitted original. |
| A manifest line is malformed or not canonical | Manifest error. |

Findings never authorize automatic changes.
Byte-identical originals may appear in many manifests.
Souvenirs rewrites a manifest only through the [atomic replacement protocol](storage.md#manifest-updates).

## Exports

Every collection directory can contain one real directory named exactly `exports`.
It can hold edited media, sidecars, documents, nested folders, and unknown files.
It requires no canonical names, datetimes, or manifest entries.

Exports belong to the collection directory, not to individual originals.
Souvenirs must not rename, flatten, or delete export content.
Exports do not affect pairing, counters, ranges, or year routing.
Folder renames carry them unchanged.
Scans and integrity checks skip export subtrees.

## Portable names

The format targets Windows, Linux, and macOS.
Labels appear only in collection folder names.
New labels use Unicode NFC normalization while retaining accepted spelling.
Souvenirs must reject labels containing:

- Empty values, leading or trailing whitespace, trailing dots, control characters, or non-space whitespace.
- `<`, `>`, `:`, `"`, `/`, `\`, `|`, `?`, or `*`.
- `.`, `..`, or Windows device names, including reserved forms with extensions.
- A first space-delimited token matching `+[0-9]+d`.

Extension recognition is ASCII-case-insensitive.
Import preserves extension spelling, including case.
Each generated component must fit 255 UTF-8 bytes and 255 UTF-16 code units.
Souvenirs must not truncate or silently substitute names.

Collision checks compare Unicode normalization and case-fold equivalents.
Case-only differences are conflicts.
No entry wins by traversal order.

## Configuration

```yaml
format_version: 2
media_extensions: [jpg, jpeg, heic, cr3, nef, arw, dng, mp4, mov]
sidecar_extensions: [xmp, aae, dop, pp3]
```

The extension lists are illustrative, not a finalized support matrix.
The two lists must not overlap under ASCII-case-insensitive comparison.
An overlap blocks mutation.
An absent version denotes the legacy layout.
Unsupported versions must refuse mutation.

## Open questions

- Is `SHA256SUMS` the right manifest name, or should it be hidden as `.SHA256SUMS`?
- Which filename patterns ship by default, and are they configurable per catalog?
- Which RAW formats and sidecar formats form the initial support matrix?
