# P19.5 — the retained unit, and a plain partition that is not one chunk

Three things land together because they are one edit to what `Partitioning`
means: the type gains a term, the query planner reads it, and the plain
source's own answer changes size. The spec's own paragraph is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md), "The
span term is charged per source, not universally"; the mechanism now lives in
[`architecture.md`](architecture.md), "Execution model and API surface", with
the sizing argument beside the leader ("The interior split").

## What the next slice inherits

**`Partitioning` is now three facts, not two.** `boundaries()`,
`partition_bytes()`, and `retained_unit()` — the last answering
`RetainedUnit::ReadChunk` or `RetainedUnit::Partition`. It is set through a
consuming `retaining(unit)` builder rather than a fourth argument on all three
constructors, and the constructors start at `ReadChunk`.

**The default is the charging arm, and that was the call.** A source that says
nothing is charged the span, which is what every source was charged before the
term existed and is the safe direction — over-charging plans fewer readers than
the budget could hold, under-charging plans readers it cannot. It is the same
argument `WaitPolicy::NeverWait`'s default makes, and it is cited to it in the
doc comment so a later source meets the reasoning rather than the value. The
cost is that `XzSource`'s block arm has to remember to say `Partition`; the
alternative — a required argument on `anywhere`/`at`/`single` — forces every
future source to think but makes the trait's own `single(0)` default noisy, and
`P14` is the one source that will next have to answer, which is why the fact was
filed into its inbox rather than left to the signature.

**`plan_partitions` charges the span only where the advice says `ReadChunk`**,
and the `PlanNoteKind::ParallelismBudgetLimited` note then reads
`max_source_span: None` rather than a number that was never charged. That field
now has two readings — the caller left the span unbounded, or the source retains
by the partition — and neither names a number, which is what the note's own doc
comment says. An **empty** match set keeps the charge: `all` over an empty
advice is vacuously true, so the emptiness is tested explicitly rather than left
to fall out, and the arm that says nothing is the one that charges more.

**A plain partition is eight read chunks, capped at `POOL_MAX_BYTES`.**
`io::PLAIN_PARTITION_CHUNKS` holds the multiple; the formula is
`chunk * 8`, floored back up to one chunk and capped at 8 MiB. Three things
follow, and each is why the formula has three terms rather than one:

- the multiple caps the tail read's waste at `chunk / partition` = 12.5%, where
  a partition of exactly one chunk read the whole file twice — `19.2`'s
  measured 1→2 step;
- the cap keeps the partition read a buffer `BufferPool::keeps`, since above
  8 MiB only a buffer of exactly the announced read length survives release,
  and an uncapped multiple would hand back a fresh `calloc` per partition and
  lose the pool on the path being sized — but see "The cap, reviewed" below,
  which is where that ceiling stopped being a term of the formula and became a
  symptom of the pool having one read unit;
- the floor means a caller whose chunk is already at or past the ceiling gets
  the answer this source gave before, which the hint clause keeps pooled.

At the shipped 1 MiB chunk the answer is 8 MiB, and at the unhinted default
(`slot_bytes` is `POOL_MAX_BYTES` when nothing was announced) it is also 8 MiB
— so `pgdq query`'s plan-time footprint, which is read before the hint reaches
the query path's source instance, did not move. `pgdq parse` is where it did.

**Eight is reasoned, not measured, and the reasoning is in the constant's own
doc comment.** The tail is one chunk however large the partition is, so each
doubling past eight buys under a percent of the read while halving how finely a
region can be cut and how many readers a stated budget affords. No sitting was
taken for it; `19.11`'s sweep is what will price the result.

## What moved that was not the library

**The leader's floor rose with the partition, and three test apparatus
constants moved with it.** `scan_region` declines a region with less than one
`partition_bytes()` left in the file, so a fixture-scale test that announced a
chunk size to make blocks splittable now has to announce one eight times
smaller. `leader.rs`'s `scheduled` helper went 64 → 8 bytes (the smallest
trailing region in its four fixtures is 103 bytes, which is now written down
where the number is chosen), `wait_policy.rs`'s parallel leg 256 → 64,
`map_file.rs`'s pair 512/4096 → 64/512, and `determinism.rs`'s legs 512/4096 →
64/512.

**`determinism.rs`'s shipped-configuration leg now sizes its dump off the
source.** It used to generate four `DEFAULT_CHUNK_SIZE`s and assert the
precondition against that constant; it now asks a probe `LocalFileSource` for
`partitions(0..1).partition_bytes()` and generates four of *those* — 32 MiB
rather than 4 MiB, about 0.7 s more in that test. Had it not, the leg would
have gone quietly vacuous: the dump would sit under the new floor, every leg
would take the serial path, and the byte-equality would be the serial cache
compared to itself. The same reasoning is why `partitioned_replay.rs`'s two
budget tests now read the unit off the source (`partition_unit`) instead of
spelling it as a chunk count.

