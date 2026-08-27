# pgdump_query

A Rust library + CLI for querying individual tables out of `pg_dump`
plain-format SQL dumps — without loading the whole file into memory —
producing Arrow data.

Two things set the direction: it is meant to be **embeddable as a query data
source** (ultimately a DataFusion `TableProvider`), and **high performance on
local files is a core goal** rather than a later optimization — dumps are
routinely hundreds of gigabytes, so the local-file reader aims to stay
device-bound rather than CPU-bound, at flat memory.

**Status**: early development, pre-1.0, with no compatibility guarantees yet.
What works today: streaming row extraction from plain-format dumps into typed
Arrow batches, a full byte-exact file map and DDL object inventory, and a
best-effort structural cache. What is next — composite/array decoding,
pushdown, engine bindings, the performance campaign, archive formats — is in
[`docs/design/roadmap.md`](docs/design/roadmap.md). See
[`docs/status/STATUS.md`](docs/status/STATUS.md) for exact implementation
state, known gaps included.

## Quickstart

```sh
cargo build --workspace

# Scan a dump once, writing a structure cache beside it (binary is `pgdq`).
cargo run -p pgdump_query-cli -- parse --source <dump.sql>

# Report what that cache holds. `info` never reads the dump itself, so this
# is instant however large the file is.
cargo run -p pgdump_query-cli -- info --source <dump.sql> --verbose
```

## Documentation

- [`docs/status/STATUS.md`](docs/status/STATUS.md) — current implementation status (what's built vs. not, plus known gaps). [`docs/status/history/`](docs/status/history/) holds dated notes for future-session pickup and plan-changing discoveries.
- [`docs/manual/`](docs/manual/) — user manual: [type handling](docs/manual/type-handling.md), [dump inspection](docs/manual/dump-inspection.md).
- [`docs/design/architecture.md`](docs/design/architecture.md) — how the built system works, filed by subject: the scanner, the file map, `DumpIndex`, the preamble grammar, type resolution, decoders, the zero-copy Arrow path, the query passes, the cache, fixtures, testing. The place to start.
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — project goals, the standing rules that cut across all work, and the phases still ahead. Each specified phase gets its own `roadmap-P<N>-<slug>.md` doc.
- [`docs/design/layering.md`](docs/design/layering.md) — the four-layer module constraint and the greps that enforce it.
- [`docs/design/postgres-invariants.md`](docs/design/postgres-invariants.md) — `pg_dump` behaviours the design relies on, with source evidence and re-verification steps.
- [`docs/design/measurements.md`](docs/design/measurements.md) — every performance figure the design relies on, each with the command that reproduces it.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/roadmap-P7-scan-performance.md`](docs/design/roadmap-P7-scan-performance.md) — performance design for the local-file read path, and what it constrains ahead of its own phase.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`docs/process.md`](docs/process.md) — the phased development process this project follows (project-agnostic; written to be copied into other projects). Agents reach it through the `process` skill, which requires reading it in full before implementing a roadmap slice.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents (full command reference, architecture detail).
