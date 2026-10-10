# Project map

Use these structures for durable coordination state in the private
`yoannchaudet/souvenirs-v2-context` repository. Use a session artifact only as a temporary
cache. Use it as a fallback source only when the context repository is unreadable. If the
configured Copilot project is unavailable, continue reading durable state through GitHub
and cache pending updates in the artifact until persistence is available.

Store `project-map.md` and `handoff.md` at the root of the default branch. If either file
is missing, treat its state as `Unknown` and create it from the matching structure through
a focused pull request.

Keep each entry concise and derived from an authoritative Souvenirs source. Use stable
GitHub links instead of copying issue or pull request bodies. Use `Unknown` when evidence
is unavailable.

```markdown
# Souvenirs project map

Updated: <ISO date>

## Milestone outcome

<One user-visible outcome>

## User journeys

| Journey | Current state | Target state | Evidence |
|---|---|---|---|

## Workstreams

| Workstream | Issues | Dependencies | Active session or PR | Status |
|---|---|---|---|---|

## Invariants

| Invariant | Source | Affected work |
|---|---|---|

## Issue reconciliation

| Issue | Keep, revise, merge, or close | Reason | Related issue |
|---|---|---|---|

## Recent changes

| Change | Backlog or architecture consequence |
|---|---|

## Decisions needed

| Decision | Why it matters | Options |
|---|---|---|

## Next review checkpoint

<Trigger and scope>
```

Use this structure for the current coordinator handoff:

```markdown
# Souvenirs coordinator handoff

Updated: <ISO date>

## Current outcome

<User-visible or architectural outcome currently being coordinated>

## Accepted decisions

| Decision | Consequence | Authoritative source |
|---|---|---|

## Integration and merge state

| Pull request or checkpoint | Verified state | Dependency or consequence | Source |
|---|---|---|---|

## Backlog reconciliation

| Issue or workstream | Current disposition | Reason | Source |
|---|---|---|---|

## Blockers and unknowns

| Item | Effect | Evidence or `Unknown` |
|---|---|---|

## Next safe action

<One concise action and its prerequisites>
```

Do not store credentials, secrets, personal data, local absolute paths, raw logs,
generated media, copied source code, or full duplicated issue or pull request bodies.
