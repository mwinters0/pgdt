# P27.9 — The bounds an `IN` implies, and the cut's keying: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 9. **Both mechanisms are in, the four figures they could move are
re-taken at `5e02bf9`, and the criterion fails: row evaluation goes off**,
which 27.10 delivers with its switch.

## What exists

- **A dynamic filter's rows meet its state in row form**
  (`predicate::ResolvedExpr::for_rows`, held beside the whole state in
  `stream::DynamicBlock`): each ordering term that a membership in the same
  conjunction implies is dropped, but only where no `Not` stands above it,
  and a conjunction left with one conjunct becomes that conjunct. So the
  costing filter, `bucket >= lo AND bucket <= hi AND bucket IN (…)`, reaches
  a row as the membership alone. Its groups, its sorted stop and the cut
  still read the whole state ("D93").
- **Implication** (`ResolvedMembership::implies`) is answered for the two
  lookups whose equality settles what an ordering term reads. For a
  `Canonical` one, the term is evaluated on each literal's file spelling.
  For a `Decoded` one, each literal's key is compared in the term's own
  kind. `Trimmed`, `Nested` and `Each` imply nothing.
- **A group's bounds are keyed once for every term of a tree reading them**
  (`predicate::KeyedBounds`), where each of a membership's terms used to key
  them again. That covers the cut at the first poll, re-pruning and the
  static filter's pruning. `SortedStop::reachable` asks each term on its
  own, and still keys once per term. It changes no answer, and it is paid
  with row evaluation off too.

## Negative results

- **A static filter keeps its implied bounds.** Its row evaluation raises
  what it reaches (D54), and a dropped bound would stop raising on a field
  it fails to key ("D93", **Rejected**). A dynamic filter's evaluation keeps
  such a row instead. So in the row form, a field the dropped bound could
  not key — something no `*_out` writes — is answered by the membership,
  just as a row settled by an earlier leaf is.
- **`Trimmed` implies nothing**, although a `character(n)` bound on the
  trimmed literal would almost always answer as the field does. A join over
  `character(n)` keys therefore keeps its bounds per row. That costs speed,
  not rows.
- **The cut does not search the sorted list within a group's bounds.** That
  would stop its cost scaling with the list's length. Keying once is what
  the spec names, and the per-term comparisons that remain are key
  comparisons, with no parsing.
- **The row form is rebuilt at each state read of each block**, a clone of
  the tree and its lookup, the cut's untimed reads included. That is once per
  block per generation, never per row.

## Tests

- `predicate.rs`, `row_evaluation_drops_only_the_bounds_a_membership_implies`:
  checks which bounds go and which stay, nested, beside another column,
  under an `Or` and a `Not`, with a `NULL` or an empty list, and on a decoded
  kind.
- `predicate.rs`, `a_tree_and_its_row_form_keep_the_same_rows`: every bound
  of four operators at seven edges, over every list of up to three values
  with and without a `NULL`, alone, beside a second column, under an `Or`
  and under a `Not`. It checks that the row form answers every row the tree
  answers alike, a `NULL` and an undecodable field included. Mutations:
  `implies` answering `true`, or `for_rows` descending beneath a `Not`,
  each fail it.
- `predicate.rs`, `a_group_s_bounds_are_keyed_once_per_column_and_kind`.
  Every existing truth-set test, and `a_groups_truths_hold_every_rows_answer`
  among them, runs through the keyed path.
- `tests/evaluation_instrument.rs` (`--features introspect`) now pins the row
  form's counts on the join-shaped state: one `Locate`, `Unescape` and
  `Lookup` a row, no `Key` or `Compare`.

## The readings and the criterion

Each figure taken alone at `5e02bf9`, the commit after the one landing the
mechanisms, and folded into [`measurements.md`](measurements.md), where
every number below it is stated:

```sh
cd scripts && uv run measure.py --figure dynamic-filter-join
cd scripts && uv run measure.py --figure dynamic-filter-topk
cd scripts && uv run measure.py --figure predicate-terms
cd scripts && uv run measure.py --figure statistics-pruning
```

- **The criterion fails on the costing input.** The unclustered join wins
  with its legs' spreads apart, and the costing input still loses with its
  legs' spreads apart, so its Δ lies inside no overlap
  ([`measurements.md`](measurements.md), "What DataFusion's dynamic filters
  buy a query"). Row evaluation goes off, "D93" reopens, and 27.10 is taken:
  the switch, off by default. **The shipped scan still evaluates rows until
  27.10 lands**, since turning it off with no switch would leave the
  unclustered join's win unreachable and re-time the figure just taken.
- **No figure is slower by its spreads, so both mechanisms stay.** The
  costing row's Δ fell inside the band 27.8's account predicted, at its low
  end; `predicate-terms` and `statistics-pruning` moved nothing beyond their
  spreads.
- **Two moves are unattributed**, each said so beside its figure: the
  unclustered join saving more than the costing row over the same removed
  leaves, and the TopK table's legs both rising, the off leg more, over
  identical scan metrics (`runs/dynfilter-27.9-explain-20260928/`).
