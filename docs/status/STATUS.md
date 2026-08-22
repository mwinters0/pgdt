# Phase 1 (MVP) — Status

Snapshot of implementation state against `docs/design/mvp.md`. Rewritten in
place as state changes — this doc describes what *is*, not how it got there.
For dated notes on what a future session should pick up, or on discoveries
that changed the plan, see `history/` (one file per day, `YYYY-MM-DD.md`) —
not a changelog, only entries worth keeping. For the design itself (section
names below track `mvp.md`'s headings), see `docs/design/mvp.md`.

Last updated: 2026-08-21.

## Done

- **Crate layout**: workspace with `pgdump_query` (lib) + `pgdump_query-cli`
  (bin `pgdq`), edition 2024.
- **Core execution model (I/O layer)**: `ByteRangeSource` trait and its only
  MVP impl, `LocalFileSource` (blocking positioned reads via
  `spawn_blocking`).
- **Error handling**: `pgdump_query::Error` (`thiserror`, currently `Io` +
  `Join` variants); CLI uses `anyhow`.
- **CLI arg wiring**: `pgdq parse <file> [--cache-path]` and
  `pgdq info <file> [--cache-path] [--verbose]` parse their arguments but
  print placeholders only — no scanning, parsing, or cache I/O behind them
  yet.
- **Fixture generation tooling**: `scripts/generate_fixtures.py` (`uv`-run)
  drives `postgres:13/16/18-alpine` containers; `fixtures/{13,16,18}/*.sql`
  populated (7 flag variants × 3 versions).
- **pg_dump compatibility matrix**: `docs/design/pg-dump-compatibility.md`
  substantially filled in from fixture tooling + koji-sample inspection.
- **Dependencies provisioned ahead of use**: `arrow`, `bincode`+`serde`
  already in `pgdump_query/Cargo.toml`, not yet referenced by any code.

## Not started

- **`COPY`-block scanner/parser** — line-anchored `^COPY ` detection, psql
  meta-command skipping, TEXT-format escape handling. This is the critical
  gap; see `mvp.md`'s "Parser robustness requirements" before building it.
- **Row/batch representation** — `Utf8View` Arrow arrays, configurable batch
  size limits.
- **Streaming API** — pull-mode `Stream`, blocking `Iterator` wrapper,
  push-mode callback, `ResumeToken`.
- **Predicate filtering** — single-column `=`/`!=` post-parse filter.
- **Index/structure cache** — `bincode` format, colocated vs. explicit path,
  incremental vs. eager (`pgdq parse`) indexing.
- **`pgq parse` / `pgq info` real implementations** — currently print
  placeholder text only.
- **Unit/snapshot tests** (`insta`) — no test infra wired in; `cargo test
  --workspace` runs zero tests.
- **Benchmarks** (`criterion`) — not wired in.
- **Full 8-version worktree fixture sweep** (`v13.0` … `v18.6`) — worktree
  binaries not yet built; the 3-version container sweep above covers routine
  needs in the meantime.

## Next up

The `COPY`-block scanner is the critical-path item — row parsing, the
streaming API, and the cache all depend on it and can't meaningfully start
first.
