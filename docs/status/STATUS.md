# Phase 1 (MVP) — Status

Snapshot of implementation state against `docs/design/mvp.md`. Rewritten in
place as state changes — this doc describes what *is*, not how it got there.
For dated notes on what a future session should pick up, or on discoveries
that changed the plan, see `history/` (one file per day, `YYYY-MM-DD.md`) —
not a changelog, only entries worth keeping. For the design itself (section
names below track `mvp.md`'s headings), see `docs/design/mvp.md`.

Last updated: 2026-08-22 (predicate filtering and the `pgdq query` CLI
subcommand land — the only item MVP had left).

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
  terminator/end offsets, row count). This is the payload the cache
  persists (see below). `DumpIndex` and `CopyBlock` also carry three reserved
  `Option` fields, always `None` in Phase 1 and never constructed by any code
  yet — `CopyBlock::sparse_index`/`SparseRowIndex` (roadmap Phase 5,
  `docs/design/scan-performance.md`), `CopyBlock::column_stats`/
  `RowGroupStats` (roadmap Phase 3), `DumpIndex::metadata`/`DumpMetadata`
  (roadmap Phase 2) — so populating any of them later, once its real shape is
  designed, doesn't force a cache format-version bump.
- **Structure cache** (`cache.rs`): `bincode`+`serde` serialization of
  `DumpIndex`, wrapped in a small `format_version`/`container_kind` envelope
  per `mvp.md`'s "Decisions that keep later phases open" — a cache whose
  version or container this build doesn't recognise (or whose bytes don't
  parse as a cache at all) is treated the same as a missing file: `Ok(None)`,
  never an error, per the "reading is best-effort" rule. Writing is not
  best-effort: `cache::save` propagates I/O failures as `Error::Io` rather
  than falling back to running without a cache. No dump-file identity check
  (size/mtime) — `mvp.md` puts that under "Configurable (future)", not MVP.
  `cache::colocated_path()` derives `<dump-path>.dqcache`; `cache::save`/
  `load` take an explicit path either way. `cache::CacheMode` is the
  higher-level entry point the CLI uses: `CacheMode::resolve(dump_path,
  cache_path)` turns a `--cache-path`-style argument into `Enabled(path)`
  (colocated default, or an explicit path) or `Disabled` (the literal path
  `none` — ignores any existing file at the would-be location and persists
  nothing); `CacheMode::require_enabled(operation)` rejects `Disabled` with
  `Error::CacheDisabled` for operations (like `parse`) whose purpose is to
  populate the cache.
- **CLI**: `pgdq parse` always does a fresh eager scan and (over)writes the
  cache (colocated, or at `--cache-path`); `--cache-path none` is rejected
  up front. `pgdq info` reads a valid cache if one exists at that path;
  otherwise it scans and writes the cache before printing, so the fallback
  path still leaves a cache behind for next time — unless `--cache-path
  none` disabled the cache, in which case the scan result is never
  persisted. `--verbose` adds file offsets either way. `pgdq query <file>
  <table> [--cache-path PATH|none] [--filter column=value|column!=value]`
  streams a table's rows tab-separated (`\N` for NULL) with a header line of
  column names, consulting/populating the cache the same way `info` does.
- **Predicate filtering** (`predicate.rs`): `Predicate { column, op, value }`
  (`op` is `Eq`/`Ne`) threaded through `table_stream`/`read_table` as
  `Option<Predicate>`, evaluated post-parse against each row's decoded field.
  `column` resolves against the matching block's own schema once per block
  (headerless blocks' placeholder names can vary block-to-block for the same
  table); an unresolvable column is `Error::UnknownPredicateColumn`, not a
  silent non-match. A NULL field matches neither operator — see
  `docs/design/mvp.md`'s "Predicate filtering (MVP)" and
  `docs/status/history/2026-08-22.md` for why. 4 unit tests in
  `predicate.rs` plus 4 end-to-end tests in `tests/batch.rs` (Eq, Ne, NULL
  exclusion under both operators, unknown-column error).
- **Error handling**: `pgdump_query::Error` (`thiserror`) — `Io`, `Join`,
  `UnterminatedCopyBlock`, `LineTooLong`, `InvalidUtf8`, `CacheEncode`,
  `CacheDisabled`, `UnknownPredicateColumn`; CLI uses `anyhow`.
