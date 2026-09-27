# P27.3 — The library trait and re-pruning between groups: notes

What the slices after this one inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Scope".
No row is filtered by a dynamic filter yet: what one drops is whole row groups
and a sorted block's tail, found by asking the stop's terms of the rows of a
group that can hold the bound.

## What exists

- **`pgdump_query::DynamicFilter`** (`stream.rs`): `generation()` and
  `current()`, the state as the library's `Expr` beside the generation it is
  the state of. `TablePartitions::stream` takes one as its third argument, the
  only entry point that does, and the contract is stated on the trait.
- **The replay asks it as each row group is entered**, never per row
  (`DynamicRead::at_row`): where the generation moved it re-reads the state and
  resolves it against the block (`resolve_loosened`, a term the block refuses
  standing as its parity says, so no state refuses a query). A group the state
  rules out ends the segment at the group's search start, and the rest of the
  segment is re-entered at the next group it keeps as an interior piece
  (`Segment::rest_from`) queued ahead of the others — so each skip costs a
  batch flushed early and one read of the search, which is what re-entering
  through the path every partition piece already takes buys.
- **`prune::DynamicPruning`** holds one block's statistics for this: each
  group's verdict reached only when the replay reaches it and forgotten when
  the state moves, and the block's `SortedStop` recomputed per state. That stop
  is **armed per group** (`DynamicPruning::arm`, whose rustdoc says why):
  asked of every row, kept or not, of a kept group whose statistics say a row
  there can pass it, and of no row elsewhere — the one place this slice
  evaluates a state per row.
- **`TablePartitions` shares each matched block's statistics** with the
  caller's map where the query uses them; `table_stream` and
  `table_stream_partitions` still hold none past planning
  (`a_replay_holds_no_block_s_statistics`).
- **What it did is counted per sub-stream**:
  `TableStream::dynamic_filter_pruned_groups`, a group counted by the
  sub-stream whose piece holds its search start where that sub-stream skipped
  it; and a stop at a dynamic bound adds its block's `EarlyStop` in file order
  where no static stop planned one (`record_unread`).
- **The provider's side** (`datafusion-pgdump`): `ReplayFilter` is the held
  filters' conjunction, each `loosened`, its generation the wrapping sum of
  theirs, translated again only when that moves. `PgDumpExec` builds its
  `StreamingTableExec` again over the same planned `Replay` whenever the
  filters it holds change — pushdown or reset — since a sub-stream's filter is
  fixed when it is built. Each partition registers
  `row_groups_pruned_dynamic_filter`, Parquet's name, beside
  `bytes_unread_early_stop`, with or without a filter held. The manual's
  "What a scan skipped" paragraph names both and what `predicate=` prints.

## Negative results

- **Re-pruning the whole block on each moved state** (`prune_block` per
  generation) is refused: a TopK tightens at nearly every boundary, so it
  would evaluate every group at every boundary. Verdicts are reached lazily,
  each group once per state the replay reads there.
- **DataFusion's `DynamicFilterTracker`** answers "moved?" with one atomic
  load, but `changed(&mut self)` is a consumer's own subscription: shared by a
  scan's sub-streams it would need a lock around the tracker, where
  `snapshot_generation` reads each filter under its own read lock.
- **Handing a partition its filter through state shared with the plan node**
  is refused: clones of a `PgDumpExec` share their streaming exec, so a reset
  node would keep handing out the filter its producer discarded.
- **Seeking the scanner in place at a skip** is refused: `ChunkCarry` and
  `RetainedChunks` assume one contiguous read per segment, and the interior
  entry every partition piece takes already re-enters a block mid-data.
- **Asking the dynamic stop in every kept group**, as the first version did,
  is refused: pruning already skips every group past the bound, so it paid a
  per-row evaluation over every group before the bound's to save at most the
  rest of that one group — row-level cost 27.5 has not priced.

## Tests

- `pgdump_query/tests/dynamic_filter.rs`, on `ordered` at 1 KiB groups over
  every major: a constant `id >= 990` emits exactly the groups from the one
  holding 990, serial and split alike, each skipped group counted once; with
  statistics off, or a state keeping everything, nothing is skipped; a state
  moving after three groups is read from the fourth, asked once per group
  entered; `id < 5` stops at the fifth row, and split its later pieces are
  pruned; an unresolvable term keeps every row under either parity.
- `stream::tests::a_dynamic_stop_is_armed_only_where_a_row_can_pass_it`:
  under `id < 150` over ascending ids, of the groups the state keeps only the
  one whose maximum reaches the bound is armed.
- `datafusion-pgdump/src/dynamic_filter/tests.rs`, **the generated check**,
  gains a dynamic leg: every generated filter, handed to three sub-streams as
  a `ReplayFilter` from the start or once the first batch is out, loses no row
  DataFusion keeps and emits none the table does not hold as often.
- `datafusion-pgdump/tests/dynamic_filters.rs`: the flags-on/off sweep fails
  unless more than ten queries prune a group and more than two stop a block
  their static filter alone did not; a join of `moods` into `ordered` prunes
  and stops, shows under `EXPLAIN ANALYZE`, and with the flags off does
  neither.
- **The floors on counts** — those two, and the generated check's share of
  filters pruning and count of replays stopping — sit below what today's
  fixtures give, so a fixture change can move them without a defect.
- Five mutations each fail one of them: the skip landing one kept group late,
  a refused term's parity flipped, every skipped group counted by each piece
  touching it, the state asked at every row, and the stop armed in every kept
  group. What this slice's pruning exposes at a float's zeros, and the test
  that shows it, is
  [`roadmap-P27.2.1-float-zero-notes.md`](roadmap-P27.2.1-float-zero-notes.md)'s.

## For 27.4

- **The cut is still made at `scan()`**, over the static filter's groups: a
  selective join on a clustered key leaves every sub-stream but one holding
  only groups its state rules out, which each skips as it enters them — the
  imbalance 27.4's cut removes. The first poll's state is `current()`, and a
  whole block's verdicts under it are `prune_block`'s loop over
  `Believed::keeps`.

## For 27.5

- **The harness met a dynamic stop only from a join's bounds**: a TopK over a
  column declared sorted the way it asks is planned as a limit, and asked the
  other way its threshold closes no bound the block's order does.
- **`--stale` holds both dynamic-filter figures red on this slice's paths.**
  The join's clustered row is the one whose on leg this slice's pruning and
  stop reach — a probe's groups past the build side's bounds skipped, its
  sorted block stopped at their maximum; what that is worth is the re-take's.
