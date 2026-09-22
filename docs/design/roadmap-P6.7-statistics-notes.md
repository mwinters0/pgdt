# P6.7 — Statistics handed to DataFusion: notes

A slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "Statistics
handed to DataFusion". A scan's plan node now carries what the dump's map
says about the table, so `COUNT(*)`, `COUNT(<column>)`, `MIN` and `MAX` can
be answered without reading a row. The why is [`decisions.md`](decisions.md), "D89".

## What 6.8 inherits

- **The library folds a table's statistics; the provider only types them.**
  `pgdump_query::table_summary` (`summary.rs`) walks every block of a table
  and every group of its stored set, choosing the set exactly as
  `prune_block` does — the kind a term compares by in the asked-for semantics,
  the kinds gathering stored for *that block*, and the DDL-believed check
  (D78, D79) — and returns each column's NULLs and extremes as one-element
  arrays of the column's Arrow type. `pgdump_query::decode_field` is
  `render_field`'s inverse, added for it, so a stored bound reads back through
  the one decoder a scan emits through.
- **A summary says whether it is complete, never how nearly.** A group whose
  every row is NULL contributes nothing and leaves the column complete; a
  group that lost a value to gathering does not. A block with no statistics
  leaves every column incomplete, its rows counted all the same.
- **`PgDumpExec` is a leaf that holds a `StreamingTableExec`.** DataFusion
  reads statistics off the `ExecutionPlan`, so the streaming exec could not
  stay the top node; making it a *child* would let an optimizer rule replace
  it under statistics taken for something else, so it is held privately and
  `children()` is empty. The cost is `StreamingTableExec`'s own
  `try_swapping_with_projection`, which a projection above the scan no longer
  reaches — the library already projects, so nothing is read that was not.
- **A filter or a fetch makes everything `Inexact`**, which no answer is read
  from: `AggregateStatistics` reads `Exact` alone. The filter is folded in at
  `scan`; the fetch is read off the node when its statistics are asked for,
  because it arrives by two roads — `scan`'s `limit` argument and
  `with_fetch` — and folding it in at either would leave the other wrong.
  Both are pinned on the plan rather than through SQL
  (`a_fetched_or_filtered_plan_states_no_exact_statistic`): DataFusion keeps a
  limit node of its own above the scan, which hides a scan that answers
  wrongly.
- **The summary is taken on the first scan, not at `PgDumpTable::new`.** A
  catalog builds a table for every `SHOW TABLES` row, and folding every
  block's groups is not what that should cost; a `OnceLock` holds it, the map
  being resident and unchanging.

## Negative results

- **A lower bound is never `Exact` for a text-ordered column**, so `MIN` over
  every `Utf8View` and `Binary` column is read rather than answered. The
  clipping flag exists while gathering (`gather.rs`, `MIN_WHOLE`) and is
  folded away at `finish`, the stored shape recording exactness of the upper
  bound alone. `KD39` carries it; the fix is a field beside `max_exact` and a
  `CACHE_FORMAT_VERSION` bump, which this slice did not make — an unattended
  session does not move the persisted shape for an optimization.
- **An enum's bounds are withheld.** DataFusion types a `MIN`/`MAX` of a
  `Dictionary` column as the dictionary's *value* type
  (`get_min_max_result_type`), and `aggregate_statistics` does not check the
  schema it rewrites, so a statistic in the column's own type would plant a
  literal of the wrong type. `KD39` again.
- **A value the Arrow type cannot hold takes its column's bounds with it, and
  not its counts.** `date`'s and `timestamp`'s infinities and a typed
  `numeric`'s `NaN` (`KD8`) are extremes, so they land in the bound, fail
  `decode_field`, and leave the column `Absent` at both ends —
  `t_date.v_date`, `t_timestamp.v_ts` and `t_numeric.v_small` hand over no
  extremes. Its NULL count is read off the text and stays `Exact`, so a
  `COUNT` of such a column answers where reading it refuses. The spec's
  verification item was amended to that rather than the statistic withheld
  ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "A
  statistic answers where reading the column refuses"); D54 gives up the same
  error when pruning skips the group that holds the value.
- **`character(n)` is unaffected after all.** Its Arrow-ordered set is keyed
  `Text` and holds the padded field text, which is what the column emits; the
  unpadded set is the PostgreSQL one, and the guard against reading it is for
  a caller asking for that semantics, not for the provider.
- **No `TableProvider::statistics()`.** There is none in DataFusion 55; the
  spec's reading of the upstream source held.
