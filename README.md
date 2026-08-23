# pgdump_query

A Rust library + CLI for querying individual tables out of `pg_dump`
plain-format SQL dumps — without loading the whole file into memory —
producing Arrow data.

Two things set the direction: it is meant to be **embeddable as a query data
source** (ultimately a DataFusion `TableProvider`), and **high performance on
local files is a core goal** rather than a later optimization — dumps are
routinely hundreds of gigabytes, so the local-file reader aims to stay
device-bound rather than CPU-bound, at flat memory.

**Status**: early development — Phase 1 (streaming, string-typed row extraction
from plain-format dumps, with a structural cache) is complete; typed columns and
everything after are not started. See
[`docs/status/STATUS.md`](docs/status/STATUS.md) for current implementation
state.

## Quickstart

```sh
cargo build --workspace

# Scan a dump and list the COPY blocks it contains (binary is named `pgdq`).
cargo run -p pgdump_query-cli -- info <dump.sql> --verbose
```

## Documentation

- [`docs/status/STATUS.md`](docs/status/STATUS.md) — current implementation status (what's built vs. not). [`docs/status/history/`](docs/status/history/) holds dated notes for future-session pickup and plan-changing discoveries.
- [`docs/manual/`](docs/manual/) — user manual, starting with [type handling](docs/manual/type-handling.md).
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — project goals and the Phase 1-8 overview; each specified phase gets its own `roadmap-phase<N>-*.md` doc.
- [`docs/design/roadmap-phase1-mvp.md`](docs/design/roadmap-phase1-mvp.md) — full Phase 1 (MVP) spec, the source of truth for the built design. [`…-notes.md`](docs/design/roadmap-phase1-mvp-notes.md) records how it landed in code.
- [`docs/design/roadmap-phase2-typed-columns.md`](docs/design/roadmap-phase2-typed-columns.md) — Phase 2 spec (current, in progress).
- [`docs/design/roadmap-phase7-scan-performance.md`](docs/design/roadmap-phase7-scan-performance.md) — performance design sketch for the local-file read path (Phase 7), and what it constrains earlier.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/postgres-invariants.md`](docs/design/postgres-invariants.md) — `pg_dump` behaviours the design relies on, with source evidence and re-verification steps.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`docs/process.md`](docs/process.md) — the phased development process this project follows (project-agnostic; written to be copied into other projects).
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents (full command reference, architecture detail).
