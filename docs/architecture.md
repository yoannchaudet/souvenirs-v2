# Application architecture

**Status: draft architecture.** This document defines the proposed application architecture.
The minimum macOS version below is an implemented bootstrap constraint.

## Goals

- Provide a polished, native macOS application.
- Keep catalog rules and storage behavior in one Rust implementation.
- Expose a stable, non-interactive CLI for tests and development agents.
- Keep the core test suite fast enough for continuous local use.
- Test the macOS interface separately from catalog correctness.
- Recover safely from process, machine, storage, and network interruption.

## Non-goals

- Support platforms other than macOS in the first implementation.
- Use the CLI as the primary human interface.
- Put catalog rules in Swift.
- Make SQLite part of the durable catalog format.
- Require NAS or UI tests in the fast local test loop.
- Distribute through the Mac App Store in the first milestone.

## System design

The application uses a native Swift interface and a Rust engine.

The macOS application starts a bundled engine process.
The application and engine exchange versioned JSON messages through standard input and output.
The CLI calls the same Rust application services without starting the macOS application.

The engine owns all authoritative catalog reads and writes.
The Swift application owns presentation state and macOS integration.

The initial implementation does not use a direct Swift-to-Rust foreign function interface.
The process boundary isolates failures and simplifies asynchronous communication.

## macOS application

The macOS application uses SwiftUI for application structure.
It uses AppKit where SwiftUI does not provide sufficient control or performance.

### Minimum macOS version

The initial application supports macOS 14 and later.
The bootstrap Xcode project sets the macOS deployment target to version 14.0.
This deployment target is an implemented constraint, not a draft architecture proposal.

SwiftUI owns:

- Window composition.
- Navigation.
- Sheets.
- Forms.
- Settings.
- State-driven progress views.
- Most collection views.

### Interface conventions

The application uses standard macOS window controls instead of custom traffic-light controls from the design mockups.

The application uses system materials, controls, focus behavior, and reduced-motion behavior.

Text can increase the height of progress cards and recovery sheets when accessibility settings require more space.

The recovery sheet keeps the primary action visible but disabled while the source is unavailable.
This behavior preserves the mockup hierarchy and exposes the unavailable action to assistive technology.

AppKit can own:

- Large tables.
- Complex selection.
- Finder-style drag and drop.
- Menu bar integration.
- Precise keyboard navigation.
- Performance-sensitive grids.

The application uses Swift Concurrency.
One Swift actor supervises the engine process and owns protocol communication.

The application must not:

- Modify catalog files.
- Allocate canonical counters.
- Update manifests.
- Write journals.
- Decide recovery actions.
- Update the catalog index directly.

The application can retain:

- Window state.
- Navigation and selection state.
- User preferences.
- Recent catalog references.
- Security-scoped bookmarks when required.

## Rust workspace

The Rust workspace separates pure rules, orchestration, storage, and client interfaces.

```text
crates/
  souvenirs-domain/
  souvenirs-application/
  souvenirs-storage/
  souvenirs-metadata/
  souvenirs-protocol/
  souvenirs-engine/
  souvenirs-cli/
  souvenirs-test-support/

macos/
  Souvenirs/
  SouvenirsTests/
  SouvenirsUITests/
```

### Domain

`souvenirs-domain` contains pure catalog rules.

It owns:

- Portable name validation.
- Collection ranges.
- Canonical filenames.
- Counter allocation.
- Logical item relationships.
- Accepted datetime rules.
- Plan values.
- Drift detection.
- Recovery decisions.
- Operation state transitions.

It must not access the filesystem, SQLite, the system clock, environment variables, or process-global state.

### Application services

`souvenirs-application` coordinates use cases.

It owns:

- Catalog initialization.
- Catalog scanning.
- Import planning.
- Plan validation.
- Import execution.
- Recovery.
- Integrity checks.
- Progress reporting.
- Cancellation policy.

It depends on explicit interfaces for storage, metadata, hashing, journals, indexes, locks, time, and operation events.
It does not know whether the caller is the CLI or the macOS application.

### Storage

`souvenirs-storage` implements durable storage behavior from [Storage and recovery](storage.md).

It owns:

- Staging.
- Verified copies.
- Atomic publication.
- Manifest replacement.
- Journal persistence.
- Catalog locks.
- SQLite access.
- Filesystem scanning.
- macOS durability operations.

The implementation isolates these macOS APIs behind a narrow interface:

- `F_FULLFSYNC`
- `F_NOCACHE`
- `renamex_np` with `RENAME_EXCL`
- Volume identity and capability queries

### Metadata

`souvenirs-metadata` extracts and normalizes media metadata.

