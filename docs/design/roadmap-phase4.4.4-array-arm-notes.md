# Phase 4.4.4 — The array arm folded into one function: notes

`pgtype::resolve_array(element, types) -> TypeOutcome` holds the whole array
decision — both refusals, the `resolve_nested` recursion, `list_of` and
`NestedPlan::Array` — over one `domain_terminal` walk.
`resolve_declared_type`'s array arm is a single delegation to it, and
`element_is_opaque`/`element_is_array` are gone. Behaviour-preserving: no test
was edited, none was added, and the whole suite passes as it stood.

The mechanism is in [`architecture.md`](architecture.md), "Type resolution",
which this slice repointed at the new function name and corrected on one
factual claim (below). These are the rest.

## What the next slice inherits

**One name to change, not three.** The array decision was previously spread
across two private predicates and the arm that ordered them, each carrying part
of the argument for the whole. Anything that touches how an array column is
typed now touches one function: the two refusal sites and the construction of
the `(DataType, NestedPlan)` pair sit in the same twelve lines, in the order
they are decided.

**`domain_terminal` has exactly one caller again**, and its doc comment says
so. The terminal is still returned as the DDL spelled it — normalizing the
array-bounds production inside the walk would have to allocate — and
`resolve_array` normalizes it where it needs to, which is 4.4.3's placement
call settled by removing the choice rather than by making it (`roadmap.md`,
"Refactor when the shape stops fitting"; the reasoning is in
[`history/2026-08-26.md`](../status/history/2026-08-26.md)).

**No input reaches both refusals**, and the code now says so where the order is
written. The opaque test matches a *bare* type name (`box`, or a `TypeDef` of
kind `Base`/`Shell`); the array test matches that same name with array bounds
appended. A terminal of `box[]` — `CREATE DOMAIN d AS box[]`, a column of `d[]`
— therefore answers `NestedArrayElement`, never `OpaqueElementType`, and did so
before this slice too. The spec's "opaque-tested first so I22's precedence
stays visible" is precedence in principle, not a live disambiguation: it is
kept, and documented as the answer that must win *if* the two ever overlap,
because `OpaqueElementType` is the strictly stronger statement (even the
element boundaries are unrecoverable, since the delimiter is not `,`).

**The one claim in the module that was false is now correct, in both places it
was written.** `array_element`'s doc said it was "not quote-aware", so `CREATE
DOMAIN "weird[]" AS integer` "reads as an array of `"weird`"; the same sentence
sat in `architecture.md`'s "Type resolution". It never did: `strip_bound`
requires a trailing `]` and `strip_array_keyword` a trailing `ARRAY`, and a
quoted name ends in `"`, so both bail — while `s."x ARRAY"[]` sheds only the
bound that falls outside the quotes. I29 records this on the `pg_dump` side and
`STATUS.md`'s "Known gaps" on ours: a quoted type name costs a *weaker* type
(the lookup misses, because `TypeDef.name` is dequoted while the declaration is
not, so the column resolves `Unknown`), never a wrong one. Fixing that is
`roadmap.md`'s "A real type-name tokenizer" and changes behaviour, so it was
out of this slice's scope by construction.

## The standing rule's documentary check

`roadmap.md`, "Refactor when the shape stops fitting", requires a refactor to
name, where it lands, which later phases it expects to survive and what it made
simpler.

**Which phases it survives.** The signature is `(&str, &[TypeDef]) ->
TypeOutcome` — the same two inputs and the same outcome type
`resolve_declared_type` already had, so nothing above `pgtype.rs` sees it at
all. Three consumers are ahead of it and none of them is a reason to unpick it:

- **Phase 5** (pushdown, per-row-group statistics) reads a resolved schema and
  never re-derives one; an array column's Arrow type and its `NestedPlan` are
  its whole interface.
- **Phase 6** (`object_store`, Python, DataFusion) exposes `TypeOutcome` and
  `ColumnResolution` outward. Both refusals keep their variants and their
  labels, so the surface it will freeze is unchanged.
- **Phase 8** (`--inserts` rows, archive formats) arrives with a different
  scanner and the same DDL, and reaches this function by the same call.

The shape it is *designed* to survive is the roadmap "Future" item that would
retire both refusals at once — a representation lossless for every array
(dimensions, lower bounds and elements in one value). That change is now local:
it replaces two `return`s and the pair-construction below them, in one place,
with the domain walk they share already done.

**What it made simpler.** Two walks of the domain chain became one. The safety
argument for the refusal order — previously an ordering claim in one doc
comment relying on a property stated in the other — is one paragraph at the
single point the order exists. And "which function normalizes the array-bounds
production" stops being a question worth flagging, because the second call site
is no longer a predicate that happens to need it: it is the array decision
itself.

## Not done here

The `4.4` and `4.4.2` notes docs name `element_is_opaque`/`element_is_array` in
their accounts of what those slices landed. They are left as written — a slice's
notes record what that slice did — and the phase wrap folds all of them into one
account, which is where the current name belongs.
