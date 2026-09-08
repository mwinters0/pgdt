# P16.13.1 — the sitting

`16.13` built two instruments and left them in `measure.UNTAKEN`, waiting on a
commit to name as their sitting. `16.13.1` is exactly that: both figures taken
at `e29939c` — the commit carrying `16.13`'s apparatus and `M70`'s deadlock fix
both — folded into [`measurements.md`](measurements.md), and both entries moved
into `measure.FIGURES`.

## The run

`cd scripts && uv run measure.py --figure parallel-scan-throughput --figure
parallel-peak-rss`, detached (`runs/parallel-figures-sitting-20260908T003940Z/`).
182 `pgdq` invocations — 140 for throughput (4 legs × 7 `--jobs` values × 5
reps), 42 for RSS (2 legs × 7 values × 3 reps) — none discarded, none retried,
no `warm-parallel` gate rejection anywhere in the log. `M70`'s fix held: every
compressed leg at every `--jobs` ≥ 2 completed, where the apparatus's first
real run had hung on exactly this shape.

Both tables pasted into `measurements.md` verbatim from `tables.md`, each
carrying its own `<!-- figure: … — taken at `e29939c` … -->` marker, placed
after "What a second decode worker buys" and before "koji full scan" per the
handoff.

## What the readings say

**Decode stays the bound, and the fused worker never beats it.** At every
`--jobs` value the `.xz` `parse` leg's implied rate is below
`xz-decode-scaling`'s decode-only rate at the same worker count (202 vs 205
MB/s at one job, 1199 vs 2212 at twenty-four) — consistent with a fused worker
paying for parsing on top of decode rather than decode being free, which is
what the phase's whole design bet on.

**The plain leg's `--jobs` past four is what is asked for, not what is
delivered.** `POOL_DEPTH` clamps the chunk pool to four slots, so `parse` on
plain input is flat (in fact very slightly *down*, 0.87×→0.76×, from
scheduling overhead) from four jobs on — the row range running past four is
what makes that ceiling legible in the table rather than inferred.

**Typed `query` on `.xz` gains far less than `parse` does, proportionally** —
1.60× at twenty-four jobs against `parse`'s 5.93× — because a typed row spends
more of the fused worker's time in scalar decode and render-back, which this
table does not split further; `nested-decode-micro` and `projection-widths`
already price that half on plain input.

**Peak RSS on the 128 MiB-block leg plateaus at ~2.11 GiB from eight jobs on**
(2112.62 → 2111.31 → 2111.37 → 2111.58 MiB across 8/12/16/24 jobs, all inside
each other's spread), where the 24 MiB-block leg is still climbing at
twenty-four jobs (1467.00 MiB, well past its own eight-job reading of 640.69
MiB). Both legs share one budget (`PARALLEL_BUDGET`, 1 GiB) and one block-pool
mechanism, so the earlier plateau on the larger block size is consistent with
the pool retaining fewer 128 MiB blocks than 24 MiB ones before the stated
budget stops affording another — the same mechanism `16.15` is chartered to
re-derive precisely (`STATUS.md`'s `16.15` row). **This sitting does not itself
say what the exact retained-block count is** — RSS includes more than the
pool's counted bytes (per-thread decoder state, batches in flight, allocator
overhead) — only that the curve visibly bends where the stated model predicts
it should.

## The one-job legs and the spec's fused-rate table

`roadmap-P16-parallel-scan.md`'s "What that buys, at 12 physical cores / 24
threads" states a **fused rate per worker** — ~419 MB/s for `parse`, 94% of it
decode — computed as the harmonic combination of a one-core decode rate and
the discovery rate, using **koji's** decode rate (~435 MB/s at its 15.70×
density). That is a different density from this figure's `control` input
(5.45×, ~205 MB/s one-core decode per `xz-decode-scaling`'s control leg), so
the two numbers are not directly comparable, and `--jobs 1` runs
`Parallelism::Serial` — the pre-`P16` single-threaded path, not the leader's
fused worker — so there is no code-level reason to expect it to match a
"fused worker" rate at all.

They turn out to match closely anyway: the harmonic mean of `control`'s own
205 MB/s decode rate and its ~7049 MB/s discovery rate is ≈199 MB/s, against
this sitting's measured 202 MB/s at one job. That is arithmetic, not a
mechanism claim — decoding then parsing the same bytes on one core costs
`size/decode_rate + size/parse_rate` regardless of whether the two stages are
fused or sequential — but it is a real check the spec's model had not had
before, and it holds.

**What this sitting does not confirm** is the spec's device-bound endpoints
(the HDD/SSD/NVMe throughput claims): all four legs here are `warm-parallel`,
reading from `/dev/shm`, so nothing here is device-bound and no registered
figure crosses `--jobs` with a real device. Those remain "arithmetic, not
readings" exactly as the spec already says, and stay a **floor** given the
scaling documented above is sub-linear past four workers on the plain leg and
never linear on the compressed one either.

## Fold-in mechanics, for the next figure that leaves `UNTAKEN`

- **The session-stamp accounting sentence is generated, not hand-counted**
  (`measure.sitting_accounting`), and lives in two places that must agree:
  `measurements.md`'s own "Session stamp" paragraph (machine-checked by
  `--check`, which prints the exact sentence to paste) and `STATUS.md`'s
  paraphrase of the same fact (not machine-checked — a prose sentence, kept in
  sync by hand). Both were updated: 17 of 21 figures now come from the
  `af15eac` sweep, the other four being `peak-rss` (`41c96bb`),
  `xz-decode-scaling` (`7d21c6e`), and `parallel-scan-throughput` /
  `parallel-peak-rss` (`e29939c`, this sitting).
- **`test_every_figure_names_its_consumers` reaches a figure the moment it
  enters `FIGURES`, not before** — `ALL_FIGURES = FIGURES + DERIVED`, and
  `UNTAKEN` is deliberately excluded so an untaken instrument is not obliged to
  declare a consumer of numbers that do not exist yet. Moving an entry out of
  `UNTAKEN` therefore obliges a non-empty `quoted_by` in the same change:
  `parallel-scan-throughput` got `roadmap-P16-parallel-scan.md` (the document
  its numbers answer); `parallel-peak-rss` got `STATUS.md`, whose `16.15` row
  already cited its `--jobs` axis going flat before this sitting existed to
  confirm it.
- **`test_both_figures_are_built_and_not_taken` (renamed
  `test_both_figures_are_taken_and_no_longer_untaken`) was asserting the
  pre-sitting state as a fact** — a test written correctly for `16.13` becomes
  the thing that would have caught this slice landing incompletely, and it
  needed rewriting rather than deletion once the state it asserted changed.
- **`UNTAKEN` is empty again**, its healthy state. Four entries have left it
  so far: `projection-widths`, `xz-decode-scaling`, then these two.

## What `16.15` inherits

Confirmed rather than assumed: `parallel-peak-rss`'s `--jobs` axis genuinely
does go flat past a point on the 128 MiB leg (eight jobs on), which is the
premise `STATUS.md`'s `16.15` checklist row already argues from. `16.15`
re-takes this figure once the stated budget divides on the block a worker
decodes rather than only the query's retained span — the flattening point
should move if that re-derivation changes what the budget affords per worker,
and this sitting's numbers are the "before" to diff against.
