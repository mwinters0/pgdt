# P35.18 notes — pgdt-mapping carved

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-mapping" tree, its `moves` block applied whole and then struck from that
doc, as 35.17's was. Every body moved whole. No persisted shape changed, so
the cache format was not bumped, and both golden digests hold. `module_graph.py`
finds no cycle in the new crate, and the two `OWED` entries for `gather/` and
`map` ↔ `backfill` are struck.

## Where the cut fell

- **The crate sits between {pgdt-dump, pgdt-codec} and pgdt-query** and names
  `arrow-schema` for `DataType` alone; `layering.py` holds it to that as it
  holds the codec. Besides the design's list it takes `futures`, `tokio` and
  `memchr`, which the interior split's scheduler and resync use: the design
  counted dependencies over query's four files, before the split joined them.
- **Its API**: at the root, `map_file`, `build_index`, `build_map`, `MapRun`,
  `BlockReread`, `bounded_columns`, `gather_block_statistics`, and the strict
  check's listing (`StrictUnchecked`, `UncheckedColumn`, `UncheckedCheck`,
  `UncheckedBound`, `strict_unchecked`); in the `statistics` namespace, the
  request with its constants and what a pass held (`StatisticsHeld`,
  `StatisticsTerms`). The strict listing is at the root rather than in
  `statistics`: it says what a strict parse checks of a block, not what a pass
  gathers. Every embedder path moved from `pgdt_query::` with it.
- **Its `internal`**: what pgdt-query's pass 1 calls (`map_forward`, `splice`,
  `MapStop`, `Pass`, `announce_read_loop`, `reread_rows`), what pruning and
  summaries read a column's statistics against (`declared_column`,
  `stored_bounds_kinds`, `stored_resolution`, `dictionary_holds_field_text`,
  `keeps_sums`), `StatisticsAccount`, which `map_forward`'s signature names,
  and `one_group_bounds` behind a `test-support` feature, for query's
  predicate tests.
- **pgdt-dump's `internal` lost the leader's seven items** and gained
  `PieceEndings`, `RowEnding` and `CopyScannerExt`, whose impl holds
  `recording_endings` and `piece_endings` with their bodies unmoved.
- **`interior/region.rs` reads its parent's private `BlockEndings` and
  `empty_census`**, a child seeing its ancestors' private items; the parent
  re-exports `RegionScan`, `Shortfall` and `scan_region` to the crate.
- **The dictionary is `column.rs`'s**, `DictionaryGatherer` keeping its name.
  `Clipped`'s fields and methods are `pub(super)`, `bounds` building one.
- **`merge_ignored` is private to `backfill.rs`**, its only caller.

## Tests

- **`interior/tests.rs` is the leader's inline module, unsplit**: the piece
  tests and the scheduler tests share their reference scan and helpers.
  `SaveThrottle`'s two tests left `pgdt-query`'s `stream/tests.rs` for
  `forward/save/tests.rs`; `gather/tests.rs` moved whole.
- **Six integration tests are pgdt-mapping's**, the ones exercising its API
  alone: `scan`, `map`, `known_failures`, `statistics_fixture`,
  `statistics_heap` and `unrepresentable`, with `tests/data/edge_cases.sql`
  and `scan`'s snapshot. The rest drive `table_stream`, `table_schema`,
  `read_table` or a summary as well, and stay.
- **The fixture vocabulary is `pgdt-mapping/tests/common/`**, `refused.rs`
  with it; pgdt-query's `tests/common/mod.rs` includes it by `#[path]` and
  adds what reads a batch back. `scripts/check.py`'s `module_files` follows a
  `#[path]` on the line before a `mod`, so a fixture change still reaches
  pgdt-query's sweeps.
- **`persisted_index_is_pinned_to_the_format_version` hashes the name
  `"tests/data/edge_cases.sql"`**, a label rather than the path read, so the
  moved file left its digest alone.

## What later cuts inherit

- **`measure.py`'s declarations follow the crate.** `STREAM` declares
  `backfill.rs`, `forward.rs`, `forward/save.rs` and `whole_file.rs`;
  `STATISTICS` the `statistics` tree; `INTERIOR`, `interior.rs` and
  `interior/region.rs`, replaces `leader.rs` in the three figures that
  declared it.
- **35.20's moves still name `pgdt-query::stream::map_query`**, which this cut
  left where it was.

## Negative results

- **rust-analyzer resolves nothing in a file a package loads from outside its
  own directory.** pgdt-query's integration tests' `common::` items have no
  occurrence in the index, as `refused.rs`'s already had none in `pgdt`'s and
  `datafusion-pgdump`'s; two closures in `tests/statistics.rs` whose return
  type rested on one crashed `rust-analyzer scip` on closure inference, and
  are a value now.
- **The design's "sheds `futures`, `tokio` and `memchr`"** held of query's
  four files and not of the crate, once the interior split came with them.
