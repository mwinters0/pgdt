# P31.15 — A `numeric(p,s)` field put through its typmod: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the entry it closes was
`KD81`.

## What exists

- **`decode::typmod_unscaled_digits` is a `numeric(p,s)` field's reader**:
  rounded to the scale half away from zero, then refused past `p` digits, as
  `apply_typmod` does (I51, marked `pg-refuses: I51`). Every field path reads
  through it — the typed read, a group's sum, an ordering key and so a
  statistics bound, a nested leaf — on the typed arm and on the text-held arm
  (`p > 76`, or a scale no Arrow decimal carries), where
  `NumericKey::from_unscaled` normalizes the result.
- **A literal is read exactly**, by `decode::decimal_unscaled_digits` on the
  typed arm and `NumericKey::parse` on the text arm (`literal_key`): the
  server coerces it with no typmod (I63), so `1.005` is still refused on a
  `numeric(10,2)` as finer than the scale, keys as itself against a
  `numeric(100,2)`, and one past the precision compares.
- **The precision travels in `NestedPlan::Decimal { precision }`** beside the
  Arrow type (D39), because `Decimal(max(p, s), s)` loses it where the scale
  exceeds it: `0.001` in a `numeric(2,5)` fits `Decimal128(5, 5)` and the
  server refuses it. `NestedPlan::is_scalar` answers for both leaf variants,
  and every `== NestedPlan::Scalar` test in the workspace now asks it; a plan
  stating none, a caller's own schema, is bounded by the Arrow precision.
  `CompareKind::Decimal` carries `{ precision, scale }`, and
  `CompareKind::Numeric` a `typmod: Option<NumericTypmod>` whose scale is
  `numerictypmodin`'s `i16` range, so `numeric(1,200)` now keeps its typmod.
- **A text-held column emits the file's text unchanged** and compares as the
  rounded value; the manual's `numeric` section says so.
- **`CACHE_FORMAT_VERSION` is 46**: such a field now keys and sums, and one
  past its precision loses its group's bounds. Neither pinned digest moved.
- **Evidence**: `decode.rs`'s
  `a_field_is_put_through_its_typmod_as_copy_puts_it` and
  `a_field_numeric_out_wrote_reads_as_its_literal`; `predicate.rs`'s
  `a_numeric_field_is_rounded_and_bounded_by_its_typmod_and_a_literal_is_not`;
  `tests/decode.rs`'s
  `a_numeric_field_is_rounded_to_its_scale_and_refused_past_its_precision`.
  Every numeric case was cast or `COPY`ed on the koji replica (PG16); no
  oracle file was regenerated, every fixture's field being `numeric_out`'s.

## Findings

- **Filed `KD82`, (c) unowned**: `=`, `!=` and `IN` on a canonicalized kind
  compare the field's spelling, so a hand-written field the decoder reads in
  another — `1.005` in a `numeric(10,2)`, and before this slice `1.0`, or a
  braced `uuid` since `M209` — is missed where a typed read takes its value.
  Not of P31's class, `pg_dump` never writing such a field.
- **The decoder bench's `numeric` group now times the field reader**,
  `typmod_unscaled_digits`, which is what a typed read calls.

## What the slices after this inherit

- **31.12 turns the precision refusal into a parse abort** with the others.
  On the text-held arm the typed read never decodes a field, so what refuses
  one at parse is gathering keying it for its bounds; whether that counts as
  the parse decoding it is 31.12's to settle against "a parse refuses what it
  decodes".
