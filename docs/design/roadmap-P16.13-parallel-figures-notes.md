# P16.13 — the two parallel figures' apparatus

What `16.13.1` inherits: two instruments built, registered and runnable, and no
readings. `measure.UNTAKEN` carries `parallel-scan-throughput` and
`parallel-peak-rss`; `cd scripts && uv run measure.py --figure
parallel-scan-throughput --figure parallel-peak-rss` takes them.

## Why the sitting is a slice of its own

**A figure published outside a stamped sweep names, inside its own marker, the
commit it was taken at** ([`measurements.md`](measurements.md), "A figure may be
published outside the sweep"), and a sitting run from a tree carrying its own
uncommitted apparatus has no such commit: the parent it could name is a tree
where the instrument does not exist. Both of these figures are outside-the-sweep
figures by construction — a sweep is an hour of a *quiet* machine and these two
deliberately occupy all of it — so the apparatus has to be committed before the
readings are taken, and an unattended round that commits nothing cannot do both.

`xz-decode-scaling` waited in `UNTAKEN` for exactly this and left when the commit
existed; that is what the list is for, and the entries below are the second
instance rather than a new mechanism.

The seam is therefore not the one the phase's other splits were at. It is not
mechanism-then-evidence and not a self-contained piece beside a rework: both
figures are one confidence, no library code moved, and the whole diff is the
harness plus its tests. What splits them is that the second half cannot be
*taken* from the tree the first half lives in.

## What the apparatus is

**One axis, seven values, shared with the decode figure.** `PARALLEL_JOBS` is
`(1, 2, 4, 8, 12, 16, 24)` — identical to `DECODE_WORKERS`, deliberately, since
`xz-decode-scaling` is the floor these are read against and a row with no
counterpart there is a comparison nobody can make. The range runs past four
because `POOL_DEPTH` clamps the chunk pool there and a plain leg's curve flattens
at that ceiling; a table stopping at four would leave a reader to infer the scan
stopped scaling.

**Three new command-shape families, and the exemption they open.** `parse-jobs-N`,
`query-typed-jobs-N` and `parse-rss-jobs-N` are the existing `parse`,
`query-typed` and `parse-rss` shapes under a stated `--jobs` and
`--parallel-memory`. Each is `SWEEP_JOBS`' first exception, so the exemption is
declared in `measure.JOBS_AXIS` and bounded from the other side by a new
`pinned_count_problems`, which `--check` fails on: `worker_count_problems`
already caught a shape pinning *nothing*, and this catches one pinning some
third number without declaring itself an axis. Between them the failure `M69`
closed — a count that moved because a default did — stays closed while the
exemption exists.

**One budget on every row, and the first row cannot state it.** Every shape
states `--parallel-memory 1073741824`. It is 1 GiB because a block-decoding
source charges a partition one decoded block plus a chunk buffer, so the widest
row over 24 MiB blocks wants ~600 MiB and anything less clamps the top of the
table silently — a lower count wearing a higher label, which is the failure
`xz-decode-scaling` refuses by asking its instrument what the plan admitted.
Here the same failure is refused by choosing the number, and a test holds
`PARALLEL_BUDGET >= jobs x (block + chunk)`.

The one-job row is different and cannot be made otherwise: `--jobs 1` is
`Parallelism::Serial`, `Parallelism::workers(1, _)` returning `Serial` by
construction, and `Serial` carries no budget — so that row runs at the library's
own `DEFAULT_MEMORY_BUDGET`. That is the arrangement this project ships and the
one every published table was taken under, which is what makes it the right
denominator for a speedup; both tables say so. Its consequence on the 128 MiB
leg is the sharpest reading in either figure: 128 MiB is a block the 64 MiB
default cannot hold, so that row reads through the streaming fallback and every
row above it block-decodes.

**Container memory is 3 GiB, an apparatus departure both figures declare.** The
register's line is a 512 MB container, which is smaller than the budget under
test. `test_measure.py` now holds the departures as a named set over
`EVERY_FIGURE` rather than a single `if` over `ALL_FIGURES`, so an untaken
instrument cannot acquire one unnoticed — which was the gap, an untaken figure
having no table in the doc to state it.

**A compressed leg's rate is per plaintext byte, and the plaintext is a
registered input.** `control_xz` is a compression of `control`, so the plaintext
volume is `control`'s own size exactly: no instrument reports it and no seek
table is walked. That is the one thing `xz-decode-scaling` could not do, its
koji leg being a prefix of a file nothing here generates. Two tests hold it —
that the divisor is a `.sql` input, and that the compressed leg actually
`derives_from` the input it is divided by, so "some plain file of about that
size" cannot pass.

**A second block size, and a generator that refuses a third.**
`generate_xz_input.py` gains `--block-size`, defaulting to the `24MiB` it
hard-coded, with an allowlist of the two sizes a figure asks for. `control_xz128`
is the same plaintext at 128 MiB. Two sizes because a compressed reader's
per-worker footprint *is* one decoded block, so the count a budget admits is a
property of the file: a resident-set figure at one block size publishes that
file's shape as the library's bound.

## The apparatus found a deadlock on its first real run

`M70`: `pgdq parse` over a block-decoding `.xz` at `--jobs` 2 or more never
finishes, at the default budget and at 1 GiB alike, while `--jobs 1` and every
plain-source count are fine. The evidence, the thread state and the mechanism
are [`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "A
`--jobs` parse of an `.xz` deadlocks on the block pool's retained charges".

It is not fixed here, and deliberately: the fix is inside `io.rs`'s buffer pool,
which is a tested core path and a different confidence from a harness diff
([`../process.md`](../process.md), "Working unattended"). It is filed as a
blocking out-of-band row so the next round takes it before `16.13.1`, which
could not take five of its seven legs until it was closed — and the next round
closed it ([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md),
"`M70`: the block pool is a retaining holder, so nothing grants it a wait").

**Two things this says about the apparatus itself, both good.** The smoke run
that found it was a deliberately small one — `PGDQ_MEASURE_SIZE_GIB=0.2`,
`--reps 1`, into throwaway cache and warm directories — and it cost four
minutes; a figure whose legs are new command shapes is worth running that way
once before it is handed to a sitting. And the harness gave no misleading
answer: it hung, visibly, on the second reading of the first figure. What it
did *not* do is time out, because a hung `pgdq` outlives `SIGTERM` — see the
history entry.

## What a later session must know

**`xz-decode-scaling` went red on this change, and the oracle that acquits it is
`--verify-additive`.** It declares `scripts/generate_xz_input.py`, and a generator
edit stales it whatever the edit is. The evidence exists but cannot be spent from
here: byte-identity of the regenerated inputs is what settles a generator change,
and both `--verify-additive` and an acknowledgement argue from a *commit*.
Verified by hand meanwhile, on a 77 MiB input at the default: the pre-change and
post-change generators write byte-identical `.xz` files (`cmp`), and
`--block-size 128MiB` writes one block where the default writes four. So the
acknowledgement is available to whichever change commits this, and the figure is
red on a path it declares, with the reason written down rather than argued away.

**The regime is `warm-parallel` and nothing else, which narrows the spec row.**
See [`../status/STATUS.md`](../status/STATUS.md)'s "Decisions worth another look"
— the entry is open, and it is the thing to settle before the sitting rather than
after it, since adding a device regime changes what the sitting takes.

**Nothing is quoted by either figure yet.** Both carry an empty `quoted_by`,
which is correct for an untaken figure — its numbers are in no document — and is
the fold-in's to fill. `roadmap-P16-parallel-scan.md`'s arithmetic ("What that
buys, at 12 physical cores / 24 threads") is what the throughput figure will
answer, so that document is the first `quoted_by` edge to add.

**Both figures stand in no borrow edge**, in either direction, which is the
condition under which a figure may be published outside a sweep at all. Keep it
that way: a `shares` edge added later would make them re-takeable only with a
sweep, and a sweep is the thing they cannot be part of.
