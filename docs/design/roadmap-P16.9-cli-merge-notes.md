# P16.9 — The CLI's k-way merge

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "`pgdq query` merges the sub-streams back
into file order".

## What exists now

**`pgdq query` is the first consumer of `table_stream_partitions`.** It holds
one batch per sub-stream, prints whichever begins earliest in the file, and
refills only the slot it drained — so the reorder buffer is N × batch and a
sub-stream that outruns the printer stops one batch in. `--jobs 1` is
`Parallelism::Serial` is one sub-stream, so the serial path is reached through
the same call rather than by a branch, and there is no second printing loop.

**`TableStream::batch_source_offset` is the merge key**, the fourth value a
stream publishes about itself while it runs. It is `RowBatcher`'s existing
`span` lower bound — the offset of the batch's first row — read at each of
`replay`'s three yield sites before the flush that clears it.

## The calls worth knowing about

**The key is a start, not the end the resume token already carried.** Merging
on `resume_token().offset` needed no new value and orders identically today,
batches of one replay never overlapping. It was refused because it is the
*scanner's* position rather than the batch's: it would go on working by
coincidence until something yielded a batch that did not end where the scanner
stood. The rejected alternative is filed beside the mechanism.

**An empty batch reports the scanner's position instead of `None`.** Only a
`max_rows` of 0 can reach it — every row event then flushes, including one a
filter rejected — and the fallback keeps the published value a plain `u64` that
is monotone within a sub-stream either way, rather than making every caller
handle an `Option` for a case no shipped option set produces.

**The plans travel with the batch; the comparison notes do not.** A held batch
carries the `NestedPlan`s of the block it came from, taken when it was taken,
because its sub-stream may since have moved to a block whose header named other
columns. The notes are announced once off sub-stream **0** — whose first
segment starts at a `COPY` header, so it has resolved a schema whether or not
it had rows — which is the same block the serial path announced from.

**Steady state is one read in flight, not N.** The fill round only polls empty
slots, and after the first round exactly one is empty. So the concurrency the
merge exposes is a burst at the start plus one lookahead batch per sub-stream;
it is an *ordering* mechanism, and the throughput it does not buy is what
16.10's workers are for. The practical consequence worth knowing is the one
that did not happen: 24 sub-streams do not turn a plain-file query into 24
interleaved sequential reads.

**Error ordering is deliberately partial here.** `join_all` answers in argument
order, which is partition order, which is file order — so the failure raised is
the earliest in the file among **one round's** reads. A sub-stream that fails
while an earlier one is still running is not held back for it. That is 16.11's
row, which is why this slice raises the lowest-index failure of a round rather
than either the first to arrive or nothing.

**The parity tests were comparing partitioned against partitioned.**
`parallelism.rs` took its reference from a `query` with no `--jobs`, which is
now this machine's available parallelism — so both sides of the assertion were
partitioned runs and any consistent mis-ordering would have passed. Both
reference runs are now pinned at `--jobs 1`, and a new test asserts the printed
`id` column against the file's own order rather than against another run.

## What the next slice inherits

**16.10's leader has a printer to hand rows to.** The CLI's side of "the
library hands out partitions; the CLI merges them" is done and does not change
when the *cold* path grows workers: the leader's output still reaches the user
through `table_stream_partitions` and this merge, provided each piece it hands
out keeps yielding batches in its own file order.

**16.11 changes the failure path only.** Nothing about the fill round's shape
has to move for it — what it needs is a policy over the *set* of live
sub-streams rather than over one round's answers, which is the `failed`
binding in `Command::Query` and nothing above it.

**Nothing spawns yet, still.** The fills are a `join_all` on the one task, so
`spawn_blocking` is reached only from `io.rs`, and the sub-streams' `'a`
borrow of the source is still what would have to change first for the CLI to
put them on separate tasks.

**No figure changed colour, and the honest thing sayable without an oracle is
narrower than 16.8's.** The diff touches `stream.rs`, `batch.rs` and the CLI,
each of which every timed figure was already stale on. Reachability does not
excuse it — the three yield sites are executed by every registered `query`
shape — and unlike 16.8 the CLI's own path *did* change: `pgdq query` now runs
the partitioned entry point at the machine's available parallelism, so a
`query` figure re-taken today measures a different arrangement than the one the
`af15eac` stamp measured. That is a reason for the red rather than an excuse
against it, and it is stated in `STATUS.md` beside the others.
