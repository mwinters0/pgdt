# P14.11 — The fetched piecewise arm keeps its handle across reads

What landed: `FetchedXzSource` keeps the block it is reading — `HeldBlock`,
one fetched window and the `xz_seek::BlockRead` decoding out of it, behind the
`Mutex` that already held the window — so a forward scan inside one block
decodes it once rather than once per read. `LiveBlock` became the state
machine both arms run, its four calls (`begin`, `seats`, `fill`, `leave`)
generic over `S: xz_seek::CompressedSource`, and `fill_from_block` folded into
`LiveBlock::fill`, having had exactly those two callers. The spec's "D21"
holds the reasoning; no `D<k>` was added, and
[`decisions.md`](decisions.md), "D15" was **corrected**, having said the
fetched arm "begins a handle per read".

`KD35` was rewritten, index line and marker together: the fetched arm is
mutex-bound now and was not before. Nothing was closed.

## What the rest of the phase inherits

- **The charge did not move**, and nothing about it was restated. What is held
  at once is still one window and one decoder, which is what
  `FetchedXzSource::decode_bytes` counts; the arm still charges
  `Partitioning::single(chunk_bytes)`. The boundary rule is satisfied by
  ordering as `14.10`'s was — the outgoing block is completed and dropped
  inside its own `spawn_blocking` **before** the next window is fetched, so a
  boundary never holds two windows.
- **The field set `14.12` extracts against is now settled**: `held` changed
  type and `decodes` was added, and neither is one of the six budget methods
  that slice gives one home.
- **The arm is one partition by requirement now, not by advice.** It keeps a
  decode position, so two readers would force each other's restarts exactly as
  two local ones would. `xz_partition_advice`'s rustdoc and the `KD35` marker
  say so of both providers; the per-reader handle that would recover the
  partitions is still that entry's remedy and still waits on its figure.
- **`PlanNoteKind::CompressedBlockPathDeclined`'s caveat is retired**, which
  is what `14.10` left for this row. Its *streaming decoder* wording — one
  live `xz_seek::BlockRead` behind a mutex, every backward read decoding
  forward from its block's start — is now true on both arms with nothing
  qualifying it.

## Negative results

- **Nothing counts decodes in a shipped build, and that was the point.**
  `crate::instrument::DecodeCounter` is a field that is absent without
  `introspect` and a `begun()` that compiles to nothing, so the property this
  slice exists for is asserted by
  `a_fetched_piecewise_scan_inside_one_block_decodes_it_once` under `cargo
  test -p pgdump_query --features introspect` and by no timed build. The
  oracle's `requests()` could not have stood in: it counts ranged GETs, and
  `14.10` already made those one per block.
- **The counter counts the decision, not the call.** It is incremented where
  the arm decides to begin rather than resume, which is on the runtime's own
  task, so a `BlockTask::begin` that then errors is counted. Counting inside
  the blocking closure would have meant an `Arc<DecodeCounter>` cloned per
  block in every build, including the ones that carry no counter.
- **`begin` stayed off the runtime thread.** It reads the block header and
  builds the decoder's dictionary, which is what every other decode call is
  moved off the runtime for, so the resumed and freshly fetched cases go into
  the closure as one `Seat` rather than the fresh one being built where the
  decision is made. That is the only thing the enum exists for.
- **The always-completed guarantee became the local arm's moment, as D20
  allowed.** The fetched arm used to complete on every read; it now completes
  when it leaves a block, so a handle dropped instead — on an error, on a
  budget raised into the block arm mid-run, or when the source drops —
  compares nothing. That is `xz_seek::Verify::Full`'s own sentence and what
  `XzSource` has done since `14.8`; D20's clause is met either way, as the
  checklist row said it would be.
- **A `--check=none` stream is abandoned rather than drained here too**, the
  local arm's rule reaching the fetched one for free once `leave` was one
  call: draining such a block would decode its remainder to verify nothing.
- **The manual was not swept.** It says a compressed remote read "fetches each
  block's compressed extent whole" and describes the streaming decoder as
  re-decoding forward from a block's start on a backward read; both are still
  what happens. How many times a *forward* read decodes was never a claim it
  made.
