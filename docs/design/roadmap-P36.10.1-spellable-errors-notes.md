# P36.10.1 notes — Errors a front end can spell

Every remedy `pgdt` spells is now visible in a variant's fields, none of it
riding in a `String`. `UnrepresentableTestUntyped` is generalised to
`pgdt_core::Error::TestNeedsDeclaredType`, which refuses at planning any term
the strings schema mode cannot answer, with the typed schema mode as its remedy.
`PredicateValueDecode` carries `unlisted`, the count of an enum's labels its
accepted form left out. `pgdt` spells the two as `--schema-mode typed` and
`pgdt info --detail`. The back-fill's declined-statistics line now names the
statistics allowance. D107 is rewritten to match.

## The shape

- **Which terms the strings schema mode refuses is decided by the operator**:
  `PredicateOp::reads_declared_type(semantics)` covers an unrepresentable test
  always, and an ordering operator in PostgreSQL's semantics only. In
  DataFusion's semantics the text is ordered bytewise, as it was before
  (`pruning.rs`'s `Strings, DataFusion` rows pin that). `query_tests` raises
  the refusal for the first such term, using `Expr::first_term`, which replaces
  `first_unrepresentable_test`.
- **`NOT_MAPPED` loses its aside.** In a query it now fires only for a
  typed-mode column whose declared type did not resolve, and no mode answers
  that, so it names no remedy. Per-block resolution still refuses the
  strings case too, but a query's plan refuses it first.
- **The overflow clause belongs to the error.** `pgdt_codec::internal::unlisted_labels`
  counts the labels past `ENUM_LABELS_SHOWN`, and `accepted_form` lists only
  the labels it shows. `PredicateValueDecode`'s sentence ends ", and N more,
  which the type's declaration lists" exactly when `unlisted` is non-zero, and
  `pgdt` spells only that case.

## Negative results

- **The strings mode's refusal now comes before filter resolution.** Under
  PostgreSQL's semantics, `nosuch > 0` is refused as `TestNeedsDeclaredType`
  rather than `UnknownPredicateColumn`, as `nosuch IS UNREPRESENTABLE`
  already was. This refusal names no `header_offset`, where the per-block one
  named a block. Reviewed and reversed: 36.10.2 checks names first
  ([2026-10-19](../status/history/2026-10-19.md), "A misspelled column is
  refused before what it asks").
- **Not counted.** The added walk runs once per block at planning and touches
  no row path.

## What the next round inherits

- **36.11's shell spelling** matches over these variants. `TestNeedsDeclaredType`'s
  remedy in the shell's grammar is a `--dump` without `:strings`.
  `PredicateValueDecode` with `unlisted > 0` points at `pgdt info --detail`,
  which is the same binary.
