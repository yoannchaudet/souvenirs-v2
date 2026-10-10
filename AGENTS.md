# Repository instructions

## Testing

Test every change at the appropriate level.
Prioritize tests that prevent regressions in core behavior and important user flows.

Keep tests proportional to risk.
Do not add exhaustive tests for trivial, low-risk interface details.
Prefer a focused behavior test over assertions about every visual or implementation detail.

Keep the core test suite fast enough for continuous local use.
Use broader integration and interface tests for boundaries that unit tests cannot validate.

## Writing

Use the user's `writing-style` skill for human-facing text when it applies.
This includes issues, pull requests, review comments, project items, status updates, and documentation outside `docs/`.

Files under `docs/` use the `tech-spec` skill and the more specific instructions in `docs/AGENTS.md`.

## Work tracking

Track implementation work in the [Souvenirs MVP project](https://github.com/users/yoannchaudet/projects/15).

When work starts on an issue:

1. Add the issue to the project if it is not already present.
2. Set the issue's **Status** field to **In Progress**.
3. Preserve its **Workstream** assignment.

Link every pull request to its issue.
Use `Part of #N` for an intermediate pull request.
Use `Closes #N` only for the final pull request that completes the issue.

Add each pull request to the project.
Keep the pull request status consistent with the linked issue.
Follow the pull request size and review boundaries in `docs/architecture.md`.

## Pull request review

Before opening a pull request, use the `rubber-duck` agent to review the complete change.
Address meaningful findings before publication.
Record the reason when a finding is intentionally deferred.
