# P10.3 — The truth-set evaluator: notes

What the later P10 slices inherit from this one, and what it did not do. The
spec is [`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Pruning" and "Dictionaries". Nothing a query returns changed: no caller outside
a test evaluates a group yet.

## What exists (`predicate.rs`)

- **`ResolvedExpr::truths(&impl GroupStatistics) -> TruthSet`** is the whole
  evaluator; `ResolvedTerm::truths` is its leaf. The consumer skips a group
  exactly when the root's set lacks `Truth::True`.
- **`GroupStatistics` is the input, a trait defined in L4**: `rows`, then per
  column — numbered by the block's unprojected column list, as a term's index
  is — `null_count`, `bounds` and `dictionary`, each optional. 10.4's
  statistics types are read through an implementation of it, so their shape is
  not fixed here. Bounds and dictionary entries are unescaped field text.
- **A bound only has to be on the right side.** The evaluator never reads one as
  a value, so an inexact `max` is read exactly as an exact one and needs no flag
  here. It is still `&str`: a truncated `max` whose incremented last byte ends
  a multi-byte character is not valid UTF-8, and 10.4 decides how to keep it
  text.
- **Which statistics a term believes is settled at resolution**, in
  `BelievedStatistics` on `ComparedTerm`: bounds only where the column's plan is
  `Compared` with no divergence, carrying the literal's key for the equality
  operators too — the "kind per term" 10.2's notes named, so `PlannedBlock` is
  unchanged; a dictionary only under the four equality operators, on a
  `Compared` column whose divergence does not reach equality. That leaves 10.8
  to check only the recorded declared type and collation.

## Tests

- `truth_sets_combine_as_their_members_do` — every pair of sets under
  `And`/`Or`/`Not`, against the row evaluator's own answers over real terms.
- `a_group_is_answered_from_its_statistics` — one hand case per rule.
- `oracle::a_groups_truths_hold_every_rows_answer` — seeded random groups over
  every value the committed oracle holds for every declared type, six majors,
  under random terms and trees: every row's answer is in the set, with the
  statistics weakened (a bound loosened to another value on its side, a count
  or a dictionary missing); and a single term over exact statistics answers
  exactly its rows' set. Its floors keep both halves from being vacuous; two
  hand mutations of `bounded` (`<`'s false side off by one, `>=` ignoring the
  bounds) each failed it.

## For the slices after

- **A row whose value does not decode is outside the contract**, since no
  statistic records one: a skipped group raises nothing, and gathering leaves
  such a value's column without bounds and `Unsorted` (spec, "Pruning" and
  "What is gathered, and what is derived"). A stored bound that does not key
  reads as no bound.
- **Terms combine as if independent**: `v < 5 AND v >= 5` is not ruled out on a
  group straddling 5. Early stop on a sorted block (10.9) is not in the
  evaluator.
- **The `statistics` fixture is not used here**: nothing gathers yet, and the
  oracle values are the population whose keys carry the semantics that matter
  (`1.5`/`1.50`, `-0`, `NaN`, padded `character`).

## Negative results

- **Equality over bounds rules out only "equal"**, never "unequal". Bounds equal
  to the literal say every value keys to it, and `Canonical`/`Trimmed` compare
  spellings, one per key only in text `*_out` wrote. On `pg_dump` output the
  wider reading would be sound; the case it gives up — `!=` on a constant group
  — is one a dictionary answers exactly unless the value is past the cap.
- **A dictionary does not answer the ordering operators**, though it would do so
  soundly — its entries are answered through `eval_value` — and would reach the
  text columns bounds skip (`KD7`). The spec names the four equality operators.
- **No property-testing crate**: SplitMix64 in the test, seeded, so a failure
  reproduces.
