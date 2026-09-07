# P16.4 — The block pool's sizing

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md).

## What exists now

**`io::BufferPool`'s slot count is derived rather than fixed.** `POOL_SLOTS`
(a constant 4) is gone; in its place are `POOL_DEPTH` — the depth the replay
path wants, still 4 — and `POOL_BUDGET_BYTES`, 64 MiB, with
`BufferPool::slots()` answering `(budget / slot_bytes).clamp(1, POOL_DEPTH)`.
`slot_bytes()` is the unit a read loop announced through `hint_read_size`, or
`POOL_MAX_BYTES` where nothing has, which is the largest buffer `keeps()` takes
in that case, so the two rules bound the same thing.

The point is not the number, it is the currency. Four slots of a decoded
128 MiB xz block is 512 MiB — the whole cgroup the measurements run in — so a
pool whose slot size is free and whose count is fixed states no bound at all.
The count is now the consequence and the memory is the constant.

**Nothing the read path does changes.** At the 1 MiB default chunk and at the
16 MiB the chunk sweep brackets, `slots()` is 4, which is the constant it
replaces; a 24 MiB block gets 2 and a 128 MiB block 1. The one-slot floor is
deliberate: refusing to keep a block at all hands every reader back the fresh
`calloc` the pool exists to remove, which is the **1.83×** a pool miss costs
([`measurements.md`](measurements.md), "What the read chunk size is worth"), and
that is the more expensive of the two ways to be wrong.

**`max_source_span` is re-derived and unchanged.** The 64 MiB was 64 default
read chunks, so the cap and the pinning bound were one number to within a
chunk. Against a decoded block the same arithmetic rounds out to 24 or 128 MiB
units, so the cap cannot round out to one 128 MiB block at all — and a batch
confined to one worker's LF-split range inside one block pins exactly that
block whatever the cap says. So on a block-shaped source this cap goes back to
being what the other two triggers are: a knob on batch size, with the pool's
slot budget becoming the bound only once `16.4.1` makes it bound what is
outstanding. Neither the default nor the trigger moved, because on a
chunk-shaped source the original derivation still holds exactly; what moved is
which mechanism the memory claim is read off. The whole statement is beside the
trigger ([`architecture.md`](architecture.md), "Three flush triggers, and only
one of them bounds memory").

## What the next slice inherits

**Backpressure is `16.4.1` and is not in the tree.** The pool still allocates
on a miss, so it bounds what it *keeps* and never what is outstanding. The
finding that split the row is the one thing to carry forward:

> The pool cannot tell its two kinds of holder apart. One serial reader
> legitimately holds `(max_source_span / chunk) + 1` buffers at once — 65 at
> the defaults, and **unbounded** under `max_source_span: None`, which is a
> supported setting — because a batch pins every chunk it has taken a view
> into until it flushes. A worker given an LF-split range inside one block
> holds exactly one slot. A `take` that waits is backpressure for the second
> and a deadlock for the first, and nothing in the call says which is calling.

So the wait arrives with the discipline that makes it safe, which the first
concurrent consumer is what supplies: **one slot per holder**. Four consequences
for whoever writes it, and the first two were settled after this slice landed
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.4
split: backpressure has no test without a second holder"):

- **The exemption is the load-bearing half, not the budget.** A wait gated on a
  caller-set budget is not a softer version of the same thing: at the defaults
  the serial reader reaches 65 MiB against a 64 MiB budget, so any budget below
  its reach deadlocks, and under `max_source_span: None` no finite budget is
  above it. Design the exemption first; the budget is a knob on the holders
  that do wait.
- **`max_source_span: None` and a waiting pool are refused together.** An
  unbounded holder makes any finite budget unsatisfiable, and the symptom is a
  hang rather than an error, so `16.4.1` raises an option-validation error
  naming both settings when the span is `None` and `Parallelism` is not
  `Serial`. Clamping `None` to the budget was rejected: `None` is a caller
  saying explicitly that they do not want this cut.
- **The budget stops being a constant.** `POOL_BUDGET_BYTES` is the serial
  path's, and a pool serving N workers a block each is given its budget by the
  caller — which is `Parallelism`'s byte half (16.7), so a wait wired before
  that has no number to wait on. Nothing sets the budget today and there is no
  setter; adding one is the first line of 16.4.1.
- **A waiting `take` must be reachable only from a holder that holds one
  slot.** That is a property of the caller, like one-off-ness is
  ([`architecture.md`](architecture.md), "Execution model and API surface"), so
  it is said the same way rather than inferred from a length.

**The `hint_read_size` announcement is now doing two jobs, and one pool can
therefore describe only one unit.** It was "keep a buffer of this length past
the ceiling"; it is now also "this is the pool's slot size", which is what
`slots()` divides the budget by. `hinted` is a single atomic, last writer wins,
so a pool serving two units is wrong for one of them whichever way it is set:
at the chunk length a decoded 24 MiB block fails `keeps()` and is dropped on
release, making every block decode a fresh `calloc` while `slots()` reports 4
against a unit nothing holds; at the block length `keeps()` is satisfied but
the chunk reads above it get block-sized slots, so `slots()` collapses to 1 or
2 and the chunk path loses the pooling the **1.83×** miss is measured against.

**So `16.5` gives the block decoder its own pool** rather than announcing a
second unit into this one — settled after this slice landed
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.5
takes a second pool, because a pool describes one unit"). Each pool keeps the
derivation above exactly as it stands, and the only new question is how the two
budgets add up, which `Parallelism`'s byte half (16.7) has to answer anyway.

## The calls worth knowing about

**The byte budget derives the count and is not also checked per `give`.** An
earlier shape capped the free list by bytes *as well as* by slots. It is
inert at every reachable configuration — a kept buffer is at most
`POOL_MAX_BYTES` unless it is the announced unit, and `slots()` has already
divided the budget by the unit — so it was a bound that could not fire, which
reads as a second authority over the same number. The count is the enforcement;
the budget is what the count is computed from.

**`POOL_DEPTH` is a clamp, not a floor.** It is what the replay path wants and
what the read-chunk figure was measured under, so the budget may lower the
count but never raise it: a 1 MiB chunk gets 4 slots rather than the 64 the
budget would afford, which is what keeps this change invisible to every figure
that times the serial path.
