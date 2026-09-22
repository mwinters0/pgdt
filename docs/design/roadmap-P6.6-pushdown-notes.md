# P6.6 — Filter pushdown: notes

The sixth slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md).
`PgDumpTable` now answers `supports_filters_pushdown` and hands its `Exact`
filters to the library as one conjunction. The translation is
`datafusion-pgdump/src/pushdown.rs`; why a literal is the library renderer's
text and a filter is `Exact` exactly where the plan resolves it is
[`decisions.md`](decisions.md), "D88". The checks are
`datafusion-pgdump/tests/pushdown.rs`. No library code changed.

## What 6.9, 6.7 and 6.8 inherit

- **Every `Exact` answer is checked against DataFusion itself.**
  `a_pushed_filter_keeps_the_rows_datafusion_keeps` walks every major's
  `default.sql` of every schema. Each column's own values are the literals,
  under all eight comparing operators, either side first. Each pushed scan
  must keep exactly the rows the physical expression a `FilterExec` would run
  keeps from the unfiltered scan. The statistics fixture is parsed with every
  statistic, so pruning and early stops run in Arrow semantics under a real
  pushed filter. A later change to Arrow semantics, or to what 6.9 gathers,
  fails here if it moves a row.
- **Every scalar type the walk reaches pushes, and no nested comparison
  does.** The walk floors the pushed count for each type. `IS [NOT] NULL`
  pushes on every column, nested ones included. It reads no value.
- **What DataFusion hands a provider, read off 55.1.0's optimizer**
  (`sql_pushes_down_what_the_library_answers_and_answers_alike`):
  - a literal's cast onto the column's type is unwrapped, so `v_integer = 0`,
    `event_date < '2024-06-01'` and `v_typed = -1.5` against a `numeric(38,10)`
    arrive as column and literal of the column's own type;
  - an `IN` list of three or fewer becomes `=` terms before it arrives;
  - a comparison the literal cannot be cast for keeps a cast around the
    column (`v_integer > 0.5`) and is not pushed.
- **The registration notes' nested claims are checked through DataFusion.**
  `a_nested_column_s_arrow_notes_are_datafusion_s_comparison` runs
  `compare_op_for_nested`: a NULL element first, and a list's `-0` below `0`
  and unequal to it. It runs on arrays of the type an `integer[]` and a
  `real[]` column emit, the fixtures holding no `real[]`.
- **The scan's `filters` are re-translated, never trusted to be `Exact`.** A
  filter that does not translate was answered `Unsupported`, and DataFusion
  keeps it above the scan.

## Negative results

- **A long `IN` list on a float column is not pushed.** DataFusion answers it
  from a set of the values and makes no `-0` into `0` first. A short list
  arrives as `=` terms and pushes.
- **`Timestamp(µs)` with no zone is not in the walk.** The only fixture
  column of that type holds `infinity` on every major (`KD8`), so its
  unfiltered read has no oracle. `timestamptz` is walked, and both kinds share
  one decoder and one renderer.
- **A literal that renders to text the decoder reads as another value is
  refused by type, not found by the walk.** Such literals are a `NaN` with
  another sign or payload and a negative `Time64`. The walk's literals are the
  column's own values, so it never builds one.
- **No `D<k>` pruned, no manual page, no figure, and no cache format
  change.** The provider's page is 6.8's.
