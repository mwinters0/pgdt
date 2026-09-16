# P20.8 — the reserve read, and the line put down

What the attribution sitting found, and why the phase stopped here rather than
setting the constant it was taken for.

## The sitting stands; the constant it was for does not get set

The legs the spec registered were run on the `introspect` build: a flagless
gathering `parse` and a flagless `query` over the cache that same container's
`parse` wrote, over a reasonable-width input and a wide-text one, on plain,
24 MiB-block `.xz` and 128 MiB-block `.xz`, across six container limits, plus
the two starve-band legs — 370 leg-runs, five reps each. The readings are a
`runs/` artifact, named by the dated entry; **nothing here is a figure** and
nothing here is a timing, an attribution build taking an atomic per allocation.

`MEMORY_RESERVE` is **left at 384 MiB**. The phase stopped because the
constant answers to an architecture still moving — P21 changes who gathers,
P22 changes what the one number means — so fitting it now prices a model that
is about to be replaced, and re-fitting one term a round against an account
nobody has finished is what the evidence rule forbids. The readings do not
expire with the phase: they are P23's input, and no sitting re-takes them.

## What the readings say

**The remainder outruns the reserve** (`KD34`). Worst 544 MiB, at
`control-xz24-2g-query`, leaving 6.5% of the limit; rounded to a 64 MiB step
that is 576 MiB against the 384 MiB held back. It is worst on *compressed
`query`* legs, which is the arrangement nobody had read: a query decodes a
whole cache's statistics and bills none of it.

**Every `wide-xz24` `query` leg from 1 GiB up was OOM-killed in all five
reps**, as was `control-xz24-starve-parse`. Those are censored readings — a
peak the process never reached — and they enter no maximum. They are also on
the instrumented build, so they say nothing about whether the shipped build
dies there; that is what a blind gate is for, and the gate is unrun.

**The plain legs read negative**, to about −57 MiB, which is `KD25` and not a
finding of this sitting: the plain source bills `PLAIN_PARTITION_CHUNKS ×
chunk` a reader where the path holds `POOL_DEPTH` chunks flat, so its charge
describes something nothing holds.

## The negative results

**The wide-text input could not do what it was built for, and the arithmetic
it rested on was wrong.** It was sized so that 64 text columns, each drawing
from a pool of exactly `DICTIONARY_CAP` distinct 128-byte values, would fill
whatever allowance a run resolved. Dictionary *text* is interned once per
block and column — a group holds `Vec<u32>` indices into
`ColumnDictionary::entries`, not the text — so the whole 3 GiB block carries
512 KiB of dictionary text once, not 512 KiB per group. Measured account peak:
**51.86 MiB, identical at every limit**, against a smallest allowance of
89.60 MiB. It declined nothing on plain at any limit, and declined on `.xz`
only because the allowance there collapses. The generator was deleted rather
than corrected, nothing now consuming it.

**Widening rows is self-defeating as a way to raise statistics volume.** Final
groups are about `min(block_bytes / 1 MiB, STATISTICS_GROUP_CAP, rows / min_rows)`,
so above a row width of `DEFAULT_STATISTICS_GROUP_SIZE / DEFAULT_STATISTICS_MIN_ROWS`
the density merge binds and doubling the row width halves the retained volume.
At 64×128 B the rows are 8 KiB: 3072 base groups collapse to 192. What raises
volume instead is many columns of *short* values, a bytewise-comparable column
so groups earn min/max bounds — the only text term that scales per group — and
more blocks.

**The statistics allowance is non-monotone in the container limit.** It is
`margin_allowance(allowance) − budget`, a residual of two large terms, and
across the `xz24` legs it ran 47.6 → 39.1 → 6.9 → 0.04 → 10.2 → 13.6 MiB as
the limit grew. A bigger box buys less statistics headroom across part of the
range, and the control declines in the cells where the wide input does. This
is P22's, being a property of the one number rather than of statistics.

## What the next phase inherits

`KD33` and `KD34`, the readings, and one instrument:
`instrument::statistics_loaded`, the heap a cache load hands a pass, which is
the only statistics term a query has — a query keeps no account, and the scope
counter alone reads a fraction of what is held. It is `introspect`-only and
the shipped binary is unchanged by it. The registered branch — a remainder
growing with the statistics volume is billed to the query, not reserved — is
unspent, and is filed for P22 as well, being a fourth consumer of the one
number.
