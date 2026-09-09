# P19.7 — the pool's accounting, made true

`BufferPool` reported a ceiling it did not hold, on two paths and for two
different reasons, and `held_bytes()` is the number `XzSource::apportion`
divides one stated budget with — so a budget rule resting on it would have been
resting on an under-report of up to 2×8. Three changes make the report a bound:
one rule for what a pool keeps, one count across a block pool's two lists, and
an affordability line that moves with the second. The mechanism is
[`architecture.md`](architecture.md), "Execution model and API surface" and
"The compressed source"; the sizing consequence is beside the leader, "The
interior split".

## What the next slice inherits

**`BufferPool::keeps` is now one sentence: `len <= slot_bytes()`.** The pool
keeps what fits a slot, and nothing else. The two rules it replaces — anything
under `POOL_MAX_BYTES`, *plus* a buffer of exactly the announced length — could
admit a buffer eight times the slot the count charged it at.
`POOL_MAX_BYTES` did not change meaning so much as narrow to the one it always
had underneath: the slot size of a pool nobody has announced a read length to
(`BufferPool::slot_bytes` already read it that way).

**`held_bytes()` is therefore an upper bound rather than an estimate**, and
that is the property `19.13`'s budget rule needs. It is asserted directly —
`the_free_list_holds_no_more_than_it_reports`.

**The parallel plain path lost its partition-buffer pooling, and that is the
finding, not a side effect.** `leader::scan_partition` reads `[start, end)`
whole — up to `POOL_MAX_BYTES` — into a pool whose slot is a *chunk*, so the
tightened `keeps` drops it on release and each partition is a fresh `calloc`.
There is no ceiling that avoids this: a single-unit pool cannot account for a
second unit, and the free list only ever held those buffers because the count
was under-reporting them. The remedy is the roadmap Future item "A two-unit
plain source", whose value this raises from "tidier" to "buys back an
allocation per partition". Two things bound what it costs: the shape is
`--jobs ≥ 2` on a *plain* file, which `19.8` is about to make non-default and
which the spec already records as slower than serial at every worker count; and
admission was never a hit rate anyone measured, `BufferPool::pick` taking the
smallest fitting buffer out of four slots shared with the tail reads. It is
filed under `STATUS.md`'s "Decisions worth another look".

**The slot count still bounds what is *outstanding*.** `--jobs 4` remains the
ceiling on concurrent plain-file readers: that comes from `obtain`'s wait
against `slots()`, which is untouched by what `release` decides to keep.

**A block pool's two lists share one slot count, through
`BufferPool::reserve`.** `BlockCache` stores its retention list's length on the
pool, and `release` pools a returned buffer only while free-plus-reserved is
below `slots()`. Before this the retained list was held at `slots()` and the
free list at `slots()` with nothing shared, so the pool's ceiling was
`2 × slots × unit` — 96 MiB at the default budget's two 24 MiB slots, and the
reason a `--jobs 24` `.xz` sitting read 1242 MiB against a stated 512.

**The ordering inside `BlockCache::slot` is load-bearing.** The reservation is
lowered *before* the evicted blocks are dropped, because a dropped block
releases its buffer straight into the pool and a release seeing the
pre-eviction reservation would discard the very buffer the following `obtain`
means to reuse. The eviction's entries are collected out of the lock and
dropped after it for the same reason the release path is kept short.

**`reserve` is a general name for one caller.** It is `Relaxed`, like the
pool's other atomics, and a value that arrives a buffer late costs one keep or
one drop. Nothing but `BlockCache` sets it, and every other pool reads zero.

**`BlockCache::affordable` asks for room for two units.** A coupled count of
one is the shape the pool rejects by name — `slot()` drains to `slots() - 1`,
so a one-slot pool retains nothing and drains before every decode. Two units is
the same sentence as "the coupled count is at least two" and introduces no
constant. Koji's 24 MiB blocks still decode at the 64 MiB default: the chunk
pool takes 4 MiB, leaving 60 against the 48 two units need.

**The decline threshold moved, so everything that states it moved with it.**
Whole-block decode is now declined for a file whose largest block exceeds
**half** the budget — about 30 MiB at the 64 MiB default, where it was about
60. `PlanNoteKind::CompressedBlockPathDeclined`'s message names twice the
largest block as the number to raise past; `cache::CompressionShape` and the
CLI's `compression_line` say so in their doc comments; and the **manual** was
corrected here rather than left to `19.10`, that rule binding absolutely
([`../process.md`](../process.md), "Where does this fact go?"). What `19.10`
still owns is the rest of its row.

**The advice is approximate in the direction it always was.** The message names
`2 × largest block` against the *stated* budget, while `affordable` compares
against the budget minus the chunk pool's share. Raising to exactly the named
number can therefore still decline, exactly as raising to the old named number
could; the imprecision is pre-existing and unchanged in kind.

## What `19.12` should expect to read

The block-decoding `.xz` legs of the reserve figure should fall by roughly a
factor of two — `19.6` read 243 MiB at a 64 MiB budget and 1242 MiB at 512 MiB,
both tracking `2 × slots × unit`. Nothing here changes the *plain* legs, which
`19.6` read flat at 37.4 MiB across the whole axis. Neither is measured in this
slice: `19.7` ships no reading, and the constant is chosen from `19.12`'s
sitting against this build.

## Tests

- `the_free_list_holds_no_more_than_it_reports` — the bound, and the partition
  buffer's refusal, in one.
- `a_reservation_takes_the_free_list_share` — free-plus-reserved is one slot
  count.
- `a_block_pool_holds_one_slot_count_across_both_lists` — the same invariant
  driven through the two calls the decode path actually makes, with the most
  recent block held live the way a reader holds its `Bytes`.
- `the_block_path_wants_room_for_two_blocks` — two units afford it, one unit
  short of two does not.
- `four_block_table()` was lifted out of
  `a_streaming_fallback_source_advises_one_partition`; a synthetic table is
  what lets a block unit be small enough for a budget to be stated in whole
  slots.
