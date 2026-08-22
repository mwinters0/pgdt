# pgdump_query

A Rust library + CLI for querying individual tables out of `pg_dump`
plain-format SQL dumps — without loading the whole file into memory —
producing Arrow data.

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
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — Phases 1-5 overview.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents (full command reference, architecture detail).
