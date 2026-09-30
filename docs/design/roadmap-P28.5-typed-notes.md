# P28.5 — The mode option and the typed mode: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decision is `decisions.md`, "D98".

## What exists

- **`QueryOptions::unrepresentable`**, an `UnrepresentableMode` of `Null`, the
  default, and `Refuse`; `PgDumpOptions::unrepresentable`,
  `pgdump.unrepresentable` on `STORED AS PGDUMP`, the shell's
  `:unrepresentable=null|refuse` suffix and `pgdt query --unrepresentable
  null|refuse`. **No `Text` variant exists**: 28.7 adds it with the widening,
  and the flag, the option and the suffix accept `text` then. The mode is in
  the resume fingerprint.
- **Which tiers a query cannot hold follows its semantics**
  (`QueryOptions::unrepresentable_reach`): the format spec's under
  PostgreSQL's, the calendar's too under Arrow's. `statistics_view` is
  `Representable` or `Displayable` from it under the null mode and typed, and
  `Every` otherwise, `SchemaMode::Strings` included.
- **`unrepresentable::UnrepresentableRead`** is one column's reading, built per
  block by `unrepresentable_reads` from the typed, census-free resolution the
  count used, `None` for a column read as `Utf8View`, and under the null mode
  for one whose block counts nothing in the tiers read. `PlannedBlock` and
  `DynamicBlock` hold one per field; `RowBatcher::push_field` appends NULL
  before decoding, and `ResolvedExpr::reading` hands each leaf its column's,
  so `eval_value` — rows, a sorted stop, a dictionary entry, a literal —
  answers the value as NULL. A block the plan does not hold (`activate`'s
  fallback) is tested throughout.
- **Under the refuse mode a value the reading calls past its tier and that
  failed to decode is `Error::Unrepresentable`**, from the batcher and from a
  filter leaf alike; one that does not parse is still `Error::FieldDecode`.
  An engine-tier value decodes, so the refuse mode still hands DataFusion a
  date it cannot print, as before.
- **`PlanNoteKind::ReadAsNull`**, a `Warning` per materialized column holding
  such a value, with the map's count over every block in the tiers read:
  `pgdt` prints it, the provider reports it at each scan's plan. It does not
  name the predicate term, which 28.8 adds.
- **The summary and the declared orderings read the query's view**:
  `table_summary` and `partition_orders` take a `StatisticsView`; NULLs are
  the view's, a dictionary entry the view nulls is left out of the distinct
  count, and a column holding a value the view nulls is declared in no order.
  **A sum leaves a `NaN` out** (`gather`, `CACHE_FORMAT_VERSION` 33), and
  the summary reads it only in a view taking `NaN` as NULL or where no group
  holds one.

## What the next slices inherit

- **The harness's typed mode is green but for `KD56`**: the two unfiltered
  `MIN`/`MAX` pairs miss where rows are evaluated and no statistics answer,
  DataFusion's aggregate filter having lost its `MIN` side to a batch holding
  no value of the column. Every other case passes in the typed mode; the
  untyped mode opens the refuse mode until 28.7, and both keep today's
  records.
- **The harness's partition check admits one partition**: typed-mode pruning
  of `v_time > '12:00:00'` keeps one partition's worth of `t_extremes`, which
  has one order.
- **28.6's planning refusal replaces `Error::Unrepresentable` at read time**,
  which is where its wording naming the modes lives now.

## Negative results

- **Nulling what fails to decode is not the typed mode**: an engine-tier value
  decodes, and text that does not parse refuses in every mode, so the test is
  the count's, before decoding.
