# P6.2.1 — Arrow semantics as DataFusion compares: notes

An earned follow-up to 6.2 in
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), which shipped Arrow
semantics measured against the bare `cmp` kernels. The measure is
DataFusion's `apply_cmp`: the spec's "Comparison means what DataFusion means".

## What later slices inherit

- **A float compares in Arrow semantics exactly as in PostgreSQL's.**
  `CompareKind::arrow_order` leaves `Float32` and `Float64` alone; the
  `Float32Total`/`Float64Total` kinds are gone. `IntervalFields` is now the
  only kind `arrow_order` alone produces. So float bounds and dictionaries are
  read in Arrow semantics too: `bounds_ordered_in(Arrow)` is true for them.
- **The library's walks emulate `apply_cmp`, and do not call it.** The oracle
  module's `arrow_against` turns a float `-0` into `0` before the kernels run,
  as `normalize_float_zero` does. `ARROW_AGREEMENT` now records both floats as
  agreeing, so 6.1's evidence walk and
  `arrow_semantics_answers_as_datafusion_does` (`predicate.rs`) are both taken
  under that measure. The check against DataFusion itself is still 6.6's.
- **A column with no plan is ordered bytewise in Arrow semantics.** Every
  ordering operator on such a column resolves to `CompareKind::Text` and reads
  no statistics, where PostgreSQL's semantics refuses it with
  `Error::UnorderedPredicateColumn`. That covers a type this build does not
  map and a column no DDL declared, which is every column under
  `--schema-mode strings` (`arrow_semantics_orders_a_column_with_no_plan_bytewise`).
  For 6.6, such a term is pushable.

## What 6.7 inherits

- **DataFusion's `MIN`/`MAX` over a float column is not `apply_cmp`'s order.**
  Its aggregate compares floats with `total_cmp`
  (`datafusion/functions-aggregate-common/src/min_max.rs`, 55.1.0), so it puts
  `-0` below `0`. A stored float bound is gathered under
  PostgreSQL's order, where the two tie. So a group holding both zeros can
  store `0` as its minimum while DataFusion's `MIN` answers `-0`. The bound is
  correct for pruning, which compares under `apply_cmp`. Handing it to
  DataFusion as an `Exact` column statistic is not correct.

## Negative results

- **No cache format change.** No kind that gathering uses orders or equates
  differently, so `CACHE_FORMAT_VERSION` and the golden order are unchanged.
