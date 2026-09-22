# P6.10 — Bounds in every exact semantics: notes

A slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), admitted in
[`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "Which
columns carry bounds in Arrow's order". A scalar column now keeps bounds and
row order in each order some semantics compares it by exactly, one set where
two coincide. `ColumnStatistics::arrow_bounds` holds the second set, and
`CACHE_FORMAT_VERSION` is 24. The kinds are `ComparisonPlan::bounds_kinds`
(`pgtype.rs`), restricted to declared scalars by
`ResolvedSchema::bounds_kinds` (`resolve.rs`). A term reads the set
`ResolvedSchema::bounds_in` names. The why is
[`decisions.md`](decisions.md), "D79".

## What 6.7 and 6.8 inherit

- **Which set holds each column's Arrow bounds.** `bounds_in(i, Arrow)` is
  `Primary` or `Arrow`, and `ColumnStatistics::bounds_in` reads it. It is
  `None` only for a nested column and one no DDL declared. So 6.7's `Exact`
  predicate for a column's bounds is `bounds_in(i, Arrow).is_some()` with
  every group carrying that set. `bounds_ordered_in(Arrow)` still answers
  the same question for a plan.
- **What each set is keyed by.** The primary set is the register's kind where
  its order is exact (`divergence: None`), and `arrow_order()` of it
  otherwise. The Arrow set is `arrow_order()` of an exact kind that differs
  from it: `Text` for an enum, a bare `numeric`, `timetz`, `inet`/`cidr` and
  `character(n)` under `C`, and `IntervalFields` for `interval`. `macaddr`
  keeps one set, read under `Text` in Arrow semantics. A planless column's
  one set is `Text`.
- **The oracle checks the Arrow set against DataFusion's extremes.**
  `arrow_semantics_answers_as_datafusion_does` (`predicate.rs`, `oracle`
  module) gathers each declared type's Arrow set over every value and over
  runs of three, on six majors, and checks it against Arrow's minimum and
  maximum. It also asserts both ways that an undivergent column reads its
  one set in Arrow semantics exactly where `ARROW_AGREEMENT` records its
  bounds agreeing, which is what keeps `macaddr`'s set shared.
- **Pruning reads the set its semantics names.**
  `an_enum_is_pruned_by_the_set_of_bounds_in_the_order_asked_for`
  (`tests/pruning.rs`) pins it on `t_enum_domain.v_mood`, whose one group is
  bounded `sad`–`has'quote` by declaration and `has space`–`sad` by label.
  A build reading the primary set in Arrow semantics fails it. The early stop
  (`prune::SortedStop`) reads the same set's sortedness.
- **`--json` exports `arrow_bounds` beside `bounds`.** `info --detail` sums
  the primary set alone and says so in the manual.

## Negative results

- **A column no DDL declared is not bounded**, though Arrow semantics orders
  it bytewise. Under `SchemaMode::Strings` every column is `NotDeclared` and
  `Refused`, including one gathering resolved typed and bounded by its own
  kind. So believing a planless column's set there would read an `integer`'s
  bounds as text. `arrow_semantics_orders_a_column_with_no_plan_bytewise`
  (`predicate.rs`) pins both roads. A data-only dump's columns go unbounded
  with them.
- **The back-fill of a column held without bounds is unreachable across the
  bump.** `StatisticsRequest::backfill`'s `missing` test answered 6.9's
  unbumped change. Every cache this build reads now holds the sets this build
  keeps. It stays as a guard; nothing exercises it outside its own test.
- **The golden order digests the stored kinds too.**
  `golden_order_is_pinned_to_the_format_version` now sorts each oracle type
  under its register kind and under each kind a set is stored by, so a moved
  `IntervalFields` or `Text` order fails it.
- **No figure moves by design.** `statistics-gathering`'s control has no
  column that gains a second set: its `numeric(20,6)` is `Decimal`. Every
  retained column is larger by one `Option<ColumnBounds>`, which the account
  charges. The re-take stays P23's.
