# P36.8.2 notes — a filter term's operator carries its value

`PredicateOp`'s eight comparing variants hold their literal (`Eq(String)`,
`Lt(String)`, …, `IsNotDistinctFrom(String)`) and the four tests hold none, so
`Predicate` is `{ column, op }`. `IsNull` with a value and `Eq` without one no
longer type-check. `PredicateOp::value()` answers the literal; `takes_no_value`
is gone, a test being the operator whose `value()` is `None`.

## The shape

- **The operator stays one enum.** Its name, `symbol()`, `is_ordering()` and
  `tests_unrepresentable()` read as before. It is no longer `Copy`, so
  `is_ordering` and the rest take `&self`.
- **The resolved term keeps the operator, value and all**
  (`ResolvedTerm::op`, a clone made once a block). The evaluator, the group
  truths and the stop's sortedness test match its tag and never read the
  value, which resolution already turned into the comparison.
- **A front end that reads the operator before its value holds the variant's
  constructor**, `fn(String) -> PredicateOp`. `pgdt`'s grammar names it
  `Comparing` (`query/filter/term.rs`), and `TermOp` no longer derives
  `PartialEq`, which nothing read. The adapter's `compared` maps a DataFusion
  `Operator` to one. `boolean_term` takes the whole operator, `boolean_text`
  spelling a boolean literal, so `IS UNKNOWN` builds `IsNull` with nothing
  beside it.
- **The resume fingerprint is unchanged**: the same tag byte for each
  operator, and the value hashed as `Option<&str>`, which hashes as the
  `Option<String>` field did. A token taken before this slice still resumes.
- **The tests' helpers take the constructor too** (`test_support`'s
  `Comparing`, with `ComparingExt` for a symbol before there is a value), so
  a call site still reads `ordered(…, PredicateOp::Gt, "9", …)`.

## Negative results

- **No value-free operator beside the value-carrying one.** A `Test` enum
  holding a value-free `CompareOp` and its literal also makes the bad pairs
  unbuildable. It adds a public type, and the spec puts the value on the
  operator. Inside the crate, a private value-free copy of the operator for
  the resolved term would mirror twelve variants to save one clone a block.
- **D53's `Rejected` line had to change.** Its reason for refusing an `IN`
  operator was that every operator would then carry a list. That stopped
  being true here: each variant now carries its own value. The line now
  says what still holds. Whether `IN` should become an operator is under
  STATUS's "Decisions worth another look".
- **Not counted.** This is not one of the three reshapes in "What a hot-path
  slice owes". The row path still matches the operator's tag. What the slice
  changed on that path is the resolved term's size: the operator went from
  one byte to a `String` and a tag.

## What the next round inherits

- **36.9**, one record per column in `ResolvedSchema`, is untouched by this
  slice.
