# P28.2 — The metadata and data levels: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decisions are `decisions.md`, "D35" and "D77".

## What exists

- **A block's level is whether it holds a census**: `CopyBlock::array_shapes`
  is `Option<Vec<ArrayShape>>`, `None` for the metadata level, and nothing
  else records it (`CACHE_FORMAT_VERSION` 30). A block holding statistics or a
  decline always holds a census, both being made only at the data level.
  28.3's count belongs under the same `Option`: a metadata-level block records
  neither, and every path that fills one fills the other.
- **Three paths census, and 28.3's count rides each**: `map_forward`, gated per
  block by its `Pass` (`Parse` asks the request's levels, `Query` censuses
  everything); the leader's pieces, handed `columns: None` for a block not
  censused, which then splits no field; and `reread_rows`, the one re-read
  that the back-fill (`reread_block`) and a query's census of a metadata-level
  table (`census_metadata_level`) share, which always censuses and answers the
  census beside what its observer gathered. `gather_block_statistics` returns
  both (`BlockReread`).
- **The level is decided by `StatisticsSelection::level`**, the most specific
  target winning, and `StatisticsRequest::tracked_columns` is the one test of
  whether a block is censused: `Some` exactly where a column, or a column-less
  header's table, is at the data level.
- **A query's cold semantics are `census_metadata_level`**, run by
  `map_for_query` after the matches are narrowed, over the matches alone, under
  `SchemaMode::Typed` only; it prints `table mapped at the metadata level:
  reading its rows again for this query's census` and writes nothing.
  `ReplayPlan::new` and `table_schema` refuse a census-less match
  (`Error::TableAtMetadataLevel`) — the provider's path and any embedder's
  `TablePartitions::plan` — and the provider refuses first, in
  `PgDumpTable::build`, with `datafusion_pgdump::Error::MetadataLevel`, naming
  `pgdt parse … --statistics-level metadata,<table>=data`; `RefusedTable::error`
  is `datafusion_pgdump::Error`, so it can carry that parse.
- **`info` prints a `level:` line under every block**, `data` or `metadata`;
  `--json` carries it as `array_shapes`, `null` at the metadata level.

## What the next slices inherit

- **A data-level map without statistics is what a query's own pass writes,
  and no `parse` does.** The provider tests' "statistics absent" configuration
  — 28.1's harness axis among them — is a data-level map with every
  block's statistics dropped (`map_ungathered` in each provider test target,
  and `parsed_copy` in `datafusion-pgdump/tests/unrepresentable.rs`); a
  metadata-level map would be refused before any case ran.
- **`measure.NO_STATISTICS` is `--statistics-level metadata`**, the flag it
  stated having gone, and `GATHER_STATISTICS` states the data level. So until
  28.9 re-points them, `statistics-pruning`'s `uncarried` leg, the one query
  over a cache a `NO_STATISTICS` builder wrote, reads the table once more for
  the census inside its timer; `statistics-gathering`'s `none` leg no longer
  censuses; and `census-brace-free` and `census-arrays` difference two builds
  of which neither censuses. Those four are barred until 28.9
  ([`../status/history/2026-09-29.md`](../status/history/2026-09-29.md),
  "28.2 leaves the harness mid-way to 28.9"). No register figure was re-taken
  and no table was edited.
- **`query --statistics all|none` is not the level**: it says whether a query
  uses the statistics a cache holds.

## Negative results

- **`&StatisticsRequest::METADATA` is not a promotable constant**: its
  selection holds a `Vec`, so a borrow of it in an argument position whose
  future outlives the statement is a temporary dropped too early. Bind it
  first.
