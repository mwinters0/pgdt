# Phase 1 (MVP) — Status

Snapshot of implementation state against `docs/design/mvp.md`. Rewritten in
place as state changes — this doc describes what *is*, not how it got there.
For dated notes on what a future session should pick up, or on discoveries
that changed the plan, see `history/` (one file per day, `YYYY-MM-DD.md`) —
not a changelog, only entries worth keeping. For the design itself (section
names below track `mvp.md`'s headings), see `docs/design/mvp.md`.

Last updated: 2026-08-22.

## Done

- **Crate layout**: workspace with `pgdump_query` (lib) + `pgdump_query-cli`
  (bin `pgdq`), edition 2024. `rustfmt.toml` sets
  `use_small_heuristics = "Max"` so `cargo fmt --check` matches the
  codebase's compact style.
- **Core execution model (I/O layer)**: `ByteRangeSource` trait and its only
  MVP impl, `LocalFileSource` (blocking positioned reads via
  `spawn_blocking`).
- **`COPY`-block scanner/parser** (`scan.rs`, `copy.rs`):
  - `CopyScanner` is a synchronous, zero-copy state machine. It does not own
    the bytes it scans — the caller owns a buffer and the scanner reports how
    much it consumed — so the same state machine can back the async driver
    and, later, a pull-mode `Stream`.
  - Line-anchored `COPY` detection with a strict
    `COPY <table> [(<cols>)] FROM stdin;` grammar (schema-qualified or bare,
    quoted or unquoted identifiers). Inside a block only the `\.` terminator
    is looked for, so data can never be read as structure. Non-matching
    `COPY` lines (`TO stdout;`, `WITH (...)`) and psql meta-commands
    (`\restrict`, `\unrestrict`, `\connect`) are skipped as ordinary SQL.
  - COPY TEXT field splitting and unescaping: `\N` NULL (distinct from empty
    string), `\b\f\n\r\t\v\\`, octal (`\101`) and hex (`\x41`) escapes,
    PostgreSQL's permissive "unknown escape stands for itself" rule.
    Non-UTF8 field bytes are a hard `Error::InvalidUtf8`.
  - CRLF line endings and a missing final newline are tolerated; an
    unterminated block and an over-long line are errors, not silent
    truncation or unbounded buffering.
  - `scan()` drives it over a `ByteRangeSource` with a configurable
    `chunk_size` / `max_line_bytes` (`ScanOptions`), pushing `Event`s to a
    sync `FnMut(Event) -> ControlFlow<()>` callback.
- **Structure index** (`index.rs`): `build_index()` runs an eager full-file
  scan into a `DumpIndex` of `CopyBlock`s (header, column names, header/data/
  terminator/end offsets, row count). This is the payload the cache will
  persist; it is **not** serialized or written to disk yet.
- **CLI**: `pgdq parse` and `pgdq info` both run a real eager scan and print
  the discovered structure (`--verbose` adds file offsets). `--cache-path` is
  accepted but inert.
- **Error handling**: `pgdump_query::Error` (`thiserror`) — `Io`, `Join`,
  `UnterminatedCopyBlock`, `LineTooLong`, `InvalidUtf8`; CLI uses `anyhow`.
- **Tests** (`cargo test --workspace`, 36 tests; see also `batch.rs`'s 10
  below):
  - Unit tests for the header grammar and escape decoding.
  - `pgdump_query/tests/data/edge_cases.sql` — hand-written and deterministic
    (no generated timestamps, so snapshots are stable) — under an `insta`
    snapshot of the whole event stream, plus a test asserting the event
    stream is identical across chunk sizes 1…4096 (chunk-boundary
    correctness).
  - The generated `fixtures/{13,16,18}` tree gets structural assertions
    (block names, columns, row counts, and offsets verified to address the
    right bytes) rather than snapshots, since its timestamps change on
    regeneration.
  - A decoder round-trip against real `pg_dump` output on all three
    versions: `fixture_schema.sql`'s `public.escapes` table holds one row per
    codepoint (`chr(n)`), so the test compares against a value it computes
    itself instead of a hand-transcribed literal. Covers every escape
    `pg_dump` emits (`\b \t \n \v \f \r \\`), the raw control bytes it leaves
    unescaped, and 2-, 3- and 4-byte UTF-8.
- **Fixture generation tooling**: `scripts/generate_fixtures.py` (`uv`-run)
  drives `postgres:13/16/18-alpine` containers; `fixtures/{13,16,18}/*.sql`
  populated (7 flag variants × 3 versions).
- **pg_dump compatibility matrix**: `docs/design/pg-dump-compatibility.md`
  substantially filled in from fixture tooling + koji-sample inspection.
- **Dependencies provisioned ahead of use**: `arrow`, `bincode`+`serde`
  already in `pgdump_query/Cargo.toml`, not yet referenced by any code.
- **Full-file scan validated against a real 784GB dump**: `pgdq info
  --verbose` over the koji sample completed clean — exit 0, 74 `COPY`
  blocks, 19,575,829,920 rows, all 784,019,857,152 bytes accounted for, no
  `UnterminatedCopyBlock`. `lock_monitor.activity` (the table whose row data
  contains a literal `COPY ... TO stdout;` substring) parsed as a single
  correct block, confirming line-anchored detection holds against the case
  that motivated it. All 74 blocks are plausible koji tables with no
  duplicates or spurious splits. Log at `runs/koji-scan.log` (gitignored).
- **Row/batch representation** (`batch.rs`): `read_table()` drives the
  scanner over a `ByteRangeSource` and assembles `Utf8View` `RecordBatch`es
  for every row of every `COPY` block matching a given table name (bare or
  qualified), pushed to a sync `FnMut(RecordBatch) -> ControlFlow<()>`
  callback — same shape as `scan()`. Batches flush at a configurable row
  count (`BatchOptions::max_rows`, default 8192) and/or byte count
  (`max_bytes`, no default cap); a table with zero matching rows produces no
  batches. A header with no explicit column list gets placeholder names
  (`column1`, `column2`, ...) sized to the first row's field count, since
  Phase 1 has no DDL parsing to name them from.
  Per `docs/design/scan-performance.md`'s constraint on this layer: a field
  with no escapes is appended as a zero-copy `StringViewBuilder` view into
  the Arrow `Buffer` backing the read chunk it came from
  (`append_block`/`append_view_unchecked`), not copied. Only escaped fields
  and the rare field whose bytes straddle two read chunks take a copying
  path (`append_value`). Chunk buffers are retained in a small deque, evicted
  once the scanner has moved past them for good; a chunk's cached
  `StringViewBuilder` block index is invalidated on every flush, since
  `StringViewBuilder::finish()` resets the builder's internal block list.
  10 tests in `tests/batch.rs`: decode correctness against the hand-written
  edge cases (NULL, all escape kinds, empty-vs-NULL, quoted identifiers, the
  no-column-list case), batch-size-limit splitting, chunk-size independence
  (exercises the zero-copy/straddling/decode-copy paths against the same
  expected output), and a `public.escapes` round-trip against real `pg_dump`
  output on all three fixture versions.

## Not started

- **Streaming API** — pull-mode `Stream`, blocking `Iterator` wrapper,
  push-mode callback, `ResumeToken`. `read_table()`'s callback is now the
  batch-level push shape `mvp.md` specifies; pull mode and the `Iterator`
  wrapper still need building on top of it.
- **Predicate filtering** — single-column `=`/`!=` post-parse filter.
- **Index/structure cache** — `bincode` format, colocated vs. explicit path,
  incremental vs. eager indexing. `DumpIndex` is the intended payload;
  nothing is serialized or written yet, and `pgdq info` therefore rescans
  instead of reading a cache.
- **Benchmarks** (`criterion`) — not wired in.
- **Full 8-version worktree fixture sweep** (`v13.0` … `v18.6`) — worktree
  binaries not yet built; the 3-version container sweep above covers routine
  needs in the meantime.

## Known gaps

- A line inside a dollar-quoted function body that starts at column 0 *and*
  matches the full `COPY ... FROM stdin;` grammar would be mistaken for a
  real block. Closing this needs dollar-quote tracking in the scanner; see
  `docs/design/pg-dump-compatibility.md`.
- Large objects in plain-format dumps (`lo_create`/loader calls, not `COPY`
  blocks) remain unmodelled.

## Next up

The streaming API on top of `read_table()`: a pull-mode `Stream`, a blocking
`Iterator` wrapper over it, and the opaque `ResumeToken`. The cache is the
other independent thread of work and can proceed in parallel — `DumpIndex` is
ready to be given a serialized form; leave room in it for three optional fields
so adding any of them later isn't a format break: a sparse row index
(`docs/design/scan-performance.md`); a dump-level metadata block — server
version, `pg_dump` version, extension list, user-defined type definitions
(`docs/design/roadmap.md`, "Companion: dump-level metadata"); and per-row-group
column statistics keyed to the sparse index's checkpoints
(`docs/design/roadmap.md`, "Companion: per-row-group column statistics"). None is
populated in Phase 1; only the slots are due.
