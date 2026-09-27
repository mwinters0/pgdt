# P27.6 — The set-membership term: notes

What 27.7 inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 6; why the term exists is `decisions.md`, "D53". **`Expr::In` has landed
in the library and in `pgdt`'s leaf grammar; neither DataFusion translator
emits it yet**, so no figure has moved and none was taken.

## What exists

- **`Expr::In(Membership { column, values })`**, `None` in `values` being
  SQL's `NULL` (`predicate.rs`). It resolves per block as one `=` term per
  non-NULL value, in list order, and keeps them: a row group's answer and the
  divergence notes are read off them exactly as off the `Or`
  (`ResolvedMembership::truths`), and only the row path answers from the
  lookup built beside them (`Lookup`) — a set of the rendered literals, of
  the trimmed ones for `character(n)`, or the decoded keys sorted by
  `compare_keys`/`compare_nested` and binary-searched, by the `Comparison`
  shape the column's `=` resolved to.
- **27.7 can hand DataFusion's list over whole**, `NULL`s included, where the
  dynamic translator drops them today (`dynamic_filter::membership`, which
  loosens to keep every row under an odd parity) and the static one refuses
  the list (`pushdown::literal_text`). A float's `IN` is still `D88`'s
  refusal: the lookup answers as the column's `=` does, never from the bits.
- **A refusal names the operator `IN`** — the `Error` variant, the value and
  the place are the first refusing `=`'s. That is how the spec's "the error
  raised wherever the `Or` would raise one" is read: the same error, worded
  for the term the user wrote.
- **`pgdt` reads `column IN (value, …)` under `--filter` as under
  `--where`**: the leaf grammar is the one both flags share
  (`decisions.md`, "D60"), and the tokenizer reads the list with the same
  scanner the term grammar does (`pgdt::in_list`), so a paren or keyword
  inside one is never structure. It has no NULL literal, as `=` has none, so
  `c in (a, b)` is `c=a or c=b` exactly.
- **A resume token stamps a membership as its own node**, so one taken under
  the `Or` does not resume under the `In` (`stream::hash_expr`).

## Negative results

- **A group's answer is no tighter than the `Or`'s**, though one row can
  equal at most one value: the terms are combined as independent, as every
  `Or` is. Tighter would break the per-group agreement the spec proves, and
  whether it would prune anything more is unmeasured.
- **Only the row path is constant in the list's length.** A group is still
  answered term by term, a dictionary entry once per term — once per block at
  plan time for a static filter, but at each group entered for a dynamic
  state whose generation moved. What that costs a TopK or an aggregate is in
  whatever 27.7's re-take reads, not attributed.
- **`Lookup::Each`**, answering term by term, is unreachable by construction
  — one column under one semantics resolves every `=` to one shape — and
  stands instead of a panic.

## Tests

- `pgdump_query/tests/membership.rs`, the generated check: every column of
  the `types`, `statistics`, `edge_cases` and `partitions` fixtures at every
  major, lists of one to six drawn values, a NULL in a quarter of them, bare,
  under `NOT` and in random trees, in both semantics — against the `Or` of
  `=`, statistics off (rows) and on (rows, the pruning note, the early stops,
  schema and notes), and the error either raises. Its floors fail a sweep
  that compared, kept, skipped or refused too little.
- **A NULL in a list is referred to `c IN (NULL)`**, the `Or` having no NULL
  literal, so that check cannot see how a NULL answers; `predicate.rs`'s
  `a_membership_answers_as_its_disjunction_of_equalities` and
  `a_membership_answers_a_group_as_its_disjunction_does` pin it, against an
  `=` over a field the row lacks.
- Mutations, each failing a test above: decoded keys left unsorted
  (`membership.rs`); an unmatched value answering `False` beside a NULL, and a
  group's answer ignoring the NULL (`predicate.rs`).
- `tests/dynamic_filter.rs`: a state holding a membership emits, skips and
  drops what its `Or` does, serial and split; one that cannot resolve is
  loosened as a term is.
- `pgdt`: the grammar in `main.rs` and `where_expr.rs`'s unit tests, and
  `tests/query_where.rs` end to end.
