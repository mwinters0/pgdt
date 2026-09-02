# P11.10.1 — Nested structural comparison: range and multirange

What the phase wrap inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "Nested columns
compare structurally", and the external evidence is I46. This doc is the part
that is neither.

## What landed

- `pgtype.rs`: `NestedCompare::Range { bound, discrete }` and
  `Multirange { bound, discrete }`; `range_comparison`, the builder both reach;
  a `BuiltinRange` struct so `builtin_range_subtype` carries `discrete` beside
  `subtype` and `multi`; a fourth step in `comparison_for` for the twelve
  built-in range names; `comparison_user_type`'s `TypeKind::Range` arm and the
  multirange-companion lookup its absent-type branch now makes (I10).
- `predicate.rs`: `NestedKey::Range`/`Multirange`, `RangeKey`,
  `RangeBoundKey`, `range_key`, `make_range`, `serialize_range`,
  `compare_bound_values`, `compare_bounds`, `compare_range`,
  `canonical_multirange`, `range_before`, `ranges_adjacent`, `bounds_adjacent`
  and `range_union`, plus the two `nested_key` arms and the two
  `compare_nested` arms.
- I46. `oracle_register.py` gains a fourth walk step —
  `builtin/range` and `builtin/multirange`, with the twelve names read out of
  `builtin_range_subtype`'s own table.
- The register-against-oracle test's `REFUSED` list drops from 14 entries to
  6, which is the slice's real evidence: every range and multirange cell of
  `comparisons.tsv` is now asserted at six majors under all six operators, and
  none of them needed an `EXCEPTIONS` entry.

## The calls worth knowing

**The canonicalization is over decoded keys, not over text, and that is what
made it small.** The obvious shape — a `canonicalize_range(text) -> String`
beside `parse_range` in `nested.rs` — cannot be written: `nested.rs` has no
type knowledge, and a successor needs the subtype's decoder. Rewriting the
`RangeBoundKey`s instead puts the whole of `make_range` in `predicate.rs`,
where both sides have already been decoded, and it collapses three of
PostgreSQL's cases into one pattern:

- `OrderKey::Int(n) + 1` is `int4range_canonical`, `int8range_canonical` **and**
  `daterange_canonical`, which differ only in the width they raise on.
- Matching *only* `OrderKey::Int` is `daterange_canonical`'s `DATE_NOT_FINITE`
  guard, exactly rather than approximately: a date `infinity` decodes to
  `OrderKey::PositiveInfinity`, so it falls out of the pattern and keeps the
  inclusivity it was written with (I34).
- `checked_add` is the overflow check, and it can only fire for `int8range`.
  `int4range '[1,2147483647]'` is accepted here and refused by the server,
  because a leaf literal is read as `i64` whatever the column's width — the
  same over-acceptance `order_key` already documents for a `smallint` literal,
  one level down, and its whole effect is a filter that matches nothing.

**`make_range` runs on the field side too.** It is idempotent on `range_out`
text by construction, so one path serves both grammars — the same shape
`nested_key`'s `input` flag has one level up. The alternative saves nothing and
leaves the field half untested.

**A `RangeBoundKey` carries which end it is.** It decides the answer whenever
two bounds hold the same value, and `bounds_adjacent` deliberately *relabels* a
pair before comparing it — the server does the same, in the one place it builds
a range out of an upper and a lower bound to ask whether anything lies between
them. Passing the end as an argument instead would have made that relabelling
invisible.

**`ComparisonPlan::Nested` is now the answer for every container kind**, which
makes `resolve_term`'s `plans[index] != NestedPlan::Scalar` branch nearly dead.
One shape still reaches it — a range whose DDL stated no `subtype`, where there
is no bound type to name a refusal after — and it is kept as the guard against
the two walks disagreeing, since the alternative to refusing is ordering a
container's literal with a *scalar* comparison. `test_types()` in
`predicate.rs` now declares `public.opaquerange` so the branch has a test.

**Two `tests/nested.rs` entries survive and were not closable there.**
`CANONICALIZED`'s two `int4range` rows and its `daterange` row, and the whole
of `SEMANTIC_REFUSALS`, describe the *grammar* layer: `parse_range` may not
tighten and `render_range` may not rewrite, so the pair still round-trips
`[1,10]` unchanged. What closed is the question one layer up, and both doc
comments now say where. 11.10's `("public.point2d", "( 1 , a )")` entry is
untouched and still declined.

**A `text`-bounded range announces a note the reader cannot act on.** A range
type carries its own `collation` parameter and the preamble grammar keeps
`subtype` and `multirange_type_name` alone, so `public.textrange` — which
declares `collation = pg_catalog."C"` — reaches `collated_text`'s no-clause arm
and warns. The rows are the server's, so it is a property rather than a
deficiency; what is worth knowing is that rewording the note was considered and
rejected beside the mechanism, because the sentence is `UnknownCollation`'s own
and the real fix is to read the parameter.

## Where the expectations came from

Every hand-written assertion in the three new `predicate.rs` unit tests and the
three new `tests/ordering.rs` ones was put to a live server before it was
written down — `postgres:16.15-trixie` and `postgres:18.6-trixie`, one query
per group, the two majors answering identically. I46's `Re-verify` carries that
probe. Two of its answers are the ones worth having asked rather than reasoned:
`{[1,5),[6,10)}` does **not** merge in `int4multirange` (the range `[5,6)` is
real), while `{[1,5],[6,10)}` does (the first member canonicalizes to `[1,6)`
first) — so adjacency is decided after canonicalization, not before.

## What the phase wrap inherits

**The checklist is fully ticked and P11 has not been wrapped.** The wrap is the
next session's work: consolidate the sixteen slice notes into one
`roadmap-P11-typed-predicates-notes.md`, delete them, delete the checklist, and
set the roadmap index row to `Complete` in the same change — after which every
`(b)` entry P11 owned has to be re-homed, which is none today.

**`KD12` was allocated by this slice** and is the only register entry it added:
a user-defined range declaring a `canonical` function is compared without it.
It is `(c) unowned`, and the reason it is not owned by anything is worth
keeping: reading the parameter is a preamble change and a cache format bump,
and knowing the parameter *exists* licenses only a refusal — a user's canonical
function is arbitrary SQL, so no amount of parsing lets this build reproduce
it. There is nothing here for a phase to plan; there is a dump that would
promote it.

**Nothing this slice touched is a declared path of any performance figure.**
`pgtype.rs`, `predicate.rs`, the test crates and `scripts/oracle_register.py`
are declared by none, so `uv run measure.py --stale` reads exactly as it did
before, and no `ACKNOWLEDGED` entry is owed.

**One number in `STATUS.md` is already wrong and is left for the wrap**, which
rewrites that file anyway: it says `--stale` "names twelve of its thirteen
figures" and the harness names **thirteen** of thirteen. It predates this slice
— nothing here touched a declared path — and correcting it inside a diff about
range comparison would be widening the review for a word.
