# P20.4.1 — The minimum's median at the upper middle group: notes

`gather::density_merges` reads the `⌊G/2⌋+1`-th smallest group instead of the
nearest-rank `⌈G/2⌉`-th; `scripts/row_density.py`'s `min_group` is the same
rule, and `choose` reads it. Why the upper middle group is
[`roadmap-P20-statistics-memory.md`](roadmap-P20-statistics-memory.md),
"Granularity follows row density"; what monotonicity buys is the rustdoc on
`density_merges`. `FORMAT_VERSION` is 20.

## The checks

- `gather.rs`'s `the_density_predicate_is_monotone_in_size` asserts the odd
  tail at both sizes — `[m, m, m, m, 0, 0, 0]` and the `[2m, 2m, 0, 0]` a cap
  would leave it — and then, over random distributions, that asking the rule
  from *any* level of the ladder lands on the same size as asking it from the
  base, which is the property `fit_density` rests on.
- `a_block_sized_by_its_minimum_gathers_what_the_size_it_reaches_gathers` now
  asserts equality in the capped branch: the size is the coarser of the cap's
  and the one the base distribution chooses, where it asserted only
  `>= capped` before. Its `finer_than_cap` counter is what makes the second
  half of that non-vacuous.
- **Neither is vacuous**: with the rank left at `⌈G/2⌉-1`, the monotonicity
  sweep fails on its literal odd-tail case *and* on a random round
  (`[6, 0, 6, 9, …]` at 25, four merges in), and the capped equality fails too.
- `test_row_density.py`'s `test_the_choice_is_monotone_in_size` and
  `test_the_minimums_median_is_the_upper_middle_group` mirror both, and fail
  with `min_group` reverted to nearest-rank.

## Negative results

- **The re-derivation moved nothing.** `row_density.py` was re-run over the
  unchanged `info --json` inputs 20.2 read — `koji-info.json` and every
  fixture and generated document under `runs/row-density-20260915/` — and
  `koji-density.{txt,json}` and `known-shapes*.{txt,json}` came back
  byte-identical. So no koji verdict and no known shape's verdict moves, and
  20.2's reading stands as written. The two rules differ only where *exactly*
  half a block's groups fall short at an even group count, which no block in
  hand is at the size its minimum is read at.
- **The bound is now reached rather than approached.** Under the upper middle
  group `⌈G/2⌉` groups hold the minimum where the nearest-rank median left
  `⌊G/2⌋+1`, so `G ≤ 2R/m` where it was `G < 2R/m`: half the groups at exactly
  `m` and half empty sit on the bound. `test_row_density.py`'s bound sweep
  asserts `<=` for that reason. Nothing reads the bound but the criterion's
  `CLOSE_RATIO`, which is 0.75.

## What the next slices inherit

- **A block's size is exactly `max(cap's size, base distribution's size)`**,
  and that is now asserted rather than bounded. 20.5's maximum passes the cap,
  so it is the one bound that does not fold into that maximum.
- **`FORMAT_VERSION` is bumped for a changed *choice*, not a changed shape.**
  A block records the request that sized it and a back-fill reads that record
  as "already sized under this minimum", so a cache written at 19 would keep
  sizes this rule would not choose; `cache.rs`'s rustdoc on the constant says
  so. 20.4's koji cache and every earlier one are unreadable again, their
  `info --json` exports still being what a re-derivation reads.
- **`row_density.py`'s lower quantiles stay nearest-rank.** Only the registered
  median moved; `quantile` is unchanged and `REPORTED_QUANTILES`' 0.25 and 0.1
  columns still read it, as does `median_group_width`'s width judgement — the
  criterion the maintainer registered before koji was read, which this slice
  does not touch.
