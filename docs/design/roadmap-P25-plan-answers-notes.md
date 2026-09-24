# P25 — Plan answers from the map's statistics: notes

What the phase built against its spec
([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)) is `STATUS.md`'s
provider row; why each answer has its shape is `decisions.md`, "D89" and "D91",
with "D51" and "D79" for the ordering and the float zeros, and the `timestamptz`
fact the distinct count rests on is `postgres-invariants.md`, "I48". The
oracles are `datafusion-pgdump/tests/statistics.rs`; the fixture shapes are
`scripts/fixture_schema_statistics.sql`, checked on the bytes at every major by
`pgdump_query/tests/statistics_fixture.rs`. What follows is what the code does
not record: what was found not to work, and what a later phase inherits.

## Negative results

- **A lone `COUNT(DISTINCT c)` is never answered from statistics in DataFusion
  55.** `single_distinct_aggregation_to_group_by` rewrites it into a count over
  `GROUP BY c`, whose rows are `Inexact` whatever the distinct count, so
  `aggregate_statistics` sees a distinct aggregate only beside another that is
  not `SUM`, `MIN` or `MAX` of the same argument — a `COUNT(*)`, or a second
  column's `COUNT(DISTINCT)`. The count still reaches every estimate: a
  `GROUP BY`'s output rows (`compute_group_ndv`) and a join's cardinality. An
  enum's and an `interval`'s answer: the count is an `Int64` whatever the
  argument, so `KD45`'s wrong literal type does not arise.
- **A declared ordering is missed in three places, none wrong.** A block
  holding a single value records `Ascending`, so a descending table holding
  one declares nothing; a constant block is not treated as neutral, no fixture
  having one inside a descending table to test that arm. A float block sorted
  only in PostgreSQL's order (`0` then `-0`) records `Unsorted`, which also
  costs pruning's sorted stop that block (`public.zeros.min_pos_first`,
  `sortedness_is_the_blocks_row_order`). And `EXPLAIN` does not show a declared
  ordering — `PgDumpExec`'s display prints partitions and the fetch — so the
  evidence is the dropped sort; the harness reads orderings over unfiltered
  scans only, the proof being one function whatever pruning drops.
- **An extreme can stay `Inexact` where it is exact.** Across groups, an exact
  bound and a clipped one with the same text keep whichever came first
  (`summary.rs`, `Accumulator::fold`), so a clipped one first leaves the
  column's `MIN` read rather than answered. No fixture reaches it, so
  preferring the exact one on a tie was not built.
- **A plan-time bound cannot include a sorted block's early stop**, found only
  as rows are read; and a partition's statistics stay unknown
  (`PgDumpExec` answers `Statistics::new_unknown` for one), though a kept
  group's rows could be attributed to the partition whose segments hold it.
  Nothing asked for either.
- **The join target moved its build side and not its mode.** With bytes
  stated, `ordered` is under DataFusion's collecting threshold, so it is the
  side collected whole (`CollectLeft`), where `long_value` was.
- **The library's file oracle models no non-bytewise `character` column.** A
  blank-padded column's own bounds are unpadded only under a bytewise
  collation; under the database's it diverges and its only set is Arrow's,
  padded (`ComparisonPlan::bounds_kinds`). The fixture's `padded` and `bare`
  are `COLLATE "C"`.

## What a later phase inherits

- **Two answers rest on upstream behaviour with no invariants entry**, the
  blind-session and estimate oracles being what re-checks each at a DataFusion
  or arrow upgrade. A `Decimal128` sum matches a read because DataFusion 55's
  `SUM` returns `Decimal128(min(38, p+10), s)` and adds with `add_wrapping`
  unchecked, and arrow-cast 59 widens a same-scale decimal without validating
  (`cast_decimal_to_decimal_same_type`); a cast that began validating leaves
  the sum unanswered, not wrong (`public.spans.huge` wraps `i128` across three
  blocks). An enum's byte bound holds because `StringDictionaryBuilder` keeps
  only the labels appended since its last `finish`, so a batch's dictionary is
  bounded by its values' text; a builder keeping labels across batches fails
  the estimate target, which measures each batch's dictionary.
- **P23: the statistics a pass holds grew twice without a figure reading
  them.** A summed column adds an `i128` a group and every tracked column a
  `u64` a group of text bytes, and `CACHE_FORMAT_VERSION` moved three times;
  `statistics-gathering` and `statistics-pruning` are red for it
  (`measure.py --stale`), and what the heavier cache costs either is unpriced
  beside what P23's sketch already owes them.
- **P27: a declared ordering and an enum.** An enum column declares its order
  in label text, its Arrow set, which is what Arrow's comparator orders a
  `Dictionary` by; `KD45`'s bounds in declaration order do not reach it. The
  dynamic filters P27 consumes would meet the same label order.
