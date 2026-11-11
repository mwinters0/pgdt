# P33.1 notes — the evidence

**`long-line-resident` was taken at `36888923`**, its readings read against
the falsifiers written before them; `KD164` is filed. The launch's log is
`runs/long-line-resident-20261101-0015/`, the sitting's tables and readings
`runs/measure-20261101T001136/`; [`measurements.md`](measurements.md), "What a
held line holds resident" carries the figure's latest take.

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

## What the later slices inherit

- **A factor read off the ladder's secant is one phase of a growth.** The
  288 MiB row falsifies it at every count, in opposite directions: under the
  secant at one worker or partition, over it at four, where a parse's 288 MiB
  lines hold about what its 512 MiB ones do. The re-takes (33.3, 33.7) read
  the factors against the 288 MiB row and the bound's, not the secant alone.
- **What one reader holds at the bound is that row**, the ladder's secant not
  reaching it: the floor's question (33.5) is read there.
- **One worker and four read through different loops**: a four-worker parse
  holds a third to a half of a one-worker one's bytes a line byte, so the
  one-worker cells price the serial loop's carry (`ChunkCarry`, two buffers
  each kept at a whole line's pages) and the four-worker ones the leader's
  regions. 33.2 and 33.3 change both; neither column stands in for the other.
- **The byte a line byte a one-worker parse holds past the carry's buffers and
  a growth's copy**, still present at 33.3's re-take, is mimalloc's retention
  (the 33.8 notes).
  The three other terms this sitting left unattributed — a one-partition
  query's from 128 MiB, a four-partition query's past four one-partition ones,
  the spread between two sweeps of one leg — are absent from the re-take.
- **A query leg's partition count is checked by nothing.** Its `scan
  started` line is the builder's, at one worker, so `resolved_jobs` is read
  for the parse legs alone.
- **A parse's `memory_bytes=67108864` under the figure's `--memory` is the
  carve, not an anomaly**: a plain source asks no charge, so its read-buffer
  budget stays `DEFAULT_MEMORY_BUDGET` under any allowance (D83; asserted by
  `status_output.rs`'s
  `a_stated_memory_budget_at_a_serial_job_count_is_not_marked_default`). It
  bounds chunks, never a held line, which nothing charges today.
- **The allowance is the re-take's too**: `long_line_allowance` is stated on
  every take, sized since 33.16.2 from the factors the sitting's instrument
  legs read, so two takes differ by the code alone where those do not move;
  why it is sized off the cluster is its docstring and
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
- **Four workers buy a long-line parse nothing today**: the sitting's log
  puts a four-worker parse behind a one-worker one at every length from
  128 MiB, in every kind. A wall clock under the resident wrapper, not a
  figure.
