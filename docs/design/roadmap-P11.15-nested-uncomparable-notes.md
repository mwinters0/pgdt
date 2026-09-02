# P11.15 — A nested uncomparable position announces itself

What the phase wrap inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "Nested columns
compare structurally" and "Equality is typed too". This doc is the part that is
neither.

This is the last slice of P11's checklist. Everything it owes the wrap is in
"What the wrap inherits" at the end.

## What landed

- `pgtype.rs`: `NestedCompare::Uncomparable` gains a second field,
  `divergence: Option<ComparisonDivergence>` — what the column's bytewise `=`
  fallback costs at that position, which is a different question from the one
  the variant's existence answers ("this position has no order"). It is
  `Some(AsText)` from `nested_position`'s `json` arm and `None` everywhere
  else.
- `pgtype.rs`: `NestedCompare::walk`'s callback takes comparability and
  divergence as two arguments rather than one nested `Option`, since the two
  are now independent. `uncomparable()` reads the first; `divergences()` reads
  the second and no longer skips an uncomparable position.
- `predicate.rs`: `resolve_term` keeps the tree a nested column fell out of in
  a `fell_back` local, and the `_ =>` arm reads its `divergences()` — filtered
  by `affects_equality` — instead of the column's resolution, for that
  population only.
- Two unit tests in `predicate.rs`
  (`a_nested_uncomparable_position_announces_under_equality`,
  `a_position_the_resolver_declined_announces_nothing`) and one more assertion
  in `pgtype.rs`'s `a_position_with_no_order_refuses_the_whole_column`. The
  `json[]` half of
  `a_column_with_no_comparison_answers_equality_and_says_when_that_is_a_guess`
  moved out of it, that test now being about the populations that stay silent.
- `docs/manual/type-handling.md` gains the `=` warning beside the `<` refusal
  it already showed.

No fixture bytes, no oracle case, no cache format change, no new error.

## The calls worth knowing

**The divergence is carried on the position, not decided at the term.** The
`_ =>` arm has no `types` list and no `collations`, so it cannot re-ask the
register why a position was refused; the alternative to a field on the variant
was for L3 to announce `AsText` for *every* uncomparable position, which the
spec's sentence reads as. That is wrong for two of the three producers, and
wrong in opposite directions: `AsText` claims the server has no comparison
here, which is true of `json` and false of `public.intarr[]` — PostgreSQL
orders that through `array_ops` and the bytewise answer agrees — while
`UnmodelledType` claims the reverse and is false of `json`. One `Option` field
on a variant that already carries the declared type lets each producer say only
what it knows.

**`divergences()` grew to include it rather than `uncomparable()` returning a
triple.** The `_ =>` arm wants every equality-reaching announcement a fallen-back
tree has, not just the position that caused the fall — a composite with a
`json` field *and* a field on a non-deterministic collation has two things to
say — and one call in walk order gives both. The structural arm above is
unaffected by construction: it is reached only when `uncomparable()` is `None`,
so no tree it sees can hold one of the new entries.

**`json` is the only type that reaches the branch at all today**, which is why
the change is smaller than the population of `Uncomparable` positions suggests.
`resolve_term` drops the plan on `ColumnResolution != Mapped` one branch
*before* it reads the tree, and every other producer of an uncomparable
position also resolves its column to text: `box[]` and `public.gtype[]` are
`OpaqueElementType` (I22), `public.intarr[]` is `NestedArrayElement` (I26), and
a scalar the register refuses is `UnknownType`, `OpaqueBaseType` or `EmptyEnum`.
So the tree's new field is read for `json[]`, for a composite with a `json`
field, and for containers over those.

**`box` beneath a container stays silent, and that is `KD10` rather than this
slice.** A `box[]` column's bytewise `=` is wrong for the reason a `box`
column's is — `box_eq` compares areas — but it arrives at the fallback through
its *resolution*, not through its tree, so closing it means widening the set of
`ColumnResolution` outcomes that announce `UnmodelledType` (today
`UnknownType` and `OpaqueBaseType`; `OpaqueElementType` would be the addition).
That is a decision about the scalar rule seen through a container, not about
the nested one, and it belongs with whatever closes `KD10`. `KD10`'s index line
and its `architecture.md` paragraph were rewritten to say so, since the entry
previously claimed the announcement was unconditional.

**Nothing about the row set moved.** The fallback is the same
`Comparison::Canonical` byte comparison it was; only the note channel changed.
That is what let the slice land as an announcement rather than a refusal —
refusing would take away an answer a user has today, over a position where a
byte comparison is at least *some* answer and the server offers none.

## What the wrap inherits

- **The checklist is complete**; the wrap is what remains of P11. It owes the
  consolidation of fourteen slice notes docs into
  `roadmap-P11-typed-predicates-notes.md`, the deletion of the per-slice files
  and of the STATUS checklist, and the roadmap index row moving to `Complete`.
  Per `process.md`, a wrap after a keystone is an **audit**: `architecture.md`
  already holds every mechanism these slices built, so the consolidated doc
  carries the phase's negative results and the facts addressed at the next
  phase, and can be short.
- **`KD7` and `KD10` are the two register deficiencies P11 leaves open**, both
  `(c) unowned`. Neither is owned by P11, so the wrap's re-homing pass has
  nothing to move; check that the phase index row and the entries still agree
  (`cd scripts && uv run deficiencies.py`).
- **No figure was turned red by this slice**: it lives in `pgtype.rs` and
  `predicate.rs`, which no figure declares. The staleness paragraph 11.14 owed
  is written into `STATUS.md` beside this one's.
- **The one place the phase's evidence is thinner than the code** is that no
  fixture carries a `json[]` column, so the announcement is asserted by unit
  test rather than end to end. Adding one means regenerating all six majors,
  which is the fixture family's whole cost for a case whose answer no server
  can be asked (`json[] = json[]` is an error), so the oracle could not carry
  it either.
