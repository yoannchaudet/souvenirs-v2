# Documentation instructions

These instructions apply to every file in `docs/` and its subdirectories.

Agents must use the `tech-spec` skill when creating, reviewing, or changing these files.
Load the skill before starting work unless it is already active.
If the skill is unavailable, report that limitation and follow the rules below.

## Writing rules

- Use present tense, active voice, and short, direct sentences.
- Target 25 words or fewer per sentence.
- Put one requirement or fact in each sentence and one topic in each paragraph.
- Use one term for one meaning and define necessary technical terms.
- Use `must` for requirements and `must not` for prohibited behavior.
- Do not use em dashes.
- Preserve technical intent and distinguish draft requirements from implemented behavior.
- Record unresolved design decisions as open questions instead of inventing behavior.
- Use only useful sections. Do not expand documents to fill a template.
- Define each requirement once and link to shared rules instead of repeating them.
- Prefer the existing topic documents over new pages.
- Keep examples, links, and expected behavior consistent with the requirements.

## Diagrams

Use D2, not Mermaid.
Keep each diagram source and its rendered SVG in a `diagrams/` directory beside the document.
Render SVGs with the D2 CLI.
Do not edit generated SVGs manually.
If D2 is unavailable, report the blocker instead of substituting another format.
