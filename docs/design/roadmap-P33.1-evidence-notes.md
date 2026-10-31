# P33.1 notes — the evidence

**The instrument is landed and `long-line-resident` is registered untaken**
(`measure.UNTAKEN`); `KD164` is filed. The readings are the next round's: a
figure is taken from a commit, never from a tree carrying its own apparatus
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep"), so they are `cd scripts && uv run measure.py --figure
long-line-resident` at the commit carrying this. That is 162 timed runs and 54
untimed builders over nine inputs, about 7.9 GiB generated once into the
measurement cache; its length is unmeasured, so it is launched detached per
`CLAUDE.md`, "Long-running processes".

- **Nine inputs, one value kind and one line length each**
  (`generate_long_lines.py`): a cluster of four consecutive lines of exactly
  32, 128 or 512 MiB, terminator included, between short rows, of `text`,
  `escaped` text (seven bytes decoded in eight) or hex `bytea` (half). A
  kind's three lengths share a sweep, its secant subtracting them.
- **Three passes at one and four**, each under the peak-resident wrapper,
  `parallel-peak-rss`'s allowance and I71's bound as its line limit
  (`LONG_LINE_BOUND`): a data-level parse stating `GATHER_STATISTICS`, the
  same parse with `--postgres-invalid-values strict`, and `pgdt sql`'s scan
  counting every value after an untimed builder, its count held to the
  input's rows (`LONG_LINE_SQL`).
- **The table is a cell per leg and a secant per pass**: the resident a line
  byte adds between the 128 and 512 MiB rows, read at the 32 MiB row
  (`long_line_secant`). What would falsify the spec's account of today's
  copies is written in `measure.run_long_line_resident`'s docstring, ahead of
  any reading.
- **A smoke run, not a reading**: every shape ran over 2 MiB lines of each
  kind on the release build, and each query counted its 20 rows.

## What the readings round inherits

- **A query leg's partition count is checked by nothing.** Its `scan
  started` line is the builder's, at one worker, so `resolved_jobs` is read
  for the parse legs alone; how many lines the four-partition leg held at
  once is its secant against the one-partition one, not an assumption.
- **A parse's `memory_bytes=67108864` under `--memory 2550136832` is the
  carve, not an anomaly**: a plain source asks no charge, so its read-buffer
  budget stays `DEFAULT_MEMORY_BUDGET` under any allowance (D83; asserted by
  `status_output.rs`'s
  `a_stated_memory_budget_at_a_serial_job_count_is_not_marked_default`). It
  bounds chunks, never a held line, which nothing charges today.
- **The figure is re-registered before its readings**
  ([2026-10-31](../status/history/2026-10-31.md), "P33.1: the figure re-shaped
  before its readings"): a length at I71's bound at one worker, 288 MiB at
  both counts, each a cluster of four, an allowance sized off the cluster in
  place of `PARALLEL_BUDGET`'s, stated on both takes, and a test holding `id`
  nullable.
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
