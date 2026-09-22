# P6.8.1 — A plan's notes at `scan()`, a scan's counts as metrics: notes

Earned after 6.8 landed, by the review
([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "The
binary's sink reports at planning, and a scan's counts are metrics"). The
checks are `datafusion-pgdump/tests/scan_reports.rs` for the provider,
`datafusion-cli-pgdump/tests/cli.rs`'s
`a_scan_s_warnings_reach_stderr_when_it_is_planned` for the binary, and
`stream.rs`'s `a_plan_note_warns_where_the_budget_declined_what_was_asked`
for the severity.

## What the wrap inherits

- **`PlanNote` is the fourth `Finding`.** Its `message` moved from an
  inherent method into the trait, as `ComparisonNote`'s did at 6.3, so a
  caller reading it imports `pgdump_query::Finding`. Its severity is derived
  from the kind: `Info` for `StatisticsPruned` and `BatchSpanNarrowed`,
  `Warning` for the three a budget declined. `pgdt` reads it back and still
  prints an `Info` as `note:`, so its output did not change.
- **The table keeps the sink it was reported to.** `PgDumpTable::report` now
  takes an `Arc<dyn DiagnosticSink>` and keeps it, under the subject it was
  given; `scan()` hands the plan's notes to it once `TablePartitions::plan`
  has settled them. `register_dump` takes the sink by `Arc` for the same
  reason, as the factory already did. A table registered by hand and never
  reported plans silently, and one reported again reports to the latest
  sink.
- **Every plan note is drained, `Info` included.** The stderr sink drops
  `Info`, so the binary prints a scan's warnings only. An embedder's sink
  also hears the pruning note, which says what the metric counts.
- **Two metrics on `PgDumpExec`, beside the streaming exec's own.**
  `row_groups_pruned_statistics` is a `PruningMetrics` under Parquet's name,
  set once for the whole node at `scan()`, because that is when the library
  prunes. `bytes_unread_early_stop` is a per-partition `Count`, added when a
  partition's stream ends or is dropped. A `LIMIT` drops partitions before
  their end, and what they had stopped by then still counts. Both are
  `Summary`, so `EXPLAIN ANALYZE` shows them at every `analyze_level`. A plan
  executed twice counts its early stops twice and its pruning once.

## Negative results

- **`EarlyStop` does not implement `Finding`.** The spec makes early stops
  a metric, and nothing drains them into a sink. `pgdt` still sums them
  after the query in `announce_early_stops`.
- **The provider adds no provenance clause to a note that quotes a budget.**
  See STATUS, "Decisions worth another look".
- **`bytes_unread_early_stop` shows `0` wherever no stop was planned.** A
  per-partition counter is registered for every partition that runs. The
  pruning metric appears only where statistics were consulted.
- **The binary's test leans on the host reporting a memory figure.** A
  `--memory-limit` beyond the allowance leaves the scans nothing only where
  `ScanBudget::discover` found an allowance. A host reporting neither a
  limit nor `MemAvailable` would plan without a warning.
- **No `D<k>` entry.** Each shape is in the rustdoc beside it and the spec
  holds the decisions. No figure was taken and no cache format moved.
