# Phase 4.4.1 — The presentation half: notes

What 4.5 inherits. `pgdq info --verbose` now states the whole Arrow schema,
and `docs/manual/type-handling.md` describes the world 4.4 created. The
mechanism is in [`architecture.md`](architecture.md), "CLI surface"; these
notes are the rest.

## The manual documents two ways a column is still a string, not three

The spec's 4.4 manual bullet lists three — opaque element type, arrays that
vary in shape, and `--schema-mode strings`. Only two of them are true today:
without a census, a column whose arrays vary in shape is not returned as text,
it is a **`FieldDecode` error**, which the same section states two paragraphs
later. Writing the third would have made the manual contradict itself on the
page.

So the section says "Two ways", and **4.5 turns it into three** — that is the
slice that makes "varies in shape → text" true, and the spec already gives
4.5 the manual paragraph where it belongs ("a column whose arrays genuinely
vary comes back as text instead"). Renumbering that heading is part of 4.5's
manual work, not a leftover.

## `--verbose`'s two arms cannot both fire, and that is load-bearing

The per-column loop prints the resolution label when a column did not map, and
the Arrow type when it did and the type is not `Utf8View`. Those arms are
disjoint because **every non-`Mapped` `ColumnResolution` resolves to
`Utf8View`** — `UnknownType`, `NotDeclared`, `OpaqueElementType`,
`OpaqueBaseType` and `EmptyEnum` alike. If a future resolution ever maps a
column to a real type *and* records a non-`Mapped` outcome, this loop silently
drops the type. Nothing enforces it; the comment at the loop says so.

The loop indexes `resolved.schema.field(i)` and `resolved.plans[i]` against
`resolved.notes`' position, which is what `ResolvedSchema`'s three parallel
vectors already promise.

## `arrow_type_label` lives in the CLI, not in L2

It is display code — the same call `resolution_label` is, and it sits beside
it in `pgdump_query-cli/src/main.rs`. L2 (`pgtype.rs`) produces the
`(DataType, NestedPlan)` pair; how a human reads it back is above L4.

The one thing it imports from the library is `pgtype::RANGE_STRUCT_FIELDS`,
and only for its **length** — the arm that collapses the range struct checks
the field count, never the names, because dispatch is the `NestedPlan`'s job
(a user composite may declare five fields called `lower`, `upper`, … and must
still print as the `Struct` it is; `tests::a_composite_wearing_the_range_structs_field_names_is_not_collapsed`
pins it).

**A plan/type disagreement falls back to arrow's `Display` rather than
panicking**, unlike `batch.rs`, which panics on the same disagreement. The
pair has one producer so neither can happen; the difference is what the two
would cost if it did — a builder that proceeds writes wrong values, while a
display that proceeds prints a longer true type.

## The CLI now has unit tests

`pgdump_query-cli/src/main.rs` grew its first `#[cfg(test)] mod tests`. There
is no CLI integration harness and this slice did not build one: the
rendering is a pure function over a `(DataType, NestedPlan)` pair, so it is
testable directly, and the wiring around it (which columns get a line) is
verified by eye against `fixtures/16/types/default.sql`, which holds every
family. A future CLI-output slice that wants golden output has `insta`
already in the workspace.

## `--json` is now further from `--verbose` than its own help text claims

`--json` refuses `--verbose` on the grounds that the struct "already carries
everything `--verbose` would add". That was already imprecise — `DumpIndex`
carries no `ResolvedSchema`, so per-column resolutions were never in it — and
this slice widens the gap by putting the resolved Arrow type on the same line.
The manual now says so plainly (`docs/manual/dump-inspection.md`). Nothing in
the CLI changed: the flag combination stays rejected, and the pooled
"per-column resolution has no machine-readable path" entry under `STATUS.md`'s
**Not started** is where the fix belongs, because the shape of a
machine-readable answer is the open question, not whether one is wanted.

## What 4.5 inherits

- **The `Range<T>` elision is only lossless because the manual states the
  struct's layout.** `docs/manual/type-handling.md` carries that table now; a
  census slice that rewrites neighbouring prose must not lose it.
- **The manual's "Two ways one of these columns is still a string" heading is
  4.5's to renumber**, per the first section.
- Nothing about the census changes what `arrow_type_label` renders: a
  censused column arrives as an ordinary `(DataType, NestedPlan)` pair —
  `List(List(Int32))` prints from arrow's `Display`, and a degraded column is
  `Utf8View` and prints no line at all. The **reason** it degraded is a new
  `ColumnResolution::VaryingArrayShape` and therefore a new
  `resolution_label` arm, which is the only CLI change the census needs.

## Verification

- `pgdump_query-cli`'s five unit tests: arrow `Display` passes through for a
  scalar (`Decimal128(38, 10)`), a list and a composite; the range struct
  collapses; a multirange and an array of the matching range render
  identically; and a composite wearing the range struct's field names does
  not collapse.
- `pgdq info --source fixtures/16/types/default.sql --verbose` prints
  `Range<Int32>`, `Range<Utf8View>`, `List(Range<Int32>)`,
  `Struct("x": Int32, "y": Utf8View)`, `List(Struct(…))`, `Struct()` for the
  zero-field composite, `List(Dictionary(Int32, Utf8))` for the enum array,
  and no line for `numeric` or `text`.
- `cargo test --workspace` (278 tests), `cargo clippy --workspace
  --all-targets` and `cargo fmt --check` are clean. No library code changed,
  so nothing else could regress.
