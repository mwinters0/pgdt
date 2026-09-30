# P28.8 — The predicate term and its function: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decision is `decisions.md`, "D101", with "D53" amended.

## What exists

- **`PredicateOp::IsUnrepresentable` and `IsNotUnrepresentable`**, taking no
  value (`PredicateOp::takes_no_value`), spelled `IS [NOT] UNREPRESENTABLE`
  by `pgdt --filter` and every `--where` leaf, `KD52`'s suffix matching
  covering them as it covers `IS [NOT] NULL`.
- **The test is the block's, handed to the resolved tree after resolution**
  (`ResolvedExpr::testing`), as the null mode's reads are (`reading`): per
  column, `unrepresentable::unrepresentable_tests`, the declared type's tier
  test in the query's reach, whatever the mode and the column's resolution —
  so a widened column is tested as its declared type, and a column only a
  filter reads under the refuse mode is tested too. `None` where the block's
  count says the column holds none, the term then `False` on every row.
  `stream::query_tests` builds it only where the filter holds a test, and
  refuses one under `SchemaMode::Strings` (`Error::UnrepresentableTestUntyped`).
- **A group answers the test off its own count** (`GroupStatistics::unrepresentable`,
  the believed column's `ColumnStatistics::unrepresentable_in`), never off its
  NULL count, which the null mode's view inflates by the values the test
  matches. `pgdump_query/tests/pruning.rs`'s generated check draws both tests
  on every column.
- **A dynamic filter's state never holds a test**, no producer stating one,
  so `resolve_loosened` loosens one to whatever keeps every row rather than
  hand a block a test it never computed.
- **`PlanNoteKind::ReadAsNull` names the term**, in its front end's spelling,
  from a `semantics` field it gained.
- **`datafusion-pgdump`'s `pgdump_unrepresentable`** (`PgDumpUnrepresentable`,
  `UNREPRESENTABLE_FUNCTION`), translated by `pushdown::translate` over a bare
  column of any type, nested included; registered and guarded as
  [28.11's notes](roadmap-P28.11-guard-notes.md) say. A filter holding it that
  translates and does not resolve is `supports_filters_pushdown`'s error, the
  library's refusal carried whole.

## What the next slices inherit

- **Nothing 28.9 times reads the term**: no figure's filter holds one.

## Negative results

- **The null mode's NULL count cannot answer the test**, in either direction:
  a group of NULLs under that view may hold nothing but such values.
