# Project map

The private `yoannchaudet/souvenirs-v2-context` repository stores durable coordination
state. Read its `AGENTS.md` before changing that state. Use a session artifact only as a
temporary cache. Use it as a fallback source only when the context repository is
unreadable. If the configured Copilot project is unavailable, continue reading durable
state through GitHub and cache pending updates in the artifact until persistence is
available.

Store the project map at `project-map.md` and the current handoff at
`handoffs/current.md`. Preserve completed integration summaries under `checkpoints/` and
accepted decisions under `decisions/`. Add integration summaries to
`checkpoints/README.md`. Add each accepted decision as the next numbered record and index
it in `decisions/README.md`. Treat the record and its required index update as one focused
change.

Keep each entry concise and derived from an authoritative Souvenirs source. Use stable
GitHub links instead of copying issue or pull request bodies. State the evidence date for
time-sensitive summaries. Use `Unknown` when evidence is unavailable.

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

Do not store credentials, secrets, personal data, local absolute paths, raw logs,
generated media, copied source code, or full duplicated issue or pull request bodies.
