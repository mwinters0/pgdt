# P19.20 — the cut width, decided by measurement

What the rest of `P19` inherits from the sitting. How the mechanisms work now is
beside them — [`architecture.md`](architecture.md), "cut-width", "The compressed
source" and "The interior split"; this doc holds the readings those sections
argue from and what the next slice must not re-derive.

## What was under test, and what shipped

`19.19` changed two things at once and only one of them had to be: the **cut
width** (a piece went from `reader_bytes` wide — 2.08–2.42 units — to one unit)
and the **read shape** (a worker's body read is the piece exactly). The row
separated them.

Six binaries, one sitting, one machine: `shipped` (`bff7b1c`: whole read, one
unit), `pre1919` (`9323d92`: whole read, 2.42 units), and `k1`/`k2`/`k4`/`k8`
(chunked read, `BOUNDARIED_PARTITION_UNITS` = 1/2/4/8). What ships is
**neither of the two extremes**: the cut stays at one unit, and the read shape
becomes the source's own statement (`io::PartitionRead`) because the winner
differs by source.

## The readings

`runs/19.20-cut-width-probe.py`, three families. A `runs/` probe, not a figure:
no document may quote a number here as a measurement.

**Family `xz24`** — `control_xz.xz`, 24 MiB blocks, 3 reps. Serial control
(`shipped --jobs 1`, 8 GiB container): **16.89 s median, 110.9 MiB**.

*Stated, `--jobs 2 --parallel-memory 136,384,064`* — two of the *pre-repair*
charge, so every binary resolves exactly two readers despite `19.19` having
moved the divisor:

| build | median | ceiling `16.89/(1+1/k)` | rss median |
|---|---|---|---|
| `shipped` (whole, k=1) | 17.52 s | 16.89 | 249.5 MiB |
| `k1` (chunked, k=1) | 17.57 s | 16.89 | 251.6 MiB |
| `k2` | 13.26 s | 12.67 | 294.6 MiB |
| `k4` | 11.60 s | 10.56 | 294.3 MiB |
| `k8` | 10.68 s | 9.49 | 363.3 MiB |
| `pre1919` (whole, k=2.42) | 11.32 s | 11.95 | 323.7 MiB |

*Flagless, 1 GiB container* — each build's own default, which is why the
resolved counts differ:

| build | median | rss median | worst | killed |
|---|---|---|---|---|
| `shipped` | 4.84 s | 854.1 MiB | 863.2 | 0/3 |
| `k1` | 4.81 s | 930.6 MiB | 960.0 | **1/3** |
| `k2` | 9.19 s | 868.9 MiB | — | **2/3** |
| `k4` | 9.50 s | 895.6 MiB | 1005.4 | **1/3** |
| `k8` | 9.71 s | 784.2 MiB | 838.4 | 0/3 |
| `pre1919` | 9.58 s | 828.1 MiB | 828.2 | 0/3 |

`shipped`, `k1`–`k8` all resolve **13** readers; `pre1919` resolves **11**, its
divisor being the pre-`19.19` one.

**Family `plain`** — `control.sql`, 3.00 GiB, 8 GiB container, 3 reps. The cut
width is invisible here (`PartitionBoundaries::Anywhere` ignores it), so `k1`
stands for all four and `pre1919` for `shipped`:

| shape | `shipped` | `k1` (chunked) |
|---|---|---|
| `--jobs 24 --parallel-memory 256m` | 1.54 s, **209.2 MiB** | 1.26 s, **9.4 MiB** |
| `--jobs 2 --parallel-memory 256m` | 1.47 s, 47.4 MiB | 1.21 s, 7.7 MiB |
| serial control | 1.25 s, 5.7 MiB | — |

**Family `confirm`** — no probe binary ran the arrangement that ships, since
each of the six reads *every* source one way. The build that combines them
(`ship19.20`) was re-taken against the build it has to match in each cell:
plain at `--jobs 24`, 1.26 s / 9.4 MiB against `k1`'s 1.28 / 9.4; compressed
flagless in 1 GiB, 5.18 s / 833.0 MiB median against `shipped`'s 5.00 / 855.7,
nothing killed. It matches both arms.

## What the readings say, in the order that decided it

**The account holds at two readers, and the wasted decode is the term.** The
prediction written into the probe before the run — a region of `B` units cut
into pieces of `k` costs `B + B/k` unit decodes, so the speedup at `w` workers
is bounded by `w/(1 + 1/k)` — tracks the stated column within 10% at every
width, and the ordering is exact. `KD20` is therefore not a suspicion but a
priced term, and the width is genuinely what amortises it.

**At the flagless default the ordering inverts, and that is what decided the
row.** Thirteen readers in 1 GiB: one unit is 4.84 s and every wider cut is
9.2–9.7 s, which is not a smaller win but *half the speed*. The mechanism is
not established. What is arithmetic: a window is `workers × k` units wide, so
from `k` = 2 the readers of one window want `13 k` distinct blocks against a
slot count of thirteen (`BufferPool::slots` under a 791 MB flagless budget and
a 24 MiB unit), and the cache cannot hold what its own readers are working on.
That is a candidate, not a finding — **do not write it down as the cause
without testing it**, which is a `BlockCache` instrumentation question and not
another sitting of this shape.

**A chunk-sized body read costs resident on the compressed path and buys
nothing.** `k1` against `shipped` is the read shape alone, same count, same
budget: identical wall clock at two readers (17.57 against 17.52) and at
thirteen (4.81 against 4.84), and **+76 MiB of median resident at thirteen**,
which crossed the 1 GiB line once in three. The cost is **unattributed**. The
candidate is that a block evicted while a `Bytes` view is out lives past the
slot cap and chunk reads multiply the eviction events by the chunk count
(`BlockCache::slot` drains to `slots() - 1` before every decode); it is not
claimed as accounted for, and the residual's shape — flat at two readers,
+6 MiB a reader at thirteen — is the evidence a next attempt starts from.

**On the plain path the same change is worth 22×.** 209.2 → 9.4 MiB at
`--jobs 24`, 47.4 → 7.7 at two, against a serial 5.7 — so the per-worker term
is 0.08 MiB where it was 8.03, which is flat. And it is **faster**, 1.26 s
against 1.54: the eight extra read syscalls and `spawn_blocking` hops per
partition that the roadmap named as this change's unmeasured price do not
appear against the `calloc` it removes. `KD18` is struck by this reading.

**So the winner differs by source, and one number cannot express it.** That is
why `Partitioning` gained `PartitionRead` rather than the leader gaining a
constant: a block-decoding piece is one unit and reading it whole is a
zero-copy slice of a block the worker was going to decode anyway, while a plain
piece is eight chunks of a file with no units at all and reading it whole
allocates a buffer no pool will keep.

## Three things a later slice must not re-derive

**`BOUNDARIED_PARTITION_UNITS` = 1 is a measured choice, and the test says
so.** `a_block_decoding_partition_spans_at_most_the_cut_width` replaces
`19.19`'s `…never_crosses_a_block_boundary`: it asserts `covering.len() <= k`
rather than `== 1`, and it walks a table of `8k + 4` blocks so the window is
satisfied out of the boundary list rather than running off the end of the file —
without which the bound is vacuous at every `k` above one. Change the constant
and the test follows it.

**The read-size invariant the row wanted pinned does not hold, and that is a
finding rather than an omission.** The row asked for "no read exceeds
`chunk_size`" on the ground that it "survives either outcome"; the measurement
refused the arrangement that has it. What is pinned is a read size **per
shape**: the chunked half here, and the whole half's own bound in `M83` below —
`no_read_a_worker_makes_exceeds_the_chunk_size`, over the plain source, which
fails on the piece-length read and tells a growth retry from a piece's opening
read by **offset** rather than by length (the growth path retries the same start
at twice the length, so it is never the first read at its offset; a read the end
of the file truncated is any length at all, which is what defeats an
arithmetic rule).

**A width that varies with the resolved count is the shape the evidence
suggests and is refused for now.** Narrow when many readers, wide when few,
which is what the two tables above would each prefer on their own. It is
refused on review grounds, not on evidence: the flagless collapse has no named
mechanism, and fitting a rule to two cells of an unexplained curve is what this
phase has already paid for twice (`19.15`'s fixed term, `19.18`'s `POOL_DEPTH`
floor). It wants the `13 k`-wanted-blocks question answered first.

**Widening it is bounded but still not safe.** `PartitionRead::Whole` was the
piece exactly, which is safe only because a piece is one unit, and the test here
bounds a piece at `k` units rather than at one — so a raised width would have
re-created the un-poolable partition-length buffer `19.19` removed with nothing
going red. `M83` closed that: `Whole` carries the source's unit and the body
read is `min(piece, unit)`, the same read at `k` = 1. It bounds the damage
rather than removing it, a one-unit read being un-poolable at either width and a
file-wide maximum still able to cross into a smaller successor block. What a
wider cut actually wants is the read clipped to the next boundary, filed beside
the mechanism ([`architecture.md`](architecture.md), "cut-width") and belonging
with whatever explains the collapse.

## For `19.17.1`

**Its premise is now undermined from two directions and it should be re-tested
before it is built.** It exists to record an OOM-killed leg and continue. On the
arrangement that ships, nothing was killed in any cell of any family here, as
nothing was in `19.19`'s acceptance run. The kills in the table above are all in
binaries that do not ship — which is, admittedly, exactly the argument *for* the
row: a diagnostic sitting over a candidate arrangement is where a kill happens,
and this sitting's `k2` cell would have taken the whole `reserve` figure down
with it under the current fail-fast. Whoever picks it up should decide on that
basis rather than on "does the shipped build die".

## For `19.18` and `19.16`

**Nothing about the compressed arrangement moved**, so their inheritance is
`19.19`'s unchanged: the thin cells are 1 GiB (11.6% left) and 1.5 GiB (10.3%),
and the per-reader and fixed terms are the ones that slice measured. The one
number this sitting adds is a second, independent reading of the flagless 1 GiB
cell on the shipping build — 833–856 MiB median, 863 worst, in three sittings
across two builds — which agrees with `19.19`'s 905.2 MiB worst to within the
spread but is **not independent of it** in the dimension that matters: same
machine, same wrapper, same container parameters, same input
(`evidence` skill, rule 3).
