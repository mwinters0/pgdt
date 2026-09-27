# P27.5 — Row-level evaluation: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Scope"
and "Evidence". **The mechanism has landed, on; the readings that decide
whether it stays on have not been taken**, a figure being taken from a commit
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep").

## What exists

- **A row the static filter keeps is evaluated against the dynamic filter's
  state before a column of it decodes**, and dropped where the state rejects
  it (`stream.rs`, `DynamicRead::rejects`). The state is the one the replay
  last read at a group boundary (`prune::DynamicPruning::state`), resolved
  against the block, so no state is read per row and a TopK's is as fresh as
  its group.
- **Only in a block whose statistics answer**: the state is read as the
  replay enters a row group, which only statistics delimit, so a block
  without believed statistics, or a query not using them, drops no row. That
  keeps the trait's sentence that a replay drops nothing on a filter's
  account where the query does not use statistics.
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

- **Evaluating in a block without statistics** is not done: it needs a
  cadence for reading the state that no group boundary gives — per row, a
  read lock per held filter per row; per segment or per batch, a new rule a
  TopK's tightening would lean on. It is under STATUS's "Decisions worth
  another look".
- **A decode failure as a rejection** is refused: it would hide a refusal
  behind whether the state had narrowed, the timing `KD8`'s refusal already
  depends on elsewhere (the P28 inbox).
- **Evaluating the state on a row the static filter rejects** is pointless:
  the row is dropped either way, and only the stop needs asking there.

## Tests

- `pgdump_query/tests/dynamic_filter.rs`: `id >= 990` now emits exactly
  990 to 1000, the kept groups' rows below it counted as dropped, serial and
  split alike, and nothing dropped with statistics off or a state keeping
  everything; `a_row_the_state_rejects_is_dropped_before_it_decodes`, over
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
  rejection, and no row ever rejected.

## The readings

From this slice's commit, each alone (neither stands in a sharing edge):

```sh
cd scripts && uv run measure.py --figure dynamic-filter-join
cd scripts && uv run measure.py --figure dynamic-filter-topk
```

- **What each row can attribute.** 27.1's "before" is flat, every leg the
  scan. The re-take's on leg carries 27.3's pruning, 27.4's cut and this
  slice's row evaluation together, and the figure records no metric saying
  which acted. The clustered join is built for pruning; the other three rows
  for row evaluation alone, but whether a group's dictionary or a TopK's
  tightening rules any group out there is unread — `EXPLAIN ANALYZE` of the
  row's SQL over the figure's input answers it, and an attribution needs it.
  The off leg holds no filter and runs none of this, so it should not move;
  one that does prices unrelated work since `2f94f14`.
- **What they decide**: a loss on the costing input — every row passing a
  150-term `Or` — reopens the question as a switch against a set-membership
  term, and whichever way it reads is filed against "D53"'s **Reopens**,
  with the box ticked then.
