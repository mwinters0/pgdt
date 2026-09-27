# P27.7 — The translators emit the membership: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 7. **Both DataFusion translators now hand an `IN` over as one
`Expr::In`, and both figures are re-taken from the commit landing it**; row
evaluation's default is not this slice's but item 9's.

## What exists

- **The static translator** (`pushdown::translate`) turns an `IN` over
  literals into one membership, `NOT IN` into `Not` over it, and now pushes a
  list holding a `NULL` `Exact`, where it refused one before: each value goes
  through `pushdown::member`, a `NULL` becoming the membership's `None`. A
  float's list and the empty one are refused as before (`decisions.md`,
  "D88"); a list of three or fewer values without a `NULL` never reaches it,
  DataFusion's simplifier having made it `=` terms.
- **The dynamic translator** (`dynamic_filter::membership`) hands a join's
  list over whole, `NULL`s included, under either parity. A float's `NOT IN`
  still has no term; a list holding a non-literal still stands as whatever
  keeps every row.

## Negative results

- **A float's `NOT IN` holding a `NULL` now keeps every row** where 27.2
  answered it exactly (it can keep none): the float rule is asked first.
  No producer publishes an `IN` beneath a `NOT` — a join's list is built
  unnegated (DataFusion 55.1.0, `joins/hash_join/shared_bounds.rs`) — so
  nothing a query prunes is lost.

## Tests

- `datafusion-pgdump/src/dynamic_filter/tests.rs`,
  `an_in_list_reaches_the_library_as_one_membership`: both translators emit
  `Expr::In`, a `NULL` kept in its place, `NOT IN` as `Not` over it.
- `datafusion-pgdump/tests/pushdown.rs`: the generated check pushes each
  column's list both ways round, with and without a `NULL`, against
  DataFusion's own evaluation; three SQL cases are pushed with a `NULL` in the
  list, an enum's among them.
- Mutations, each failing a test above: a `NULL` refused by the static
  translator (the SQL cases), and dropped from the list by either translator
  (each generated check, on a `NOT IN`).

## The readings

**Taken at `11e13f2`**, each alone, raw sitting
`runs/measure-20260927T221125/`, with `EXPLAIN ANALYZE` of each on leg in
`runs/dynfilter-27.7-explain-20260927/`; the numbers and the account are
[`measurements.md`](measurements.md), "What DataFusion's dynamic filters buy a
query".

- **The metrics count what they counted at `28e804f`**: the membership term
  moved what a row costs, not which mechanism acts on which row.
- **The unclustered join now wins and the costing input still loses**, each
  with its legs' spreads apart, so the spec's criterion would fail on the
  costing row as it stands; it is not applied here (item 9).
- **27.5's cost a term leaves most of the costing row's cost a row
  unattributed.** Three leaves a row, each unescaping the field, fall well
  short of it; a fixed cost of evaluating a row at all and the lookup's own
  cost could each hold the rest, and neither reading separates them. That is 27.8's to attribute, the account first, then a `perf` profile
  of the costing on leg and a per-term `introspect` reading.
- **The TopK's off-leg median reads below every `28e804f` reading**, the
  spreads overlapping, and the `dd` floor fell too, with no change between
  the commits reaching a scan holding no filter; the difference is the
  sitting's and unattributed, and no Δ carries it.