The implementation must validate candidate libraries against a representative media corpus.
The corpus must include each format and source device supported by the first milestone.

Other crates consume normalized metadata values.
They must not depend on parser-specific types.

### Protocol

`souvenirs-protocol` defines requests, responses, progress events, and structured errors.

Every request contains:

- A request identifier.
- A protocol version.
- A method.
- Typed parameters.

Every response contains:

- The request identifier.
- The protocol version.
- A result or structured error.

Long operations return an operation identifier.
The engine emits ordered progress events for that operation.

The first request performs a handshake.
The handshake reports the engine version, protocol version, compatible client version, and capabilities.
The `compatible_client_version` value is the oldest macOS client version that the engine supports.
Client versions contain three unsigned decimal components without leading zeros.
Each component fits in an unsigned 64-bit integer.
A macOS client accepts the engine when its client version is equal to or newer than this minimum.
A client rejects an engine that requires a newer client version.
The protocol version remains the wire compatibility boundary.

The Rust types generate a committed JSON Schema.
Swift protocol fixtures must validate against this schema.
Run `just protocol-schema` to regenerate `crates/souvenirs-protocol/schema/v7.json`.

The `catalog.collections` query returns engine-owned collection identities.
A dated identity contains the exact year directory and collection directory.
An ongoing identity contains the portable label collision key.
Each ongoing result also contains the exact annual part directories.
The earliest annual part supplies the ongoing display label.
Dated collections sort by date range, label, and exact identity.
Ongoing collections follow dated collections and sort by collision key.

The `catalog.browse` query returns manifest-admitted originals for archive presentation.
It preserves the same collection identities, ranges, ongoing grouping, and annual part directories.
Rust groups originals by accepted date and returns media paths in deterministic manifest order.
The macOS client must not reconstruct collection paths or read manifests directly.
Published originals do not enter browse results until a manifest admits them.

### Engine

`souvenirs-engine` exposes application services to the macOS application.

It reads one JSON request per line from standard input.
It writes responses and events to standard output.
It writes diagnostics to standard error.

The engine does not listen on a network port.
It exits when the parent closes the connection.
It rejects incompatible protocol versions.

The macOS application reports unexpected engine termination.
It can restart the engine for read operations.
An unfinished mutation remains governed by its catalog journal.

### CLI

`souvenirs-cli` exposes application services for automation.

Initial command groups are:

```text
souvenirs catalog initialize
souvenirs catalog inspect
souvenirs import scan
souvenirs import plan
souvenirs import execute
souvenirs operation status
souvenirs operation recover
souvenirs operation cancel
souvenirs integrity check
souvenirs index rebuild
souvenirs protocol schema
```

The CLI must:

- Support structured JSON output.
- Support JSON Lines progress output.
- Keep result data on standard output.
- Keep diagnostics on standard error.
- Use stable exit codes.
- Avoid interactive prompts.
- Avoid implicit destination selection.
- Avoid executing an unconfirmed plan.

Human-readable output is secondary.
Tests and agents must not parse human-readable output.

## Plans

Every mutation uses an immutable, serializable plan.

The saved import plan is a versioned envelope.
The envelope contains `schema_version`, `confirmed`, `source_root`, and `import_plan`.

The import plan contains a plan document and its digest.
The version 1 digest payload is the complete `PlanDocument`.
The digest excludes the `digest` field because that field contains the result.
The digest excludes the saved-plan envelope because that envelope wraps the import plan.

The plan document contains:

- A schema version.
- A unique identifier.
- Catalog identity.
- Source identities.
- Observed manifest fingerprints.
- Source and destination paths.
- Accepted datetimes.
- Exclusions.
- Planned filesystem steps.
- Required free space.
- Creation time.

Version 1 normalizes ordered collections before serialization:

- Manifest fingerprints use path order.
- Items use item identifier order.
- Item files use source path, then source file identifier order.
- Unhandled content uses source path, then source file identifier order.
- Ambiguous sidecar candidates use identifier order.
- Steps use sequence order.

The digest is SHA-256 over the normalized plan document.
The digest input uses compact UTF-8 JSON from the exact workspace-pinned `serde_json` version.
Rust field order and enum representation define JSON member order.
Version 1 preserves null values and uses the serializer's string, number, and escape encoding.

Changes to fields, member order, enum representation, normalization, encoding, or digest algorithm require a new plan schema version.
Readers must not reinterpret version 1 data with a later digest contract.

Version 2 adds the `ambiguous_sidecar` item finding.
Version 2 keeps the version 1 field order, normalization, JSON encoding, and digest algorithm.
Each `ambiguous_sidecar` item finding requires an unhandled sidecar record that names the item among at least two valid candidates.
Readers validate each document against its declared version.
Readers continue to verify version 1 digests with the version 1 contract.

