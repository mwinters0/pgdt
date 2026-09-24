# P25.2 — The filtered row bound: notes

The slice built the spec's "A pushed filter's rows bounded by what pruning
kept" ([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)). The rule
is in `decisions.md`, "D89". The bound is `TablePartitions::kept_rows`. The
provider sets it in `datafusion-pgdump/src/table.rs`, `scan`.

## What the next slices inherit

- **The bound is summed where pruning is settled.** `prune_blocks` in
  `stream.rs` adds each consulted block's `BlockPruning::kept_rows` and each
  other block's `row_count`. A block counts as not consulted when it has no
  usable statistics, when the query turned statistics off, or when the filter
  reads no field. So with no filter the bound is the table's rows. The
  provider still hands those over `Exact` from `table_statistics`, and uses
  `kept_rows` only under a pushed filter.
- **25.6's byte size under a filter should be summed the same way.** Add a
  kept-bytes field to `BlockPruning` next to `kept_rows`, summed over the same
  kept groups, and have unconsulted blocks contribute their whole size. The
  spec bounds a filtered byte size "by the kept groups as the rows are".
- **Only `TablePartitions` exposes the bound.** `TableStream` and `pgdt
  query` don't, because they plan through no optimizer.
- **Column statistics under a filter are still the whole table's,
  `Inexact`.** So a column's NULL count can be larger than the row bound.
  Nothing clamps it, because the spec keeps column statistics as they were.
  25.3's `distinct_count` will become `Inexact` under a filter through the
  same `to_inexact`, which is what `nothing_exact_per_column` requires.
- **A fetch combines with the bound.** `exec.rs`'s `fetched` lowers any
  `num_rows` above the fetch to the fetch, so a filtered and fetched scan
  states the smaller of the two.
- **The estimate target now counts `tightened`**: filtered scans whose bound
  is below the table's rows. It asserts that each of them pruned a group, and
  that they make up more than nine tenths of the pruning scans. The rest
  pruned only groups that no row starts in (`RowGroup::rows` of zero).
- **A plan-shape target, `a_filtered_side_s_bound_moves_a_join_s_build_side`.**
  It joins `public.ordered` to itself, filtering one side on `reversed`, which
  is not the join key, so only that side is filtered. With the new bound the
  filtered side is swapped in to build. Stated as the whole table's rows the
  two sides tie, and the left side builds. That was checked by reverting the
  provider line: the test then fails with `Some((false, CollectLeft))`. The
  library target `a_plan_bounds_its_rows_by_the_groups_its_pruning_kept` in
  `pgdump_query/tests/pruning.rs` pins the sum exactly on `public.ordered`.

## Negative results

- **A sorted block's early stop is not in the bound.** Where the block
  stops is only found while its rows are read, so a bound set at plan time
  can't include it. The spec's grilling facts already say this.
- **Per-partition statistics are still unknown.** A kept group's rows could
  be attributed to the partition whose segments hold it. Nothing asked for
  that, so `PgDumpExec` still answers `Statistics::new_unknown` for a
  partition.
