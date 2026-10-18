# P36.7.1 notes — the mapping pass takes the arrangement

`pgdt_mapping::map_file(opened, read, &Arrangement, invalid, &request,
&cancel)` reads under the arrangement's parallelism and bounds its account by
the arrangement's statistics allowance, as `backfill_block(source, read,
&Arrangement, invalid, …)` does. `GatherOptions` is gone. `ScanOptions` is
taken only by the passes that gather nothing. `Arrangement::resolve` was
already its only constructor, and still is. Why the pass takes the carve whole
is D83's `Rejected` line.

## The shape

- **Inside the pass nothing is regrouped.** `map_file` builds the
  `ScanOptions` its read loops share from `read` and the arrangement's
  parallelism, and hands the private `map_file_watched` and
  `backfill_statistics` the mode and the allowance as two arguments where
  they took a `&GatherOptions`.
- **`pgdt parse` hands over the arrangement it resolved**:
  `Resolved::arrangement` lends it, and `args::scan_options` serves only
  `--preamble-only` and `query`'s mapping pass.
- **The tests state an arrangement through `pgdt_fixtures::arranged(jobs)`**:
  `jobs` stated under `Allowance::NoneFound`, so the serial path stating no
  budget at one and `DEFAULT_MEMORY_BUDGET` above it — exactly the two
  `ScanOptions` values every test passed before. pgdt-mapping's integration
  tests take `pgdt-fixtures` without `mapping`, so each writes it again.
- **A test needing a statistics allowance resolves one.** `holding(jobs,
  bytes)` in `pgdt-query/tests/statistics.rs` states a gibibyte and draws the
  share's excess over `bytes` (`ArrangementRequest::drawn`). It asserts the
  worker count and budget equal `arranged(jobs)`'s and the share equals
  `bytes`, so the two allowance tests read at the parallelism they read at
  before.

## Negative results

- **Stating a small allowance directly does not serve the four-worker
  allowance tests.** Under a stated allowance below `MEMORY_RESERVE` plus
  `DEFAULT_MEMORY_BUDGET` the budget is carved below the default, so the
  leader's split declines, and those tests' four-worker legs would read
  serially. Above it, the share left to statistics is tens of MiB, more than
  the test's quarter of a peak. `drawn` is what takes the share down without
  touching the budget.
- **A test-side `GatherOptions` was not kept.** The tests' value pairs were
  re-spelt as a `ReadOptions` and a worker count, not grouped again.

## What the next round inherits

- **36.8 builds a `Dump` from a `MapRun`.** `map_file`'s arguments are now
  the shape a `Dump`'s opening pass will take: a `ReadOptions`, the
  arrangement, the mode, the request and the token.
- **`pgdt parse` still clones the source to resolve the arrangement before
  the pass** (36.7's notes, "What the next round inherits").
