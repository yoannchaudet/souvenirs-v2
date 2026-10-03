# Souvenirs2

Souvenirs2 is the next catalog format for [Souvenirs](https://github.com/yoannchaudet/souvenirs), a photo and video organizer.
The file system is the catalog.
Files stay understandable without Souvenirs.

## Status

This repository contains specifications only.
Nothing is implemented yet.
Every document describes proposed behavior.

## Specifications

| Document                                 | Scope                                                                                    |
| ---------------------------------------- | ---------------------------------------------------------------------------------------- |
| [Catalog collections](docs/catalog.md)   | The base contract: on-disk format, collections, names, counters, audit log, and exports. |
| [Datetime proposals](docs/datetimes.md)  | Metadata fields, proposal kinds, interpretations, and recorded origins.                  |
| [Operations](docs/operations.md)         | Plan lifecycle, write eligibility, decisions, outcomes, and each operation's contract.   |
| [Catalog reading](docs/reading.md)       | Traversal, logical items, findings, coverage, and the inventory interface.               |
| [Architecture](docs/architecture.md)     | Crates, platform primitives, audit log schema, and recovery states.                      |
| [Legacy comparison](docs/legacy.md)      | Differences from original Souvenirs and conversion obligations.                          |
| [Open questions](docs/open-questions.md) | Decisions needed before implementation, with recommendations.                            |

The catalog specification is authoritative.
Companion documents refine it and must not contradict it.
