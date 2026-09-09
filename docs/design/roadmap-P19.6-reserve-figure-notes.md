# P19.6 — the reserve, measured

The budget rule the phase ships rests on one number, and this slice registers
the instrument that takes it and takes it once, diagnostically. The spec's
paragraph is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md), "The
reserve is a figure, and it is measured uncapped"; the instrument is
`measure.py`'s `reserve` entry, and the sitting is
`runs/measure-20260909T084032/` — **`--alone`, and marked NOT PUBLISHABLE by
construction**.

No library code changes here.

## The reading, and why it does not settle what it was convened to settle

**On a plain source the reserve is a constant. On a block-decoding `.xz` source
it is not a reserve at all — it is a multiple of the stated budget.** Median
peak resident set, three reps, `--jobs 24`, `postgres:16` at `-m 3g`:

| stated | `.xz`, resident | `.xz`, reserve | plain, resident | plain, reserve |
|---|---|---|---|---|
| 64 MiB | 243.44 MiB | +179.44 MiB | 37.43 MiB | −26.57 MiB |
| 128 MiB | 465.52 MiB | +337.52 MiB | 37.44 MiB | −90.56 MiB |
| 256 MiB | 698.60 MiB | +442.60 MiB | 37.36 MiB | −218.64 MiB |
| 512 MiB | 1241.84 MiB | +729.84 MiB | 37.54 MiB | −474.46 MiB |

(the uncapped-arena leg; the other two agree with it, see below). Beside them
the shipped serial arrangement — `control` at `--jobs 1` stating no budget —
holds **5.86 MiB** against the library's 64 MiB default. That run is
`peak-rss`'s own `control` row spec for spec and is measured here rather than
borrowed, for the reason under "What `19.11` inherits"; that it lands on
`peak-rss`'s published **5.85 MiB** is the cheapest available check that this
apparatus is the one that took it.

**`budget = limit − reserve` therefore cannot bound a compressed parallel scan
under any constant.** Substituting the worst cell — 730 MiB — into a 3 GiB
limit gives a 2.3 GiB budget, which by the same line reads about 5 GiB
resident. The term that actually bounds the process is the *fraction ceiling*
the spec calls a number that "bounds nothing anyone has measured".

**What the growth is made of, and it is not the allocator.**
`stream::worker_count` divides the stated budget by `partition_bytes`, which
for this file is a 24 MiB block plus a 1 MiB chunk buffer — 25 MiB — so the
four rows plan **2, 5, 10 and 20** sub-streams. Against those counts the line
is `160 MiB + 54 MiB` a sub-stream (least squares over the four cells), and the
wall clocks corroborate the counts independently: 9.9 / 7.4 / 4.4 / 2.9 s
against `parallel-scan-throughput`'s published 16.00 s serial is 1.6× / 2.2× /
3.6× / 5.5×, which lands between that figure's 2-, 4-/8-, 8-/12- and 16-/24-job
rows.

