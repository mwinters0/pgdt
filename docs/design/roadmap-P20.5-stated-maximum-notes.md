# P20.5 — The stated maximum: notes

The bound is `StatisticsRequest::max_rows`, read at
`statistics::max_rows_group` and applied by `gather::density_merges` beside the
minimum; the re-read is `StatisticsRequest::backfill` over
`BlockStatistics::breaks_max_rows` and `::predicted_group_size`, and the record
is `GroupSizing::Density`'s new `max_rows`. `FORMAT_VERSION` is 21. Why the
maximum is shaped this way is
[`roadmap-P20-statistics-memory.md`](roadmap-P20-statistics-memory.md),
"Granularity follows row density"; what monotonicity buys is the rustdoc on
`density_merges`, and why the second read is the last one the rustdoc on
`backfill`. The user-facing statement is
[`../manual/dump-inspection.md`](../manual/dump-inspection.md),
"`--statistics`: what `parse` records for later queries", and the flag's help.

**The register carries the maximum.** `D82` is retitled to the merge between a
minimum and a maximum and says a stated maximum stops the merge first and lifts
the cap; `D34` says a back-fill re-reads once for a maximum it breaks. Both were
amended in place rather than joined by a `D83`, and the rustdoc keeps the two
monotonicity proofs, mechanism not being an entry's business.

## The checks

- `gather.rs`'s `a_stated_maximum_stops_the_merging_whatever_the_minimum_asks`
  holds the rule to the ladder `chosen_merges_between` walks, over random
  distributions, and asserts the maximum's own predicate is monotone in size —
  once the 90th-percentile group passes the maximum, no coarser size brings it
  back, which is what makes "stop at the first merge that breaks it" the same
  as "take the coarsest size that holds it".
  `a_block_sized_between_its_bounds_gathers_what_the_size_it_reaches_gathers`
  gathers random blocks serially and in pieces under both bounds and holds each
  to an exact gather at the size it reaches.
- `tests/statistics.rs`'s
  `a_block_breaking_a_stated_maximum_is_reread_once_at_the_size_it_predicts`
  walks the whole path over a generated dump of two blocks: flagless, then
  under a maximum both break, then again; `dense` meets it after the re-read
  and `clustered`, whose rows all start in one stretch, does not and is left
  alone; a tighter maximum re-reads both again and an unstated one re-reads
  nothing; a four-worker scan gathers what the serial one does.
  `a_stated_maximum_outranks_the_cap_and_the_minimum` is the block that meets
  neither bound.
- **Neither unit test is vacuous**: with the maximum's `break` removed, and
  with the maximum read at the median instead of the 90th percentile, both
  fail.
- CLI: `a_stated_maximum_rereads_the_dense_table_and_says_where_it_still_misses`
  is the whole path through the binary — the back-fill's counts, the shortfall
  line, the recorded bounds in `info --json`, and a second run that re-reads
  nothing and says it again.
  `contradictory_or_empty_statistics_flags_are_refused` gains the maximum
  beside `none`, beside a stated group size, beside `--preamble-only`, below a
  stated minimum and below the default one.
- The `introspect` reconciliation passes over the changed replacement charge,
  `runs/statistics-account-20260916-maximum.log`; its back-fill legs are
  cache-loaded blocks, so they hold the `Term::Loaded` branch.

## Negative results

- **A freshly mapped block can now lack something, which the account's
  replacement charge had assumed away.** `backfill_statistics` credited every
  replaced block's heap to `Term::Loaded`, which was right while only a
  cache-loaded block could lack statistics; a block this run gathered is in
  `Term::Retained`, and so is one this loop has already re-read, so the
  subtraction wrapped and the account's total became garbage — the first run of
  the new tests panicked inside `StatisticsAccount::update`. `map_file` now
  collects the header offsets the *cache* supplied statistics for and the loop
  releases each replacement from the term that carried it.
- **The cap cannot yield to the maximum block by block.** Deciding mid-scan
  whether a merge would break the maximum reads a partial distribution, which
  depends on where the leader split the block, so serial and parallel would
  disagree. A stated maximum turns the cap off for every block instead, which
  is also what "as a stated group size is" asks for.
- **The record cannot say whether the maximum was met.** A block records the
  maximum it was sized under whether or not its groups reach it, so what stops
  a third read is the *size*: a re-read leaves the block finer than the size a
  gather starts from, and only a block still at that size is re-read for a
  maximum.

## What the next slices inherit

- **A stated maximum is the only thing that lifts `STATISTICS_GROUP_CAP`**, so
  it is the one request under which statistics grow with a block's bytes again.
  20.7's decline is what keeps that from costing the process; until it lands,
  a maximum on a long dump is bounded by nothing.
- **A block is read at most twice in a run**, the bound written as the `0..2`
  loop in `backfill_statistics`; `MapRun::backfilled` counts blocks, not reads.
- **The shortfall is reported by every run under the maximum**, not only by the
  one that re-read: `stream::report_density_shortfall` walks the finished index
  once, so a block nothing will read again still says what it holds.
- **`row_density.py` mirrors the minimum alone.** Its `choose` walks the
  ladder with no maximum, which is what the shipped default does, so 20.2's
  koji reading is untouched by this slice.
- **What the re-read costs is unpriced**: a second pass over a block's bytes
  and a gather at a finer size than any figure takes. Nothing in
  `measurements.md` states a maximum.
