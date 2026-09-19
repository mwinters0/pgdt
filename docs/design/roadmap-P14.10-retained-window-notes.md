# P14.10 — The fetched piecewise arm stops re-fetching a block it still holds

What landed: `FetchedXzSource` keeps the compressed window its piecewise arm
last read out of — `HeldWindow`, one block's whole extent behind a `Mutex` —
so a forward scan inside one block fetches it once rather than once per read.
The decode did not move: each read still begins, skips into and completes its
own `xz_seek::BlockRead`, which is `14.11`'s half. The spec's "D21" holds the
reasoning; no `D<k>` was added, and [`decisions.md`](decisions.md), "D15" was
**corrected**, having said the fetched arm "keeps none".

## What the rest of the phase inherits

- **The charge did not move**, and nothing about it was restated.
  `FetchedXzSource::decode_bytes` already counted the decoder plus this file's
  largest window, so one retained window is the same peak as one transient
  window and only its lifetime differs. The arm still charges
  `Partitioning::single(chunk_bytes)`.
- **The boundary rule is satisfied by ordering, not by arithmetic.** A window
  for another block is dropped *before* the next is fetched — the `drop` is a
  statement of its own ahead of the `await`, because a `match` arm would keep
  the scrutinee alive across it — so a block boundary never holds two windows
  where `decode_bytes` counts one. The same drop guards the whole-block arm,
  which reaches a source that declined earlier when a budget is raised mid-run.
- **The window is taken for a read's duration rather than borrowed.** A
  concurrent read finds an empty slot and fetches its own, which is the number
  of windows in flight the arm had before this slice; there is no shared
  position, so nothing here decides anything about partition count. `KD35` is
  untouched except in its marker's wording, which said the fetched arm "keeps
  nothing between reads" — it keeps a window and no decode position, and that
  is still not the reason its count is one.
- **The arm is still stateless as to decoding**, so nothing is decided here
  about holding a handle across an `await`. That is what `14.11` spends, and
  what makes `KD35`'s rewrite that slice's rather than this one's.
- **`PlanNoteKind::CompressedBlockPathDeclined` carries a caveat now**: its
  *streaming decoder* wording describes one live handle behind a mutex, which
  is the local arm alone until `14.11`. The caveat names that slice and is
  retired by it, rather than the string being churned twice.

## Negative results

- **The window could not be retained by cloning it into the decode.**
  `xz_seek::Window<Bytes>` is `Clone` and a `Bytes` clone is a refcount bump on
  one allocation, so a clone would have cost nothing — but it would put the
  retention *beside* the decode instead of after it, and the block a read ends
  in is exactly the one worth keeping. The window is returned out of the
  `spawn_blocking` closure the way the chunk buffer already is, which keeps one
  owner throughout and needs no argument about what a clone counts as.
- **A failed read retains nothing.** The slot is written back past the short-read
  check, so every way out but the successful one drops the window — mirroring
  the local arm's `*live = None`: a window kept past an error is state a later
  read would have to reason about, and the fetch it saves is one request.
- **The test had to distinguish the arm, not only the count.** Four reads
  inside one block cost one fetch under the block arm too, the `BlockCache`
  retaining what it decoded — so
  `a_fetched_piecewise_scan_inside_one_block_fetches_it_once` also reads the
  next block and then reads back into the first, which costs a fetch here and
  nothing at all under a retained block. Without that third read the assertion
  would pass on a source that never took this arm.
- **The manual was not swept.** It says a compressed remote read "fetches each
  block's compressed extent whole", which is what still happens; how many times
  it does so per block is not a claim it makes.
