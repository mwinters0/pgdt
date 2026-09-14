# P10.11 — The early stop reported after the fact: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Pruning", its "An early stop is found while rows are read" paragraph; the stop
itself is [the 10.9 notes](roadmap-P10.9-sorted-stop-notes.md). No figure was
taken.

## What exists

- **`TableStream::early_stops`**, a `Vec<EarlyStop>` in file order: one entry
  per block the stream replays with a `SortedStop` planned, pushed as the
  replay starts, so a drained stream tells a block with no stop planned (no
  entry) from one whose stop saved nothing (`unread_bytes: None`).
- **Per block, keyed by `header_offset`, not a count.** A partitioned replay
  hands one block's pieces to several sub-streams, and each piece past the
  stopping row stops at its own first row, so a per-stream block count summed
  over sub-streams counts one block once per sub-stream. `pgdq query`'s
  `announce_early_stops` merges by `header_offset`.
- **`unread_bytes` runs from the end of the stopping row to the piece's
  `limit + 1`**, capped at the block's terminator — a lower bound, short by
  the tail of the one row straddling a piece's limit, which the piece owns
  and the replay never reads to find. It never overlaps a skipped group's
  bytes, so `skipped_bytes + Σ unread_bytes ≤ bytes`. A stop on a piece's last
  row leaves zero and records nothing.
- **`pgdq query` prints one `note: reading stopped early in N block(s) …`**
  after the rows and before the row count, only where some entry holds bytes;
  a failed query prints none, the error being raised first.

## Tests

- `tests/pruning.rs`'s
  `a_sorted_block_is_read_no_further_than_its_first_row_past_the_bound` now
  asserts the report too: serially exactly the bytes from the stopping row's
  end to the terminator; split, fewer but some; `id <= 1000` and `id < 1000`
  (the last row passes the bound) an entry holding `None`; a filter no order
  closes, and `use_statistics: false`, no entry.
- `a_pruned_stop_reports_the_rest_of_its_run_beside_the_skipped_groups`, at
  1024-byte groups under `id < 500`: the report is the run's end minus the
  stopping row's end, and read + unread + skipped falls short of the block's
  rows by less than a group; split, no more than serial.
- The generated check asserts every unpruned leg reports no entry and every
  pruned one keeps `skipped_bytes + unread ≤ bytes`, with a floor on the
  comparisons whose stop left bytes unread.
- The CLI's `query_notes_what_an_early_stop_left_unread_only_where_one_fired`,
  at one and three sub-streams.

## For the slices after

- **10.10.** `statistics-pruning`'s range leg can attribute the bytes the
  process did not read: the pruning note's `skipped_bytes` plus the stop
  note's, the second a lower bound by one row tail per piece.

## Negative results

- **Mutations.** Recording a stop that left zero bytes failed the `id < 1000`
  case; measuring to `limit` rather than `limit + 1` failed the pruned test by
  a byte; keying the CLI's merge by anything but the block failed the CLI test
  at three sub-streams.
- **The CLI's `--jobs 3` legs ran as one sub-stream.** The default budget
  affords one query sub-stream (the decode charge plus a 64 MiB batch span), so
  the merge mutation passed until the test stated `--parallel-memory`;
  `query_skips_the_groups_its_statistics_rule_out_unless_told_none` states it
  too now, and still passes split.
- **Few generated comparisons fire a stop at 32-byte groups**: a row is wider
  than a group, so the stopping row starts in a group of its own, whose bounds
  already skip it, and the kept run ends before it. Most of what the report
  covers is at group sizes wider than a row, which the hand tests take.
