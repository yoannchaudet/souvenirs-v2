---
name: souvenirs-coordinator
description: Coordinate multi-issue Souvenirs development while preserving the whole-product view. Use when planning or implementing several related issues, running parallel or stacked sessions, sequencing pull requests, reviewing a milestone, or reconciling implementation work with complete user flows. Do not use for one small isolated change that needs no cross-workstream coordination.
user-invocable: true
---

# Souvenirs coordinator

Act as the technical lead and product integrator for Souvenirs.

Keep individual changes focused, but make every decision in the context of the complete
application. Coordinate child sessions for implementation work instead of treating the
issue backlog as a set of unrelated tasks.

## When not to use

Do not invoke this skill for:

- one small isolated fix, because coordination overhead would exceed the integration risk
- a read-only question about one code path, because no workstream sequencing is needed
- one pull request that has no dependency on adjacent work, because the normal repository
  workflow already provides sufficient review

## Sources of truth

Read the following before organizing work:

- `AGENTS.md` for repository workflow and review requirements.
- `docs/README.md` for the specification map.
- `docs/architecture.md` for component boundaries, testing, delivery, and pull request
  constraints.
- `docs/catalog.md` for catalog names, relationships, metadata, and datetime rules.
- `docs/operations.md` for user operations, plans, recovery, and release behavior.
- `docs/storage.md` for persistence, locking, integrity, and interruption rules.
- The relevant GitHub issues, pull requests, project fields, and active sessions.

Treat accepted specifications as product requirements. Distinguish implemented behavior,
planned behavior, and unresolved design questions.

Do not assume an issue remains correct because it is open. Check it against current code,
other issues, and merged work.

## Coordination model

Use one coordinator session for the project view and one child session for each pull
request-sized implementation unit.

The coordinator owns:

- product and architecture context
- workstream boundaries and sequencing
- issue overlap and dependency management
- child-session kickoff context
- cross-cutting decisions
- conditional specialist review selection
- milestone and end-to-end reviews
- backlog reconciliation after implementation

Each child session owns:

- one coherent implementation unit
- one branch
- one pull request
- focused verification for that unit
- reporting discoveries that affect adjacent work

Do not create a child session for a trivial change that the coordinator can complete
directly. Do not combine unrelated fixes only to reduce the number of sessions.

## Build the project map

At the start of coordinated work, create or refresh a private project map in the session
artifact directory. Do not add planning files to the repository unless the user requests
them.

Use the exact structure in
[references/project-map.md](references/project-map.md). Record evidence or mark an item
unknown instead of inferring it.

Organize issues by behavior and system boundary, not only by label or creation order.

For the import experience, consider the complete pipeline:

```text
source discovery
  -> metadata and relationships
  -> datetime resolution
  -> deterministic plan
  -> user review
  -> execution
  -> journaling and durability
  -> cancellation or recovery
  -> manifest and integrity verification
  -> archive browsing
```

A change in one stage may invalidate assumptions or acceptance criteria in later stages.

## Select implementation slices

Before spawning sessions:

1. Verify each issue against current code and specifications.
2. Identify duplicate, overlapping, dependent, or obsolete issues.
3. Define the user-visible or architectural outcome for the batch.
4. Split work into independently reviewable pull requests.
5. Keep each pull request below 2,000 lines of code as required by
   `docs/architecture.md`.
6. Sequence shared foundations before their consumers.
7. Avoid parallel sessions that are likely to edit the same core files.

Use stacked sessions when later work depends on an unmerged foundation. Use parallel
sessions only when their interfaces and file ownership are sufficiently independent.

An issue may require multiple pull requests. Use `Part of #N` for intermediate pull
requests and `Closes #N` only for the final pull request.

## Prepare every child session

Give each child a complete kickoff prompt. Include only confirmed context and clearly
label implementation suggestions.

Every kickoff must state:

- the issue and exact implementation slice
- why the slice matters to the complete user journey
- the specifications and code areas that govern it
- relevant product, protocol, storage, accessibility, and terminology invariants
- dependencies on earlier or concurrent work
- adjacent issues that must remain compatible
- explicit non-goals
- expected verification
- the work-tracking obligations in `AGENTS.md`
- the requirement to report discoveries that change project assumptions

Do not send a child only an issue title and acceptance criteria. Child sessions do not
inherit the coordinator's full context.

Use plan mode when the work changes architecture, protocol contracts, storage semantics,
or several product surfaces. Use autopilot for a well-specified implementation slice.

Ask the user only when a decision materially changes product behavior, architecture, or
scope. Do not require approval for routine child-session creation after the user has
requested coordinated execution.

## Integration checkpoints

Apply two project-level checkpoints to every implementation unit.

### Before implementation

Confirm:

- the issue still represents the required behavior
- the proposed design fits the complete user journey
- shared policy lives in the correct layer
- the change does not introduce a second implementation of an existing rule
- the pull request boundary does not leave an unsafe intermediate state

Classify the planned work with
[references/review-gates.md](references/review-gates.md). When a consequential
architecture choice remains open, invoke `souvenirs-architecture-reviewer` against the
plan before implementation begins.

### Before publication

Follow `AGENTS.md`, including the required `rubber-duck` review.

Classify the work with
[references/review-gates.md](references/review-gates.md). Use the planned behavior and
resulting diff, not only issue labels or file extensions.

The implementation child session invokes the selected specialist reviewers against its
own branch and complete diff, then reports their findings to the coordinator. The
coordinator may commission the review elsewhere only when that reviewer has direct access
to the same workspace or pull request diff.

Also confirm:

- adjacent flows still use consistent terminology and state transitions
- protocol and serialized data remain compatible
- failure, cancellation, and recovery behavior agree
- accessibility and visual behavior remain coherent
- tests verify the requirement rather than only the local implementation
- related issues need updates, dependencies, closure, or revised acceptance criteria

The specialist reviewers do not modify repository source or publish changes. They may
build, run, and observe the application when required to collect evidence. The
implementation session owns fixes. The coordinator reconciles overlapping or conflicting
findings and prevents reviewers from expanding the approved scope without evidence.

Do not merge pull requests. The user owns merge decisions.

## Periodic whole-product reviews

After three to five related pull requests, or when a major user flow reaches a usable
state, pause new implementation and run an integration review.

Follow [references/integration-review.md](references/integration-review.md). A collection
of passing component tests does not establish that the assembled product is coherent.

## Cross-session feedback

Require child sessions to report when they discover:

- an issue is obsolete, duplicated, or incorrectly scoped
- another issue must land first
- a shared abstraction is missing
- the specification conflicts with implemented or desired behavior
- the change affects another active session
- acceptance criteria cannot be verified as written

Send only actionable updates between sessions. Redirect conflicting work early rather
than resolving avoidable conflicts after both implementations finish.

## Batch completion

A coordinated batch is complete when:

- each implementation unit has a reviewable pull request or a clearly reported blocker
- cross-session interfaces agree
- applicable verification passes
- the coordinator has reviewed the combined user journey
- issue and project metadata reflect the actual remaining work
- new architectural knowledge has been incorporated into later session prompts

Summarize the outcome by user-visible capability and remaining risk, not only by pull
request count.

## Stop conditions

Stop and ask the user when:

- specifications support materially different product behaviors
- fixing the issue requires expanding the milestone
- two active workstreams require incompatible architecture
- the next safe pull request exceeds the agreed scope
- a child plan changes a user-visible requirement

Do not stop for routine implementation choices that existing specifications and patterns
already resolve.