**One new library test**, `a_block_shaped_source_is_not_charged_the_batch_span`:
one budget, one span and one job count put to both source shapes, asserting
that the plain leg's note names the span it was charged and that the `.xz` leg
is silent and splits. It fails on the pre-slice build.

## `scripts/measure.py` held a second copy of the arithmetic

`QUERY_SUBSTREAM_CAP` hand-computes what `PARALLEL_BUDGET` affords a typed
`query` on each input, and its `.xz` entry was the span-charged divisor. It is
now `{"control": 14}` alone: the `.xz` divisor is 25 MiB rather than 89 MiB, so
1 GiB affords forty sub-streams — past the top of `PARALLEL_JOBS`, leaving
nothing to annotate. The renderer skips the annotation for a leg with no entry,
`parallel-scan-throughput`'s prose says why that leg carries none, and
`test_measure.py` asserts the absence with the reason rather than the old
`.xz < plain` relation.

**`measurements.md` was not touched, and that is the rule rather than an
omission.** The pasted section describes a sitting taken at `20fd77c` under the
span-charged arithmetic; re-rendering it now would put the new renderer's prose
against the old readings. `parallel-scan-throughput` was already stale and stays
stale — `19.11`'s sweep republishes it, and that is the sitting that will show
what the sizing repair bought on the plain `parse` column.

## What this does *not* do

The **depth** term is untouched: `LocalFileSource::hint_parallelism` still
passes `POOL_DEPTH`, so a fifth fused worker on a plain source still waits for a
slot. `19.2`'s sitting measured that clamp and found lifting it worth 4–10%
above four workers while leaving the 4→8 step in place; the spec's slice row for
this slice names the sizing repair only, and the plain worker default `19.8`
ships is serial either way.

Repair 2 — sizing the tail read to a row rather than to a chunk — is still out
of this phase and now buys less: it would shrink the compressed source's 4% and
the plain source's remaining 12.5%, where before it faced 100%. It is filed as a
rejected alternative beside the mechanism ("The interior split") rather than as
a `KD<k>`, since nothing is now wrong that it would fix.

## The cap, reviewed

The partition size was filed under "Decisions worth another look" and has been
reviewed. **The code stands; the reason recorded for it does not.** Five facts
came out of reading the mechanism cold, and the last two are what `19.7`
inherits.

**The partition read really is one buffer.** `leader::scan_partition` reads
`[start, end)` in a single `read_range` (`want = end - start`) and only then
falls to `want = chunk_size` for the tail, so `partition_bytes` is a length
this source allocates rather than an accounting unit.

**The cap is a consequence, not a choice.** `BufferPool` holds one announced
length and the parallel plain path has two read units, so the partition read
cannot be the announced one and survives release only under the ceiling. Every
alternative ceiling the original entry weighed — cap on the stated budget, or
eat the `calloc` — answers the wrong question; the arrangement that answers it
is the two-pool one `XzSource` already runs, now a roadmap Future item ("A
two-unit plain source"). The doc comment and
[`architecture.md`](architecture.md), "The interior split", say this rather
than presenting the ceiling as a merit.

**The cap makes pooling possible and does not make it happen.**
`BufferPool::pick` takes the smallest free buffer with `buf.len() >= len`, and
`POOL_DEPTH` slots are shared between 8 MiB partition reads and 1 MiB tail
reads across every worker — so a partition read finding only tail buffers free
allocates anyway. Nothing has measured the hit rate, and the 1.83× pool-miss
figure the entry reasoned from is a *chunk* reading.

**The pool's accounting under-reports this path by 8×, and `19.7` owns it.**
`slots()` is a count at `slot_bytes()` — the announced chunk, 1 MiB — while
`keeps` admits anything up to `POOL_MAX_BYTES`, and `release` tests
`free.len() < slots` without rescaling by bytes. So the free list can hold four
8 MiB partition buffers, 32 MiB, which `held_bytes()` reports as 4 MiB.
`held_bytes()` is exactly what `XzSource::apportion` subtracts to divide one
budget between two pools, so this is the number a budget rule would trust. The
spec's `19.7` row was amended to cover it.

**The shipped default sits exactly on the cap.** `DEFAULT_CHUNK_SIZE * 8` is
`POOL_MAX_BYTES` to the byte, and the three constants are justified
independently, so a later change to any one of them would silently cap the
default configuration with no test failing.
`a_shipped_plain_partition_is_eight_whole_chunks` is the guard; deriving one
constant from another was rejected, because it reads as though one *causes* the
other and buries two independent justifications in one expression.
