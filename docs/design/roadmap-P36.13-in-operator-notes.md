# P36.13 notes — `IN` is an operator

`PredicateOp::In` and `NotIn` hold their list, `Vec<Option<String>>`, `None`
a NULL; a membership is an `Expr::Term` like any other test, and `Membership`
and `Expr::In` are gone. `PredicateOp::operand()` answers what an operator
holds — `Operand::None`, `Operand::Literal(&str)` or `Operand::Set(&[…])` —
in place of `value()`. D53 is rewritten.

## The shape

- **Resolution keeps the membership its own form.** `resolve_leaf`
  (`predicate/resolve.rs`) routes a term: `In` to a `ResolvedExpr::In`,
  `NotIn` to `ResolvedExpr::Not` over one, everything else to
  `resolve_term`, which no membership reaches. So `NOT IN` answers as `In`'s
  negated truth through the tree's own `Not`, and `for_rows` drops bounds
  only beside an `In`, a `NotIn` being beneath a `Not` — pinned in
  `eval/tests.rs`'s `row_evaluation_drops_only_the_bounds_a_membership_implies`.
- **A refusal names the operator written**: `resolve_membership` takes the
  column, the list and the symbol, so a `NOT IN` literal that does not decode
  is refused naming `NOT IN`.
- **The adapter emits both.** `filter/pushdown.rs` builds `In` or `NotIn` from
  `InList { negated }`; `filter/dynamic.rs` reads a `NOT IN`'s list at the
  flipped parity, as it read the list beneath its `Not` before, and stands as
  whatever keeps every row where none translates — `And([])` at even parity,
  where it was `Not(Or([]))`, the same truth.
- **`pgdt` builds `In` alone.** Its grammar has no `NOT IN` spelling, and
  adding one is a CLI change (spec, "Scope"); `--where 'not c in (…)'` stays
  `Not` over an `In` term, which answers as `NotIn` does.
- **The resume fingerprint** hashes `In` and `NotIn` as tags 12 and 13 with
  the operand. A token is valid only in its own process, so nothing was kept
  for a token taken before this slice.
- `tests/membership.rs` now checks `NotIn` against the `Not` over the `Or` of
  `=`, where it checked `Not` over the membership.

## Negative results

- **`resolve_term` panics on a membership** (`unreachable!`), rather than
  taking a value-free operator: `resolve_leaf` is its only router, and a
  second, value-free copy of the operator to make the case unrepresentable
  is what 36.8.2 already refused.
- **Not counted.** Not one of "What a hot-path slice owes" reshapes: the row
  path still answers a membership through `ResolvedMembership`, unchanged.

## What the next round inherits

- 36.8.2's notes name `PredicateOp::value()`, which this slice replaced with
  `operand()`; the wrap's consolidation reads this doc beside them.
- **36.11** rebuilds the adapter over a `Dump`; the translation it carries
  over now builds terms only.
