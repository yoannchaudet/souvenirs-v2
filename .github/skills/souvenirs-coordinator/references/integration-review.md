# Whole-product integration review

Run this review after three to five related pull requests or when a major user journey
first becomes usable.

## Prepare

1. Identify the complete user journey and every affected state.
2. List the merged pull requests and unresolved issues in that journey.
3. Select applicable fake-engine scenarios.
4. Identify boundaries that require the production engine or real storage.

## Review

Inspect:

- the complete journey in the running macOS application
- default, minimum, resized, and restored window states
- loading, empty, ready, error, cancellation, recovery, and completion states
- fake-engine scenarios for deterministic interface behavior
- real-engine smoke coverage for filesystem and protocol boundaries
- visual hierarchy, terminology, accessibility, and error recovery
- consistency between implementation, specifications, issues, and project status
- duplicated policy or temporary shortcuts that have become permanent

Use screenshots and direct interaction when visual composition matters.

## Reconcile

For each confirmed finding:

1. Identify the violated requirement or user consequence.
2. Check whether an existing issue already covers it.
3. Update or close obsolete and duplicate issues.
4. Create a focused issue only when the finding is evidenced and distinct.
5. Update dependencies and later child-session context.

Do not create speculative backlog items from preference-only observations.

## Output

```markdown
## Integration verdict

<coherent | coherent with follow-ups | needs revision>

## Journey reviewed

<scope and states>

## Confirmed findings

| Level | Finding | Evidence | Existing or new issue |
|---|---|---|---|

## Backlog changes

<updates, closures, dependencies, or None>

## Next coordinated batch

<ordered implementation slices or None>
```
