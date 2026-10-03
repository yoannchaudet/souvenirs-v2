# AGENTS.md

Instructions for AI coding agents working on this repository.

## Project status

Souvenirs2 is a specification-only repository.
No code exists yet.
Do not describe any behavior as implemented.

The original implementation lives in [yoannchaudet/souvenirs](https://github.com/yoannchaudet/souvenirs).
Its guide at commit `5d8202a` is the legacy baseline.

## Document map

- [`docs/catalog.md`](docs/catalog.md) is the authoritative base contract.
- [`docs/datetimes.md`](docs/datetimes.md), [`docs/operations.md`](docs/operations.md), and [`docs/reading.md`](docs/reading.md) refine it.
- [`docs/architecture.md`](docs/architecture.md) proposes an implementation structure.
- [`docs/legacy.md`](docs/legacy.md) compares the format with original Souvenirs.
- [`docs/open-questions.md`](docs/open-questions.md) tracks unresolved decisions.

## Changing specifications

- A companion document must not contradict `docs/catalog.md`.
  Change the base contract first, then the companion.
- A behavior that depends on an unresolved decision cites its `OQ-n` entry.
- When a decision is made, move the rule into the owning document.
  Record the outcome in the open-questions entry.
- Keep entry numbers stable. Do not renumber resolved entries.
- Keep `docs/` for product specifications, not development-process artifacts.

## Writing rules

Specifications use Simplified Technical English.
Use the user's `tech-spec` skill when available.
Otherwise:

- Use present tense, active voice, and short, direct sentences.
- State one fact per sentence. Put each sentence on its own line.
- Use consistent terms from the concept tables.
- Use `must` for requirements.
- Do not use em dashes.
- Do not claim formal ASD-STE100 compliance.

## Formatting

Format Markdown with [dprint](https://dprint.dev) after edits:

```bash
dprint fmt
```

`npx dprint fmt` works without a local install.

## Commits

Use Conventional Commits, such as `docs(catalog): clarify counter allocation`.
