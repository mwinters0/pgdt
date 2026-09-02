# P11.14 — A user range's `canonical` parameter

What 11.15 and the phase wrap inherit. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "Nested columns
compare structurally" and "Ordering operators compare typed"; the external
evidence is I10 (amended here) and I46. This doc is the part that is neither.

## What landed

- `preamble.rs`: `TypeKind::Range` gains `canonical: Option<String>`, kept
  verbatim from the same `key = value` split that already produced `subtype`
  and `multirange_type_name`.
- `pgtype.rs`: `ComparisonPlan::Unanswerable(UnanswerableReason)`, the fourth
  outcome, with one reason today — `RangeCanonical { range_type, function }`.
  `nested_position` now returns `Result<NestedCompare, UnanswerableReason>`;
  `array_comparison`, the `TypeKind::Composite` arm and `range_comparison`
  propagate. `unanswerable_range` is the one producer, reached from
  `comparison_user_type`'s `TypeKind::Range` arm and from its absent-type
  (multirange-companion) branch.
- `error.rs`: `Error::UncomparablePredicateColumn`.
- `predicate.rs`: `unanswerable_reason`, and one arm ahead of the nested
  checks in `resolve_term`.
- `cache.rs`: `FORMAT_VERSION` 15.
- `main.rs`: `type_kind_summary` names the function, so `info --verbose` shows
  it.
- I10 gains a third claim (the `canonical` parameter is written, and where in
  the body); a `pg-dump-compatibility.md` row marks it untested against real
  `pg_dump` output and names the blocker. `KD12` is **struck**.

## The calls worth knowing

**Unanswerability propagates; comparability is inherited.** The spec asked for
a refusal on "a column whose range type declares a canonical function", and a
column is one of those at any depth: `myrange[]`, a composite with a `myrange`
field, a domain over either. A `NestedCompare` tree cannot carry the fact — the
tree's vocabulary for a bad position is `Uncomparable`, which means "no order,
`=` still as text", and that is exactly the answer this must not give. So the
fact leaves the walk instead of joining it: `nested_position` returns a
`Result`, and its three callers turn an `Err` into the *column's* plan. The
`Err` is not a failure — it is a stronger outcome travelling on the error
channel because that is the channel that short-circuits.

**The payload is a typed reason, not a sentence.** `UnanswerableReason` has one
variant and looks like speculative generality; it is not. The alternative is a
`String` built in `pgtype.rs`, which puts user-facing prose in L2 — the thing
`accepted_form` and `nested_refusal` already exist to avoid. A named reason
also forces the next producer to say which kind of unanswerable it is rather
than inventing a message beside the code that discovered it.

**A fourth `Error` variant, because the third one's sentence is wrong here.**
`Error::UnorderedPredicateColumn` ends `; use `=` or `!=` for a text
comparison`, which is the whole point of the refusal it is offering — and here
`=` is refused too. Making that trailing clause conditional would have made one
message say two things; a second variant says one thing each. Both carry a
`String` reason for the same reason the first does.

**The refusal is checked before the two nested branches, not after.** A range
column's `NestedPlan` is not `Scalar`, so had the arm gone last the column
would have hit the `plans[index] != NestedPlan::Scalar` guard and been refused
as "nested" for the ordering operators and answered bytewise for `=` — the
defect, with a different message. The order of that `else if` chain is
load-bearing.

**The end-to-end evidence is a hand-written dump, and has to be.** A canonical
function is declared against the shell type and `CREATE FUNCTION` refuses a SQL
function there (`ERROR: SQL function cannot accept shell type`), so no fixture
can carry one without I11's `LANGUAGE internal` recipe *plus* a built-in symbol
that computes the subtype's successor. `tests/ordering.rs`'s
`a_range_declaring_a_canonical_function_refuses_every_operator` writes the DDL
in the shape `dumpRangeType` emits it — shell first, then the body with
`canonical` after `multirange_type_name` — and drives `table_stream` over it.
That is what makes the parameter's *placement* claim in I10 something the code
is exercised against rather than only read from source. It also asserts the
refusal's **whole message** word for word, because `type-handling.md` prints it
for a user to recognise — the manual's number is an illustration, its sentence
is not.

**The preamble unit test carries every parameter `dumpRangeType` can write**,
in emission order, so the two the grammar keeps are found past the three it
steps over. `collation = pg_catalog."C"` is the one worth having there: its
value holds a quoted identifier and a `.`, which is the shape a naive split
would trip on.

## What this did not do

**No fixture, no oracle case.** The oracle's cells come from a server that
would need the same `LANGUAGE internal` function to hold such a type, and the
register-to-oracle reconciliation reads *arms*, not outcomes — the
`TypeKind::Range` arm is unchanged and still covered by `public.myrange`. So
`fixtures/*/oracle/` is byte-unchanged and `oracle_register.py` reports exactly
what it did before.

**The nested-position path is unexercised beyond `comparison_for`.** The
`pgtype.rs` test asserts that `public.canonrange[]`, `public.holder` and
`public.holder[]` all answer `Unanswerable`; no test drives a *query* over one,
because building a dump with a composite holding such a range adds nothing the
range column itself does not already prove about `resolve_term`.

**No performance figure moved.** `cache.rs` and `preamble.rs` are declared
paths (of `per-block-quadratic` and `preamble-prepass`), and both figures were
already red before this slice — `uv run measure.py --stale` names the same
thirteen it did, so no `ACKNOWLEDGED` entry is owed.

## What the phase wrap inherits

**One box is left**, 11.15, and it changes an already-tested path: every nested
column's `=` reports no divergence today and the oracle asserts it. Nothing
here touched the `_ =>` arm it will edit — the new refusal returns before that
`match` is reached — so the two do not collide.

**`STATUS.md`'s figure count is still wrong** and is still left for the wrap,
on 11.10.1's reasoning: it says `--stale` "names twelve of its thirteen
figures" and the harness names thirteen of thirteen. It predates both slices,
and correcting it inside a diff about range comparison would be widening the
review for a word.
