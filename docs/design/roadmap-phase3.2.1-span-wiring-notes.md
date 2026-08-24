# Phase 3.2.1 — the map becomes `DumpIndex`'s primary structure: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table. That doc states the design; this one records how slice 3.2.1
landed in code and the decisions made while doing so. Per `CLAUDE.md`, this
file is consolidated into a single phase-level notes doc (and removed) once
all of Phase 3 lands.

## What landed

- **`DumpIndex::spans: Vec<Span>` is the primary structure.** The old
  `blocks: Vec<CopyBlock>` field is gone; `DumpIndex::blocks()` is a filtered
  iterator over `spans` (`SpanBody::Data(block) => Some(block)`), and
  `blocks_for`/`total_rows` are built on top of it. A block's byte offsets
  now have exactly one owner, per the design's "The map is the structure, not
  a description of it".
- **`build_index` builds spans in its existing scan pass.** It drives
  `crate::map::Builder` directly (now `pub(crate)`, alongside
  `crate::preamble::PreambleBuilder`) from the same `Event` stream, instead of
  constructing `CopyBlock`s by hand. `build_map` is now a thin wrapper around
  the same `Builder` — kept for its own module's tests and any caller that
  just wants a span list — so producing an index no longer costs two passes
  over the file where it used to (`build_index` + a separate `build_map`
  call).
- **The cache persists spans.** Format version bumped 2 → 3
  (`crate::cache::FORMAT_VERSION`) — pre-1.0, free.
- **`scan_preamble` also builds spans**, covering `[0, preamble_end)` via the
  same `Builder`, and returns them alongside its `DumpMetadata` and the
  preamble-end offset. It deliberately does *not* feed the `Builder` the
  `CopyStart` event it stops at: that would open a `Data` span the scan never
  closes, when what should happen instead is whatever TOC comment precedes
  that header (if any) flushing as its own trailing span up to `preamble_end`
  via `Builder::finish`.
- **`preamble_only` is the one place `SpanBody::Unscanned` is produced by a
  real (not synthetic/test) scan.** It's a genuinely partial scan — unlike
  `build_index`, which always reaches EOF — so after merging `scan_preamble`'s
  spans into the base index, it appends one `Span { start: preamble_end, end:
  file_size, body: Unscanned }` before persisting, whenever `preamble_end <
  file_size`. Covered by
  `tests/cache.rs::preamble_only_persists_a_real_unscanned_tail`, which also
  checks the result against `check_tiling`.
- **`crate::stream::table_stream`'s `Recorder` appends `Span::Data`.**
  `Recorder::record` used to push a bare `CopyBlock` onto `index.blocks`; it
  now wraps it in `Span { start: header_offset, end: end_offset, database,
  body: Data(block) }` and pushes onto `index.spans`, with the same
  dedup-by-`header_offset` guard as before (via `blocks()` instead of the old
  field).
- **Test coverage**: `tests/map.rs::build_index_spans_match_build_map_exactly`
  pins `build_index` and `build_map` to produce byte-identical spans across
  every fixture plus the hand-written `edge_cases.sql`, so the two producers
  can't silently drift apart now that they share `Builder`. Every existing
  `.blocks`-field test (across `tests/scan.rs`, `tests/cache.rs`,
  `tests/query_cache.rs`, `pgdump_query-cli/src/main.rs`) was mechanically
  updated to the `.blocks()` method and still passes unchanged in substance.

## What this slice does not do, and which slice does it

Split out as **3.2.1.1** (earned mid-slice — see
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md) for why):

- **`DumpMetadata` is not yet a derived view over `spans`.** It's still
  populated by its own `PreambleBuilder` pass, run alongside (not built from)
  the span `Builder`, inside `build_index`. The two producers read the same
  `Event` stream in lockstep and can't disagree in practice, but this isn't
  the "parsed content lives in the span, once" shape the design's "The span
  is the container" section calls for. Blocking it: no `SpanBody` variant
  carries the two version-header lines (they classify as generic `Framing`
  today, with no data attached), and `crate::map::classify` gives a
  `--binary-upgrade` dump's `ALTER TYPE ADD VALUE` statements their own
  `Unparsed` span rather than folding the label into the `TypeDef` span it
  belongs to (`PreambleBuilder::dispatch` does this fold; `crate::map` has no
  equivalent). As a consequence, `preamble.rs`'s now-dead
  `current_database_name` helper was removed — `map::Builder`'s own
  `\connect` tracking (identical logic, read from the same lines) is what
  attributes a live-discovered `CopyBlock`'s database now, and nothing else
  needed the `PreambleBuilder` copy of that fact.
- **`scan_preamble`'s spans aren't merged into `stream.rs`'s prepass index.**
  `table_stream`'s call site discards the `Vec<Span>` `scan_preamble` now
  returns (`_spans`), keeping only `DumpMetadata` and the preamble-end offset
  — exactly what it read from that call before this slice. Merging them would
  need a trailing `Unscanned` span that gets truncated the moment the live
  segment below starts recording `Data` spans past that point, which is
  exactly the bookkeeping the next point's gap leaves unsolved anyway.
- **A query-built `DumpIndex` does not tile.** `Recorder` records each
  live-discovered block as its own `Span::Data` but never classifies the DDL
  between them — `stream.rs`'s live segment only ever tracked
  `CopyStart`/`CopyEnd`/`\connect`, deliberately kept minimal because it runs
  in the per-row query hot path, and 3.2.1 didn't change that. `check_tiling`
  is therefore only ever exercised against `build_index`'s output (always a
  full scan) and `crate::map`'s own unit tests, not against anything
  `table_stream` produces. This is an accepted gap, not a silent one: nothing
  yet reads a non-`Data` span back out of a `DumpIndex` (that's Phase
  3.3/3.4), so an incomplete tiling costs nothing today.

## A pre-existing bug, found while smoke-testing this slice

`pgdump_query-cli`'s `Command::Info` (no `--preamble-only`) trusts whatever
`CacheMode::load` returns without checking that it actually covers the whole
file. Running `pgdq info <file> --preamble-only` and then `pgdq info <file>`
prints the preamble-only cache's metadata but reports zero `COPY` blocks
instead of running a full scan. Reproduced against `main` before this slice's
changes, so it predates 3.2.1 and isn't part of its contract; recorded in
`STATUS.md`'s "Known gaps" rather than fixed here, per "never mix
high-confidence and low-confidence work in one review cycle."
