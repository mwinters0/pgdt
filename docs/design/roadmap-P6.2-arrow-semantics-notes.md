# P6.2 — Arrow-semantics comparison mode: notes

The second slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md). A
query now carries `QueryOptions::semantics`, and `ComparisonSemantics::Arrow`
answers every comparing operator in Arrow's order of the value the column
emits. The mode itself is `pgtype.rs` (`CompareKind::arrow_order`,
`ComparisonPlan::bounds_ordered_in`, `ComparisonPlan::dictionary_answers_in`)
and `predicate::resolve_term`. The why is
[`decisions.md`](decisions.md), "D40".

## What 6.3 and later slices inherit

- **Every scalar kind now has an Arrow-mode answer, checked against the
  kernels.** `arrow_semantics_answers_as_arrow_s_kernels` (`predicate.rs`,
  `oracle` module) runs all eight comparing operators over every committed
  oracle value, each value in turn the literal, on six majors, and compares
  the answer with `lt`/`eq`/`gt` on the array a batch builds. Three kinds were
  added, and no register arm produces them: `Float32Total`, `Float64Total`
  (IEEE `totalOrder`) and `IntervalFields` (months, days, time, one after
  another). The six kinds emitted as text or as a dictionary compare as
  `Text`: `Enum`, `Numeric`, `TimeTz`, `Network`, `Jsonb` and `PaddedText`.
- **Arrow semantics refuses every comparing operator on a nested column**,
  with `Error::UncomparablePredicateColumn`. That includes `=`, which
  PostgreSQL's semantics answers structurally or as text. A column with no
  plan still answers `=` bytewise, which is Arrow's `=` over the `Utf8View` it
  emits. Its ordering operators stay refused, as in PostgreSQL's semantics.
  The refusal is raised at plan time, before a row. So for 6.6, a
  `supports_filters_pushdown` that asks the library whether a term resolves
  in this mode gets `Unsupported` for every such term. See "Decisions worth
  another look" in STATUS.
- **An Arrow-semantics term announces no divergence.**
  `TableStream::comparison_notes` is empty for it. Divergence from PostgreSQL
  is reported per column at registration (6.3), and nothing per term remains
  to drain there.
- **Which stored statistics hold in Arrow's order.** Bounds and row order are
  gathered once, at parse, under the register's order. They are read in
  Arrow's semantics only where `arrow_order` leaves the kind unchanged:
  `ComparisonPlan::bounds_ordered_in(ComparisonSemantics::Arrow)`. That is
  the predicate 6.7 needs for `Exact` against `Absent` column bounds. A
  dictionary holds wherever it was gathered, except `character(n)`'s, whose
  entries are stored unpadded. The generated check in `tests/pruning.rs` now
  also runs its trees in Arrow semantics and floors how many it compares,
  prunes and stops.
- **Collated text has no Arrow-mode bounds.** Gathering takes no bounds for a
  column whose PostgreSQL order diverges (D79), and every bare `text`,
  `varchar` or `character` column diverges (`UnknownCollation`). So in
  Arrow's semantics only a `COLLATE "C"` column or a `name` column prunes an
  ordering term by its bounds, and 6.7 has no `Exact` text bounds to hand
  DataFusion. See "Decisions worth another look" in STATUS.
- **The resume fingerprint covers the semantics**
  (`tests/batch.rs`, `a_resume_token_covers_the_whole_conjunction`).

## Negative results

- **A value the emitted type cannot hold still answers in Arrow's
  semantics.** Examples are a `date`'s `infinity`, a `numeric(p,s)`'s `NaN`
  and an `interval`'s infinities (`KD8`). The value keeps its rank, so a
  filter can drop a row whose value would raise `Error::FieldDecode` when
  built. With the filter pushed down, such a query can therefore answer where
  the same query with the filter left to DataFusion raises. This is D54's
  existing property: a decode failure surfaces only where evaluation reaches
  it. 6.6's "pushdown on and off answer alike" check has to project only the
  columns that decode, as `tests/pruning.rs` already does.
- **No cache format change.** No existing kind orders or equates differently,
  and the three new kinds are never gathered, so `CACHE_FORMAT_VERSION` and
  the golden order stand.
