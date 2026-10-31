# P33.1 notes — the evidence

**The instrument is landed and `long-line-resident` is registered untaken**
(`measure.UNTAKEN`); `KD164` is filed. The readings are the next round's: a
figure is taken from a commit, never from a tree carrying its own apparatus
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep"), so they are `cd scripts && uv run measure.py --figure
long-line-resident` at the commit carrying this. That is 270 timed runs and 90
untimed builders over fifteen inputs, about 23 GiB generated once into the
measurement cache; its length is unmeasured, so it is launched detached per
`CLAUDE.md`, "Long-running processes".

- **Fifteen inputs, one value kind and one line length each**
  (`generate_long_lines.py`, `measure.LONG_LINE_LENGTHS`): a cluster of four
  consecutive lines of exactly 32, 128, 288 or 512 MiB or I71's bound,
  terminator included, between short rows, of `text`, `escaped` text (seven
  bytes decoded in eight) or hex `bytea` (half).
- **Three passes at one and four**, the bound at one alone, each under the
  peak-resident wrapper, an allowance sized off the cluster
  (`long_line_allowance`) and I71's bound as its line limit
  (`LONG_LINE_BOUND`): a data-level parse stating `GATHER_STATISTICS`, the
  same parse with `--postgres-invalid-values strict`, and `pgdt sql`'s scan
  counting every value after an untimed builder, its count held to the
  input's rows (`LONG_LINE_SQL`).
- **A kind is two sweeps** (`long_line_groups`): the bound's cluster beside
  the ladder outgrows `warm_bound`, so the bound is swept beside the 512 MiB
  input again, at one worker, and its residual is read against that anchor.
- **The table is a cell per leg and a secant per pass**: the resident a line
  byte adds between the 128 and 512 MiB rows, and its residual at 32 and
  288 MiB and at the bound (`long_line_secant`, `long_line_residual`). What
  would falsify the spec's account of today's copies is written in
  `measure.run_long_line_resident`'s docstring, ahead of any reading.
- **A smoke run, not a reading**: every shape ran over 2 MiB lines of each
  kind on the release build, each query counting its 20 rows; and the three
  passes at one worker read one line at the bound of `text` and of `bytea`,
  each query counting its 17 rows (`runs/p33.1-bound-smoke/`).

## What the readings round inherits

- **A query leg's partition count is checked by nothing.** Its `scan
  started` line is the builder's, at one worker, so `resolved_jobs` is read
  for the parse legs alone; how many lines the four-partition leg held at
  once is its secant against the one-partition one, not an assumption.
- **A parse's `memory_bytes=67108864` under the figure's `--memory` is the
  carve, not an anomaly**: a plain source asks no charge, so its read-buffer
  budget stays `DEFAULT_MEMORY_BUDGET` under any allowance (D83; asserted by
  `status_output.rs`'s
  `a_stated_memory_budget_at_a_serial_job_count_is_not_marked_default`). It
  bounds chunks, never a held line, which nothing charges today. Its
  `statistics_bytes` is what the allowance leaves, gigabytes, of which a
  table of twenty rows holds under a mebibyte (the smoke's log).
- **The allowance is the re-take's too**: `long_line_allowance` is stated on
  both takes, so the two differ by the code alone; why it is sized off the
  cluster is its docstring and
  [2026-10-31](../status/history/2026-10-31.md), "P33.1: the figure re-shaped
  before its readings".
- **No gate twin**: no held line is charged today, so no cell is held to its
  allowance (`LONG_LINE_MEMORY`). The re-take under the ledger is where one
  belongs.

## Negative results

- **DataFusion 55.1 measures no `Binary` value**: `octet_length` and
  `bit_length` refuse a `bytea` column or its view at planning, and `length`
  fails on its bytes, so the query counts its values under a filter keeping
  every row rather than summing their lengths.
- **Not `pgdt query`**: its printing renders each value into a buffer of its
  own, a CLI's term no library charge may carry.
