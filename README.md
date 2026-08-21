# pgdump_query

A Rust library + CLI for querying individual tables out of `pg_dump`
plain-format SQL dumps — without loading the whole file into memory —
producing Arrow data.

**Status**: early development. Phase 1 (MVP) is fully designed; the I/O
layer exists but the `COPY`-block scanner/parser itself is not yet
implemented.

## Quickstart

```sh
cargo build --workspace
cargo run -p pgdump_query-cli -- parse <dump.sql>   # binary is named `pgdq`
```

## Documentation

- [`docs/design/mvp.md`](docs/design/mvp.md) — full Phase 1 (MVP) spec, the current source of truth.
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — Phases 1-5 overview.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents (full command reference, architecture detail).
