# Phase 4.4 — Flipping type resolution: notes

What 4.4.1 and 4.5 inherit. The four container families now resolve to real
Arrow types and decode end to end on the optimistic path; `Deferred` and
`DeferredKind` are gone. The mechanism is in
[`architecture.md`](architecture.md), "Type resolution" and "Nested columns:
`NestedPlan` travels beside the `DataType`". These notes are the rest.

## The three decisions this slice had to make that the spec did not settle

**The cache format is now v10, so 4.5's census bump is v11.** The spec says
the census takes the cache "v9 → v10"; this slice got there first, because
`TypeKind::Composite`'s field list had to become an `Option` (below) and that
reshapes a persisted field. The decision the spec records — the census bumps
the format — is unchanged; only the integer moved. Nothing migrates pre-1.0,
so a v9 cache written before this slice is simply treated as absent.

**A zero-field composite needed two things the spec's "it already works"
reading did not cover.** `decode_record("()")` reports *one NULL field* (I23 —
the literal cannot distinguish the two cases), so `append_typed`'s `Record`
arm special-cases an empty declared field list and accepts exactly that
literal. And `StructArray::new` **panics** on a zero-field struct: it reads the
row count off the first child and there is none, so `finish_column` uses
`try_new_with_length` with the validity length. Both are one-liners; neither
is discoverable without a fixture that has the value, which is why 4.1.1
existed.

**`element_is_opaque`'s domain walk is bounded by the type list's length**,
where `resolve_declared_type`'s own recursion is unbounded. The unbounded form
is justified by I24 (added this slice: PostgreSQL refuses to create a type that
contains itself through composites, ranges, domains or array elements), and
that argument covers the loop equally — but an *iterative* walk on a
hand-edited cyclic file would hang silently where the recursion aborts, which
is a worse failure for the same impossible input. A domain chain visits each
`CREATE DOMAIN` at most once, so the bound is exact rather than a magic number.

## What resolution actually produces

`resolve_declared_type` returns `TypeOutcome::Mapped(DataType, NestedPlan)` —
the pair, from the one producer — and `ResolvedSchema` carries `plans` as a
third positional vector beside `columns` and `notes`. Every column has an
entry, `Scalar` included, so no consumer branches on whether the vector
applies.

Four helpers in `pgtype.rs` are the whole flip: `list_of`, `range_struct`,
`resolve_nested` (any non-`Mapped` outcome becomes `Utf8View` *in that
position*), and `builtin_range_subtype` (the twelve hardcoded names, with the
range/multirange half as a `bool`). Recursion is `resolve_declared_type`
calling itself through `resolve_nested`, so nesting composes in both orders
with no depth logic anywhere.

`element_is_opaque` is the only thing that looks at an element type *before*
resolving it, and it is deliberately not "did the element map?" — `interval[]`
and `money[]` stay `List<Utf8View>`, which recovers the element boundaries.
Only `box`, `TypeKind::Base` and `TypeKind::Shell`, through any chain of
domains, are refused, because those are the ones whose `typdelim` may not be
`,` (I22).

## Non-obvious calls

**`TypeKind::Composite::fields` became `Option<Vec<…>>`, and the empty body is
`Some(vec![])`.** `parse_create_type` treats a whitespace-only body as a real
zero-field composite and requires *every* fragment of a non-empty body to
parse. The distinction has to live in the type definition, not in whether the
`TypeDef` exists at all: I23's "Relied on by" is explicit that "no fields
parsed" and "no fields declared" must stay apart, and dropping the whole
`TypeDef` would also hide the type from `pgdq info`'s census. An unparseable
body resolves `UnknownType` — an existing bucket, since the spec's
`ColumnResolution` surgery was `OpaqueElementType` in and `Deferred` out, and
adding a third variant for a shape no real dump produces would be inventing a
distinction nobody reads.