So **a decoding worker holds about 2.2× what the divisor charges it**. The
divisor charges the decoded block and a chunk buffer; the worker also holds an
8 MiB LZMA2 dictionary, its compressed input window and the batch in flight.
This is not a defect — the manual already tells a user that an `.xz` parallel
scan holds more than `--parallel-memory` names
([`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and
`--parallel-memory`") — but the excess being **proportional** is what a rule
subtracting a constant cannot absorb. The fixed term, ~160 MiB on `.xz` and
37.4 MiB on plain, is the part a reserve constant genuinely describes.

**The plain leg is flat in the budget and in the worker count.** 37.4 MiB at
every cell, where `worker_count` plans 8, 16, 24 and 24 workers over an 8 MiB
partition: `POOL_DEPTH` slots of `POOL_MAX_BYTES` is 32 MiB and the pool does
not grow past it, so the stated budget over-states this path by 27–475 MiB.

## The arena legs agree, and that is the second finding

**Capping the arenas moves nothing measurable on this shape.** Uncapped,
`MALLOC_ARENA_MAX=25` (the spec's recommendation: the worker count plus one)
and `MALLOC_ARENA_MAX=2` (what the manual publishes today) agree within the
per-rep spreads at every one of the eight cells, and the one pair whose ranges
are disjoint — `.xz` at 128 MiB, 456–477 against 400–420 — **reverses sign** at
512 MiB, where the capped leg is the higher of the two. There is no direction
to read.

That is against the spec's premise, which expects "two reserves … differing by
roughly the 200 MiB the arenas hold". The explanation is `19.3`: with a
`current_thread` runtime the threads are the blocking pool's and are created on
demand, so the arena count already follows the dispatched concurrency and a cap
at or above it binds nothing. **It is not a controlled refutation of the koji
probe** that produced the 200 MiB — that was a different file at a different
scale under `rt-multi-thread` — so the claim this sitting supports is the
bounded one: *on a 3.00 GiB 24 MiB-block `.xz` at `--jobs 24`, after `19.3`, no
arena effect is measurable.*

`19.10` inherits it: the manual's "either set `MALLOC_ARENA_MAX` (2 is enough)
or budget a few hundred megabytes above what `--parallel-memory` names" is
half-confirmed and half-stale. The second half is understated — the margin is a
multiple, not a few hundred megabytes — and the first half now buys nothing
this instrument can see. Neither sentence is corrected here, because these
readings are a diagnostic and no document may quote them as measurements.

## What the instrument is

`reserve` sits in `measure.UNTAKEN`, beside `rss-attribution`: registered and
selectable by name, taken by no sweep, and carrying no marker in
`measurements.md`. The spec gives it no published table until `19.11`.

**One axis, three legs, two sources.** `RESERVE_BUDGETS` is the axis — 64, 128,
256 and 512 MiB, starting at `LIBRARY_DEFAULT_BUDGET` because that is the
conservative end if resident is flat in the budget (it is, on the plain source)
and the number a run that states nothing gets. `RESERVE_ARENAS` is the three
`MALLOC_ARENA_MAX` settings above. `RESERVE_INPUTS` is `control_xz` and
`control`, because a reserve that is a property of the source is not a
constant — which is exactly what the reading turned out to say.

**`RESERVE_JOBS` is 24, stated on every shape**, being `PARALLEL_JOBS[-1]` and
this machine's `available_parallelism()` — the count `Parallelism::discover()`
will resolve here. It is a declared axis for `pinned_count_problems` in the
same sense `JOBS_AXIS` is, with the two swapped: the count is fixed and the
budget varies.

**The arena setting is an environment assignment in front of the RSS wrapper,
not on `nerdctl run`.** The wrapper `exec`s its arguments, so the child inherits
it; written in front of `/pgdq` it would be a further argument to `perl` and
would set nothing at all — a leg that silently set nothing would read as "the
cap buys nothing", which is the conclusion this sitting reaches and is exactly
why the placement is asserted rather than remembered.

**The shape name carries `rss`** (`parse-rss-reserve-<arena>-<budget>`), because
`Session.time_run` reads the wrapper's report only for a shape whose name says
it has one.

## What `19.11` inherits

**The `Shared` edge is not declared yet, and the reason is `rss-attribution`'s.**
This figure's serial-default row is `peak-rss`'s `control` row spec for spec —
same binary, command, input and regime — so the two must share a reading rather
than take one each. Declaring the edge from `UNTAKEN` would entangle
`peak-rss`, whose table the doc carries from a standalone `41c96bb` sitting, and
`--check` would refuse that marker with no sweep to cure it. The edge goes on in
the change that moves this entry into `FIGURES`, and `test_measure.py` holds the
two halves together.

**That change owes a harness change with it.** `Session.borrow` copies
`self.readings` — wall clock — and nothing else, so an **RSS** reading cannot
cross a share today. The three resident figures the closing sweep collapses
(`peak-rss`, `rss-attribution`, `reserve`) all publish a resident set, so the
first of them to declare a borrow has to carry `self.rss` across too. It is not
done here because nothing calls it yet.

**A re-take is owed after `19.7`, not before.** `19.7` fixes `BufferPool`'s
accounting, which under-reports whenever `keeps` admits a buffer larger than
`slot_bytes`; that is the number a budget rule trusts, so every cell above may
move. Re-taking now would price a build the phase is about to replace.

## The apparatus

Three reps, interleaved and reversed, every reading accepted at the first
attempt: CPU stall ≤1.21%, machine ≤41% busy, steal 0.00%, busiest core
≥3.60 GHz, ≤72°C. I/O stall reached 79.08% and is **not gated** here —
`warm-parallel` gates steal alone, the readings deliberately occupying every
hardware thread — so it is reported rather than acted on.
The container is `postgres:16` at `-m 3g --memory-swap 3g`, an apparatus
departure from the register's 512 MB and named as one in
`test_measure.MEMORY_DEPARTURES`: the largest budget under test is itself
larger than 512 MB.

## What this leaves open

Two entries under `STATUS.md`'s "Decisions worth another look", both about the
spec's reserve section rather than about anything built here: whether `19.7`
ships `limit − reserve` as specified now that the compressed path refutes it,
and whether the "measured both ways, ship the uncapped number" decision and
`19.10`'s manual recommendation still stand with no arena effect to price.
