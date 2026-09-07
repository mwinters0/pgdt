# P16.10 — The interior split

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "The interior split".

## What exists now

**`leader.rs` (L4) holds the parse half of the leader arrangement**, and
nothing calls it. `scan_piece` is the body one fused worker runs over one
LF-split piece of an open `COPY` block's interior; `merge` is the fold the
leader performs over the pieces at `CopyEnd`. Both are pure and synchronous:
`scan_piece` takes a `&[u8]` and an absolute base offset, not a source, and
neither function spawns, reads, or knows how the cuts were chosen.

**The cut semantics are the replay's**, deliberately and not by coincidence: a
piece's `limit` is not where reading stops, it runs through the line ending at
the first LF at or after `limit`, and a piece that does not begin on a row start
resyncs by a real forward read to the next LF. That is `stream::Segment`'s rule
stated from the cold side, so the two arrangements the phase builds cut the same
way and a future reader has one rule to learn.

**`map::census_row` is now a free function with two callers.**
`Builder::on_row` delegates to it, and an interior worker holds its own
accumulator and calls the same fold. The doc comment and
`measurements.md`'s census-off recipe both moved onto it in the same change;
the recipe is unchanged in shape and is now more correct, since a `return;`
there isolates the census for every caller rather than for the mapping pass
alone.

## The calls worth knowing about

**A piece past the block's end may report a terminator, and that is left
alone rather than prevented.** Such a piece is scanning DDL as though it were
rows, and a `\.` line in a dollar-quoted body reads as a terminator to it.
`merge` takes the **earliest** reporting piece and every piece before the true
terminator is inside the block, where I7 makes a `\.` line unambiguous — so the
earliest report is always the real one, and the scheduler is spared having to
know where the block ends before it hands the pieces out. Which is the whole
point: it does not know.

**A piece never claims end of file.** `scan_piece` passes `eof: false` to the
scanner always, so a piece that runs out of bytes inside the block reports no
terminator instead of `Error::UnterminatedCopyBlock`. Only the leader knows
whether more of the file follows, so only the leader may raise that — which is
also why `scan_piece` passes a `header_offset` of `0`: the field is read back
out of that error and no other, and this function cannot produce it.

**`merge` answering `None` is the leader's signal, not a failure.** It means
the window did not reach the terminator and more of the interior has to be
handed out. Distinguishing that from a scan error is what keeps the leader's
window loop from needing a sentinel.

**The module allows dead code.** Its consumer is `16.10.1`, and the alternative
— exporting `scan_piece` and `merge` from `lib.rs` so that something names them
— would put an unfinished surface in the public API for the sake of a warning.
`ByteRangeSource::partitions` (16.6) and `Parallelism` (16.7) both landed with
no consumer; the difference here is only that this one is `pub(crate)`.

## What the next slice inherits

**16.10.1 supplies both halves of `scan_piece`'s contract, and the second is
easy to miss.** The bytes handed to a piece must run *past* its limit, far
enough to complete the line that straddles it — a piece given exactly
`[base, limit)` is not wrong, it just does not count its last line. On a
block-decoding source that means a piece's read covers its block plus a little
of the next, which is a chunk-pool assembly rather than the zero-copy slice a
whole-block read gets, and the memory that costs is the scheduler's to account
for.

**The dispatch shape that the current API admits.** `spawn_blocking` needs
`'static`, and the source is a `&'a dyn ByteRangeSource` everywhere — which is
what 16.9's notes record as the reason its fills are a `join_all` on one task.
A worker does not need the source to be `'static`, though: `read_range(..)`
already spawns its own blocking task (and, on `XzSource`, decodes there), and
what it hands back is an owned `Bytes`. So `let bytes = source.read_range(..)
.await?;` followed by `spawn_blocking(move || scan_piece(&bytes, ..))` is a
genuinely fused worker — decode then parse, one piece, both on the blocking
pool — with no change to the borrow. That is why this slice's parse takes a
slice rather than a source.

**How the leader decides whether to split at all is still open, and the
material for it is `partitions()`.** The spec refuses parallel plain-file
discovery outright, and `PartitionBoundaries` is the only channel L4 has:
`Anywhere` is a source where a positioned read costs the same at every offset —
no decoder in front of it — and `At` is one that named seams because reaching an
arbitrary offset is expensive. Keying the refusal on that is the reading this
slice's docs assume; it is written up as a question rather than settled, under
`STATUS.md`'s "Decisions worth another look", because a remote source (P14)
will answer `Anywhere` and *does* have latency to hide.

**No figure changed colour.** The diff touches `map.rs` — which every figure
declaring it was already red on — plus a new module nothing calls and the docs.
`leader.rs` is unreachable from every registered command shape, which is
reachability, the one mechanical oracle that settles an executable diff; the
`map.rs` hunk is `on_row`'s body moving into a free function it now calls, which
no oracle settles and which is red on `map.rs`'s own terms along with everything
else.
