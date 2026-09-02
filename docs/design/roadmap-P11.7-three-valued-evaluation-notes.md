# P11.7 — three-valued evaluation

What the rest of P11 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "Predicates".

## What landed

`QueryOptions::filters: Vec<Predicate>` is gone, replaced by
`QueryOptions::filter: Expr`.

- **`Expr`** in `predicate.rs` — `Term(Predicate)`, `And(Vec<Expr>)`,
  `Or(Vec<Expr>)`, `Not(Box<Expr>)` — with `Expr::all(terms)` for the
  conjunction and `Default` = `Expr::all([])`, the filter that keeps every row.
- **`Truth`** — `True`/`False`/`Unknown`, public, with `is_true` the only
  public method. It is exported because it is the domain `Expr`'s and
  `Predicate`'s doc comments are written in; nothing public returns one.
- **`ResolvedExpr`**, the same tree with each leaf resolved against a block's
  schema. It is what `Active` carries and what `stream::resolve_expr` builds.
- **`ResolvedTerm` gained `op`**, and `Predicate::matches` became
  `ResolvedTerm::eval -> Result<Truth>`. `matches_all` is gone.
- **`PredicateOp::IsDistinctFrom` / `IsNotDistinctFrom`**, which route through
  the same equality comparison `Eq`/`Ne` do and differ only in the NULL rule.
- **`hash_expr`** in `stream.rs`, replacing the flat term loop in
  `query_fingerprint`.

**The CLI adapts and gains nothing.** `main.rs` wraps its parsed terms in
`Expr::all`, and that is the whole diff there. `Or`, `Not` and the two
`IS DISTINCT FROM` forms are reachable from the library only until 11.8 adds
`--where`; `parse_filter` is untouched, so no `--filter` string changes
meaning.

## The three calls worth knowing about

**`ResolvedTerm` carries its own operator, and that is why the flat parallel
vectors could go.** The old row path zipped `&[Predicate]` with
`&[ResolvedTerm]`, an invariant maintained by construction and checked by
nothing. A tree cannot be zipped that way without walking two trees in
lockstep, which is the same invariant with more places to get it wrong, so the
operator moved into the resolved leaf and the resolved tree became evaluable on
its own. 11.10 inherits this: a nested comparison's `ResolvedTerm` will need
its path too, and there is now one place to put it.

**Short-circuiting is defined against the root, not against the node, and that
is the one non-obvious piece of the evaluator.** Correct Kleene `And` cannot
stop at an `Unknown` — a later `False` still makes the conjunction `False`, and
a `Not` above would tell those apart. But nothing *except* `Not` tells them
apart: the row path asks only "is the root `True`". So `eval` takes an `exact`
flag, false at the root, inherited by `And`/`Or`, and set true by `Not` for
everything beneath it. With `exact` false an `And` may return `False` for an
unknown and stop — which is exactly the short-circuit the bare conjunction
always had, so the ordinary filter's per-row cost did not move.

The flag is a refinement, so it needs an oracle rather than an argument:
`the_root_verdict_survives_the_short_circuit` builds every tree of height three
over the three truth values (1179 of them, leaves being real terms over a row
whose first field is `1` and second is NULL) and asserts the inexact root
verdict equals the exact one. Anything added to `eval` that the flag touches
should keep that test.

*Rejected: exact Kleene everywhere, with no flag.* It is four lines shorter and
it makes a conjunction whose leading column is often NULL walk every remaining
term per row — a regression in the shape the architecture doc already names as
the filter's cost model, bought for nothing, since no caller can observe the
difference.

**A decode failure now surfaces in a place that depends on the tree, and on
whether a `Not` is above it.** The spec blesses the first half; the second half
is this slice's addition, and it is the direct consequence of `exact`. It is
asserted in `a_decode_failure_surfaces_only_where_it_is_reached`, which is the
test to read before changing anything about `eval`'s control flow.

## What the oracle now witnesses

`the_register_answers_every_committed_oracle_cell` compared a NULL-field cell
against `Ok(false)` before, which conflated the server's `u` with its `f`. It
now compares against `Ok(Truth::Unknown)`, so all six majors' `u` cells are
evidence for the three-valued restatement rather than for the collapse it
replaced. Nothing else in that test moved and the 45,394-cell count is
unchanged; the exception set, the refused set and the announcement rule are all
as 11.6.1 left them.

`IS DISTINCT FROM` earns **no** oracle case. The fixture's operator set is the
six the case table was generated with, and the two new operators are the
equality comparison plus a NULL rule that no cell of that table exercises — a
case for them would have to be generated, which means regenerating six majors
to observe a rule stated in one line of `ResolvedTerm::eval`. They are asserted
in `is_distinct_from_counts_null_as_a_value`, beside the `Not` that is the
reason they exist. `oracle_register.py` is unaffected: it joins register
*arms*, and the register did not gain one.

## What 11.8 inherits

- The leaf grammar is untouched, so `--where`'s delegation to `parse_filter`
  works as the spec describes it.
- `Expr::all` is the constructor for the ANDed `--filter` list; a query giving
  both flags ANDs the two, which is `Expr::And(vec![where_tree, all(filters)])`
  or a flattening of it — nothing in the library prefers either.
- The two `IS DISTINCT FROM` forms have no CLI spelling yet. Their symbols are
  already `"IS DISTINCT FROM"` / `"IS NOT DISTINCT FROM"` in
  `PredicateOp::symbol`, which is what every refusal message names them by, so
  the grammar and the messages will agree without a second table.
- The usage message `query_ordering.rs` asserts over lists only the operators
  the CLI accepts, so it did not have to change here and will when the grammar
  does.
