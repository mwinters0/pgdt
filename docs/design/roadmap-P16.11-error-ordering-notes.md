# P16.11 — Error ordering: the lowest-offset error is the one raised

What the next slice inherits from the change that made a parallel failure
report the same byte a serial one does.

## Two sites, one rule, and they are not symmetric

The phase has two places where several readers can fail at once, and the spec
row names one of them while `STATUS.md`'s partitioned-replay entry named the
other. Both landed here, because they are one rule — *index order is file
order, so raise the lowest index* — but the mechanisms differ, and the
difference is the thing to carry forward:

- **`leader::run_region` (the mapping pass's window).** A window's pieces are
  dispatched together and consumed together, so ordering is a *collection*
  problem: swap `futures::future::try_join_all` for a
  `futures::stream::FuturesOrdered` drained with `?`. Same concurrency, same
  polling, results in argument order.
- **`pgdq query`'s k-way merge (the replay).** A sub-stream is a long-lived
  producer, so ordering is a *state* problem: the round that meets a failure
  cannot know whether an earlier sub-stream will fail later, and the earlier
  one may need many more rounds to find out. So the failure is recorded and the
  merge keeps going.

Anything else that grows a second concurrent reader falls into one of those two
shapes. If the readers are consumed in one batch, `FuturesOrdered` is the whole
answer; if they outlive a round, the merge's bookkeeping is the pattern.

## What the merge's bookkeeping actually is

Three lines of state, and the third is the one that is easy to get wrong:

1. `failed: Option<(usize, Error)>` — the lowest-indexed sub-stream that has
   failed, and its error.
2. `live` — `failed`'s index, or `slots.len()`. Only slots below `live` are
   refilled, so the sub-streams at or after a failure are never polled again.
3. Every slot from `live` up is set to `Slot::Done` **at the top of each
   round**, which discards a batch such a slot was holding.

(3) is why recording a failure does a `continue` rather than falling through to
the merge, and it is the load-bearing half rather than a tidy-up. The first
round fills *every* slot, so when sub-stream `i` fails the ones after it are
routinely holding a batch — they have simply never been printed, the merge
always having a lower-indexed batch to print first. Left in their slots those
batches would print the moment the sub-streams before `i` drained, which is rows
past the error a serial replay never reached. Looping back marks them dead
before the printer next picks.

Termination is not an argument about progress: a recorded failure strictly
lowers `live`, and a new failure can only come from a slot below `live`, so the
`continue` path runs at most N times.

The error is raised **after** the merge loop breaks, not when it is recorded, so
the rows before it print exactly as the serial path prints up to the row it dies
on.

## The tests are the deliverable, and both had to fail first

The spec row says "asserted, not documented", and the assertion is worth more
than usual here because *both* sites pass a naive test by accident. Each test
was run against the pre-change code and shown to report the wrong row:

- `the_lowest_offset_error_is_the_one_a_split_region_raises`
  (`pgdump_query/tests/map_file.rs`). A `FailsOutOfOrder` source fails every
  read at or past the block's `data_offset`, and makes the read starting
  *exactly* at `data_offset` yield eight times before it fails. Under
  `try_join_all` the second piece's failure is returned (`read failed at 94`);
  under `FuturesOrdered` the first piece's is (`read failed at 30`). The yields
  make that deterministic rather than a race — nothing sleeps.
- `the_lowest_offset_error_is_the_one_the_merge_raises`
  (`pgdump_query-cli/tests/parallelism.rs`). A generated 40,000-row dump with
  `zzzEARLY` at row 18,000 and `zzzLATE` at row 22,000. At `--jobs 2` the byte
  midpoint of the data region falls near row 20,900, so `zzzLATE` is in
  sub-stream 1's *first* batch while `zzzEARLY` is three batches into
  sub-stream 0. The old merge named `zzzLATE`; the new one names `zzzEARLY`, as
  `--jobs 1` always did.

**The row-count margins in the second test are the fragile part.** It asserts
only that the earlier value is named, which is correct at any split — but the
test stops *discriminating* if a change to `cut`/`distribute` puts both bad rows
in one sub-stream, or if `max_rows` (8192) rises above sub-stream 0's row count.
Nothing checks that, so a slice that moves either should re-derive the two row
numbers rather than trusting a green run.

## What did not change

- **No library API moved.** `table_stream_partitions` still hands back N
  sub-streams and still says nothing about which of them may fail; the ordering
  is the *caller's*, and `pgdq query` is the caller. An embedder scheduling the
  sub-streams itself gets whatever order it arranges — which is the same
  division `16.9` drew for file order, and P6 is where what the embedded API
  promises is decided.
- **Nothing spawns**, on either side. The leader's window futures and the
  merge's fill futures are still polled by the one task that owns them, so
  "drop the pieces after the failing one" is a drop and not a cancellation
  protocol.
- **The `MayWait` argument is untouched.** A dropped piece releases its buffer
  the way a finished one does, and a worker still holds exactly one read at a
  time.

## Figures

`leader.rs` is declared by no figure. `pgdump_query-cli/src/main.rs` is
declared by eight, and every one of them was already red on it. The leader hunk
is unreachable from every registered command shape — all of them state
`--jobs 1` since `M69`, which makes `scan_region` decline before `run_region`
is called — and the CLI hunk is reachable but is one comparison per slot per
fill round on the no-error path. No figure goes red that was not already.
