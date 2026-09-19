# P14.7 — Remote `.xz`

What landed: `io::walk_seek_table`, which drives `xz-seek`'s sans-IO footer
walk from a future; `io::FetchedXzSource`, an `.xz` source over **any**
`ByteRangeSource`, reading each block out of a compressed window it fetched;
`open_remote` composing the two, with the cold walk announced before it is paid
for; `KD36` for that walk; and the manual's compressed-remote paragraph. The
spec's D1, D13 and D20 hold the reasoning. No `D<k>` was added —
[`decisions.md`](decisions.md) is at its cap and "D14", "D18" and "D19" already
carry recognition by content, the persisted table and *announce rather than
refuse*; "D15" was **corrected**, its "decodes read through a `File`, not a
`Window`" having named this composition as the reopening and now being true of
`XzSource` alone.

## What the rest of the phase inherits

- **The transport is `dyn`, so the fetched source is not remote-only.** It
  holds an `Arc<dyn ByteRangeSource>` and delegates `modified`, `stored_size`,
  `remote_identity`, `hint_cancellation` and `hint_in_flight_identity` to it,
  which is why the whole composition is exercised over a local file in
  `io.rs`'s own tests with no server and no `http` feature. That is also the
  shape D20 asks 14.8 for: the window-fed path has no local twin to disagree
  with, so putting a file through it makes a divergence surface as two read
  paths over one byte stream disagreeing.
- **Two read arms, and the budget picks.** `block_path()` is
  `XzSource::block_path` unchanged — `BlockCache::affordable` against the
  stated budget — and where it declines, `read_in_pieces` drives
  `xz_seek::BlockRead` over the same fetched window. **14.8 migrates the local
  small-budget read onto that arm**, which is now proven where no alternative
  existed.
- **The compressed window is inside the charge, not outside it.**
  `decode_bytes` is `Layout::decoder_bytes()` — the decoder over a source that
  *lends*, so no input chunk is built — plus the widest `total_size()` in the
  table. Everything downstream (`affordable`, `partitions`,
  `default_worker_memory`, `block_decode_bytes`) is then the local source's
  arithmetic with that one term substituted, which is what keeps
  `MEMORY_UNPOOLED_BOUND` from absorbing a term whose length is known before
  the fetch (D20).
- **The walk starts on bytes the probe already holds.** `Walk::begin`'s first
  request is the six magic bytes at offset zero, which is exactly
  `ORIGIN_LEADING_BYTES`, so `walk_seek_table` takes them as an argument and
  spends no round trip on them; passing an empty slice is correct and costs
  one. `the_walk_spends_no_fetch_on_leading_bytes_the_caller_holds` asserts the
  difference rather than the count.
- **Nothing was added to `ByteRangeSource`** beyond a doc comment on
  `read_range` stating what every implementation here already did: exactly
  `len` bytes or an error, never a prefix. Upstream's walk reads a short supply
  as the *file* being shorter than the size it was constructed with, so that
  contract is what keeps a truncated read and a truncated file distinguishable
  ([2026-09-19](../status/history/2026-09-19.md)).
- **`xz_partition_advice` and `xz_short_read` are free functions now**, both
  having had two callers the moment this source existed.

## Negative results

- **The announcement is a status line, not a `Diagnostic`.** D1 says the source
  announces the walk "at open through the diagnostic channel", and it cannot:
  a `Diagnostic` is carried on a `DumpIndex` or a `CacheStatus`, both built
  from a source that already exists, which is *after* the walk — and the whole
  point is that the line arrives before. `tracing::warn!` is where the local
  source's own "seek table build started" already goes and reaches all three
  commands on stderr ([`decisions.md`](decisions.md), "D64"). A diagnostic
  raised afterwards would name a cost already paid.
- **`KD36` is `(c) unowned`, not "owned by the phase that tunes the
  network".** The checklist row asked for the latter; a `(b)` stance needs its
  destination to exist in [`roadmap.md`](roadmap.md)'s phase index, and no such
  phase is listed ([`../status/STATUS.md`](../status/STATUS.md), "Known
  deficiencies"). The entry names that phase as what promotes it instead.
- **`default_workers` stays at one and `default_worker_memory` does not.** D7
  answers both conservatively for the *plain* remote source, on the ground that
  it has no block structure to cut at. A fetched `.xz` does, so the memory
  recommendation is the file's own charge — the same shape
  `XzSource::default_worker_memory` answers — while the worker count stays at
  the trait's default, concurrency over a network being the tuning this phase
  defers. Keeping `partitions()` honest is what forced the split:
  `stream::compressed_block_path_declined` reads a single partition over a
  seekable table as a decline, so advising one unconditionally would have
  reported every fetched `.xz` run as having declined the block path it was
  taking.
- **The piecewise arm is not free of the budget either.** It holds the window,
  the decoder and one chunk — `BlockCache::reader_bytes` less the block unit —
  so a budget below *that* is exceeded exactly as the local streaming
  fallback's is. Not a new deficiency: it is the same shape and the same
  precedent, and it is stated beside `block_path`.
- **A re-read of one block is a re-fetch and a re-decode on the piecewise
  arm.** Nothing caches there, deliberately: what the arm exists to avoid is
  holding the plaintext. On the whole-block arm the `BlockCache` is shared with
  the local source and `KD20`'s double decode applies unchanged, which is why
  no second marker was allocated for it.
- **This source refuses the wait a read loop grants**, alone among them. Every
  other one takes its chunk buffer inside a `spawn_blocking` closure, so a
  `WaitPolicy::MayWait` holder blocks a blocking-pool thread; here the fetch
  must `await`, so the buffer is obtained on the runtime's own task, where
  `BufferPool::obtain`'s `Condvar` would block the `current_thread` runtime
  this project dispatches from ([`decisions.md`](decisions.md), "D12") and the
  releasing sibling would never run. The cost is the direction
  `WaitPolicy::NeverWait` is already safe in — the pool allocates past its
  budget rather than hanging. Granting it again means obtaining the buffer
  inside the decode's closure, which this phase's scope buys nothing by.

- **The oracle needed no new knob.** Every misbehaviour a fetched `.xz` can
  meet is one the plain source already meets, the walk and the block reads
  going through `RemoteSource::read_range`; what the `.xz` tests add is a
  two-stream fixture, which is what makes the walk's per-stream cost visible at
  all.
