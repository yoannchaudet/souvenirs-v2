# Specialist review gates

The required `rubber-duck` review is the default. Add a specialist only for a material
risk that needs domain-specific evidence.

```text
Is there an unresolved architecture risk involving shared policy, contracts, or durable
state that the general review cannot adequately cover?
  yes -> invoke souvenirs-architecture-reviewer
  no  -> continue

Is there an unresolved UX or accessibility risk that requires assembled-application or
platform-specific evidence?
  yes -> invoke souvenirs-ux-reviewer
  no  -> continue

If neither specialist applies, use only the required rubber-duck review.
```

## Architecture gate

Consider `souvenirs-architecture-reviewer` when the work materially changes:

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

Run architecture review during planning when a consequential choice remains open. Run it
before publication when no plan review covered the choice, or when implementation changes
that choice or exposes a new consequence.

## UX gate

Consider `souvenirs-ux-reviewer` when the work materially changes:

- SwiftUI views, navigation, sheets, overlays, or window behavior
- user-visible states, workflows, errors, progress, cancellation, or recovery
- terminology, labels, help text, or accessibility content
- keyboard, focus, VoiceOver, or other interaction behavior
- visual hierarchy, layout, sizing, or behavior represented in approved mockups
- engine or domain behavior that changes what the interface presents to users

Do not invoke it for internal changes that preserve all user-visible states and
interactions. These changes cannot produce UX findings without speculative reasoning.
This exclusion takes precedence over the trigger list.

Run UX review only when the relevant behavior cannot be established from the diff,
focused tests, and current visual evidence.

## Shared rules

Test-only work does not require specialist review unless it changes or codifies product
or architecture behavior.

One specialist is normally enough. Invoke both only for distinct material architecture
and UX risks. Reviewers reuse current evidence and run only the smallest additional check
needed for a verdict.
