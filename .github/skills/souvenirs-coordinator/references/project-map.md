# Project map

The private `yoannchaudet/souvenirs-v2-context` repository stores durable coordination
state. Read its `AGENTS.md` before changing that state. Use a session artifact only as a
temporary cache or fallback when the repository is unavailable.

Store the project map at `project-map.md` and the current handoff at
`handoffs/current.md`. Preserve completed integration summaries under `checkpoints/` and
accepted decisions under `decisions/`. Add integration summaries to
`checkpoints/README.md`. Add each accepted decision as the next numbered record and index
it in `decisions/README.md`. Treat the record and its required index update as one focused
change.

Keep each entry concise and derived from an authoritative Souvenirs source. Link instead
of copying source text. Add an evidence date only to time-sensitive summaries, and use
`Unknown` when evidence is unavailable.

If `project-map.md` is missing, create it with this starting structure through a focused
pull request:

```markdown
# Souvenirs project map

Evidence checked with GitHub: <ISO date>

## Current milestone

<One user-visible outcome>

## User journeys

<Current and target journeys with authoritative links>

## Workstreams

| Workstream | Authoritative records | Coordination state |
| --- | --- | --- |

## Invariants

<Verified product and coordination invariants>

## Issue reconciliation

| Record | State | Reconciliation |
| --- | --- | --- |

## Recent merged pull-request consequences

<Concise consequences with authoritative links>

## Decisions

<Accepted coordination decisions with links to `decisions/`>

## Next checkpoint

<Trigger and scope>
```

If `handoffs/current.md` is missing, create it with this starting structure through a
focused pull request:

```markdown
# Souvenirs coordinator handoff

Evidence checked with GitHub: <ISO date>

## Active work

<Verified current work with authoritative links>

## Blockers

<Verified blockers and material `Unknown` state>

## Exact next actions

<Ordered actions and prerequisites>
```

Update the map only when a material decision, dependency, integration result, or next
action changes. Do not record routine progress.

Do not store credentials, secrets, personal data, local absolute paths, raw logs,
generated media, copied source code, or duplicated issue or pull request bodies.
