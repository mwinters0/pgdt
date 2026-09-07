# P16.7.1 — What the stated budget decides, and the flags that state it

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "Execution model and API surface" and
"The compressed source".

## What exists now

**A sixth defaulted trait method carries the caller's numbers to the source.**
`ByteRangeSource::hint_parallelism(Parallelism)` sits beside `hint_read_size`
and is announced at the same three points: `scan` and `map_forward` from
`ScanOptions::parallelism`, the replay loop from `QueryOptions::parallelism`.
That split is what makes a query's two passes separately bounded, which is why
the value was put on both structs.

**`BufferPool` carries a budget and a depth, both settable, neither a
constant.** `slots()` is `(budget / slot_bytes).clamp(1, depth)`, defaulting to
`DEFAULT_MEMORY_BUDGET` (64 MiB, public and re-exported) and `POOL_DEPTH` (4) —
so at every chunk size the register measures the count is unchanged, and every
figure was taken under numbers that still hold.

**The two numbers are read by different pools, on purpose.** The **bytes** size
every pool. The **jobs** count is the *block* pool's depth only, floored at
`POOL_DEPTH`; the chunk pool keeps `POOL_DEPTH` whatever `--jobs` says. A chunk
buffer is taken and released inside one `read_range`, so its free list wants
the replay path's depth, and raising the ceiling with a worker count would grow
a query's resident set by `(jobs − POOL_DEPTH)` chunks the moment
`RetainedChunks` releases what a flushed batch pinned — 20 MiB at the CLI's
default on this machine, bought for concurrency nothing produces. Re-derive
that against real holders when the scheduler lands; it is not a standing rule.

**`XzSource` divides one budget between its two units, chunks first.**
`apportion()` gives the chunk pool the stated number and the block pool what is
left after `BufferPool::held_bytes()` — 4 MiB at the 1 MiB default chunk, so
the block pool sees 60 MiB of a 64 MiB budget and a 24 MiB-block file still
gets its two slots, which is the 64.7 MiB `parse` probe 16.5 took. It is
recomputed from the source's own stored `budget`/`jobs` on **both**
announcements, so the two hints may arrive in either order and a source nobody
announces to still holds a coherent split.

**`BLOCK_DECODE_MAX_BYTES` is gone.** `BlockCache::for_table` now refuses only a
file with no blocks; whether the block path is *taken* is
`BlockCache::affordable()` — `unit <= pool.budget()` — asked per read through
`XzSource::block_path()`, which `read_range` and `partitions` both go through.
Deciding it per read rather than at construction is what lets the answer depend
on a budget the caller states after the source exists.

## The call that had two defensible answers

**The default budget stays at 64 MiB, so the block-decode line falls from a
flat 256 MiB to ~60 MiB.** Both answers were named in 16.7's notes and neither
is free. What decided it:

- Keeping 64 MiB makes the stated number *true*. The pool's one-slot floor
  means a unit above the budget is still allocated at its own size, so the only
  way to honour a budget is to refuse the unit — which is exactly what
  `affordable` does, and what a raised default would give up.
- Every published figure was taken under 64 MiB. Raising the default to 256 MiB
  costs the *serial* path twice as much resident on a 24 MiB-block `.xz` (four
  retained blocks where two suffice) and four times as much at a raised
  `--chunk-size`, which is memory paid by every user to spare two file shapes a
  slower read.
- The recourse is the flag this slice adds, and it is in the user's hands.

**What it costs is real and is named where it bites** — `architecture.md`, "The
compressed source". Two ordinary shapes fall back to the streaming reader under
the default where a flat cap took them down the block path over budget:
`xz --block-size=128MiB` (which is `koji-…blocks128.xz`) and `xz -9 -T0`, whose
threaded block size is three times its 64 MiB dictionary. `parse` does not care
— it never reads backwards, and 16.5's probe had streaming marginally *faster*
— but `query` re-decodes forward from a block start on every backward read.

**The fallback is silent, and the review reversed that.** The reasoning this
slice recorded — that saying it per run "would need the source's budget to
reach `index.rs`, which is a trait method for a sentence" — was wrong on the
fact. `ByteRangeSource::partitions` is already that trait method, and
`XzSource` answers `Partitioning::single` on exactly the declined-on-budget
arm, so the detection is a comparison between two values `index.rs` already
holds. The budget default itself was affirmed, including against a per-pass
split this slice never considered. What a diagnostic must look like, and why
`info --verbose` reports the file's shape beside it, is
[`architecture.md`](architecture.md), "The compressed source"; the work is
`M67` ([`roadmap.md`](roadmap.md), "Out-of-band work").

## What the next slice inherits

**16.4.1's two waits are over.** 16.5 supplied the second holder and this slice
gave the pool a budget the *caller* states, which is what a wait has to wait
on. What remains is the wait itself, one slot per waiting holder, and the
exemption by holder class — a read that will be retained into a batch never
waits. Nothing here changes that analysis; it only removes the reason it could
not be written.

**A scheduler that raises `--jobs` gets more retained blocks and nothing else
until it also raises `--parallel-memory`.** At the default budget a 24 MiB-block
file affords two slots whatever `jobs` says, because the budget binds first.
16.8 and 16.10.1 will want to state both numbers together, and the arithmetic
they should size against is the partitioning advisory's
`partition_bytes` — a decoded block plus a chunk buffer per partition, plus the
LZMA2 dictionary and `xz-seek`'s input buffer, which that method deliberately
excludes ([`roadmap-P16.6-partitioning-advisory-notes.md`](roadmap-P16.6-partitioning-advisory-notes.md)).

**The chunk pool's depth is the number to revisit.** It is pinned at
`POOL_DEPTH` today for the reason above, and the moment N readers genuinely run
concurrently, N chunk buffers are outstanding at once and the free list should
hold them. That is a one-line change in `LocalFileSource::hint_parallelism` and
`XzSource::apportion`, and it belongs to whichever slice first has real
concurrent holders to measure it against.

**No figure changed colour.** The diff touches `io.rs`, `scan.rs`, `stream.rs`,
`batch.rs` and the CLI — declared paths for most of the register, and every
figure declaring any of them was already stale on that same path. Nothing is
claimed excused. What is worth knowing for the next sweep is that **the
register's own inputs are unaffected in fact as well as in colour**: no
registered figure has a compressed input, the CLI's new defaults resolve to the
same pool sizes the constants gave (`--jobs 24 --parallel-memory 64M` is
`POOL_DEPTH` chunk slots and two 24 MiB block slots, exactly as before), and
`measure.py` invokes `pgdq` without either flag.
