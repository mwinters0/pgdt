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
reader.** A *single-block* file's one block is the whole plaintext, and
decoding koji whole is an allocation the size of the file rather than a read.
`BlockCache::for_table` answers `None` there and every read goes through
`Reader::read_at` exactly as it did before this slice.

That fallback is the ordinary path for a large archive, not an edge case: `xz`
writes one block per stream unless it is threading, so plain `xz bigfile`
produces it — `xz -0 -T1` over 240 MB gives 1 stream and 1 block, against 8 for
`xz -0 -T4 --block-size=32MiB`. **What the cap declines is memory, not
seekability**, and it is keyed on the largest block rather than the block count,
so a multi-block file written with large blocks is declined by it and does have
parallelism to lose. Both statements and the alternatives refused with them are
beside the mechanism ([`architecture.md`](architecture.md), "The compressed
source").

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

**16.4.1 now has its second holder, and release-before-acquire is not the
discipline it looked like.** The block pool is a holder that is not the serial
reader, and the retention gives up a slot down to `slots - 1` and then takes —
which is sufficient on `parse`, where no batch is built and the cache's own
reference is the only one. On a query it frees nothing: the drain drops the
cache's reference while the batch's views keep the buffer outstanding, so the
two holders are the same slots counted twice
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The wait
is exempted by holder class, not validated by an option pair"). What makes a
wait safe on both shapes is the exemption stated by holder class: a read that
will be retained into a batch never waits.

**Two concurrent misses on one block decode it twice.** Accepted rather than
coordinated: an in-flight map would put every reader through a second lock —
serializing the common case, different readers on different blocks — to spare a
duplicate decode that only a shared block boundary produces. What keeps
concurrent readers off each other's blocks is how the range was split, which is
16.6's advisory to answer and 16.8/16.10.1's to honour. **A scheduler that hands
workers ranges not aligned to block boundaries pays this twice over**, since
two workers inside one block each decode all of it.

**The retained set is per source, so its size is the parallelism budget's.** At
`POOL_BUDGET_BYTES`' 64 MiB a 24 MiB-block file gets two slots and a 128 MiB
one gets one — which is fine for a serial reader and *not* enough for N
workers, each of which needs a block of its own retained or it re-decodes on
every chunk. 16.7's byte half is what raises it; a `--jobs 8` run against
today's constant would thrash, and that is the first thing 16.8 or 16.10.1 will
see if the budget is still a constant when they land.

**`BLOCK_DECODE_MAX_BYTES` becomes that budget's consequence, not a second
constant beside it.** The 256 MiB line and `POOL_BUDGET_BYTES`' 64 MiB are
unrelated numbers today, and the smaller is not a bound above its own slot
size: `BufferPool::slots` clamps to at least one, so a unit larger than the
budget still gets a slot at that unit's size — a 128 MiB block is 128 MiB
resident against a 64 MiB budget. Once a caller states a budget, "a slot must
fit what the caller allowed" is the line and the constant retires into it,
which is also what would let a client willing to allocate read a file with
512 MiB blocks block-wise. 16.7's spec row carries this
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The
block-decode cap declines memory, not seekability").

**A compressed scan's resident cost must first be measured on a query, not a
`parse`.** `parse` builds no batches, so `RetainedChunks` never runs and the
64.7 MiB above is the retention cap and nothing else. On the query path a
retained chunk is a zero-copy view into a whole decoded block, so retaining
1 MiB of a 24 MiB block holds all 24 and a batch bounded by `max_source_span`'s
64 MiB pins several blocks — more than the cap holds, and the thing that
actually sets the number. 16.13's `parallel-peak-rss` is where that first gets
one; nothing measures it today.

**A whole-block decode verifies before it returns**, which is stronger than the
streaming path's `Verify::Full` and needs no setting. On the block path a check
failure can never arrive from a later call than the one that handed over the
bytes.
