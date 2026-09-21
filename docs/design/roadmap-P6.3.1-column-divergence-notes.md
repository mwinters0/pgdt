# P6.3.1 — Each column's divergence, at registration: notes

An earned follow-up to 6.3 in
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "Comparison means what
DataFusion means". `pgdump_query::column_divergences(&ResolvedSchema,
ComparisonSemantics)` returns every column's divergence from PostgreSQL as
`ComparisonNote`s, which `diagnostic::drain` hands to a sink with no term
named. The why is [`decisions.md`](decisions.md), "D59".

## What 6.5 and later slices inherit

- **The report is read off the resolved schema, and 6.5 needs one per
  table.** Today a `ResolvedSchema` comes from a stream
  (`TableStream::resolved_schema`), per block. The provider's registration
  should report over the one schema 6.4 settles, not over the first block's.
  A note names its column and not its table (6.3's notes), so the provider's
  sink adds the table.
- **In PostgreSQL's semantics a column reports what its terms would
  announce**, the union over `=` and `<`. A refused operator adds nothing.
  `a_column_reports_what_its_terms_announce` (`predicate.rs`, `oracle`
  module) holds the report to `resolve_term`'s notes, case by case, on six
  majors.
- **In Arrow semantics the report is about what the column emits, not about
  which terms the library answers.** Six `ComparisonDivergence` variants are
  Arrow semantics' own and never come from the register: `LabelText`,
  `ValueAsText`, `IntervalFields`, `PaddedText`, `NestedArrowOrder`,
  `UnnormalizedZero` (the last two as 6.3.2 left them). A scalar kind's
  variant is `CompareKind::arrow_divergence`,
  beside `arrow_order`, and the Arrow walk checks that one is `Some` exactly
  where the other moves the kind. A collation divergence the register
  announced is kept beside it. `jsonb`'s string collation is dropped, because
  the whole value is compared as text.
- **Every oracle cell Arrow semantics answers unlike the server falls on a
  column that reports it**, under that cell's operator
  (`a_column_reporting_no_arrow_divergence_answers_as_the_server`), which
  6.3.2 extended to nested columns.

## Negative results

- **The equality claims of `ValueAsText` and `PaddedText` are right for
  every kind they cover**, `timetz`, `inet`/`cidr` and `character(n)`
  included: DataFusion compares a user's literal bytewise with the emitted
  text, so `'12:00+00'`, `'10.0.0.1/32'` or an unpadded `'abc'` misses a value
  PostgreSQL would match. The oracle walk cannot show it, its literals being
  the server's own spellings; 6.3.2's unit case pins one.
- **`pgdt` is unchanged.** It still announces per term, in PostgreSQL's
  semantics, and never calls the report.
- **No cache format change.** Nothing gathered or persisted moved.
