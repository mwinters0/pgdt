# Phase 3.6 — the bulk-region fast path: how it landed

Companion to
[`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"Bulk regions" and "Verification". Per `CLAUDE.md`, consolidated into the
phase-level notes doc (and removed) once all of Phase 3 lands.

**Status: complete.** Code landed and tested; the koji regression measurement
finished and is interpreted below under "The koji regression check".

## What landed

`SpanBody::Data`'s payload widened from `CopyBlock` alone to
[`crate::map::DataBlock`], an enum of `Copy(CopyBlock)` (unchanged),
`InsertRun(InsertRun)`, and `LargeObjects(LargeObjectRegion)` — one `Data`
span *kind* still, per the design's "Bulk regions" decision, but the three
producers don't share a shape: only `CopyBlock` carries the inner offsets a
row reader seeks by, since it's the only one of the three anything reads rows
out of yet (Phase 8, unscheduled, adds an `INSERT`-run reader). `DumpIndex::blocks()`/`blocks_for`
now filter on `DataBlock::Copy` specifically — their signature (`&CopyBlock`)
is unchanged, so every caller outside `map.rs`/`index.rs` was untouched.
Cache format v7→v8 (`cache.rs`).

### `INSERT` runs — a `map.rs`-only concern, no new scanner state

A run of `INSERT INTO <table> ...;` statements (`--inserts`/`--column-inserts`)
becomes one `Data(InsertRun)` span instead of one `Unparsed` span per
statement. This is entirely `crate::map::Builder`'s doing: a new
`Mode::InsertRun`, entered from `Mode::Statement`'s very first line via
`parse_insert_target` (`INSERT INTO ` + `crate::preamble::parse_qualified_name`,
made `pub(crate)` for this), which then keeps folding completed statements
into the same span (tracked via the already-hardened
`crate::preamble::statement_complete`/`push_stmt_line`/`in_open_quote` — no
new quote/paren tracker) until either a different table's `INSERT INTO`
arrives, or (the ordinary case) the next TOC comment reasserts a boundary the
same way it always has for `Mode::Statement`.

**Deliberately not a scanner-level fast path**, unlike large objects below.
The win here is not re-parsing/re-storing text per row (which reusing the
existing statement accumulator, without pushing a span per statement, already
gets) — and the phase's own "Verification" section doesn't gate `INSERT`-run
*throughput* specifically, only the large-object region's. A scanner-level
`INSERT`-run fast path (skip-and-count without decoding into `Event::Line` at
all) is possible future work if that throughput ever needs measuring, but
nothing in this phase depends on it.

`push_statement_span`'s `extract_statement_cross_refs` scan (`OWNER TO`/
`GRANT`/...) is deliberately **not** run over `INSERT` statement bodies —
`push_insert_run` calls `push_span` directly. A text column's value
containing `" TO "` or `"GRANT "` would otherwise be a false-positive
role/tablespace reference.

### The large-object region — a real scanner-level fast path

`crate::scan::CopyScanner` gains `State::InLargeObjectRegion`, the same tier
`State::InCopy` sits at: `BEGIN;`, byte-exact at the start of a line
(mirroring `is_terminator`'s treatment of `\.`), opens it;
`crate::scan::Event::LargeObjectStart`/`LargeObjectEnd` bracket it; every line
in between is skipped **unread** — no `Event::Line` at all, unlike the
`INSERT`-run path above. I12 is the reason this is scanner-level and INSERT
isn't: on v17+ the region can run to hundreds of gigabytes, and I12's proof
(`StartRestoreLOs`/`EndRestoreLOs`, `pg_backup_archiver.c`) is that a bare
`BEGIN;`/`COMMIT;` pair never appears anywhere else in plain `pg_dump`
output, so the scanner can recognize it with no help from TOC context at all.
An unterminated region is `Error::UnterminatedLargeObjectRegion`, the same
treatment `Error::UnterminatedCopyBlock` gets.

`crate::map::Builder` merges v17+'s several `BLOB METADATA`/`BLOBS`-per-object
entries back into **one** `Data(LargeObjects)` span, matching v13-16's single
shared entry. The merge rule turned out simpler than
`roadmap-phase3-object-inventory.md`'s prose suggested it might need to be:
`Builder::on_large_object_start`/`on_large_object_end` don't push a span at
all, just extend a `pending_large_objects: Option<(start, end, toc)>` field;
`Builder::push_span` unconditionally flushes that pending region first (so
*any* other span implies the region isn't being extended further). Since
`BLOB METADATA` definition entries sort entirely ahead of the pre-data
boundary and every `BLOBS` data entry sorts together in its own contiguous
priority band (I12's "Proof" section, `pg_dump_sort.c`'s
`dbObjectTypePriorities`), nothing else can legitimately arrive between two
of a file's `BLOBS` entries — so "no span was pushed since the last
`COMMIT;`" is already exactly "the next `BEGIN;` continues the same region,"
with no need to re-inspect each entry's own TOC `Type:` field. `on_copy_start`
and `on_large_object_start` itself both flush a stale pending region
defensively (never observed in real output — I12 puts the whole region after
every `COPY` block — but every byte must land somewhere). `Builder::snapshot`
gained a `debug_assert!` that no region is pending when it's called, which
holds for its one real caller (`crate::stream::map_forward`, right after
`on_copy_end`) for the same I12 reason.

Each merged-in entry's own TOC `Owner:`/`Tablespace:` still feeds the
cross-reference set (`Builder::on_large_object_start` does this directly,
independent of whether the entry becomes its own span); the merged span's
single `Span::toc` is the *first* entry's header, since `Span::toc` holds one
and a v17+ run can carry several.

### Fixtures needed no changes

Both fast paths are exercised entirely by fixtures earlier slices already
generated: `fixtures/*/edge_cases/inserts.sql`/`column-inserts.sql` (3.1) for
`INSERT` runs, `fixtures/*/objects/default.sql`/`verbose.sql` (3.1, the only
schema with large objects — two of them, by hand, since no `pg_dump` flag
conjures one) for both the v13-16 single-entry and v17+ per-object-entry
large-object shapes. No new fixture-generation work was needed for this
slice.

### Tests

- `tests/scan.rs`: `large_object_region_is_skipped_without_surfacing_lines`,
  `unterminated_large_object_region_is_an_error`,
  `v17_plus_reports_one_large_object_region_per_blobs_entry` (asserts the
  *scanner* reports two separate regions on v17+ — merging is `crate::map`'s
  job, checked separately).
- `tests/map.rs`: `the_large_object_region_is_one_data_span_on_every_routine_version`
  (all 6 versions, both `default.sql` fixtures) and a rewritten
  `inserts_dump_with_an_embedded_newline_value_still_tiles` (was asserting the
  *old* one-`Unparsed`-span-per-statement behavior; now asserts five
  `Data(InsertRun)` spans, the zero-row `empty_table` entry's TOC comment
  absorbed into the next entry's span per the ordinary comment-boundary rules,
  and the embedded-newline `escapes` row still producing the right
  `row_count`). Every existing whole-fixture-set test
  (`every_fixture_tiles_exactly`, `build_index_spans_match_build_map_exactly`,
  `build_index_records_referenced_roles_and_tablespaces`, ...) passed with no
  changes beyond the `SpanBody::Data` payload type, which is strong evidence
  the merge/flush logic tiles correctly and preserves cross-reference
  attribution across every fixture shape, not just the two new targeted
  tests.

### The synthetic large-object measurement

`scripts/generate_large_object_bench.py` (new, mirrors
`generate_perf_data.py`'s "shaped to satisfy the grammar, never checked
against real `pg_dump`" style) generates a `BEGIN;`/`lo_open`/`lowrite*`
(`LOBBUFSIZE`-chunked, matching real `pg_dump`)/`lo_close`/`COMMIT;` region of
a given size. Run at 3GB on `/mnt/ssd/fedora/pgdump_query-bench/large_object_bench.sql`,
`pgdq info --verbose --cache-path none` in a 512MB-limited container:
~1.6-1.7s wall time (~1.9GB/s) — an order of magnitude above Phase 1's
243MB/s `COPY`-decode baseline, which itself does real decode work this pure
skip doesn't. That gap is the "skipped, not walked" evidence the phase's
"Verification" section asks for: walking the region statement-by-statement
(the pre-3.6 behavior) would mean one `Unparsed` span *and one stored text
string* per `lowrite` call — on the order of hundreds of thousands of spans
for a region this size — which this measurement's memory bound (512MB) and
wall time both rule out. Full command and figures:
`docs/status/history/2026-08-24.md`.

## The koji regression check

The detached re-scan (`pgdq-koji-3.6`, `runs/koji-3.6-scan.log`) finished
`exit=0` with **74 COPY block(s), 19,575,829,920 row(s), 784,019,857,152
bytes scanned** — byte-for-byte identical to the pre-3.6 baseline
(`docs/status/history/2026-08-22.md`), including every block's header/data/
terminator/end offset. That is the actual regression check: this slice's code
does not touch `COPY`-block control flow (only what happens *between*
blocks), and identical offsets on every one of koji's 74 blocks is direct
evidence nothing shifted.

**The raw throughput figure is not directly comparable to the 243 MB/s
baseline, and that's environmental, not a code regression.** Wall time
(container `StartedAt`/`FinishedAt`) gives ~110 MB/s; `node_exporter`'s
`node_disk_read_bytes_total{device="sdb"}` confirms the same ~120-130 MB/s
sustained read rate directly off the disk throughout the scan, against
~240-256 MB/s during the baseline window. The gap is fully accounted for by
contention: `node_disk_written_bytes_total{device="sdb"}` shows a steady
~30 MB/s of *writes* to the same physical disk throughout the 3.6 scan
(absent during the baseline window), matching `koji-pg`'s own logs for the
same interval — a `postgres:16` container restoring onto
`/mnt/wd12t/fedora/koji/pgdata` (same HDD as the dump file), checkpointing
every 10-25s with `distance=~540MB` each time. `sdb` was ~93-95% busy in both
runs; the difference is that the 3.6 run was sharing that near-saturated disk
with an unrelated ~540MB/15s write workload, which on a single HDD costs far
more in seek overhead than its raw byte share would suggest. Confirmed
directly by the maintainer mid-session: a Postgres restore was running on the
HDD during this scan.

Taken together — identical scanned output down to the byte offset, an
unchanged control-flow argument, and a root-caused confound on the only
number that moved — this is read as a clean pass, not a partial result.

## What the next slice inherits

- `DataBlock`/`InsertRun`/`LargeObjectRegion` are all exported from the crate
  root (`lib.rs`), alongside `Span`/`SpanBody`.
- Phase 3.5 (CLI surface, next) can report `InsertRun`/`LargeObjects` spans in
  its `--map` listing exactly like `Data(Copy(_))` ones — same `SpanBody::Data`
  match arm, `DataBlock`'s three variants distinguished only where the CLI
  wants to say more than "bulk data".
- Phase 8 Track A's `INSERT`-run row reader has its target already located
  and attributed (`InsertRun::table`, `row_count`) — it adds parsing, not
  scanning, per the design's original framing for this slice.
- `crate::scan::Event` is no longer just the five Phase 1 variants that every
  exhaustive match site needs to know about — `LargeObjectStart`/`LargeObjectEnd`
  join `CopyStart`/`Row`/`CopyEnd`/`Line`/`DollarQuoteEnd`. Every call site
  that matched `Event` exhaustively (not a bare `_`) needed an update; a
  future new `Event` variant will hit the same set:
  `crate::index::build_index`/`scan_preamble`, `crate::stream::map_forward`
  and its replay loop, `crate::map::build_map`, and three spots in
  `tests/scan.rs`.
