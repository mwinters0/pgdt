# P10.9 — Early stop on a column sorted over its block: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Pruning", its "Early stop" paragraph; what a row not read raises is
`decisions.md`, "D54". No figure was taken.

## What exists

- **`prune.rs`'s `SortedStop`**, settled by `prune_block` beside the kept
  runs: the terms `ResolvedExpr::required_ordering_terms` returns — the root
  term, or a member of a root conjunction with nested `And`s flattened, under
  an ordering operator whose column's bounds the term believes — kept where
  the block's believed `ColumnBounds::sortedness` closes that term's bound:
  `Ascending` under `<`/`<=`, `Descending` under `>`/`>=`. It is believed
  under the declared type and collation the bounds are.
- **`ReplayPlan::stops`**, keyed by `header_offset` like `kept`, filled only
  under `use_statistics` and for a block with a `PlannedBlock` whose
  statistics fit. `prune_blocks` returns a `Pruned` in place of its pair.
- **The replay checks a row only when the filter rejects it**
  (`SortedStop::passed`, `ResolvedTerm::is_false`): a kept row made every
  required term `True`. A `False` term ends the segment exactly as reading past
  its limit does — the post-segment flush, and a token whose `in_copy` is
  `None` — so a resumed stream re-enters at the stopping row and stops again.
  `is_false` raises nothing: a NULL, a missing field and a value that does not
  decode all answer "not past".
- **No note counts a stop**: it is found as rows are read, after the plan's
  notes are settled. The manual, `QueryOptions::use_statistics` and `pgdq query
  --statistics`'s help say so.

## Tests

- `tests/pruning.rs`'s
  `a_sorted_block_is_read_no_further_than_its_first_row_past_the_bound`, at the
  shipped group size, where `ordered` is one group and pruning skips nothing:
  `id <`, `stepped <=`, `reversed >`, `gappy <` with NULLs between, and a term
  inside a nested conjunction stop — serially no read starts past the stopping
  row, split each sub-stream reads a few chunks past it — while `id >`,
  `reversed <`, `unsorted <`, an `Or` and a `Not` read to the block's end, as
  does every case under `use_statistics: false`.
- `a_stopped_stream_resumes_to_the_same_rows`, a pause on every batch
  boundary under `id <= 19`, the last kept row included.
- The generated `every_fixture_prunes_to_the_rows_it_returns_unpruned`
  compares every stop against the statistics-free reference; its random roots
  are a bare term or an `And` often enough to reach one.

## For the slices after

- **10.10.** `statistics-pruning`'s range filter on a sorted id column is
  stopped as well as pruned, and the note's skipped bytes count only the
  groups skipped: what the stop saved inside the straddling group of each block
  is in no count the process reports.

## Negative results

- **Mutations.** The direction check widened to `>`/`>=` on an ascending
  column failed the hand test and the generated check; a NULL stopping failed
  both; `required_ordering_terms` descending into `Or` failed the hand test.
  Stopping on "not `True`" rather than `False` passed the generated check,
  being equivalent: it differs only on a value that does not decode, which no
  sorted block holds. Skipping a stopped block's later segments in one
  sub-stream passed too, and was not kept: pruning already skips a later run,
  and it would save a later piece of the same run only the read before its
  first row, which no test sees.
- **A `Not` over the opposite operator is not a stop** (`NOT id >= 20`),
  though it would be sound: the spec names a conjunction holding an ordering
  term.
- **`=` and `IS NOT DISTINCT FROM` do not stop**, though on a sorted column
  the first value past the literal is past every equal one: the spec names the
  ordering operators, and a `Canonical` equality carries no order to place a
  value past the literal.
- **10.8's recording source never split**: `Recording` did not forward
  `ByteRangeSource::partitions`, so `a_pruned_query_reads_nothing_deep_inside_a_skipped_run`'s
  split leg ran as one sub-stream. It forwards it now, and that test passes
  split.
