# P11.10 — Nested structural comparison: array and composite

What 11.10.1 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "Nested columns
compare structurally", and the external evidence is I45. This doc is the part
that is neither.

## What landed

- `pgtype.rs`: `NestedCompare` — `Leaf`/`Uncomparable`/`Array`/`Record` — and a
  third `ComparisonPlan` variant, `Nested`, plus `ComparisonPlan::orders()`.
  `comparison_for`'s array branch and `comparison_user_type`'s composite arm
  build the tree; the range arm and the multirange companion's early return
  still answer `Refused`.
- `predicate.rs`: `NestedKey`, `nested_key(plan, text, input)`,
  `compare_nested`, `compare_slot`, `Comparison::Nested`, and the nested branch
  of `resolve_term`. `ComparisonNote` gains a `path`, `ComparedTerm` a *list* of
  divergences, and `ResolvedTerm::comparison_note` becomes `comparison_notes`.
- `error.rs`: `UnorderedPredicateColumn::reason` is a `String`, so a refusal can
  name the position and the type that caused it.
- I45; the manual's "Filtering one of these columns"; the register table's two
  nested rows.

## The calls worth knowing

**`orders()` is the question, not the variant.** A `Nested` plan is not by
itself an order — a tree holding an `Uncomparable` position is the register
saying the column has none. Every caller that used to ask
`matches!(plan, Compared { .. })` asks `plan.orders()` instead, the oracle test
included. Getting this wrong is not a compile error and shows up as a column
being ordered that the resolver has already declined.

**The register mirrors `resolve_array`'s two refusals, and it has to.** An
array whose element is opaque (I22) or is itself an array (I26) resolves the
*column* to `Utf8View`, so `resolve_columns` zeroes its plan anyway — but
`comparison_for` is also asked directly (the oracle test does), and a register
that answered "ordered" for `public.intarr[]` would disagree with the resolver
about the same column. So the tree carries `Uncomparable` at that position
rather than recursing.

**One comparison path, not the scalar's three.** `Comparison::Nested` serves
both operator families; `=` is the key walk answering `Equal`. That is sound
because `array_cmp` returns zero on exactly the condition `array_eq` returns
true (I45). The canonicalize-the-literal-once fast path a scalar `=` gets was
considered and rejected — its rationale is in `architecture.md`, and the short
version is that the oracle exercises the fast half of such a pair and not the
slow one.

**The literal side and the field side are one walk with a flag.** `input: bool`
picks `parse_*` over `decode_*`. Two grammars, one traversal, so a nesting
level cannot read leniently on one side and strictly on the other by accident.

**A leaf is read in its own type's output form on both sides**, through
`order_key`, and that call is **settled — 11.10.1 does not reopen it**. It
means `--filter 'p=( 1 , a )'` is refused, where `record_in` keeps the blanks
and `int4in` throws them away, so 11.9's `CANONICALIZED` entry for
`("public.point2d", "( 1 , a )")` is **declined rather than closed** and its
comment says so. **11.10.1 meets the identical question at a range bound**
(`[ 1 , 10 )`) and answers it the same way: read the bound as the subtype's
`*_out` writes it, and refuse the padding. Reasoning and the corrected fact set
are in `architecture.md`, "Nested columns compare structurally" (the two
`Rejected:` paragraphs) and
[`../status/history/2026-09-02.md`](../status/history/2026-09-02.md), "The leaf
grammar and the refusal's shape are both affirmed".

**A zero-field composite needs the arity question asked twice.** `record_out`
writes `()` for a zero-field composite *and* for a one-field composite holding
NULL (I23). `parse_record` takes the field count and answers correctly;
`decode_record` cannot and returns `[None]`, so `nested_key` clears it when the
plan says zero fields — the same rule `batch.rs` already applies one layer over.

## What 11.10.1 inherits

**The tree, the walk and the notes are done; only two node kinds are missing.**
Adding `NestedCompare::Range(Box<_>)` and `Multirange(Box<_>)` means: a
`nested_key` arm each (`parse_range`/`decode_range` and the multirange pair), a
`compare_nested` arm each, and the canonicalization — which is the actual work
and has no place to live yet.

**Four canonicalizations, and the register has to know which apply.**
Canonicalization is a property of the *range type*, not of its subtype: only
`int4range`, `int8range` and `daterange` have a canonical function among the
built-ins, and a user-defined range declares one in DDL `pg_dump` writes but
this build does not parse. So `public.myrange` — a range over `integer` with no
`canonical` parameter — does **not** canonicalize, and a register that keyed the
rewrite off the subtype would get it wrong. `daterange_canonical` also skips any
bound that is `DATE_NOT_FINITE`, so `[2020-01-01,infinity]` keeps its inclusive
upper.

**Two refusals a range needs that the grammar cannot see**, both recorded as
exact sets in `tests/nested.rs`'s `oracle` module: bounds out of order
(`int4range '[10,1)'` is `22000`, and it is `SEMANTIC_REFUSALS`'s only member),
and a multirange member that canonicalizes to empty. Closing either deletes its
entry there.

**`range_cmp` is not the array's shape.** Empty sorts below everything; two
non-empty ranges compare lower bound then upper, and a bound comparison settles
infinity first, then the value, then inclusivity — an exclusive *lower* is above
an inclusive one at the same value, and an exclusive *upper* below. That is
`range_cmp_bounds`, and it is worth transcribing rather than deriving.

**The oracle already carries the cells.** `int4range`, `numrange`, `daterange`,
`tsrange`, `tstzrange`, `public.myrange`, `public.textrange`, `int4multirange`
and `public.myrange_multi` are all in `comparisons.tsv` at six majors, and all
nine are in the oracle test's `REFUSED` list today. Landing 11.10.1 removes them
from it, and whatever disagrees will be reported as a candidate `EXCEPTIONS`
entry — which is how the six `text[]` entries this slice added were found.

## Where the expectations came from

I45's probe was run against `postgres:16.15-trixie` and `postgres:18.6-trixie`
before the comparator was written: the shape tie-break, the NULL rule at both
levels, and `'{}'::json[] < '{}'::json[]` raising
`could not identify a comparison function for type json`. The spec had the array
rule as *shape first, then elements*, which is `array_eq`'s route and not
`array_cmp`'s; the committed oracle says so too — `{1,2}` is above
`[0:1]={1,2}` and below `{{1,2},{3,4}}`, and a shape-first order answers the
first of those backwards.
