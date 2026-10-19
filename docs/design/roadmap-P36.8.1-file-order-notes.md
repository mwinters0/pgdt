# P36.8.1 notes — the plan's whole table in file order, and its serial resume

`TablePartitions::in_file_order(max_rows)` runs every partition of a plan at
once and merges them on each batch's source offset into one stream in file
order (`stream/merge.rs`), holding one batch per partition and raising the
lowest-indexed failing partition's error after every row before it (D52).
`TablePartitions::resume(token, max_rows)` replays the rest of the table
serially from a token such a stream handed back. `read_table` is gone. The
slice split: the cold path is 36.8.4
([history, 2026-10-19](../status/history/2026-10-19.md), "36.8.1 splits: the
plan's streams land, the cold path is 36.8.4").

## The shape

- **The merge is `pgdt query`'s, lifted**: the same slots, rounds and
  discard-past-the-failure, over `TableStream<'static>`s. It is a function
  over a plan's sub-streams and a `StreamShared`, so `TablePartitions` hands
  it `stream(i, max_rows)` for each partition and nothing in the merge knows
  a plan.
- **The merged stream's token is the whole table's.** It takes the emitting
  partition's token at the batch and restamps it with the fingerprint no
  partition carries (`query_fingerprint(…, None)`) and the merged row count.
  A partition's token resumes nothing, as D51 has it.
- **Resume is serial whatever the plan's cut**: the plan's own segments per
  block (`ReplayPlan::segments`), each `resumed_at` the token, as
  `table_stream` resumes. So a token from a four-way plan resumes on a serial
  plan of the same query and the reverse. Its plan notes are the plan's own,
  not the cut's: one stream reads them.
- **Schema and comparison notes are published before the first poll**, from
  the plan's first block, every block of a table resolving the same
  (`TableColumns`). A sub-stream still publishes them on entering its first
  block.
- **Early stops are merged per block**, unread bytes summed across the
  partitions that read pieces of it; the dynamic-filter counts are summed.
  Republished at each yield and at the end.
- **`read_table`'s tests and bench drain `table_stream`.** Its push-mode
  `Break` test went; the resume-after-one-batch test reads the token off the
  stream instead.

## Negative results

- **The merge is written twice until 36.8.4**: `pgdt query` still merges the
  sub-streams `table_stream_partitions` hands back, and the concept map's
  "The replay" carries a **Not in one place** line for it. Moving `pgdt` onto
  the plan's stream needs its cold path on a `Table`, which is 36.8.4's.
  `KD57`'s marker stays on `pgdt`'s merge, the one that ships.
- **A resumed stream is not re-partitioned.** Cutting the rest of the plan
  again from the token would need a cut starting inside a block at a token's
  in-copy state; the spec asks for resume on the serial stream only.
- **A failing-source test needs the source's cut advice.** A wrapper that
  inherits `partitions`' default declines to be split, and the plan is then
  one partition at every worker count — so a merge raising the first failure
  in time passed. `tests/file_order.rs`'s `FailsWithin` delegates it; that
  mutation and dropping the discard both fail the test.

## What the next round inherits

- **36.8.4 moves the cold path onto the `Dump`**, everything 36.8.1's row
  named but the plan's streams and `read_table`: the early-stopped pass
  yielding an owning `Table` (a crate-private constructor from the parts,
  per 36.8.3's notes), `max_rows` on the stream, the selector, `pgdt query`
  through `CacheMode::open` and onto `in_file_order` — its `Slot` merge gone,
  `KD57`'s marker moving to `stream/merge.rs`, the **Not in one place** line
  struck, and its printer reading the nested plans off the stream's schema,
  known before the first poll.
