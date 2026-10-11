---
name: souvenirs-coordinator
description: Coordinate a multi-issue or multi-pull-request Souvenirs batch with real sequencing or integration risk. Use for parallel or stacked work, milestone integration, and cross-workstream decisions. Do not use for a single independent pull request or routine review.
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

Start with `AGENTS.md`, the relevant issue or pull request, and the current project map
and handoff. Read only the specification sections, GitHub records, decisions, and active
sessions that affect the batch.

Do not read every product specification or coordination record by default. Follow links
from `docs/README.md` when the governing source is unclear.

`yoannchaudet/souvenirs-v2` remains authoritative for specifications, source, tests,
issues, pull requests, and GitHub Project fields. The context repository contains only
derived coordination state and stable links to authoritative sources.

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
- merge readiness and merge sequencing
- backlog reconciliation after implementation

Each child session owns:

- one coherent implementation unit
- one branch
- one pull request
- focused verification for that unit
- reporting discoveries that affect adjacent work

Do not create a child session for a trivial change that the coordinator can complete
directly. Do not combine unrelated fixes only to reduce the number of sessions.

## Startup and state sync

At the start of a coordinated batch:

1. Read `project-map.md` and `handoffs/current.md` once from the default branch of
   `yoannchaudet/souvenirs-v2-context` through GitHub before organizing a new batch.
   Follow only links relevant to the batch.
