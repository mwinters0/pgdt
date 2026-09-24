# P25.4 — The declared ordering: notes

The slice built the spec's "A declared output ordering from recorded
sortedness" ([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)).
The proof is `summary::partition_orders`, settled in `TablePartitions::plan`
and read through `TablePartitions::orders`. The provider turns it into
DataFusion orderings in `datafusion-pgdump/src/statistics.rs`,
`output_orderings`. The one refusal the code cannot explain, not cutting
partitions at block boundaries, is `decisions.md`, "D51".

## What the next slices inherit

- **The proof runs once per plan, not once per table.** It depends on where
  the partitions were cut and on which blocks pruning dropped whole, so it
  can't be cached next to the table's statistics. It is cheap. The NULL check
  reads every group's count, but the boundary proof parses only two bounds
  per boundary: the last group holding a row in the earlier block and the
  first in the later one.
- **The order is always Arrow's**, whatever semantics the query compares in.
  The stored set is found the same way `table_summary` finds its bounds:
  `bounds_read_by(Arrow)`, keyed against the kinds the block was gathered
  under, and believed only under the DDL it was gathered under. A `:strings`
  registration therefore declares an order only for a column gathered with a
  bytewise text set.
- **An enum column is declared**, in label-text order: that is its Arrow
  set, and it is what Arrow's comparator orders a `Dictionary` by. `KD45`'s
  type problem is about a `MIN`/`MAX` literal and doesn't apply here. No
  fixture held a sorted enum, so the statistics schema gained
  `public.moods`: `m` ascends by label text and descends in declared order.
  `statistics_fixture.rs` asserts that on the bytes at every major, and the
  ordering target asserts that `m` is declared ascending. The `default` flag
  set now has nine blocks.
- **25.5 admits floats by deleting one `matches!`** in `column_order`, once
  the sign is gathered. Under `KD42`, `public.zeros` breaks the harness: the
  mutation run with floats allowed failed on `min_pos_first`.
- **What the harness now pins.** In
  `a_declared_ordering_holds_in_every_partition_and_drops_the_sort`,
  `crossing_proved` asserts over `public.spans` read as three blocks, at
  every partition count where a partition crosses a boundary. `part`, `id`,
  `reversed`, `label`, `i4` and `ident` are declared in their directions.
  `local`, which restarts in each block, and `gappy`, which holds a NULL, are
  not. The `ORDER BY` check now runs all four direction and NULL-placement
  combinations. Three mutation runs each failed the target: no boundary proof,
  no NULL check, floats allowed.

## Negative results

- **Only one NULL placement can be declared.** DataFusion 55 keeps one
  `SortProperties` per expression (`get_expr_properties`) and matches a
  nullable column's placement exactly (`options_compatible`). Declaring both
  placements of a column with no NULLs left `ORDER BY c ASC NULLS FIRST`
  sorting anyway. So each column declares SQL's default: NULLs last when
  ascending, first when descending. The opposite placement still plans a
  sort.
- **A block holding a single value is recorded `Ascending`**, so a
  descending table with such a block declares nothing. A block holding no row
  orders nothing and is skipped. A constant block isn't treated as neutral,
  because no fixture has one inside a descending table to test that arm.
- **A filtered scan isn't checked for its ordering.** The proof is the same
  function whichever blocks pruning drops, and the harness reads the ordering
  only over unfiltered scans.
- **`EXPLAIN` doesn't show the ordering.** `PgDumpExec`'s display prints the
  partitions and the fetch only. What shows it is the plan dropping the sort.
