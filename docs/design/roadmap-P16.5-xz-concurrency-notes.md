# P16.5 — `XzSource` internally concurrent

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "The compressed source".

## What exists now

**A `.xz` read decodes the blocks it lands in, outside any lock.**
`XzSource::read_range` turns its range into block indices
(`SeekTable::blocks_in`), takes an `xz_seek::BlockTask` for each, and decodes
the block whole into a slot of a pool of its own. A read inside one block is a
**slice** of that block — the common case copies nothing, where every read used
to be copied out of the decoder — and a read straddling a boundary is assembled
into a chunk-pool buffer. Concurrent `read_range` calls therefore genuinely
decode concurrently, which is the serialization point the phase exists to
remove.

**The mutex did not go away, and what it now covers is the point.** A
`BlockTask` can only come from an `xz_seek::Reader`, and `Reader::read_at`
takes `&mut self`, so the reader stays behind `std::sync::Mutex`. The block
path locks it to *name* a task — a `Copy` value read out of the seek table, no
I/O — and releases it before the decode. `size()` and `seek_table()` take no
lock at all any more: the table is cloned out of the reader once at
construction and held beside it.

**Retention is what makes per-call decode affordable.** A read loop asks for
`chunk_size` at a time, so a 24 MiB block decoded afresh per 1 MiB call would
be 24× the decode work. Decoded blocks are kept LRU-first, capped at the block
pool's own `BufferPool::slots`, and **eviction runs before a slot is taken** —
so the buffer the next decode reuses is the one eviction has just released, and
the byte budget bounds the retained blocks and the free ones together rather
than each separately. Getting that order wrong is worth 24 MiB: the first shape
of this bounded the two independently and read 89.2 MiB where this one reads
64.7.

**A block above `BLOCK_DECODE_MAX_BYTES` (256 MiB) keeps the streaming
reader.** Every shape `xz` writes is far below it — the largest file in hand is
a `--block-size=128MiB` recompression — but a *single-block* file's one block is
the whole plaintext, and decoding koji whole is an allocation the size of the
file rather than a read. Such a file also has no parallelism to lose, one block
being one decode unit, so the cap declines nothing that could have been read
concurrently. `BlockCache::for_table` answers `None` there and every read goes
through `Reader::read_at` exactly as it did before this slice.

**Three file handles now.** The reader owns one for the fallback decode,
`stat_file` answers `stored_size()`/`modified()`, and `data_file` is what block
decodes read compressed bytes through — positioned reads only, so a decode
running outside the mutex shares no cursor with anything.

## What it cost and what it bought, measured as a probe

Not a figure and quotable as none — one sitting each on the 3.00 GiB `.xz`
control (129 blocks of ~24 MiB, 5.45×), `pgdq parse` on the SATA SSD:

| | wall | peak RSS |
|---|---|---|
| before | 15.41 s | 16.2 MiB |
| after | 15.95 s | 64.7 MiB |

The wall clock is decode-bound either way (3.22 GB at ~200 MB/s on one core is
~16 s), so the whole-block decode neither adds nor removes decode work — it
moves *when* it happens. The resident set is the block pool's two 24 MiB slots
plus what a plain scan holds, and it is the price of the decode unit being a
block: nothing decodes a 24 MiB block without holding one. **The two runs
produce byte-identical `.dqcache` files**, which is the correctness check that
matters most here and the same property 16.12 asserts across worker counts.

No registered figure covers a compressed scan, so nothing in
[`measurements.md`](measurements.md) moves; every figure that times a run was
already stale on `io.rs`.

## What the next slice inherits

**16.4.1 now has its second holder.** The block pool is a holder that is not
the serial reader, and it keeps the discipline a wait needs in a form the notes
for 16.4 did not anticipate: not *one slot per holder*, but **release before
acquire**. The retention gives up a slot down to `slots - 1` and then takes, so
the slot a decode is about to want is one the cache has already freed; a holder
that frees before it asks can wait without deadlocking whatever else that pool
serves. Whoever writes the wait should hold the block pool to that property
rather than to a per-holder count, and the serial chunk reader still needs the
exemption for the reason 16.4 recorded.

**Two concurrent misses on one block decode it twice.** Accepted rather than
coordinated: an in-flight map would put every reader through a second lock —
serializing the common case, different readers on different blocks — to spare a
duplicate decode that only a shared block boundary produces. What keeps
concurrent readers off each other's blocks is how the range was split, which is
16.6's advisory to answer and 16.8/16.10's to honour. **A scheduler that hands
workers ranges not aligned to block boundaries pays this twice over**, since
two workers inside one block each decode all of it.

**The retained set is per source, so its size is the parallelism budget's.** At
`POOL_BUDGET_BYTES`' 64 MiB a 24 MiB-block file gets two slots and a 128 MiB
one gets one — which is fine for a serial reader and *not* enough for N
workers, each of which needs a block of its own retained or it re-decodes on
every chunk. 16.7's byte half is what raises it; a `--jobs 8` run against
today's constant would thrash, and that is the first thing 16.8 or 16.10 will
see if the budget is still a constant when they land.

**A whole-block decode verifies before it returns**, which is stronger than the
streaming path's `Verify::Full` and needs no setting. On the block path a check
failure can never arrive from a later call than the one that handed over the
bytes.
