# P6.12.1 — A plan note names only the levers that move it: notes

An earned follow-up to 6.12, built by the amendment to
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "Diagnostics: one sink"
([`../status/history/2026-09-23.md`](../status/history/2026-09-23.md), "Which
keys a budget-quoting plan note's clause names"). The checks are
`pgdump_query/src/stream.rs`'s `a_plan_note_lists_only_the_levers_that_move_it`,
the lever assertions added to `pgdump_query/tests/partitioned_replay.rs`'s
floor and compressed-decline tests, `datafusion-pgdump/src/report.rs`'s two
unit tests, and `tests/scan_reports.rs`'s floor, which names no chunk key.
The user's text is
[`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md),
"What it says on stderr".

## What the wrap inherits

- **`PlanNote` has a second public field, `levers: Vec<PlanLever>`**, in
  `PlanLever`'s order: `LargerAllowance`, `SmallerReadChunk`,
  `FewerSubStreams`. A caller building a `PlanNote` by hand now has to supply
  it. `StatisticsPruned` always has an empty list.
- **`plan_partitions` computes the levers from the source and the budget.**
  `Parallelism::allowance_raises_budget` sits beside `fit` and shares its
  "recommends nothing" rule, and `Partitioning::sized_by_read_chunk` reads
  `PartitionRead::Chunked`. The plan asks the source for its
  `default_worker_memory`, the same recommendation both callers carve with,
  so the library says nothing about who carved the budget or how (D64).
- **The lever lists follow each sentence, filtered to this source and
  budget.** The count note lists the allowance where it raises the budget,
  and the chunk where the reader is chunk-sized or a span was charged; that
  span is always at its chunk floor when the note fires. The floor note lists
  the allowance where it raises the budget, and the chunk where the reader is
  chunk-sized and the budget is above zero. The span note lists the allowance
  where it raises the budget, plus fewer sub-streams. The decline lists the
  allowance.
- **Only `AllocationBelowFloor`'s sentence changed.** It now names a larger
  memory budget with its siblings' caveat, and the chunk exactly where its
  levers list one. `pgdt`'s wording otherwise stands, and `pgdt` reads no
  levers.
- **The provider maps levers to keys and nothing else.** The pool's key comes
  only with the allowance's, and a note listing no lever says that no
  setting moves it.

## Negative results

- **The first scan after `SET pgdump.chunk_size` still prices the old
  chunk** (`KD41`). So the chunk lever, and the floor note's cap
  arrangement, follow the chunk the source was last told. A lever's presence
  does not depend on the chunk's size, but whether a floor fires at all can.
- **`BatchSpanNarrowed` lists no chunk lever, though a chunk-sized source's
  span would widen under a smaller one.** This follows the row, which changed
  only the floor note's sentence, against the spec's rule; `M134` adds it.
- **No `D<k>` entry.** The spec holds the decision, and the rule is in the
  rustdoc on `PlanNote::levers` and `BudgetedPlanNote`. No figure was taken
  and no cache format moved.
