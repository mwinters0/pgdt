# P31.10 — A float spelling past `DBL_MAX`: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The session-setting axis"; the entry it closes was `KD72`.

## What exists

- **`decode_f64` and `decode_f32` read a spelling in digits that overflows
  the type as its largest finite value of that sign**, `finite_spelling` in
  `decode.rs`: `*_out` writes an infinity only as `Infinity` (I57), so a
  finite spelling is a finite value, and the nearest one is `±DBL_MAX`.
  `real` gets the same rule, though no fixture reaches it: `float4out` rounds
  `FLT_MAX` past itself only at `--extra-float-digits` of `-2` or less.
- **Every reader of a float goes through those two**, typed read, ordering
  key, literal and statistics bound alike, so nothing else changed.
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it: the
  persisted index moved, a statistics bound being chosen by the ordering key
  this reads.
- **Evidence**: `value_oracle.rs` holds `extra-float-digits-0`'s
  `t_extremes.v_double` to the server's `DBL_MAX` at every major, and
  `datafusion-pgdump/tests/statistics.rs`'s
  `statistics_never_change_an_answer` sweeps that fixture like any other;
  unit test `decode.rs`'s
  `a_finite_spelling_past_the_largest_finite_value_reads_as_it`.
- **The exclusions went with the entry**: `known_failures.rs` lost its `KD72`
  row and, with it, its only `FiniteStaysFinite` case; `statistics.rs` lost
  its known-failure table, which held only that row, and
  `scripts/test_deficiencies.py`'s `KnownFailureTables` no longer lists it.

## Findings

- **The value oracle needed no tolerance for the fix**: the text is past
  `DBL_MAX` by less than half a unit of its fifteenth digit, so `DBL_MAX` is
  within the precision `Floats::Digits` already allows, and is in fact the
  oracle's bits exactly.
- **Such a dump does not restore that value**: `float8in` refuses the
  spelling as out of range (I57's scope limit). pgdt reads what the writer
  held, not what a reload would.
