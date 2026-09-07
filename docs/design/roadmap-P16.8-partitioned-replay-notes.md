# P16.8 — Partitioned replay

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "Partitioned replay".

## What exists now

**`table_stream_partitions` — the first consumer `ByteRangeSource::partitions`
has had.** One map, N sub-streams, each a `TableStream` and each internally in
file order; the caller runs them. `Parallelism::Serial` is one sub-stream, so
the serial path is reached as a property of the value rather than by a branch.
Library only — `pgdq query` still calls `table_stream`, and the k-way merge
that lets the CLI use this is 16.9.

**It is an `async fn` returning `Vec<TableStream>`**, where `table_stream` is a
synchronous constructor whose whole body is lazy. That is forced: the mapping
pass has to finish before there is anything to split, so the prologue cannot
stay inside a generator. Every error `table_stream` reports as its stream's
first item — `AmbiguousTable`, `ScanCancelled`, `CacheSourceMismatch`,
`DuplicateProjectionColumn` — this one returns from the `await` instead.

**One replay implementation, two entry points.** `stream.rs` now has
`map_for_query` (pass 1 whole) and `replay` (pass 2 over a `Vec<Segment>`), and
`table_stream` is `replay` over one segment per matching block. So "running the
partitions sequentially *is* the serial path" is a property of the code rather
than a claim about it, and the equality test cannot pass by the two paths
agreeing on a bug in only one of them.

## The calls worth knowing about

**A `Segment`'s two offsets are not a byte range.** `start` is where the
*search* for the first row begins; `limit` is a line the piece reads *through*,
not one it stops before. The rules and the two edge cases they settle — a row
straddling a cut, two cuts inside one row — are in the architecture section.
What matters for the next slice is that a cut needs to know nothing about rows,
which is the whole reason a source can advise cuts at block boundaries.

**The resync is a real forward read, not a scanner started one byte early.**
Starting the scanner at `start - 1` and discarding its first row costs no read
and is unsound: in `InCopy` state every line is a row, and a row's tail can be
exactly `\.` — a value ending in an escaped backslash, cut between the two
bytes — which `is_terminator` would fire on. I7's guarantee is about a line
*start*; its "Relied on by" line now names this mechanism.

**`hint_parallelism` is announced before `partitions()` is asked.** A
compressed source's answer is read off whether it can afford to decode a whole
block, which is a function of the stated budget — so asking under the mapping
pass's budget plans against a read path the replay will not take. This is the
first caller for which the two budgets differ in practice.

**The bytes bind on the sub-stream count, not on each block's cut.** Capping a
*block's* pieces at `memory_bytes / partition_bytes` and then giving one
sub-stream per piece multiplies the allowance by the block count, since the
sub-streams are what run at once. So `worker_count` is computed once, over the
largest footprint any matched block advised.

**Grouping is contiguous, which was a choice against a simpler one.**
Round-robin balances at least as well and is shorter; it gives up file order
across the returned `Vec`, and with it the strongest statement available about
the split — that concatenating the sub-streams *is* the serial stream. The
rejected alternative is filed beside the mechanism.

**A sub-stream's resume token is stamped with its partition**, so feeding one to
`table_stream` is `Error::ResumeQueryMismatch` rather than a silent superset of
the rows that partition had left. Resuming a partitioned replay is not
supported and this is what says so out loud.

**Three call sites became one.** `activate` is the shared body behind a `COPY`
header the scanner read, the first row of a headerless block, and a piece that
took the header off the map. The `pending` arm now takes the field count from
the header when it has columns rather than always counting delimiters — a no-op
for the live paths, which only ever set `pending` for a headerless block, and
required for the interior one.

## What the next slice inherits

**16.9 has partitions to merge.** Each sub-stream yields batches in file order
and the sub-streams themselves are in file order, so a k-way merge on source
offset holding one batch per partition is exactly the arrangement the spec
names. `RecordBatch` carries no source offset, so the merge key has to come
from somewhere — the batch's first row's offset is not on the batch today, and
that is the one piece of plumbing 16.9 has to invent rather than inherit.

**A sub-stream's `resolved_schema()` is empty until its own first block
resolves.** The CLI prints a header before the first batch, so it reads the
schema off the *first* sub-stream, whose first segment starts at a `COPY`
header. A caller that reads it off an arbitrary partition gets the default
`ResolvedSchema` until that partition has produced something.

**Nothing spawns yet.** The sub-streams are `Send` and the tests drive them
interleaved through `join_all`, but no thread is created and
`tokio::task::spawn_blocking` is not reached from here — that is 16.10's
worker, and it is also what first grants a `WaitPolicy::MayWait`.

**The chunk pool's depth is still `POOL_DEPTH`.** 16.7.1 left it there because
nothing ran concurrently; N sub-streams polled at once is the first real
holder set, and the number is now measurable against something. It was not
changed here, because this slice adds no figure and the change belongs beside
one.

**No figure changed colour, and one honest thing can be said without an
oracle.** The diff touches `stream.rs` and `lib.rs`; every figure declaring
`stream.rs` was already stale on it. Reachability does not excuse the change —
`stream.rs`'s replay loop is executed by every registered `query` shape — but
`table_stream`'s own path through it is the same segment list it had before
(one per matching block, `limit` at `end_offset`, so the `past_limit` check
never fires), and `measure.py` invokes no partitioned query.
