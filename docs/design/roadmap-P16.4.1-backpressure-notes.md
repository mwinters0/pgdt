# P16.4.1 — Backpressure

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "Execution model and API surface".

## What exists now

**`BufferPool::obtain` is the read path's one acquisition, and whether it waits
is the caller's permission.** It blocks while `slots()` buffers taken under a
granted wait are already out, then takes the smallest free buffer that fits or
allocates — the wait and the take under one guard, so a slot released between
them cannot be taken by somebody else. The old `take` survives only as the
private `pick` underneath it; the four sites that used to build a
`PooledBuffer` by hand now get one from `obtain`, which is what carries the
charge.

**The exemption is a seventh defaulted trait method.**
`ByteRangeSource::hint_wait_policy(WaitPolicy)` is announced once per read loop
beside `hint_read_size` and `hint_parallelism`, and `WaitPolicy` has two
states: `NeverWait`, which is the `Default`, and `MayWait`, which permits the
block. `LocalFileSource` states it to its one pool; `XzSource` states it to
both, the two units being read by the same loop.

**All three read loops state `NeverWait`** (`M68`). The replay loop could not
do otherwise — `batch::RetainedChunks` pins every chunk a batch has taken a
`Utf8View` into and the batch then goes to the caller. `scan` and `map_forward`
could grant a wait safely, the carry copying what it keeps and an `Event`
borrowing only for the callback, and still do not: what a shipped loop's grant
buys is exposure rather than coverage, the wait's own test driving a bare pool.
What each loop states is asserted rather than reasoned about —
`tests/wait_policy.rs` records what a real `build_index` and a real
`table_stream` say.

**The charge is the buffer's, not the pool's.** `PooledBuffer` carries a
`charged` flag set at acquisition, so successive loops may state different
policies over one source while an earlier one's buffers are still out. And it
is discharged whether or not the released buffer is *kept*: the ceiling refuses
an unannounced buffer above `POOL_MAX_BYTES`, and a charge tied to the keep
would leak a slot per refused release until every waiting reader blocked.

**`LocalFileSource` now takes its buffer inside the `spawn_blocking` task**, not
before it. `obtain` can block, and blocking a runtime thread that is meant to be
draining the reads which would free a slot is the one way to turn this wait into
a deadlock in a build whose scheduler does not exist yet. Every other
acquisition — `XzSource`'s two — was already inside one.

## The calls worth knowing about

**Only buffers taken under a granted wait are counted.** `PoolState::charged`
is the first of the bound's two terms — the ceiling the library sets — and the
second, what in-flight batches pin, is deliberately not in it. Counting *every*
outstanding buffer would state one number instead of two and would let a loop
that granted nothing block one that granted a wait, which is the deadlock read
back in through the counter after the exemption removed it from the wait.

**Two pools, one acquisition order.** A read that needs both takes the chunk
slot first and the block slot inside it (`XzSource::read_by_blocks`), never the
other way round, so two waiting readers cannot hold each other's next slot.
Nothing enforces this beyond the one call site; it is worth re-checking when a
scheduler adds a third.

**`set_limits` notifies.** A raised ceiling is the only thing besides a returned
buffer that can release a waiter, and a source re-apportions its pools on every
hint — so a `hint_parallelism` arriving while a read waits must not leave it
waiting against the number it was refused under.

**The wait cannot fire in this build, and that is the expected reading.** No
loop grants the permission (`M68`), and no loop here runs two concurrent
readers over one source anyway. What has landed is the bound, tested against
real contention by two threads on a one-slot pool
(`io::tests::a_permitted_wait_takes_a_slot_rather_than_allocating`) — the
assertion 16.4 could not make, and the reason the wait waited for a second
holder.

## What the next slice inherits

**Nothing arms the wait, and arming it is the fused worker's to do.** Why the
two discard loops grant nothing, though they could, is
[`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The
holder class is a permission, and the shipped loops take the exempt one": the
wait's own test drives the pool directly and so never depended on what a read
loop states, so a grant on a production loop buys exposure rather than
coverage, and the two failure directions are not comparable.

**A scheduler's workers grant the wait and must hold one slot each.** That is
the discipline, and it is a property of the worker loop rather than anything
the pool can check: a worker that decodes a block, parses its rows and drops
them fits; one that accumulates two ranges' buffers before parsing deadlocks
against a one-slot pool. `16.10`'s fused worker is the first real instance.

**A partitioned *replay* (`16.8`) grants no wait either, and that is not an
oversight.** Splitting replay into N sub-streams does not change what a batch
pins — each sub-stream still hands its batches to the caller — so every
partition of it is exempt and the pool's ceiling bounds nothing there. The
memory that path holds is the second term, `max_source_span` rounded out to the
retained unit times the batches the caller keeps, and it is the caller's.

**The chunk pool's depth is still the number to revisit**, unchanged from
`16.7.1`: it is pinned at `POOL_DEPTH` because no concurrent holders exist, and
the moment they do, N chunk buffers are outstanding at once. With a wait in
place that number is now a *blocking* ceiling rather than an idle-capacity one,
so getting it wrong costs throughput rather than memory.

**No figure changed colour.** The diff touches `io.rs`, `scan.rs` and
`stream.rs` — declared paths for most of the register, and every figure
declaring any of them was already stale on that same path. Nothing is claimed
excused. The read path's steady state is unchanged in fact as well as in colour:
no loop grants a wait, so `charged` stays at zero, no read takes the condvar's
slow path and no read allocates that did not allocate before.
