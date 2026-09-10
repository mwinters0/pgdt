# P19.14 — what a compressed reader is charged

What the next slices inherit from correcting the compressed divisor. The spec
row is [`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; how the mechanism works now is
[`architecture.md`](architecture.md), "The compressed source" and "Execution
model and API surface".

## The charge, and the numbers behind it

`XzSource::partition_advice` charges a partition
`2 × unit + chunk_bytes + xz_seek::Reader::decode_footprint()`, and
`BlockCache::affordable` compares that same sum against the stated budget.
`BlockCache::reader_bytes` is the one function both call, which is what stops
them being two sentences that drift — the state this slice found them in, with
`affordable` and `slot` saying two units while the divisor charged one and
nothing at all for the decoder.

The decoder term is read once, in `XzSource::assembled`, off the reader before
it goes behind the mutex. On an 8 MiB-dictionary file it is **9,471,776 B** —
8,388,608 dictionary + 1,048,576 input chunk + 34,592 of `liblzma` state — so
koji's shape, and `control_xz`'s, charge **58.03 MiB** a sub-stream against
`19.12`'s fitted 59.4. The chunk term is `pool.slot_bytes()`, which is the
announced read size (1 MiB by default) once any read loop has hinted the source
and `POOL_MAX_BYTES` before that; a fully cached query is the shape that asks
before a hint, and it charges the larger number.

**A file with small blocks is now dominated by the dictionary.** The 512-byte
and 64-byte-block fixtures charge ~8.4 MiB and decline the block path under any
budget below it — which is why two library tests that stated a budget in read
chunks now state it in reader charges, read off the source through
`ByteRangeSource::block_decode_bytes` rather than restated.

## What `19.13` inherits

**At the library's own 64 MiB default, an ordinary 24 MiB-block `.xz` affords
exactly one reader.** `worker_count` is `min(jobs, 64 / 58.03) = 1`, so between
this slice and `19.13` a flagless compressed scan resolves the machine's cores
on its status line and reads with one of them. That is the state the reserve
review predicted and the reason `19.14` lands first: the budget rule's constant
is read off a plan that charges honestly, and the *source's own budget
recommendation* is what makes a flagless scan parallel again.

The number that recommendation has to clear is therefore `k × 58.03 MiB` for
`k` readers of a koji-shaped file — 116 MiB for two, 1.36 GiB for
twenty-four — and the `RT8` half-of-`MemAvailable` cap is what supplies it on an
unlimited host. A recommendation below 58 MiB buys nothing at all on the
compressed path, which is the floor to check the rule against.

## What `19.11` inherits

- **`measure.PARALLEL_BUDGET` is 2 GiB.** At 1 GiB the corrected charge admits
  seventeen readers, so the widest `.xz` rows would have been clamped; at 2 GiB
  it admits thirty-five and the top rows are the twenty-four they are labelled,
  which is the arrangement the `af15eac` sitting was taken under.
- **`measure.QUERY_SUBSTREAM_CAP` is now empty**, because 2 GiB affords 28
  sub-streams on the plain typed-`query` leg and 35 on the compressed one, both
  past the top of `PARALLEL_JOBS`. The table's paragraph about it is emitted
  *from* the dict (`measure._substream_note`), so the prose and the cells cannot
  disagree, and the renderer's column placement is still asserted — against a
  patched dict, since the shipped one prints nothing.
- **The plain typed-`query` leg will run more sub-streams than the published
  table's**: 24 where the `af15eac` sitting planned 14. Its held batches are
  `24 × 72 MiB` under a 2 GiB budget in a 3g container, which is the one cell of
  the sweep worth watching for an OOM. `PARALLEL_MEMORY` was left at 3g
  deliberately — sizing the container off the axis is refused for the reason in
  its own docstring — so if that leg does not fit, the reading to change is the
  container for that figure and its table says so.
- No figure went from green to red: every figure this touches was already stale
  on `io.rs`, `stream.rs` or `measure.py`, and `nested-decode-micro` — the one
  green figure — declares none of them.

## Calls made here that the spec did not decide

- **The streaming fallback still charges the chunk buffer alone.** That path
  keeps one `xz_seek::Reader` behind a mutex however many readers a caller runs,
  so the decoder is a fixed cost of the source rather than a per-reader one, and
  charging it per partition would over-charge every reader but the first. The
  fixed term is what a budget's reserve covers.
- **`affordable` compares against the source's whole stated budget**, not the
  block pool's share of it. The chunk and the decoder are paid on the streaming
  path too, so they are not what a decline saves; putting them on the left of
  the comparison says "these come off the top, and what is left must hold two
  blocks", which is the same inequality the block pool's share expresses and one
  fewer number to keep in step.
- **`ByteRangeSource::block_decode_bytes` is a new defaulted trait method**,
  answering `None` for every source without a container. It exists for the
  decline note, which names what to raise the budget *to*: deriving that in
  `stream.rs` would have put a second copy of the source's own arithmetic beside
  the first, which is the defect this slice exists to remove. Detecting the
  decline is still read off `partitions()`.
- **The leader's floor rose with the charge.** `scan_region` leaves a region
  smaller than one `partition_bytes()` to the serial scanner, and that number is
  now a memory charge rather than anything with a span's shape — so a compressed
  region between 25 and 58 MiB is read serially where it used to be cut. Erring
  toward serial is the direction that bound is wanted in: the alternative admits
  readers the budget was divided as though it had not.
- **The trait's defaulted methods stopped being counted in prose.**
  `architecture.md` said "seven" over a listing of six that was already missing
  `default_workers`; the count is nine now, the code block lists all of them, and
  the three "a fifth/sixth/seventh defaulted method" phrasings became "a
  defaulted method of its own" so the next addition does not falsify four
  sentences in two documents.
