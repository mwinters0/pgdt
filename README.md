# pgdump_query

A Rust library + CLI for querying individual tables out of `pg_dump`
plain-format SQL dumps — without loading the whole file into memory —
producing Arrow data.

Two things set the direction: it is meant to be **embeddable as a query data
source** (ultimately a DataFusion `TableProvider`), and **high performance on
local files is a core goal** rather than a later optimization — dumps are
routinely hundreds of gigabytes, so the local-file reader aims to stay
device-bound rather than CPU-bound, at flat memory.

**Status**: early development. See [`docs/status/STATUS.md`](docs/status/STATUS.md)
for current implementation state.

## Quickstart

```sh
cargo build --workspace

# Scan a dump and list the COPY blocks it contains (binary is named `pgdq`).
cargo run -p pgdump_query-cli -- info <dump.sql> --verbose
```

## Documentation

- [`docs/status/STATUS.md`](docs/status/STATUS.md) — current implementation status (what's built vs. not). [`docs/status/history/`](docs/status/history/) holds dated notes for future-session pickup and plan-changing discoveries.
- [`docs/design/mvp.md`](docs/design/mvp.md) — full Phase 1 (MVP) spec, the current source of truth for design.
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — project goals and the Phase 1-6 overview.
- [`docs/design/scan-performance.md`](docs/design/scan-performance.md) — performance design sketch for the local-file read path (Phase 5), and what it constrains earlier.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents (full command reference, architecture detail).
