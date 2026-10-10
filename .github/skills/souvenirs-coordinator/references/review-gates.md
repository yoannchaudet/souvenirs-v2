# Specialist review gates

Classify both the intended behavior and the resulting diff.

```text
Does the work change architecture, shared policy, a contract, or durable state?
  yes -> invoke souvenirs-architecture-reviewer
  no  -> continue

Does the work change anything users see, understand, navigate, or interact with?
  yes -> invoke souvenirs-ux-reviewer
  no  -> continue

If neither gate applies, use only the required rubber-duck review.
```

## Architecture gate

Invoke `souvenirs-architecture-reviewer` when the work changes:

- component or crate boundaries
- shared domain policy or ownership of a rule
- protocol, schema, command, or serialized-data contracts
- storage, concurrency, durability, cancellation, or recovery semantics
- dependencies or abstractions used by more than one flow
- an intermediate state that temporarily changes a contract, schema, or durable-state
  invariant that later pull requests must complete

Do not invoke it for isolated copy, styling, fixture, or test-maintenance changes that
preserve existing contracts. These changes need normal correctness review but do not
benefit from architecture analysis. These exclusions take precedence over the trigger
list.

Run architecture review during planning when a consequential technical choice remains
open. Run it again before publication only when implementation differs materially from
the reviewed plan or exposes new consequences.

## UX gate

Invoke `souvenirs-ux-reviewer` when the work changes:

- SwiftUI views, navigation, sheets, overlays, or window behavior
- user-visible states, workflows, errors, progress, cancellation, or recovery
- terminology, labels, help text, or accessibility content
- keyboard, focus, VoiceOver, or other interaction behavior
- visual hierarchy, layout, sizing, or behavior represented in approved mockups
- engine or domain behavior that changes what the interface presents to users

Do not invoke it for internal changes that preserve all user-visible states and
interactions. These changes cannot produce UX findings without speculative reasoning.
This exclusion takes precedence over the trigger list.

Run UX review against the assembled application when practical. A source diff alone
cannot establish visual composition, focus behavior, window sizing, or transitions.

## Shared rules

Test-only work does not require specialist review unless it changes or codifies product
or architecture behavior.

The reviewers do not modify repository source or publish changes. They may build, run,
and observe the application when evidence requires it. The implementation session owns
fixes. The coordinator reconciles conflicting findings and prevents unapproved scope
expansion.
