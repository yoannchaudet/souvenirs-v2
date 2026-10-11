# Whole-product integration review

Run this review after three to five related pull requests or when a major user journey
first becomes usable.

## Prepare

1. Identify the changed journey and material affected states.
2. List the pull requests and unresolved issues that can affect the verdict.
3. Select the minimum fake-engine and real-boundary evidence needed.

## Review

Inspect the assembled journey and the states changed by the batch. Include window,
accessibility, failure, recovery, or real-engine evidence only when those boundaries
changed or remain uncertain. Reuse current screenshots and test results.

Do not rerun every state or platform boundary as a fixed checklist.

Use screenshots and direct interaction when visual composition matters.

## Reconcile

For each confirmed finding, identify the requirement or consequence, reconcile it with an
existing issue when possible, and update only the dependencies and later work it affects.

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
