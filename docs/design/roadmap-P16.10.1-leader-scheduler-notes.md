# P16.10.1 — The leader's scheduler

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "The interior split".

## What exists now

**`leader::scan_region` is the scheduler over `16.10`'s two functions**, and
nothing calls it. It is handed a source, a `ScanOptions`, the header's offset
and width, and the file's size; it answers the block's totals as the serial
scanner would have stated them, a `Declined` that leaves the region to the
serial scanner, or a `Cancelled`.

**The row was split, and this is the half that landed.** `16.10.1` paired a
self-contained new scheduler with a rework of `stream::map_forward` — the loop
every `parse` and every query's first pass runs through — which is the seam
`../process.md`'s "Size a slice by its review, not by its scope" names. The
wiring is `16.10.2` ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md),
"16.10.1 split again: the scheduler, then the loop that runs it").

## The calls worth knowing about

**A worker is two reads, and the second is not an optimization to remove.** The
first is the piece exactly, `[start, end)`, which on a block-decoding source is
one whole block and therefore a zero-copy slice of it. That read can *never*
finish the piece — `scan_piece`'s contract is that the piece owns the line
ending at the first LF at or after `end`, which needs bytes past `end` — so a
second, chunk-sized read follows and is scanned as an ordinary contiguous piece
beginning at a row boundary. The alternative considered was one read of
`[start, end + overrun)`: on a block source that is a block plus a chunk copied
into a fresh chunk-pool buffer, which is a memcpy of the whole block and
double the footprint the source advertises. The two-read shape is what makes
`partition_bytes` = *a block plus a chunk buffer* the true number rather than an
aspiration.

**Several `PieceScan`s per worker, not one.** The tail is an ordinary piece, so
the caller concatenates and merges exactly as it does across workers and
`merge` stays the only fold. That is also what makes the growth path — a row
longer than the tail read — cost nothing structural: it is one more piece.

**`PieceScan::next` was added because two cases are indistinguishable from
outside.** A piece that consumed a line boundary leaves `through` just past an
LF; a resyncing piece that found no LF leaves it at the end of the bytes it was
given, mid-row. Both can report the same `(rows, through)`, and a continuation
that guessed wrong would hand the scanner the mid-row byte the resync exists to
prevent. It is additive to `16.10`'s type and its tests were unaffected.

**The decline floor is checked against what is left of the file.** The region's
extent is not known at the header — finding it *is* the work — and the region
cannot exceed the rest of the file, so that is the bound available. At the
shipped 1 MiB chunk this declines every `COPY` block any fixture has, which is
why the tests announce a 64-byte read size: the source's advised partition is
its own slot size, so a fixture-sized block is one partition at the default and
there is nothing to schedule.

**Cancellation is answered between windows, not between pieces.** It is the
same argument the mapping loop's per-chunk check makes: a window is the
shortest boundary inside a `COPY` block that a stop can be answered at, and a
region can be hundreds of gigabytes.

**A window that consumes nothing stops the loop.** That is a truncated dump —
the file ended inside a line — and it is the only thing between such an input
and a scheduler re-reading the same window forever. It is a check rather than
an argument for exactly that reason.

**`stream::cut` and `stream::worker_count` became `pub(crate)`.** One cut rule
and one affordability rule for the two arrangements, on the same argument that
made `map::census_row` shared: two copies are two rules free to drift. The cold
path and the cached one differ in *what* they cut, not in where a source
permits a cut or how many readers the caller can afford.

**The wait is granted here and taken back here.** `scan_region` is the one read
loop that states `WaitPolicy::MayWait`, which is safe because a worker holds
exactly one read at a time — the `Bytes` moves into the `spawn_blocking`
closure that parses it and drops when that closure returns. It restores
`NeverWait` before returning, unlike the three top-level loops, which state
their own policy and leave it stated: this one runs *inside* one of them, and
an enclosing loop reading on under a permission it never granted is the failure
that restoration prevents.

**The wait genuinely blocks in the tests.** At eight jobs the local source
advises eight workers, and its pool clamps to `POOL_DEPTH` slots — so four
workers hold a slot and four block. Instrumenting `BufferPool::obtain` shows
328 blocked acquisitions across
`a_scheduled_region_answers_what_the_serial_scanner_answers`, all at
`charged=4` against `slots=4`, and the run completes. That is exposure rather
than coverage — the wait's contract is still the bare-pool test's — but it is
the first time a read loop's grant has been exercised at all.

## What the next slice inherits

**`16.10.2` is the wiring, and the shape it needs is settled.** In
`map_forward`'s `CopyStart` arm, after `builder.on_copy_start`, offer the region
to `scan_region`. On `Closed`, the block's census goes onto the `Builder` and
its `CopyEnd` through the same path a serial one takes — the splice, the
throttle's save, the target check — and then the serial scanner, the
`ChunkCarry` and `read_pos` are repositioned at `end.end_offset`. On `Declined`,
nothing happens and the serial scanner keeps the region.

**Two things make that a rework rather than an addition, and they are why it is
its own slice.** The `CopyEnd` arm's body has to become callable from two
places, or the two paths are two copies of a subtle sequence that `16.12` would
catch and a reviewer should not have to; and the event loop is a `while let`
inside a `for pass` inside the chunk loop, so leaving it to reposition needs
labelled breaks and a re-announcement of the enclosing loop's own hints.

**The `Builder` needs one new method.** `Interior::census` is a
`Vec<ArrayShape>` that has to replace or merge into `Builder::pending_census`
before `on_copy_end`. `map.rs` is L1 and `leader.rs` is L4, so the method takes
`&[ArrayShape]` (L1) rather than an `Interior`.

**`ScanOptions::parallelism`'s doc comment says no worker scheduler reads it.**
That is still true after this slice and stops being true in `16.10.2`; the same
sentence appears on `Parallelism` in `io.rs` and in `STATUS.md`'s capability
table.
