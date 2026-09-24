# P25.3 — The distinct count from the dictionaries: notes

The slice built the spec's "A distinct count derived from the dictionaries"
([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)). The rule is in
`decisions.md`, "D89". The count is `ColumnSummary::distinct` in
`pgdump_query/src/summary.rs`, and the provider hands it over in
`datafusion-pgdump/src/statistics.rs`. The `timestamptz` fact it rests on is
`postgres-invariants.md`, "I48".

## What the next slices inherit

- **The rule for which columns qualify comes from how the block was
  gathered, not from how the query resolves the column.**
  `gather::dictionary_holds_field_text` reads the typed resolution
  (`gather::stored_resolution`, which the observer is now built from as well).
  A column qualifies where gathering keeps a dictionary and the kind is not
  `PaddedText`. So a `:strings` registration counts the same columns a typed
  one does, and a `character` column is excluded in both modes. Its entries
  drop the trailing blanks that the column still emits.
- **The union is taken over each group's own indices, not over the block's
  `entries` list.** The list can keep an entry that no group names any more.
  The set borrows its `&str`s from the map. It lives while one table's
  statistics are computed, and it is billed to nothing. It holds at most one
  slot per dictionary entry the map already keeps.
- **A dictionary is believed only under the declared type and collation it
  was gathered under**, the same check the bounds get (D78).
- **Under a filter or a fetch** the count becomes `Inexact` through
  `to_inexact`. `nothing_exact_per_column` already asserts that.
- **The harness now asks `SELECT COUNT(DISTINCT c), COUNT(*)`** and records
  `Seen::distinct` in both schema modes. A new target,
  `a_distinct_count_is_exact_only_where_every_group_kept_a_dictionary_of_emitted_text`,
  pins which columns of `public.spans` are `Exact` and which are `Absent`.
  It also checks that an enum and an `interval` column answer.
  A mutation run showed both targets catch it when `character` is allowed
  in: `bare`, whose `'a'` and `'a '` share an entry, answers 2 where the rows
  say 3.

## Negative results

- **A lone `COUNT(DISTINCT c)` is never answered from statistics in
  DataFusion 55.** The logical rule `single_distinct_aggregation_to_group_by`
  rewrites it into a count over `GROUP BY c`. That grouping aggregate states
  its rows `Inexact` whatever the NDV, so `aggregate_statistics` never sees a
  distinct aggregate. It sees one only where the rewrite does not fire:
  beside another aggregate that is not `SUM`, `MIN` or `MAX` of the same
  argument, such as `COUNT(*)` or a second column's `COUNT(DISTINCT)`. The
  count does still reach DataFusion's estimates in every shape: a `GROUP BY`'s
  output rows (`compute_group_ndv`) and a join's cardinality.
- **An enum's `COUNT(DISTINCT)` answers, unlike its `MIN`.** The count is an
  `Int64` whatever the input type is, so `KD45`'s type problem does not
  arise. DataFusion also counts labels, which is what the dictionary holds.
  The same goes for `interval`'s `MonthDayNano`.
