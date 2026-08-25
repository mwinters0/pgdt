# Phase 4.3 — `ColumnBuilder`'s `List` and `Struct` arms: notes

What 4.4 inherits. `batch.rs` can now fill a `List<T>` or `Struct<…>` column
from a PostgreSQL literal and render it back, and nothing reaches those arms:
`resolve_declared_type` still answers `Deferred` for all four families, and
`RowBatcher::new` passes `NestedPlan::Scalar` for every column.

The mechanism is in [`architecture.md`](architecture.md), "Nested columns:
`NestedPlan` travels beside the `DataType`". These notes are the rest.

## The one design decision this slice had to make

**`NestedPlan` exists because the Arrow type cannot say which literal fills
it.** `int4range[]` and `int4multirange` both resolve to
`List<Struct{lower, upper, …}>`; the first is written `{"[1,10)","[2,3)"}` and
the second `{[1,10),[2,3)}`. A composite that happens to carry the range
struct's five fields is the same collision one level down. So the plan is a
tree passed beside the `DataType`, and it is what selects a `nested.rs` codec
at each level.

This was not in the spec — the spec's mapping table stops at the Arrow type —
and it is the reason `render_field` grew a sibling. It is filed under STATUS's
"Decisions worth another look".

**What 4.4 owes this slice: produce the plan.** `resolve_declared_type` has to
return a `(DataType, NestedPlan)` pair rather than a `DataType` alone, and
`ResolvedSchema` has to carry one plan per column so that:

- `RowBatcher::new` passes `&resolved.plans[i]` instead of
  `&NestedPlan::Scalar`; and
- every caller of `render_field` that can see a nested column — the CLI's
  `query` output, `tests/decode.rs`'s `rows_of` — switches to
  `render_field_with_plan`.

Until then `render_field`'s `unreachable!` arm is still unreachable, because
no `List`/`Struct` column can be resolved.

## Non-obvious calls

**The plan is `pgtype.rs`'s (L2), not `batch.rs`'s (L3).** It is a statement
about PostgreSQL type semantics, produced by resolution and consumed by
assembly; L3 naming an L2 type is dependencies pointing downward, which is
fine.

**`NestedPlan::Range` carries one plan, used for both bounds**, because a
range's two bounds are the same subtype by construction. `new_column_builder`
clones it into positions 0 and 1 and fills the three flags with `Scalar`.
Likewise `NestedPlan::Multirange(bound)` builds a `Range(bound)` child rather
than the caller having to spell the nesting out.

**A null struct row still appends a null to every child.** Arrow requires a
struct's children to be exactly as long as the struct; a shorter child is an
invalid array, not a compact one. Same for a null list row, which still needs
an offset entry.

**A nested decode failure is attributed to the whole field.** The internal
helpers return `Result<(), ()>` and `append_typed` maps that to the field's own
text, because `Error::FieldDecode`'s `value` is the field's value everywhere
else and `declared_type` names the *column's* type. The alternative — naming
the fragment — reads wrong beside a `declared_type` of `integer[]`.

**`{}` fits any `List` depth.** It is checked before the dimensionality check,
because `array_out` collapses a zero-element array of every dimensionality to
`{}`, so there is no depth it disagrees with.

**Multi-dimensional support is already there.** The `Array` arm recurses one
`List` level per dimension, so a column resolved as `List<List<T>>` works
today even though nothing resolves to one until 4.5's census. `array_depth`
reads the depth off the builder chain rather than off the plan, so the two
cannot disagree.

## What 4.5 inherits

The refusals this slice implements are exactly the optimistic path's failure
mode the census removes: `is_decorated()` and a dimensionality mismatch. When
a census resolves a column to `List<List<T>>` or degrades it to `Utf8View`,
nothing in `batch.rs` changes — the census changes the *plan and type*, and
these arms already handle both outcomes.

## Verification

`batch.rs`'s own tests build every nested shape from a hand-written
`DataType` + `NestedPlan` pair and require the literal to survive
build → render unchanged, including:

- `{}` versus a NULL array versus `{NULL}`, which are three different values;
- `empty` versus `(,)`, which differ only in the range struct's fifth field;
- both both-escape-conventions nesting orders;
- the array-of-range/multirange pair, asserted to produce **identical Arrow
  data** from different text — the collision stated as a test;
- every refusal, checked to report the whole field;
- a `RecordBatch::try_new` over three nested columns, which is what pins the
  built arrays' types to the schema's.
