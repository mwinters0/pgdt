# P10.7 — Back-fill of blocks lacking the requested statistics: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"How gathering is asked for", "Group size" and "Reporting"; the call on what a
re-read keeps is `decisions.md`, "D34". Nothing a query does changed, and no
figure was taken.

## What exists

- **`StatisticsRequest::backfill(&CopyBlock) -> Option<StatisticsBackfill>`**
  is the one test of "lacks": no statistics, a tracked column not gathered, or
  a *stated* size other than the block's. Its answer gathers the union of the
  block's columns and the request's, at the stated size or else the block's
  own, so a re-read never narrows a block.
- **`gather_block_statistics(source, options, metadata, block, backfill)`** is
  the per-block entry point the spec names. It offers the block to
  `leader::scan_region`, reads it serially where that declines
  (`stream.rs`, `observe_rows`), and answers `Ok(None)` on a cancel. A block
  whose re-read `CopyEnd` is not the map's is `Error::CachedBlockChanged`.
- **`map_file` back-fills after its map reaches EOF** and after the whole-file
  metadata is recomputed, so each block resolves as a straight-through pass
  resolved it. `backfill_statistics` saves through a `SaveThrottle` of its own
  and on an interrupt. `MapRun` carries `lacking_statistics` and `backfilled`;
  a run interrupted before EOF counts neither.
- **Stderr** gets `statistics back-fill started blocks=N` and, once every block
  is re-read, `statistics back-fill complete blocks=N`; nothing when none lacks.
  Neither line holds the word `scan`, which `status_output.rs` filters on. On
  stdout, `nothing to scan` gives way to a line saying only lacking blocks were
  re-read; an interrupt inside the back-fill words both stderr lines for it.
- `gather::observer_tracking` is `observer_for` from a column mask and a size.

## Tests

- `tests/statistics.rs`: every major, re-read from none, from the default size,
  from a table selection and from one column into the one-pass map and cache;
  an unstated size keeping a block's size; a narrower request never dropping a
  column; a selection re-reading its table alone; the entry point on one block;
  the leader-split re-read against the serial pass; the same-size rewrite.
- `tests/map_file.rs`'s `an_interrupted_backfill_banks_the_blocks_it_reread`,
  the cancelling source at the second block's data.
- `pgdump_query-cli/tests/determinism.rs`'s
  `every_fixture_backfills_to_the_cache_one_gathering_parse_writes`, byte for
  byte from `--statistics none` serially and from the default at `--jobs 8
  --chunk-size 64`; `tests/statistics.rs`'s
  `a_backfilling_parse_counts_the_blocks_it_rereads` for the lines.

## For the slices after

- **10.8.** Every block a gathering `parse` leaves holds what its request
  asked, so a query meeting a block with `statistics: None` under a whole-file
  `parse` meets one gathered under `none` or outside a selection. Statistics
  of mixed group sizes across a table's blocks are ordinary.
- **10.10.** `statistics-gathering` times a cold gathering `parse`; a back-fill
  is a second, block-by-block read, unmeasured.

## Rejected, on review

- **A back-fill narrowed to its request**: `--statistics colA` re-reads no
  block already holding `colA`, so a narrowed re-read would drop another column
  only from the blocks some other lack sent back to the source. A cache that
  shrinks on a selection would strip unrequested columns from every block in
  place, needing no read; as built, the cache only accumulates, and dropping a
  statistic means deleting it.
- **Storing, skipping or re-mapping a block that no longer ends where the map
  says.** Stored, its statistics contradict the offsets and row count beside
  them; skipped, the back-fill goes on writing statistics for a file it knows
  was rewritten; re-mapped, the library replaces cache data unasked ("D20").
  Cached replay trusts the map's offsets unchecked, so `CachedBlockChanged` is
  the only place a same-size rewrite moving a block's end surfaces.

## Negative results

- **Five hand mutations each failed a new test**: the stated size ignored, the
  union narrowed to the request, an unstated size taking the default, the
  `CopyEnd` check dropped, and the leader's pieces discarded on a closed region.
- **The save on an interrupt is not pinned apart from the throttle's**: the
  throttle starts due, so the first re-read block is banked either way, and the
  interrupt test cannot tell the two saves apart.
- **The shortfall line is once per pass**, so a `--jobs` a budget cannot pay
  prints `scan arrangement` for the scan and again for the back-fill.
