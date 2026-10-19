# P36.12 notes — The embedder test

`crates/pgdt-query/tests/embedder.rs` walks the spec's five journeys over
`fixtures/16/statistics/default.sql`'s `public.ordered`, copied into a
temporary directory: open and map with statistics, load the cache and judge
it, list the tables and their Arrow types, stream a projection under a filter
four ways, and summarise. It names the seven libraries' API and nothing else
from the workspace, and every journey passed against the surface as it
stood: **the phase's claim held without a library change.**
`scripts/layering.py` holds the file to the embedders' rule
(`EMBEDDER_TEST`), refuses it missing, and reads every file its `mod` lines
reach; `test_layering.py`'s `EmbedderTest` holds each rule.

## The shape

- **It names no `pgdt_fixtures`, and the script refuses it**, beside
  `internal` and the two macros: no embedder can depend on the fixture
  vocabulary (`GRAPH` lets only a dev-dependency reach it), so a test naming
  it would claim a path no binding has. It reaches the fixture by
  `CARGO_MANIFEST_DIR` and declares no module, `tests/common` naming
  `pgdt_codec::internal::column_of`.
- **Its other `pgdt-query` tests are not held**: they test the library from
  inside, `internal` included, and only this file stands for an embedder.
- **What each journey calls** is the reference a binding starts from:
  `Origin::local` and `CacheMode::resolve(&origin, None).open(&origin)`;
  `Arrangement::resolve` over `Allowance::Stated` and
  `ArrangementRequest::of(source, Some(jobs))`; `map_file` then
  `Dump::mapped`; `Dump::opened` over a loaded open, `OfflineCache::load`
  for the cache alone; `Dump::table(&TableSelector)`; `Table::schema`,
  `plan`, `summary`; `TablePartitions::stream`, `in_file_order`, `resume`;
  `render_field` over the plan's column records.

## Negative results

- **A stated allowance below the carve's reserve gathers no statistics and
  plans serially, and says so**: at 256 MiB the data-level pass recorded
  `statistics_declined` on the block and kept no statistics, and a plan
  stated four workers came out one partition with `AllocationBelowFloor`
  and `ParallelismBudgetLimited` among its notes. The test states 2 GiB.
  The reserve itself is `pgdt_io::internal`'s (D11), so a binding offering
  an allowance surfaces those two signals rather than checking a floor.

## What the next round inherits

- **Every box of P36 is ticked**: the wrap is next.
