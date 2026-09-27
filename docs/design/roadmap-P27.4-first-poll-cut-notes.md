# P27.4 — The byte cut at the first poll: notes

What the slices after this one inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Scope".
Planning is unchanged: the count, the statistics DataFusion reads, the static
pruning and the plan notes are settled at `scan()`.

## What exists

- **A dynamic filter enters by `TablePartitions::under`**, which hands back a
  `DynamicPartitions` whose `stream(partition, max_rows)` makes each
  sub-stream; `TablePartitions::stream` takes no filter any more. The spec's
  reason for the one entry point, that it never resumes, holds unchanged: the
  handle exists because it holds the cut, which every sub-stream of one run
  must share. `PgDumpExec` makes one per streaming exec it builds
  (`Replay::streaming`), so a reset or a new filter set gets a new cut.
- **The cut** (`TablePartitions::cut_under`) is made by whichever sub-stream is
  polled first: the state is read once (`current()`), each block with
  statistics is asked group by group over the runs the static filter kept
  (`DynamicPruning::kept_within`), and the runs left are cut by `cut_blocks`,
  the planned cut's own function, into the plan's count by the advice the plan
  was made with. A sub-stream the new cut leaves nothing to is empty.
- **The planned cut stands** where the state rules out no group, which is a
  TopK's and an aggregate's at the first poll, and where the new cut would put
  two blocks in one sub-stream that nothing proves in an order the plan
  declared (`keeps_orders`, `KD53`). A table of one block never meets the
  second.
- **A group is asked once per state.** The cut keeps each block's verdicts
  beside the generation it read, and a sub-stream reading that generation
  takes them (`DynamicPruning::seed`) rather than asking the groups again.
- **Counting:** the groups the cut rules out are counted by the first
  sub-stream to take it; groups a later state rules out, as 27.3 counts them.
  The scan's metric sums both, each group once.
- `prune_block`'s run arithmetic is three helpers (`group_run`, `push_run`,
  `or_header_run`) that the cut shares.

## Negative results

- **The cut held by `TablePartitions`** is refused: every streaming exec a
  `PgDumpExec` builds over one planned replay would share it, so it would
  outlive the filter it was made under, and a recursive query's next
  iteration needs rows the last one's filter ruled out.
- **A cut keyed by the filter's `Arc`** is refused: identity by pointer, where
  a handle says which sub-streams belong to one run.
- **A count re-planned at the first poll** is impossible: DataFusion reads a
  leaf's partition count off its `PlanProperties` when it plans.
- **Asking the groups again in the replay** is refused: on the costing input
  of `dynamic-filter-join` every group meets a 150-term `Or` at the cut, and
  would meet it a second time as the replay entered it.

## Tests

- `pgdump_query/tests/dynamic_filter.rs`, over every major's `ordered`:
  `a_selective_state_on_a_clustered_key_is_cut_balanced_over_the_groups_it_keeps`
  — the membership of a build side holding ids 400 to 499, at three
  sub-streams, each within two rows' bytes of a third of what the kept groups
  hold, where the planned cut gives one of them everything; the sorted-stop
  test counts the groups past the bound as the cut's. On a synthetic table
  of two blocks out of order, `a_cut_that_would_lose_a_declared_order_is_not_made`;
  and with a filter breaking its contract so the verdicts' source shows,
  `a_replay_takes_the_cut_s_verdicts_under_the_generation_it_read`.
- `datafusion-pgdump/tests/dynamic_filters.rs`:
  `a_selective_join_s_probe_side_is_cut_over_what_its_filter_keeps` — a real
  `CollectLeft` join at three partitions, each of whose outputs joins rows,
  where with the producers' flags off one joins them all. It is what shows the
  spec's fact that a join's filter is complete at the probe's first poll.
- Mutations, each failing one of them: the cut never made (the balance test,
  the sorted-stop count, the provider join), the order check skipped, the
  verdicts not taken.

## For 27.5

- **The costing join's on leg asks each group of its probe once**, as at
  27.3, but all of them at the first poll rather than as each is entered; the
  one state a join publishes never moves after, so the replay asks none.
- **Both figures run one sub-stream**, which the cut still rewrites: the
  clustered join's replay is handed the kept runs from the start rather than
  entering the block at its header and skipping.
