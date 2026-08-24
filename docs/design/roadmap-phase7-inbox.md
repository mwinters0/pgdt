# Phase 7 inbox — facts filed for its grilling

Evidence found in earlier phases that Phase 7 (scan performance) will need.
**This is a queue, not a document**: when Phase 7 is grilled, walk every entry,
fold it into [`roadmap-phase7-scan-performance.md`](roadmap-phase7-scan-performance.md)
or discard it as stale, and delete this file. See `docs/process.md`,
"Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## `stream::splice` assumes coverage is a contiguous prefix

**Fact.** A query's mapping pass rebuilds `DumpIndex::spans` as `prefix ++
built ++ [Unscanned tail]`, where `prefix` is every span with `end <=
seg_start`. The seam between the two is closed by **extending the last prefix
span** to where the first newly-built span starts. Both halves of that depend
on the prefix being a complete tiling of `[0, seg_start)`: if coverage had an
interior hole, `prefix` would not tile, and extending its last span across the
seam would silently paper over the wrong range.

**Why Phase 7 cares.** This is the *one* place the phase-3 spec's "coverage is
a prefix, by construction" stopped being an observation and became an
assumption in code. That spec section already names Phase 7's device-aware
parallelism as "the plausible future source of interior holes" and argues a
span list (rather than a watermark) is what makes them expressible. It is
right that the *format* allows them — but `splice` does not, and out-of-order
NVMe scanning is exactly what would produce them. Whatever Phase 7 does about
scan ordering has to either keep coverage prefix-shaped or rework `splice`'s
seam rule, and that should be a decision, not a discovery.

**Origin.** Slice 3.2.1.2.1, 2026-08-24. See
[`roadmap-phase3.2.1.2.1-mapping-streaming-split-notes.md`](roadmap-phase3.2.1.2.1-mapping-streaming-split-notes.md),
"What the next slice inherits".

---

## The mapping pass does no per-row work, and reads the target block twice

**Fact.** Since 3.2.1.2.1, a query is two passes. The mapping pass walks bytes
with `Event::Row(_) => {}` — it allocates no `SourceChunk`s, takes no
zero-copy views, and parses no fields; all it needs from a block is the extent
the `\.` terminator gives it. The row pass then re-reads the queried block's
bytes to build batches. So a cold query reads the target block twice, and the
first read is far cheaper per byte than the second.

**Why Phase 7 cares.** [`roadmap-phase7-scan-performance.md`](roadmap-phase7-scan-performance.md)'s
"Two workloads, two algorithms" splits structure discovery from row
extraction; that split is now real in the code rather than notional, and the
two have measurably different cost profiles. The double read is the obvious
thing to measure and the obvious thing to want back — the deferred fix (carry
the live segment's in-flight spans in the opaque `ResumeToken`, so a single
pass can emit rows again without reintroducing the unmapped hole) is written
up under "Future — wanted, unscheduled" in [`roadmap.md`](roadmap.md). Phase 7
is where it should be measured before it is built.

**Origin.** Slice 3.2.1.2.1, 2026-08-24. Decision in
[`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md),
"Mapping and streaming are separate passes".

---

## `attach_text` re-slices every span at every checkpoint

**Fact.** The mapping pass persists after each completed block, and each time
it calls `map::attach_text` over the *whole* span list, not just the new
spans. That is correct rather than lazy — the last span's `end` grows as the
scan advances, so its text has to be re-taken anyway — and the reads are
coalesced over contiguous runs of text-storing spans. Cost is roughly
`blocks × DDL-size` per scan: on koji, ~200 × 154KB against an hour-long scan,
i.e. nothing.

**Why Phase 7 cares.** It is `O(blocks × DDL)`, and Phase 7's whole job is
knowing where the scan's time goes. The bound is fine for koji-shaped input
and could stop being fine for a dump with far more, far smaller blocks — which
is a shape worth deciding whether to care about rather than assuming away.
Phase 7's "Measurement discipline" section is the right place to settle it.

**Origin.** Slice 3.2.2, 2026-08-24. See
[`roadmap-phase3.2.2-span-text-diagnostics-notes.md`](roadmap-phase3.2.2-span-text-diagnostics-notes.md).
