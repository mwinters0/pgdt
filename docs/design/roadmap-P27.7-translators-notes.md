# P27.7 — The translators emit the membership: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 7. **Both DataFusion translators now hand an `IN` over as one
`Expr::In`; the readings are taken from the commit landing it and not yet
folded in**, and row evaluation's default is not this slice's but item 9's.

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

**Taken at `11e13f2`**, `runs/measure-20260927T221125/` (`tables.md`,
`raw.json`, `log.txt`), with `EXPLAIN ANALYZE` of each on leg in
`runs/dynfilter-27.7-explain-20260927/explain.txt`. **Folding them in
finishes the slice**: the prose under the two tables in `measurements.md`,
"What DataFusion's dynamic filters buy a query", describes the `28e804f`
sitting's `Or` of `=` and is rewritten with them. It sets the costing row's
cost a row against 27.5's cost a term, fitted with no per-row intercept and
including each term's own unescape, and says in those words what that leaves
unattributed, which 27.8 attributes. It states no default: the criterion is
applied at 27.9, after the mechanism 27.8 names.