Version 3 adds `datetime_disagreements` to each plan item.
Each entry records the metadata source and one tagged normalized value.
The value tag records its interpretation state.
An offset value is valid only for `local_with_offset`.
Datetime disagreements use source priority, source, then normalized value order.
Version 3 requires the disagreement array even when it is empty.
Relationship-resolved items report disagreements from the primary file and its owned sidecars.
Relationship-unresolved items have no selected datetime candidate and use an empty disagreement array.
Versions 1 and 2 omit the field and keep their original digest contracts.

Version 4 adds a duplicate review to the plan document.
The review records the selected catalog and source duplicate choices.
It also records catalog matches and within-source digest groups.
Each original records a transfer or retained duplicate disposition.
A retained catalog duplicate names the first component-wise catalog path.
A retained source duplicate names the selected source file.
Sidecars use the transfer disposition.
Execution copies a sidecar only when its item transfers an original that remains its local owner.

The catalog skip choice takes precedence when a digest matches the catalog and the source.
The one-copy choice selects the earliest accepted wall-clock datetime.
It then selects the first component-wise source path.
The source file identifier is the final stable tie-breaker.

Version 4 sorts catalog findings by source file identifier.
It sorts each catalog path list component-wise.
It sorts source groups by digest and source file identifiers.
Source groups must exactly cover every repeated digest among source originals.
The planner establishes catalog finding completeness from the catalog lookup.
Versions 1 through 3 omit all duplicate fields and keep their original digest contracts.

The CLI emits version 4 plans.
Planning records structured duplicate choices and derives copy and manifest steps from those choices.
Planning binds the disposable index to the catalog identity.
The plan snapshots every manifest in the namespace used by duplicate lookup.
Execution revalidates missing, changed, and unexpected manifest entries under the catalog lock before it creates the operation journal.

The saved-plan `confirmed` field is not part of the digest.
Execution must reject the plan unless this field is `true`.

The saved-plan `source_root` field is not part of the digest.
Initial import execution resolves planned source paths below this root.
Initial import execution must compare each transferred or excluded item file with its planned size and digest.
Changing the root is valid only when these planned files still match.

Run `just import-plan-schema` to regenerate every schema in `crates/souvenirs-cli/schema/import-plan/`.
Each committed schema describes one complete saved-plan envelope version.

The CLI can save a plan to a file.
Execution accepts the exact saved plan.

```text
souvenirs import plan ... --output import-plan.json
souvenirs import execute --plan import-plan.json
```

Execution revalidates mutable assumptions.
It rejects source or catalog drift.

## Index

