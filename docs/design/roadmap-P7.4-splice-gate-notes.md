# P7.4 — the splice rides the throttle's gate

What the next slice inherits from `KD5`'s discharge. The mechanism itself is
filed by subject: [`architecture.md`](architecture.md), "`parse` resumes, and
saves as it goes".

## The spec's gate could not be written literally, and the reason is a reader

The spec says the splice moves inside `if settled || cancelled ||
throttle.due()`. That condition cannot be evaluated: `settled` is
`target_settled(index, …)`, which reads `index.spans` — the very list the
splice rebuilds — so a gate that consults it before splicing reads a map that
stops at the last gate opening and can never see the block that just closed. A
literal implementation compiles, passes every existing test, and silently
costs the early stop: a cold query would map to EOF.

The gate therefore has **three** openers, and the third is the repair: a
completed block whose `COPY` header names the queried table. It is sound
because `target_settled` can only turn from false to true when a block it
*counts* is added — its other two tests (a `Connect` span anywhere, a
`partition_root` on a matching block) only ever veto — so splicing for those
blocks keeps the answer exact while every other block skips.

It matches on the **header alone**, which is deliberately a superset of
`target_settled`'s own test: a block whose database the selector excludes
cannot settle the target either, but repeating that test at the gate would tie
the gate's width to `target_settled`'s body. Over-splicing costs one clone on a
block already named by the query; under-splicing costs the early stop, and only
one of those two failures is visible.

**The evidence, since neither half is visible from a three-block fixture.**
Instrumenting the gate over the 40-block dump
`tests/map_file.rs::block_rich` builds: 39 of 40 blocks report `due=false`, and
the settling block reports `targets=true cancelled=false due=false`. The third
opener is the only thing firing there.

## Removing the splice removed most of the saving too

The two costs were multiplying, not adding. `SaveThrottle` is a ratio against
*elapsed scan time*, so a scan inflated by per-block splicing is a scan that
keeps earning further saves. At 4000 blocks the save count falls **105 → 5**
with no change to the throttle, and the whole `parse` falls 20.75 s → 0.113 s —
a factor the spec's own arithmetic (19.0 s → ~1 s) did not predict, because it
priced the splice against a scan whose length the splice was setting.

Two consequences for later slices:

- **The interrupt's window is smaller than the spec quantified.** It was
  budgeted at ~0.2 s of scanning at 4000 blocks, on 105 saves across a 20.8 s
  scan. It is 5 gate openings across a 0.113 s scan, so ~20 ms.
- **A save count is a weaker signal than it was.**
  [`architecture.md`](architecture.md)'s "A save count is a property of the
  apparatus, not only of `K`" still holds and has been widened: the count is
  now dominated by how fast the rest of the loop is, not only by how expensive
  the cache is, because the loop and the cache share one gate. It is a reading
  about the whole apparatus and never a number to compare across builds.

## The residual, and the repair that was refused

`--dqcache none` is **unchanged**: 19.07 / 3.98 / 1.004 s at 4000 / 2000 /
1000 blocks against 19.03 / 4.56 / 1.012 before. `cache.save` is a no-op there,
so `last_cost` is ~0, `due()` is always true, and the gate never closes. That
is `KD5`'s whole remainder and it is measured rather than argued — which is why
`map-only` was re-taken beside `per-block-quadratic` even though the change was
expected to leave it alone.

Giving the throttle a floor so a disabled cache throttles too was considered
and refused; the reasoning is beside the mechanism
([`architecture.md`](architecture.md), same section) and the short form is that
`map_file` under a disabled cache hands its `DumpIndex` back to the caller, so
a gate that never opens hands back a map of nothing.

`KD5` is therefore rewritten rather than struck, and re-homed onto the
parallel-scan phase, which the P7 spec already named as the destination for the
appendable-spans rework.

## Three figures were blind to the file they measure

`measure.py`'s `MAP` constant was `("pgdump_query/src/map.rs",)`, and both
figures that price the map — `per-block-quadratic` and `map-only` — declared it
without `pgdump_query/src/stream.rs`, where `splice` lives. This change would
have read **green** against the two tables it moved by two orders of magnitude.
They now declare `MAP_BUILD`, which is both files.

`preamble-prepass` was blind in a second way, and it is the one to look for
next: its second row is not its own measurement but `per-block-quadratic`'s
4000-block "after" reading, *borrowed* within the session (`requires`). A
borrowed row carries the lending figure's staleness edges and this one declared
none of them, so a change to the map moved a published row here that read
green. It now declares them explicitly. The harness models the sharing
(`Figure.shares`) without deriving the *staleness* edges from it; deriving
them is the general fix and was left alone as harness surgery this slice did
not owe.

Worth generalising when the next figure is registered: a figure's `depends`
must name every file on the path it times *and* the edges of every reading it
borrows, not the file the mechanism is *named* after.

## The spec is not amended, and that is the rule working

`per-block-quadratic`'s `quoted_by` names
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md), so the
fold-in re-read it. Its lever table still says "19.0 s of a 20.8 s 4000-block
`parse`" and "Discharging `KD5`" still prices the gate at roughly 19.0 s → 1 s.
Both stay. Those are the readings the phase was *committed to* when it chose
this lever, and progress never goes in the spec
([`../process.md`](../process.md), "Progress lives in STATUS, never in the
spec"). The outcome lives here, in
[`architecture.md`](architecture.md) and in the checklist entry.

## What 7.13 inherits

The 4000-block `parse` profile that `architecture.md` quotes — 43%
`stream::splice` over 57% allocator traffic — described the build this slice
replaced, and that shape's allocator picture is now mostly gone. It does **not**
move 7.13's allocator question: `--figure allocator` runs on the 3.00 GiB
single-block control and arrays files, where there is one `CopyEnd` and the gate
is open at it, so nothing this slice did reaches those three readings. What is
gone is the *evidence* for reading the allocator's cost off a block-rich `parse`;
7.13's own case rests on `read_range`'s per-chunk `vec![0u8; 1 MiB]`, which the
control profile prices at 23.2% of user time and which this slice did not touch.

## What 7.12's sweep inherits

Three tables were taken in one sitting on this build — `per-block-quadratic`,
`map-only` and `preamble-prepass`, which borrows the first's 4000-block "after"
reading and so had to be in the same session (`runs/measure-20260903T182834`),
the way 7.3 took `allocator`. Six other figures
went red on `pgdump_query/src/stream.rs` in this change —
`census-brace-free`, `census-arrays`, `scan-throughput-cold`,
`scan-throughput-warm`, `projection-widths` and `allocator`. All six time
single-`COPY`-block inputs, where the gate is open at the one `CopyEnd` there
is, so the path is reachable and identical; there is no mechanical oracle for
that, so they stay red with the reason written down
([`../status/STATUS.md`](../status/STATUS.md)).
