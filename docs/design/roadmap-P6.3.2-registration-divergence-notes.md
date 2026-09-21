# P6.3.2 — What Arrow-semantics registration announces: notes

An earned follow-up to 6.3.1 in
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "Comparison means what
DataFusion means", admitted by the review in
[`../status/history/2026-09-21.md`](../status/history/2026-09-21.md), "6.3.2:
what Arrow-semantics registration announces". `column_divergences` in Arrow
semantics now reports a divergence once, where it reaches.

## What 6.5 and 6.8 inherit

- **A nested column reports `NestedArrowOrder` for its order alone**, and
  each position inside it its own divergence under its path
  (`NestedCompare::arrow_divergences`): `numeric[]` reports `ValueAsText` at
  `[]`, `text[]` its element's collation, `public.mood[]` `LabelText`. A
  nested column whose plan has no tree — a range whose DDL stated no subtype,
  one declaring `canonical` — reports its order and nothing under a path.
- **A column that fell back to text reports nothing on the comparison
  channel.** Its `ColumnNote` is a `Warning`, and its `message` now ends by
  saying the value is the file's text and compares as that text. So a dump
  registered under `:strings` warns once per column, through the column
  channel, not twice. `pgdt` prints `describe`, not `message`, so its output
  is unchanged.
- **The text-emitted kinds' sentences name a literal's spelling**
  (`ValueAsText`, `PaddedText`). A unit case pins one:
  `a_literal_not_spelled_as_the_server_writes_misses_in_arrow_semantics`.
- **The oracle walk now answers nested cells as DataFusion does.**
  `a_column_reporting_no_arrow_divergence_answers_as_the_server` builds each
  nested pair with `batch::column_of` and compares them with Arrow's
  `make_comparator`. On six majors every nested disagreement with the server
  is an ordering cell: `int4range`, `numrange`, `integer[]`, `text[]`,
  `public.mood[]`, `public.point2d`, `public.tagged`. No nested equality cell
  disagrees.

## Negative results

- **A float inside a nested column diverges where a float column does not.**
  `make_comparator` compares floats by `total_cmp` and nothing normalizes
  `-0` first, unlike DataFusion's `apply_cmp` on a float column. So `{-0}`
  sorts below `{0}` and is unequal to it. The new
  `ComparisonDivergence::UnnormalizedZero` reports it at the float's path,
  under both operator families. `a_nested_float_s_negative_zero_is_not_zero_to_datafusion`
  pins it against the arrays a batch builds. The oracle holds no float array
  or float field, so the walk cannot see it.
- **`EmittedText` is gone.** A mapped scalar column with no comparison plan
  would report `UnmodelledType` instead, and no declared type reaches that
  arm today.
- **No `D<k>` entry.** The rustdoc on `column_divergences`,
  `NestedArrowOrder` and `UnnormalizedZero` states each shape, and D59
  already covers the channel.
