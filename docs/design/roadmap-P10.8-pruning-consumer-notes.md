# P10.8 — The pruning consumer: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Pruning", "When a statistic is believed" and "The correctness check"; the call
on what a skipped group raises is `decisions.md`, "D54". No figure was taken.

## What exists

- **`prune.rs` (L4)** answers one block: `prune_block` walks its groups through
  `ResolvedExpr::truths` over a `GroupStatistics` view of `BlockStatistics`, and
  returns the runs of kept groups as segment search bounds. A column's bounds
  and dictionary are believed only where its recorded `declared_type` and
  `collation` equal what the metadata declares now (`gather::declared_columns`
  is the one lookup both sides use); its NULL counts always. Statistics that do
  not fit the block — more groups than its data holds, a column count other
  than its header's — prune nothing.
- **`ReplayPlan` settles pruning once**, beside `plan_blocks`: `kept` holds the
  runs of each block that skipped at least one group, and `pruned` the
  `PlanNoteKind::StatisticsPruned` note. Nothing is pruned under
  `QueryOptions::use_statistics: false`, for a filter that reads no field, or in
  a block with no column list.
- **A pruned replay is a segment list with gaps.** `Segment::over` enters the
  run holding group 0 at the header and every other inside the data; the serial
  `table_stream` replays the runs, and `plan_partitions` cuts each run into a
  share of the block's worker count proportional to its bytes, so `distribute`
  balances what remains.
- **A block whose every group is skipped keeps an empty run on its header's
  LF**, which reads the header line and no row; and an interior piece holding
  no row now publishes its block's schema and comparison notes before it is
  skipped. Both keep a sub-stream's `resolved_schema` and `comparison_notes`
  what the unpruned query's are, which `pgdq query`'s warnings read off the
  first sub-stream. The second changes the unpruned split too: a sub-stream
  handed only cuts inside one row used to report the empty schema.
- **`TableStream::plan_notes` returns a `Vec`**, the serial stream filling its
  pruning note once the plan is settled at the first poll; a partitioned
  sub-stream holds its notes when handed back, the pruning note after the
  budget's.
- **Resuming**: `Segment::resumed_at` drops a segment whose rows all precede
  the token and enters one begun before it on the paused row's LF; `replay`
  hands the paused scanner only to a segment holding the pause, and drops the
  paused block state otherwise, every row between being in a skipped group.
  `use_statistics` is outside the fingerprint (`decisions.md`, "D50").
- **`pgdq query --statistics <all|none>`**, `all` the default; the note prints
  as `note:`, the budget's notes staying `warning:` with their origin.

## Tests

- `tests/pruning.rs`'s `every_fixture_prunes_to_the_rows_it_returns_unpruned`
  is the spec's check: every fixture gathered at 32 bytes, terms under all ten
  operators with literals from stored bounds, dictionary entries, their
  neighbours and the oracle, random trees, at one worker and at three; rows,
  schema and comparison notes identical. The unpruned leg reads a cache holding
  no statistic, which loads far faster in a debug build; the switch alone is
  pinned by the hand tests. One thread per major; its floors count the
  comparisons, the ones that skipped something and the unpruned errors
  tolerated.
- Hand tests: an ascending `id` under `>=` skipping exactly the groups whose
  maximum is below the bound; `low_card = bravo` skipped by the dictionary
  alone; relabelled `declared_type` pruning nothing; a pruned stream resumed at
  every batch; and a recording source showing no read starts deep inside a
  skipped run, split and resumed, a pause past a run's end included.
- `stream.rs`'s `a_segment_resumes_from_the_row_its_pause_left`; the CLI's
  `query_skips_the_groups_its_statistics_rule_out_unless_told_none`.

## For the slices after

- **10.9.** The early stop reads `ColumnBounds::sortedness` and cuts inside a
  kept run; the plan already holds each block's runs and its resolved filter,
  and a stop inside a run is a smaller `limit`, which `resumed_at` and the
  split's cut both take as they are.
- **10.10.** `statistics-pruning` times `pgdq query` against `--statistics
  none`; the note's skipped bytes are the process's own count of what pruning
  saved, for attribution beside the timing.

## Negative results

- **Mutations.** The group's search start off by one byte, the empty run
  dropped, an empty piece's schema left unpublished, the paused scanner handed
  to any segment, `resumed_at` dropping a segment at its limit or entering at
  the pause itself, the declared type ignored, a run's first split piece
  entered at its first data byte, `<=`'s true side narrowed in `bounded`, and
  every seventh group skipped regardless: each failed a test here. The paused
  scanner mutation passed until the recording test paused on the last row
  starting in a group before a gap.
- **The generated check is slow in a debug build**, most of it loading the
  32-byte gathered cache of the `statistics` fixture once per query; per-major
  threads, the plain cache for the unpruned leg and for resolving terms, and
  two literals per operator are what keep it inside a test run. A term is
  resolved before it joins a tree, so no tree holds a refusal; an unpruned
  query that still raises at a row is tolerated rather than compared, and the
  floor keeps those under one in a hundred.