- **Tests** (`cargo test --workspace`, 65 tests; see also `batch.rs`'s 15,
  `stream.rs`'s 5, `cache.rs`'s 9, and `query_cache.rs`'s 6, below):
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
- **Streaming API** (`stream.rs`): `table_stream()` is now the primitive — a
  pull-mode `Stream<Item = Result<RecordBatch>>` built with `async-stream`
  (new dependency) directly on `CopyScanner`/`RowBatcher`. `read_table()`
  (push mode) is reimplemented on top of it, draining the stream and driving
  the `FnMut(RecordBatch) -> ControlFlow<()>` callback internally, per
  `mvp.md`. `BlockingTableIter` wraps a `TableStream` in a dedicated
  current-thread `tokio` runtime for sync callers with no ambient runtime
  (not yet wired into the CLI, which has no table-query subcommand yet).
  `ResumeToken` (opaque, no public fields) captures the scanner's file
  offset, a cumulative row count, and — for a token taken mid-`COPY`-block —
  the block's header and in-block row count, letting a fresh stream/
  `read_table` call reconstruct the exact same schema and continue without a
  gap or a repeat; a reserved `generation` field for the future structural
  cache is always 0 in Phase 1. 5 tests in `tests/stream.rs` (pull mode
  matches push mode, resume across arbitrary row-count splits, resume mid a
  headerless-schema block, resume exactly at a block boundary, the blocking
  iterator) plus 2 more in `tests/batch.rs` covering `read_table`'s own
  `Break`-returns-a-token path.
  9 tests in `tests/cache.rs`: colocated-path derivation, a missing file and
  foreign (non-cache) bytes both load as `Ok(None)`, a saved index round-trips
  exactly including the three reserved `None` fields, `save` overwrites an
  existing file at that path, a write failure (unwritable path) propagates as
  `Error::Io`, `CacheMode::resolve` picks the colocated default/explicit
  path/`Disabled` correctly, a disabled cache ignores an existing file at its
  would-be location and leaves it untouched on `save`, and
  `require_enabled` errors on `Disabled`.
- **Incremental indexing consulting the cache during a table query**
  (`stream.rs`): `table_stream`/`read_table` take a `CacheMode` directly
  (`Disabled` — pure streaming, no side effects — or `Enabled(path)`), per
  `mvp.md`'s "Configurable (MVP): eager vs. incremental indexing". A query
  resolves the cache (if any) into a sequence of `Segment`s up front: one
  `Segment::Known { start, end }` per already-cached block matching the
  query table (replayed for its rows, at zero I/O cost for every
  non-matching cached block in between — nothing outside a `Known` range is
  ever read), then one trailing `Segment::Live { start }` covering whatever
  lies past the cache's watermark, scanned as before. Both segment kinds
  drive the exact same `CopyStart`/`Row`/`CopyEnd` handling; a `Live`
  segment additionally hands every block it finds (matching the query table
  or not) to a private `Recorder`, which persists the growing index back to
  the cache after every completed block (a dedup guard by `header_offset`
  makes this idempotent) and once more at the segment's true EOF — so an
  early `ControlFlow::Break`/dropped stream still leaves correct, usable
  partial progress behind, not just a full-scan-or-nothing cache. A `resume`
  `ResumeToken` takes priority over cache replay (see `mvp.md`'s "Known
  gap" on what that costs in the one case where a token was taken mid-replay).
  `CacheMode::Disabled` skips the recorder's bookkeeping entirely. 6 tests in
  the new `tests/query_cache.rs`: a counting `ByteRangeSource` test double
  proves a non-matching cached block costs zero bytes; cached-block replay
  (headered and headerless) matches a fresh scan; `Disabled` is a
  byte-for-byte no-op that never writes a cache file; one query against a
  cold cache leaves behind an index covering every table in the file, not
  just the one queried; dropping a stream mid-block still persists only the
  fully-completed blocks with a consistent `scanned_through`, and a later
  query against that partial cache still finds everything; repeat queries
  against an already-fully-cached file don't grow or duplicate the index.

## Not started

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
- Resuming a `ResumeToken` taken from partway through a cache replay treats
  the rest of the file as unscanned live territory rather than continuing
  the replay — see `mvp.md`'s "Index / structure cache" for why. Correctness
  is unaffected; it only gives back some of the I/O saving for that one
  combination.

## Next up

Every functional item `mvp.md` specifies is now implemented; what's left
under "Not started" (benchmarks, the full worktree fixture sweep) is
groundwork that doesn't block moving on. **Phase 2 (typed columns,
`docs/design/roadmap.md`) is the natural next step, but per the roadmap's own
rule each phase "gets its own full grilling session when it becomes
current"** — that hasn't happened yet, so this session stopped short of
starting Phase 2 implementation rather than presuming its design. The next
attended session should either run that grilling session or explicitly
choose one of the "Not started" items instead.

Two judgment calls made implementing predicate filtering this session are
worth a second look (reasoning in
`docs/status/history/2026-08-22.md`): a NULL field matches neither `=` nor
`!=`, and a `--filter` column absent from the queried table's schema is a
hard error (`Error::UnknownPredicateColumn`) rather than a silent non-match.
Neither was pinned down by `mvp.md` before this session.