The engine owns the rebuildable index described in [Storage and recovery](storage.md#index).
It updates the index after the matching durable catalog change.

## Test architecture

### Fast core tests

The fast suite covers `souvenirs-domain` and most application services.

It uses fixed clocks, deterministic identifiers, seeded property tests, and in-memory test doubles.
It does not launch processes or access real storage.

The initial performance target is five seconds on a development Mac.

Property tests cover:

- Portable canonical names.
- Collision-free counter allocation.
- Destination overwrite prevention.
- Recovery deletion limits.
- Collection range containment.
- Plan serialization.
- Traversal-order independence.

### Filesystem integration tests

Storage tests use real temporary directories.
A virtual filesystem does not replace these tests.

The suite covers:

- Exclusive creation.
- Atomic publication.
- Manifest replacement.
- Lock contention.
- Destination conflicts.
- Source disappearance.
- Cancellation.
- Recovery after each durable transition.
- Unicode and case-fold collisions.

The storage implementation provides test-only failpoints around durable transitions.
A test process can terminate at each failpoint.
The next process runs recovery and verifies the resulting catalog.

### Storage environment tests

Storage tests run at different frequencies.

| Environment | Frequency |
| --- | --- |
| Local temporary directory | Targeted tests on every pull request; full suite on the main branch |
| APFS attached volume | Before release |
| exFAT attached volume | Before release |
| SMB test server | Nightly and before release |
| Forced SMB interruption | Nightly and before release |

### Protocol tests

The repository keeps golden protocol requests, responses, errors, and progress streams.

Rust and Swift tests use the same fixtures.
The suite covers version mismatches, event ordering, cancellation races, malformed messages, and engine termination.

### CLI end-to-end tests

CLI tests create temporary sources and catalogs.

Each test can:

1. Create a source fixture.
2. Request a plan.
3. Inspect the JSON plan.
4. Execute the plan.
5. Inspect the resulting catalog.
6. Run an integrity check.
7. Compare the result with an expected tree.

The CLI is the primary automation interface for development agents.

### Import performance measurements

`just measure-import-performance` measures warmed release builds of import scan and import planning.
The command uses a deterministic temporary source and catalog.

The source contains 512 media files, 128 sidecars, and 64 unsupported files.
It contains 2,146,432 bytes.
The source includes 32 catalog duplicates and 32 source duplicate groups.
The catalog contains 32 files in eight manifests and uses 133,728 bytes.

The command runs one warm-up sample and five measured samples.
It reports the median, minimum, and maximum duration for each operation.
Each result includes the source size, catalog size, workload identifier, host conditions, and operation counts.
The workload identifier covers every relative path and file digest.

The scan duration measures the complete `import scan` command.
The plan duration measures the complete `import plan` command.
The plan command includes the required source revalidation scan.
This scope measures user-visible command performance without bypassing drift protection.

The measurement is informational because shared and virtualized runners have variable CPU, storage, and thermal behavior.
The nightly macOS job stores each JSON Lines result as a workflow artifact for 90 days.
Nightly results are comparable only with results from the same runner environment.
Pull requests do not enforce an environment-dependent duration threshold.

The initial development baseline uses workload `3096319b5a2f893e73f21e10bcbab74abd48007e6daca9b1e0e605bf2a9a8c1c`.
It runs on a Mac14,12 with macOS 26.5.2, APFS storage, and Rust 1.81.0.
Two independent runs produce scan medians from 333 to 337 milliseconds.
The same runs produce plan medians from 666 to 675 milliseconds.

A timing change is actionable when all comparison results use the same workload identifier.
The host must also use the same Mac model, macOS major version, Rust toolchain, and filesystem type.
A median increase of at least 20 percent must reproduce in two independent command runs.
The host must not have known thermal pressure or competing storage work.

### Swift state tests

Swift state tests use a fake engine client.
They do not launch the application.

They cover button availability, blocking decisions, errors, progress, reconnection, recovery routing, navigation, and selection.

### macOS UI tests

XCUITest covers a small set of complete user journeys.

Most UI tests launch the application with a deterministic fake engine.
The fake engine serves named scenarios for important states and failures.

Every interactive control has a stable accessibility identifier.
Tests use accessibility values instead of pixel coordinates.

A smaller smoke suite launches the real engine with a temporary catalog.
It covers startup, catalog creation, import planning, execution, progress, and recovery routing.

## Development commands

The repository uses `just` to provide stable development commands.

```text
just test-core
just test-rust
just test-cli
just test-swift
just test-ui
just test-engine-architectures
just test-integration
just test-storage
just check
just test-all
```

`just test-core` targets a five-second runtime.
`just check` runs formatting, static checks, core tests, and Swift state tests.
`just test-all` runs all tests available on the current machine.

## Continuous integration

Pull requests run:

- Rust formatting.
- Clippy with warnings denied.
- Core tests.
- Protocol tests.
- CLI end-to-end tests.
- Swift compilation.
- Swift state tests.
- A small fake-engine UI suite.
- Engine architecture verification.
- A universal macOS application and engine build.

The main branch also runs the full local filesystem integration suite, real-engine UI smoke tests, and failpoint recovery tests.

Nightly jobs also run SMB tests, large fixture tests, repeated interruption tests, metadata corpus tests, and performance tests.

## Distribution

The first milestone uses direct distribution with Hardened Runtime, code signing, and notarization.
The bundle includes a signed engine executable.

The first milestone does not require the Mac App Sandbox.
The sandbox adds risk around arbitrary volumes, NAS paths, persistent access, and child processes.

Mac App Store support requires a separate validation of security-scoped resource access across the engine process boundary.

## Delivery constraints

One implementation issue can produce more than one pull request.
Each pull request must remain below 2,000 lines of code.
Pull requests should remain comfortably below this limit when practical.

Each pull request must provide one independently reviewable capability.
Tests and generated files count toward review cost even when line-count tools exclude them.

## Initial sequence

1. Validate metadata extraction, filesystem durability, and engine packaging.
2. Implement pure catalog rules and plans.
3. Implement the CLI on local temporary catalogs.
4. Implement journals, verified copies, failpoints, and recovery.
5. Implement the engine protocol.
6. Build the simplified native import interface.
7. Add fake-engine UI scenarios.
8. Add real-engine UI smoke tests.
9. Validate attached-drive behavior.
10. Validate SMB interruption and recovery.

## Open questions

- Which media formats and source devices are required for the first milestone?
- Which Rust libraries satisfy the metadata corpus?
- Does a sandboxed child process retain access to every required security-scoped location?
