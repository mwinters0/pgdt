# P36.7 notes — back-fill as one call; the pass hands back its parts

`pgdt_mapping::backfill_block(source, read, &Arrangement, invalid, &request,
metadata, database, &mut CopyBlock) -> Result<bool>` re-reads one mapped block
until it holds what the request asks, and records each read in the block. It
answers whether it read the block. `gather_block_statistics` is gone.
`StatisticsBackfill`, `StatisticsRequest::backfill`, `bounded_columns` and the
re-read's outcome are crate-private. `KD115` is struck. `MapRun` carries the
source and the watch beside the index.

## The shape

- **One per-block step, two callers.** `Rereads::fill` in `backfill.rs` is
  what `map_file`'s back-fill runs on each block and what `backfill_block`
  runs on one: a read for what the block lacks, a second at the finer size a
  stated maximum asks for, each read recorded as it closes (census,
  statistics or the allowance they declined under, the fields an untracked
  column went past, a clean strict check). The pass keeps its own loop for
  saving, the throttle and the cancellation.
- **`StatisticsRequest::backfill` computes the mask itself**, from the
  metadata and the block's database, and only for a block that holds
  statistics, the only case that reads it. `backfill_bounded` takes the mask
  as a closure, so a unit test can still state one that no column resolves to.
- **The allowance is the arrangement's, and it bounds the call's account.**
  `backfill_block` reads `Arrangement::statistics_allowance` and bounds a
  `StatisticsAccount` by it, charging the block's held statistics as
  `Term::Loaded` until a read replaces them. That is the pass's accounting
  applied to one block, so a read past the allowance declines and the block
  records it, and the same arrangement then reads nothing (D85). The old
  `gather_block_statistics` charged an unbounded account and read the
  allowance only for the decline record, so nothing it read could decline.
- **The strict check comes with it.** Under `PostgresInvalidValues::Strict`
  a block no strict parse checked is read even where it lacks no statistics,
  as the pass reads it.
- **`MapRun` hands back `source` and `watch`** and is `#[non_exhaustive]`, on
  `Opened`'s reasoning (`M226`): only a run pairs a map with the source and
  the watch it was built over. Its `Debug` is written by hand, since the
  source is not `Debug`. `map_file` keeps them while the pass borrows them,
  through a private `Mapped`, and hands them back once the watch has
  attributed any failure. The mode does not come back: the `Dump` 36.8 builds
  never writes the cache.

## Tests moved

- **Gathering at a hand-built plan is `pgdt-mapping`'s own test.** The two
  `every_fixture_block_…_gathers_what_its_final_size_gathers` tests build
  plans no request answers (a cap of two groups, a minimum of three rows over
  an eight-byte base), so they moved from `pgdt-query/tests/statistics.rs` to
  `crates/pgdt-mapping/src/backfill/tests.rs`, unchanged in what they assert.
  The mask assertions moved there too, as
  `a_held_column_lacks_bounds_only_where_gathering_keeps_them`.
- **What stays in `pgdt-query`** goes through the public call:
  `one_block_is_gathered_on_its_own` now asserts the whole block, census
  included, and that a block holding what was asked is not read again;
  `a_text_column_held_without_bounds_is_reread_for_them` back-fills the
  stripped block alone and matches the pass's result; and
  `one_block_backfilled_past_its_allowance_declines_until_a_larger_one` is
  new.

## Negative results

- **`backfill_block` taking `map_file`'s `&ScanOptions` and `&GatherOptions`**
  was refused. That pair states the parallelism and the statistics allowance
  in two values that can disagree, and the spec and 36.2's notes give the
  call the arrangement, which carries both. `map_file` takes it too at
  36.7.1 (history, 2026-10-18, "The mapping pass takes the arrangement").
- **Taking `&mut DumpIndex` and a header offset**, so the metadata and the
  database could not be passed apart from the block, was refused. It needs a
  lookup that can fail, and the spec and 36.3.2's rule ("a function that
  resolves a block against the DDL takes the database as its own argument,
  after `metadata`") give the metadata, the database and the block.
- **Exposing `StatisticsBackfill` through `internal`** for the moved tests
  was refused. The spec makes it `pub(crate)`, and the tests sit in the crate.

## What the next round inherits

- **36.8 builds a `Dump` from a `MapRun` run to the file's end**: its
  `source`, `watch` and `index`, with `index.is_complete(size)` deciding
  whether it is one.
- **`pgdt parse` still clones the source before `map_file`**, to read its
  size and resolve the arrangement before the pass. Nothing reads the
  returned `source` yet.
