# P20.4 — The density minimum: notes

The rule is `gather::Gatherer::fit_density` over `gather::density_merges`, run at
a block's end after the cap; the request is `StatisticsRequest::min_rows` with
`DEFAULT_STATISTICS_MIN_ROWS`, and the record `BlockStatistics::sizing`, a
`GroupSizing`, which `StatisticsRequest::backfill` compares. Why it is shaped
that way is [`decisions.md`](decisions.md), "D82" and "D34"; the user-facing
statement is [`../manual/dump-inspection.md`](../manual/dump-inspection.md),
"`--statistics`: what `parse` records for later queries", and the flags' help.
Which group the median reads is 20.4.1's
([`roadmap-P20.4.1-upper-middle-group-notes.md`](roadmap-P20.4.1-upper-middle-group-notes.md)).

## The checks

- `gather.rs`'s `density_merges_choose_the_finest_size_whose_median_holds_the_minimum`
  holds the rule to the ladder `scripts/row_density.py`'s `choose` walks, over
  random rows per group; `a_block_sized_by_its_minimum_gathers_what_the_size_it_reaches_gathers`
  gathers random blocks serially, in pieces and exactly at the size reached,
  with and without a cap ahead, and uncapped holds the size to that ladder over
  the base size's rows.
- `tests/statistics.rs`'s
  `every_fixture_block_sized_by_its_minimum_gathers_what_its_final_size_gathers`:
  every block of every fixture at 8 bytes under a minimum of three rows —
  serial, split at three and eight workers, exact at the size reached, and that
  size the ladder's over an exact 8-byte gather; most blocks coarsen.
  `a_stated_minimum_rereads_a_block_sized_under_another` walks the back-fill
  rule per bound.
- **Neither is vacuous**: with `fit_density` not called, the unit test and the
  fixture sweep both fail.
- The `introspect` reconciliation passes,
  `runs/statistics-account-20260915-density/log-3.txt`. Its flagless wide- and
  long-text legs now merge at their block's end — the first legs to reconcile a
  merge.

## Negative results

- **A dictionary's renumbering, landed with the cap, did not reconcile once a
  leg merged.** `log.txt` beside the passing run is the long-text flagless leg
  failing both ways. Over: the entries and keys it let go were freed before
  the update releasing its scratch, which then read them as held. Short: the
  interning map was pruned with `HashMap::retain`, which leaves the table its
  size while the capacity it reports falls, so `map_heap` read a smaller table
  (`log-2.txt`, the over fixed and the short left). The map is rebuilt at its
  kept size now, its table charged ahead and the old one released with the
  texts in one update (`Charge::ahead_freeing`), and
  `a_map_made_to_hold_its_entries_allocates_the_table_it_is_charged` holds the
  arithmetic and that pruning in place still shrinks the reported capacity.
- **The minimum cannot be read at `2^20` once the cap has merged**: the finer
  rows per group are summed mid-scan, and keeping them grows with the block.
  It reads the size the cap left, which is why the predicate has to be monotone
  in size — this slice's nearest-rank median was not, and 20.4.1 is that.

## What the next slices inherit

- **`GroupSizing::Density` is where 20.5's maximum goes**, beside `min_rows`,
  and `StatisticsBackfill` carries the bound applied apart from the record, as
  it does the cap. `--statistics-min-rows` is refused beside
  `--statistics-group-size` by clap's `conflicts_with_all`, and beside `none`
  by `statistics_request`; a maximum joins both.
- **A stated minimum re-reads a block from its first row**, even where merging
  what the block holds would reach it; a stated size compares sizes alone, a
  merge being exact; a re-read at the size a block holds keeps its record.
- **A block no size reaches is one group**, as `choose` answers — the fixtures'
  `long_value` block by default, and any block holding fewer rows than the
  minimum. No floor sits under the coarsening; the maintainer weighed one and
  refused it (`../status/history/2026-09-15.md`).
- **A density reading over a cache this build writes flagless reads coarsened
  groups.** `row_density.py` wants the base distribution, so its gathering
  `parse` states `--statistics-min-rows 0`, as its docstring says; 20.2's koji
  cache is unreadable at every version since, its `koji-info.json` still being
  what a re-derivation reads.
- **`statistics-gathering` no longer prices the flagless default.** Its legs
  state `--statistics-group-size 1048576`, gathered exactly, while a flagless
  `parse` coarsens the generator's rows of about 4 KiB a row; a stated size
  cannot be combined with a minimum, so what 20.9's re-take states is open.
- **On koji the minimum should size `public.archiveinfo` alone** (20.2's notes),
  which is 20.10's check on it.
- **What the rule costs a block's end is unpriced**: a merge per doubling, each
  walking every column's groups and renumbering its dictionary, and the rows
  per group sorted once per size, a scratch the account does not see and the cap
  bounds.
