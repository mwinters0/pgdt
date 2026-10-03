# P31.10 — A float spelling past `DBL_MAX`: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The session-setting axis"; the entry it closes was `KD72`.

## What exists

- **A spelling in digits that overflows the type is never read as an
  infinity**: `*_out` writes an infinity only as `Infinity` (I57). It is
  refused, as `float8in` refuses it (I59;
  [31.12's notes](roadmap-P31.12-field-refusal-notes.md)). `real` goes the same way,
  though no fixture reaches it: `float4out` rounds `FLT_MAX` past itself only
  at `--extra-float-digits` of `-2` or less.
- **The exclusions went with the entry**: `known_failures.rs` lost its `KD72`
  row and, with it, its only `FiniteStaysFinite` case; `statistics.rs` lost
  its known-failure table, which held only that row, and
  `scripts/test_deficiencies.py`'s `KnownFailureTables` no longer lists it.

## Findings

- **The text is past `DBL_MAX` by less than half a unit of its fifteenth
  digit**, so `DBL_MAX` is the oracle's bits exactly, within the precision
  `Floats::Digits` allows.
