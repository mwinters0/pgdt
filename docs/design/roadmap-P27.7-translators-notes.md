# P27.7 — The translators emit the membership: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 7. **Both DataFusion translators now hand an `IN` over as one
`Expr::In`; the readings that decide row evaluation's default are not taken**,
a figure being taken from a commit and this tree carrying the change it
prices ([`measurements.md`](measurements.md), "A figure may be published
outside the sweep").

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

**Owed, and they finish the slice**, from the commit landing this tree:

```sh
cd scripts && uv run measure.py --figure dynamic-filter-join,dynamic-filter-topk
```

A sitting of both took about five minutes at 27.5. The criterion is the
spec's, set before them: row evaluation stays on only if the unclustered join
wins with its legs' spreads apart and the costing input's Δ lies inside its
legs' overlapping spreads; failing that it goes off and "D93" reopens. The
prose under the two tables in `measurements.md`, "What DataFusion's dynamic
filters buy a query", describes the `28e804f` sitting's `Or` of `=` and is
rewritten with them, `EXPLAIN ANALYZE` of each on leg attributing the rows
again.