**`pgdq query` moved to pull mode.** Rendering a nested column needs the
stream's `NestedPlan`s, and `read_table` hands the `ResolvedSchema` back only
once the stream is drained — the architecture doc already points a caller who
needs the schema *during* the callback at pull mode. It is the same scan
(`read_table` drains this stream internally), and `print_batch` re-reads
`stream.resolved_schema().plans` per batch rather than once, because a table's
blocks each carry their own schema. This added `futures` to the CLI's
dependencies.

**`render_field`'s signature changed rather than gaining a sibling.**
`render_field_with_plan` took the name; there is no plan-less entry point, so
a caller that has not thought about plans is a compile error rather than a
runtime panic. Tests over scalar-only fixtures pass `&NestedPlan::Scalar`
explicitly and say why in a comment.

**`resolve.rs`'s module doc was rewritten.** It still described
`ResolvedSchema` as a *preview* whose types nothing applied — stale since
typed decoding landed, and doubly so now.

## What 4.4.1 inherits

- **The manual is wrong right now.** `docs/manual/type-handling.md`'s "Arrays,
  composites, ranges, and multiranges are strings for now" is false as of this
  slice; the spec puts its rewrite in 4.4.1, and `STATUS.md` carries it as a
  known gap until then.
- **`resolution_label` has an `OpaqueElementType` arm** — written to compile,
  not to be final. It says "the array's element type is information-free in
  the dump"; the delimiter half of the reason (I22) is deliberately not in the
  label and belongs in the manual.
- **`pgdq info` still prints only the *declared* type**, never the resolved
  Arrow one, so the compact `list<struct<a: int32, b: string>>` rendering has
  nothing to replace — it is an addition, not a substitution.

## What 4.5 inherits

- **The census changes the pair, not the builders.** `batch.rs` already
  handles both outcomes: `List<List<T>>` works today (the `Array` arm recurses
  one `List` level per dimension), and degrading a column to `Utf8View` is
  just a scalar column. The census's job is to rewrite `(DataType,
  NestedPlan)` after resolution and before the schema is fixed — the one
  transform site.
- **The optimistic path's failure is pinned by
  `tests/decode.rs::a_multidimensional_or_decorated_array_is_refused_on_the_optimistic_path`**,
  which asserts the error names `t_array_shape.v_multidim`, `integer[]` and
  the whole field text. With a census, `v_multidim` becomes `List<List<Int32>>`
  and `v_mixed_dim`/`v_lbound` become `Utf8View`, so that test is the one that
  has to change shape — deliberately, not incidentally.
- The cache bump is **v11**, per the first section.

## Verification

- `tests/decode.rs::nested_columns_round_trip_against_strings_mode` is the
  slice's central evidence: every nested column of `t_array`, `t_composite`,
  `t_range`, `t_user_range`, `t_text_range`, `t_multirange` (PG14+) and the
  two refusal tables, typed and rendered back, required to equal
  `SchemaMode::Strings`'s own text on 13/16/18. `tests/nested.rs` proves the
  codec is its own inverse; this proves the *typed path* is — that resolution
  picked the plan the values are actually written in. A plan/type disagreement
  surfaces here and nowhere else.
- `tests/pgtype.rs::the_container_families_resolve_to_the_arrow_types_the_mapping_table_names`
  states the mapping table against real `pg_dump` output, including the
  zero-field `Struct`, the enum-inside-a-list, and the multirange companion's
  bound type coming from the range that names it.
- The twelve-name distinction is asserted in `pgtype.rs`'s own tests:
  `int4range[]` and `int4multirange` produce the *same* `DataType` and
  different plans.
- `tests/batch.rs::a_predicate_on_a_nested_column_matches_the_literal_text_in_either_schema_mode`
  is the regression guard for the one thing the flip could have changed
  invisibly.
- **Not run: the koji scan.** It is an hour on the HDD and needs a detached
  container run (`CLAUDE.md`, "Long-running processes"). Nothing in this slice
  touches the map, the scanner or the row counts, and koji has no non-scalar
  columns at all — but the spec lists it under Verification, so it is
  outstanding rather than passed.
