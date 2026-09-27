# P27.5 — Row-level evaluation: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Scope"
and "Evidence". **The mechanism has landed, on, and the readings are taken:
row evaluation acting alone loses on both inputs that isolate it**, which
reopens "D53" for a set-membership term; 27.6 lands it, 27.7 re-takes the
figures, and 27.9 decides the default by the spec's criterion.

## What exists

- **A row the static filter keeps is evaluated against the dynamic filter's
  state before a column of it decodes**, in every block, statistics or not,
  and dropped where the state rejects it (`stream.rs`, `DynamicRead::rejects`).
  The state lives on the block reader (`DynamicBlock`), resolved against the
  block, with the statistics' pruning beside it only where they answer.
- **The state is read at each chunk the replay takes and at each group it
  enters**, wherever the generation has moved (`DynamicRead::at_chunk`,
  `::at_row`), and a read inside a group judges that group again, skipping
  the rest of it with each later group it rules out (`decisions.md`, "D93").
  The chunk check comes before the chunk is read, so a skip saves that read.
  A group ruled out after part of it was read is not counted in
  `row_groups_pruned_dynamic_filter`, its read rows having been evaluated.
- **A field the state reads that does not decode keeps its row**, and the
  error is raised where the static filter or the row's own decoding reaches
  it (`decisions.md`, "D54").
- **The stop is asked of a kept row only where the state rejected it**, a
  row the state keeps making every term of its stop `True`; of a row the
  static filter rejects, as before, only in an armed group.
- **Counted per sub-stream**: `TableStream::dynamic_filter_pruned_rows`, and
  under `EXPLAIN ANALYZE` the scan's `rows_pruned_dynamic_filter`, named
  beside `row_groups_pruned_dynamic_filter` rather than as Parquet's
  `pushdown_rows_pruned`, which counts its static filter's rows too. The
  manual's "What a scan skipped" paragraph names it.

## Negative results

- **Evaluating only in a block whose statistics answer**, and each cadence
  but the chunk's, are refused ("D93"), settled with the maintainer
  ([`../status/history/2026-09-27.md`](../status/history/2026-09-27.md),
  "27.5 evaluates a dynamic filter's rows in every block").
- **A decode failure as a rejection** is refused: it would hide a refusal
  behind whether the state had narrowed, the timing `KD8`'s refusal already
  depends on elsewhere (the P28 inbox).
- **Evaluating the state on a row the static filter rejects** is pointless:
  the row is dropped either way, and only the stop needs asking there.

## Tests

- `pgdump_query/tests/dynamic_filter.rs`: `id >= 990` now emits exactly
  990 to 1000, the kept groups' rows below it counted as dropped, serial and
  split alike; with statistics off no group is skipped and all 989 rows are
  dropped, and a state keeping everything drops nothing. Over one group
  read in small chunks, a state moving inside it is read at the next chunk,
  statistics or not
  (`a_state_moving_inside_a_group_is_read_at_the_next_chunk`), and one
  ruling the group out skips its rest, uncounted, where without statistics
  it drops each row after
  (`a_state_read_inside_a_group_it_rules_out_skips_the_rest`);
  `a_row_the_state_rejects_is_dropped_before_it_decodes`, over
  one group whose first fifty rows' `v` no decoder reads — `id > 50` emits
  the rest where the unfiltered replay refuses, and `v > 100` refuses as it
  does. The balance test's filter keeps every row once the cut is made
  (`CutOnly`), so what a sub-stream emits is still what it was handed.
- `datafusion-pgdump/src/dynamic_filter/tests.rs`, the generated check: its
  dynamic leg now meets row-level evaluation, so "loses no row DataFusion
  keeps" is checked per row, and it fails unless a tenth of its replays drop
  one.
- `datafusion-pgdump/tests/dynamic_filters.rs`: the sweep fails unless more
  than twelve queries drop a row, and flags off drop none; the join test finds
  the metric under `EXPLAIN ANALYZE`.
- Mutations, each failing a test above: a decode failure taken as a
  rejection; no row ever rejected; no read at a chunk; a chunk's read never
  judging its group; rows evaluated only where statistics answer; a group
  ruled out mid-read counted as pruned.

## The readings

`dynamic-filter-join` and `dynamic-filter-topk`, taken at `28e804f`, each
alone (neither stands in a sharing edge); the numbers, the metrics that
attribute each row and the per-term account are
[`measurements.md`](measurements.md), "What DataFusion's dynamic filters buy a
query".

- **The two wins are group pruning, not row evaluation.** `EXPLAIN ANALYZE`
  of each on leg over the figure's input names the mechanism: the clustered
  join and the TopK each prune nearly every group and evaluate few rows; the
  unclustered join and the costing input prune nothing, and there row
  evaluation acts alone.
- **Alone, it loses, and by more where it was meant to pay.** The added time
  is in proportion to the terms evaluated per row, the `IN` reaching the
  evaluator as an `Or` of `=`, each leaf unescaping and comparing its field
  again. How a term's cost divides between the unescape, the comparison and
  the tree walk is unattributed.
- **Filed against "D53"**: its **Reopens** is met, and answered by a
  set-membership term — not a switch, and not a stream ceasing to evaluate a
  filter that rejects nothing (`decisions.md`, "D93"). The mechanism ships on,
  the tree unchanged by the readings.
- **The off legs did not move** beyond their spreads since `2f94f14`, so no
  unrelated work is priced in the on legs' Δ.
