# P14.8 — The local read inside a block moves onto the block handle

What landed: `XzSource` holds an `xz_seek::Layout` and a live
`xz_seek::BlockRead` where it held an `xz_seek::Reader` behind a mutex, so a
read inside a block is one mechanism on both providers and the source is the
difference — a `File` the crate pulls from here, a fetched `Window` there. The
spec's D13 and D20 hold the reasoning. No `D<k>` was added:
[`decisions.md`](decisions.md) is at its cap and "D15" already carried this
rule, and was **corrected** to name the mechanism that now serves it. `KD35`'s
index line and marker were rewritten for the same reason; nothing was closed.

## What the rest of the phase inherits

- **Nothing in this tree holds an `xz_seek::Reader`**, which is what D13 said
  14.8 would settle. Both `.xz` sources are built from
  `xz_seek::Builder::new().layout(table, stored_size)`, both name a `BlockTask`
  off that layout with no lock, and what we vet of that crate is now the walk
  machine, `Layout` and the block handle. `XzSource::open` walks with
  `xz_seek::SeekTable::from_source`.
- **`XzSource` keeps one file handle**, not three. The reader owned one for a
  live cursor; nothing here has a cursor any more, so `stat` and every decode
  read through the same `Arc<std::fs::File>` — which is itself an
  `xz_seek::CompressedSource`, so it is handed to the crate directly.
- **Three functions are the shared mechanism**: `piece_of`, which answers which
  part of a read a block covers for all four `.xz` read arms; `fill_from_block`,
  which turns `BlockRead::read`'s one-decoder-call return into this crate's fill
  contract over any `CompressedSource`; and `PIECEWISE_SKIP_CHUNK`, which was
  `FETCHED_SKIP_CHUNK`. The covering-block loops stay one per arm, the fetched
  one having to `await` a window between blocks.
- **The advice is unchanged and `KD35` is still open.** The piecewise arm still
  answers `Partitioning::single`, and for the same reason it always did: one
  live handle goes forward only, so two workers on different partitions would
  each force the other's block to be left and begun again. What changed is that
  the per-reader handle `KD35` names as its fix is now a type that exists and is
  `Send` — the marker says so, and the figure is still the half nobody can skip.
- **The user-facing name is unchanged.** `PlanNoteKind::CompressedBlockPathDeclined`,
  the manual and the CLI help still say *streaming decoder*, which describes
  what a user sees — a file decoded as a stream rather than a block at a time,
  with a backward read paying a re-decode — and that is still what happens. The
  two names are tied together at that variant's rustdoc, once.

## Negative results

- **The handle had to be kept across reads, and that is not an optimization.**
  `xz_seek::Reader::read_at` continued a live decode where it could reach the
  target and restarted the covering block otherwise (`vendor/xz-seek/src/reader.rs`,
  `route`). A handle begun and completed per read instead — the shape
  `FetchedXzSource::read_in_pieces` takes — skips from the block's start on
  every call *and* drains the remainder on every call, so a forward scan of a
  single-block file costs one whole-file decode per chunk read rather than one
  in total. That is a path turned unusable, not a path made slower, which is why
  `LiveBlock` exists and why the mutex it sits behind is the same mutex as
  before.
- **Continuing is never dearer than restarting, so there is no route
  arithmetic.** Restarting skips `from − block.start` and continuing skips
  `from − position`, and a live handle's position is never before its block's
  start; `Reader::route` compares the two and this resolves the comparison. The
  rule is `position() <= from` and nothing else.
- **Completion is the seek-away escape, not a per-read call.** Leaving a block
  completes it, which is where its check is compared, and a block still live
  when the source drops is not completed — `xz_seek::Verify::Full`'s own
  sentence, and today's guarantee carried across the swap rather than a new one.
  It is *not* the fetched arm's pattern, which completes on every read; the two
  agree on "every block read from and moved on from is verified" and differ in
  how often they say it. That divergence is under
  [`../status/STATUS.md`](../status/STATUS.md), "Decisions worth another look".
- **A `--check=none` stream is abandoned rather than drained.** `BlockRead::complete`
  decodes the remainder whatever the check is, and on such a stream there is
  nothing to compare — so `LiveBlock::seated` drops the handle instead, which is
  the one case `Reader::leave_live_block` also skips.
- **The block arm lost its only lock.** Naming a `BlockTask` took the reader's
  mutex for a table lookup; a `Layout` answers it from the table alone, so a
  cache *miss* is now as lock-free as a hit was.
  `a_block_path_read_is_served_while_the_live_handle_is_held` is the old
  retained-block test retargeted onto the one mutex that is left.
- **The piecewise arm's charge did not move.** It holds the chunk buffer, the
  decoder and — over a file, which does not lend — the compressed input chunk,
  which is `xz_seek::Layout::decode_footprint()` exactly as the reader's was.
  `Partitioning::single` still charges the chunk alone, the decoder being a
  fixed cost of a source that serves one partition.
- **`measurements.md` was not swept for the name.** Its `.xz` legs say
  *streaming fallback*, and they were taken on `Reader::read_at`: renaming them
  would describe a reading by a mechanism it did not run on. The figures are
  what say which arm a leg took, and the budget that decides is unchanged.
