# Project map

Use this exact structure for the coordinator's private project map. Store it in the
session artifact directory, not in the repository.

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

Use `Unknown` when evidence is unavailable. Do not silently fill gaps with assumptions.