2. Verify only state-sensitive claims that affect sequencing, scope, or readiness.
3. Reuse this working view until relevant live data or accepted specifications change.
   Persist it only at the triggers in
   [Update durable coordination state](#update-durable-coordination-state).

If either durable file is missing, treat its state as `Unknown`. Bootstrap the missing
file from [references/project-map.md](references/project-map.md), subject to the context
repository's `AGENTS.md`, through a focused context-repository pull request. Do not block
product work while the bootstrap pull request is in progress.

Use the session artifact only as a temporary cache or fallback. Do not treat it as the
durable source when the context repository is readable. If the private repository is
unreadable, continue product work with the session artifact and report the durability
limitation. If its configured Copilot project is unavailable, continue reading durable
state through GitHub but report that persistence is unavailable.

Follow the context repository's existing structure and
[references/project-map.md](references/project-map.md). Record an authoritative source
for each state-sensitive claim and an evidence date for time-sensitive summaries, or mark
the information `Unknown` instead of inferring it. Prefer stable GitHub links to copied
issue or pull request text.

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

## Update durable coordination state

Change the context repository only from a session and project configured for
`yoannchaudet/souvenirs-v2-context`. Do not edit the context repository's checkout from a
Souvenirs source session.

Do not add coordination or planning files to the `yoannchaudet/souvenirs-v2` product
repository. Durable coordination state belongs only in the private context repository.

Keep the one-session-per-pull-request model for context changes. Use the app's multi-repo
orchestration model to create a focused context-repository session and pull request
instead of expanding a product pull request.

Before changing context state, read and follow its `AGENTS.md`. Update only the smallest
relevant project map, decision, integration checkpoint, or current handoff. Add accepted
decisions as numbered records under `decisions/` and index them in
`decisions/README.md`. Add integration summaries to `checkpoints/README.md`. A required
index update is part of the smallest relevant change. Apply the context repository's
complete-diff review and `git diff --check` requirements before publication. Shepherd
each context pull request through its checks, review, and merge. The Souvenirs product
issue-linking, MVP project-tracking, specialist-review, and product pull-request size
rules do not apply unless the context repository adopts them.

Create or update a context-repository pull request after:

- a material product or architecture decision
- a whole-product integration checkpoint
- a merge sequence or dependency order changes
- backlog reconciliation changes future work
- a coordinator handoff

Do not create a context pull request for routine progress that does not change what a
future coordinator would do.

Store concise derived state and links only. Never store credentials, secrets, personal
data, local absolute paths, raw logs, generated media, copied source code, or full
duplicated issue or pull request bodies, or archived or transient session identifiers.

Do not publish links to the private context repository in public Souvenirs issues, pull
requests, or review comments. Link from private context state to the authoritative
Souvenirs source instead. Repository-tracked instructions may name the private repository
without linking to its contents.

## Select implementation slices

Before spawning sessions, make the minimum decisions needed to avoid overlap:

1. Verify the selected issues against the governing code and specifications.
2. Identify material overlap, dependencies, duplicate work, and obsolete work.
3. Define the batch outcome and independently reviewable pull requests.
4. Keep each pull request below 2,000 lines of code as required by
   `docs/architecture.md`.
5. Sequence shared foundations before consumers and avoid conflicting parallel edits.

Use stacked sessions when later work depends on an unmerged foundation. Use parallel
sessions only when their interfaces and file ownership are sufficiently independent.

An issue may require multiple pull requests. Use `Part of #N` for intermediate pull
requests and `Closes #N` only for the final pull request.

## Prepare every child session

Give each child a self-contained but concise kickoff. Include the issue, exact slice,
governing sources, material invariants and dependencies, non-goals, expected verification,
and required work tracking. Link to sources instead of restating broad project context.
Require the child to report only discoveries that change project assumptions.

When a child can change durable coordination state, its kickoff must also state:

- which context files and claims the change can affect
- the authoritative Souvenirs sources that the child must verify
- the requirement to report the concise state delta to the coordinator
- the requirement not to edit the context repository from the product checkout
- whether the coordinator must create or update a separate context-repository pull
  request after the result is accepted

Do not send a child only an issue title and acceptance criteria. Child sessions do not
inherit the coordinator's full context.

Use plan mode when the work changes architecture, protocol contracts, storage semantics,
or several product surfaces. Use autopilot for a well-specified implementation slice.

Ask the user only when a decision materially changes product behavior, architecture, or
scope. Do not require approval for routine child-session creation after the user has
requested coordinated execution.

## Integration checkpoints

Use the following checkpoints without repeating evidence gathered earlier.

### Before implementation

Confirm that the issue remains valid, shared policy has one owner, and the pull request
boundary is safe.

Classify the planned work with
[references/review-gates.md](references/review-gates.md). When a consequential
architecture choice remains open, invoke `souvenirs-architecture-reviewer` against the
plan before implementation begins.

### Before publication

Follow `AGENTS.md`, including the required `rubber-duck` review. Use
[references/review-gates.md](references/review-gates.md) to add a specialist only when
the change has a material risk that the general review cannot adequately cover. Do not
invoke a specialist solely because a file type matches a gate, and do not repeat a plan
review when implementation preserves the reviewed design.

The implementation session invokes selected specialists against its own branch and
complete diff and reports their findings. Commission a review elsewhere only when the
reviewer has access to the same diff.

Confirm only the affected contracts, adjacent states, failure behavior, accessibility,
and tests.

The specialist reviewers do not modify repository source or publish changes. They may
build, run, and observe the application when required to collect evidence. The
implementation session owns fixes. The coordinator reconciles overlapping or conflicting
findings and prevents reviewers from expanding the approved scope without evidence.

### Before merge

The coordinator shepherds each pull request through merge after the user requests
coordinated execution. Do not require separate approval for each merge unless the user
asks to retain merge control.

Before merging, confirm required checks and reviews, resolution of material findings,
scope and size, dependency order, and accurate issue and project links. Reuse publication
evidence unless the branch or dependency state changed.

Do not merge a draft pull request, a pull request with unresolved blocking findings, a
pull request with failing required checks, or a pull request that no longer combines
cleanly with its dependencies.

Merge foundations before their consumers. After each merge, confirm the resulting branch
state, retarget stacked dependents when their base changes, update issue and project
metadata, and revalidate affected dependent pull requests.

## Periodic whole-product reviews

After three to five related pull requests, or when a major user flow first becomes
usable, run a focused integration review.

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

- each implementation unit is merged, awaits a user-controlled merge, or has a clearly
  reported blocker
- cross-session interfaces agree
- applicable verification passes
- the coordinator has reviewed the combined user journey
- issue and project metadata reflect the actual remaining work
- new architectural knowledge has been incorporated into later session prompts
- the durable project map and current handoff reflect material decisions, integration
  results, merge order, backlog reconciliation, remaining blockers, and the next safe
  action

When the batch changes what a future coordinator would do, create or update a focused
context-repository pull request before handoff. Verify its state-sensitive claims against
live GitHub data and identify the authoritative source for each claim. The batch is not
complete until that pull request is merged or its merge blocker is reported explicitly.
If durable state cannot be updated, preserve the handoff in the session artifact and
report the limitation.

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
