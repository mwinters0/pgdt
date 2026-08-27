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
[`architecture.md`](architecture.md),
"What the next slice inherits".

---

## The mapping pass reads the target block twice, and does almost no per-row work

**Fact.** Since 3.2.1.2.1, a query is two passes. The mapping pass walks bytes
allocating no `SourceChunk`s, taking no zero-copy views, and building no
batches; all it needs from a block is the extent the `\.` terminator gives it.
The row pass then re-reads the queried block's bytes to build batches. So a
cold query reads the target block twice, and the first read is far cheaper per
byte than the second.

**Amended by 4.5/4.5.1** (entry below): the mapping pass is no longer
per-row-free at all. Every mapping pass records the array-shape census from
`Event::Row` — `build_index`, `build_map` and `stream::map_forward` under
either `ScanExtent`, a cold `UntilTargetSettled` query included. The first
read is still far cheaper per byte than the second, but "no per-row work" is
now "a brace/bracket pre-filter per row, and a field split on the rows that
pass it".

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
[`architecture.md`](architecture.md),
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
[`architecture.md`](architecture.md).

---

## An `INSERT`-run scan is CPU-bound at ~5× a `COPY` scan's per-byte cost

**Fact.** Three 3.00 GiB synthetic dumps, same disk, same session, three runs
each (a whole-file `pgdq` scan, 512MB-limited container): a `COPY` block
scans in 2.55–3.27 s, a large-object region in 3.61–4.99 s, an `INSERT` run in
14.55–14.79 s, against a 3.67–3.85 s `cat`-to-`/dev/null` floor for the same
files. The first two are at the I/O floor; the `INSERT` scan is four times
above it, ~11 s of CPU per 3 GiB. The cause is structural, not incidental:
slice 3.6 gave the large-object region a `crate::scan`-level fast path (lines
skipped unread) but left `INSERT` runs decoding every line into `Event::Line`
and pushing it through `preamble::statement_complete`, folding only the
*spans* into one.

**Why Phase 7 cares.** This is a second scanner-level fast path — the same
mechanism `State::InLargeObjectRegion` already is — and Phase 7 owns scan
performance and the "two workloads, two algorithms" split. It is also the one
place where this project's cost claim is currently false in the direction that
matters: `--inserts` output is a shape the fixture tooling generates routinely,
and a koji-scale `--inserts` dump maps in ~75 minutes against the ~15 the
`COPY` rate implies. The design constraint to carry in: an `INSERT` run's end
has no invariant behind it the way `COPY`'s `\.` (I7) and `BLOBS`' `COMMIT;`
(I12) do, so a skip-and-count path needs the string-aware `'`-tracking scan
[`architecture.md`](architecture.md)
("The three regions do not share an end marker") specifies — which Phase 8
Track A's row reader needs anyway.

**Origin.** Out-of-band item M3, 2026-08-25. Measurement and full numbers:
[`../status/history/2026-08-25.md`](../status/history/2026-08-25.md).

---

## Nested Arrow values are built by copying, and a large share of them are viewable

**Fact.** Phase 4 builds every `List`/`Struct` value by copying, including the
`List<Utf8View>` that an array of a string-ish element type resolves to. A good
share of those elements could be views instead: `copy::decode_field` returns a
borrow of the read chunk whenever the field carries no COPY escapes, and `"` is
not in COPY TEXT's escape set (I15), so an ordinary quoted array literal
(`{"a,b","c d"}`) arrives borrowed and each element's content is a contiguous
sub-slice `append_view_unchecked` could point at. Only an element whose text
contains `\` — which forces COPY escaping and makes the whole field owned — or
one needing unescaping has to be copied.

**Why Phase 7 cares.** It owns the zero-copy path and its three sharp edges
(chunk retention in the deque, `StringViewBuilder` block-index invalidation on
every flush, the straddling-field case), and widening that path into a
*recursive* builder means honouring all three at every level of `List` and
`Struct` nesting. Phase 4 deliberately declined to do that so a new family's
correctness would not ride on the most delicate machinery in the codebase, and
left its own array measurement as an honest copying baseline to improve on. The
measurement to take first is that baseline against a viewing variant on the
array-heavy stress section — the gap is what says whether the nesting
complexity is worth it.

**Origin.** Phase 4 grilling, 2026-08-25. Decision and its rationale:
[`roadmap-phase4-composite-decoding.md`](roadmap-phase4-composite-decoding.md),
"Nested elements copy".

---

## Every mapping pass now does per-row work, and its cost is only half measured

**Fact.** Slice 4.5 put the array-shape census in `map::Builder::on_row`, fed
from `Event::Row` by `build_index`, `build_map` and `stream::map_forward`;
4.5.1 removed the `ScanExtent::Full` gate, so **a cold query's mapping pass
censuses too**. Every data row of every block any mapping pass maps is now
inspected. A row containing neither `{` nor `[` is rejected after one pass over
its bytes and never split into fields; a row containing either is split by
`copy::split_fields` and every field's first bytes examined.

Measured on the 3.00 GiB `COPY` control, alternating pre-census and census
binaries in one session: **free at the available resolution** (differences of
−3% to +2.5%, falling on both sides of zero, while both tracked the warming
page cache from 5.9 s to 3.2 s). But that control holds **no `{` or `[` in any
data row**, so it measures the pre-filter and nothing else. The cost on
array-bearing rows — where every field is inspected — is unmeasured;
[`measurements.md`](measurements.md), "The array shape census costs nothing on
brace-free data", says so explicitly.

**Why Phase 7 cares.** The double-read entry above recorded that the mapping
pass did no per-row work; that is no longer true of any mapping pass, and the phase's
device-bound targets are set against a scan loop that has since grown a
per-row stage. Two specific consequences: a parallel or reordered scan has to
carry the census with whatever unit it splits the file into (it accumulates
per block and is finalized at `CopyEnd`), and any decision to widen the census
— per-path keying, or the per-row-group statistics `RowGroupStats` reserves —
lands on the same per-row stage and should be measured against array-bearing
data, which does not exist as a benchmark input until slice 4.6 generates it.

**Origin.** Slices 4.5 and 4.5.1, 2026-08-26. See
[`roadmap-phase4.5.1-census-consumption-notes.md`](roadmap-phase4.5.1-census-consumption-notes.md)
and [`architecture.md`](architecture.md), "The array shape census".


---

## Mapping is O(blocks²) after the save throttle, and the remaining half is the span splice

**Fact.** `pgdq parse` used to serialize the **whole** cache at every `CopyEnd`
watermark. Slice 9.5 throttled that (`SaveThrottle`: skip a save unless 20x the
last save's own duration has elapsed), which cut 4000-block saves from 4003 to
195 and 44.3 s to 23.6 s. The series is **still** 4x per doubling, because a
second cost has the same shape: every `CopyEnd` rebuilds `DumpIndex::spans`
whole — `map::Builder::snapshot` clones the builder's span vector, then
`stream::splice` clones the prefix and concatenates — so the map alone is
O(blocks²) with the cache disabled entirely (19.7 s for 4000 blocks under
`query --dqcache none`, against under 10 ms for the same bytes in one block).
koji cannot show either half: 74 blocks over 784 GB.

**The cheap fix, and what it costs — decided against, so the phase does not
re-derive it.** `index.spans` is rebuilt at every `CopyEnd`, but for `pgdq
parse` nothing reads it between saves: `target` is `None`, so `target_settled`
never runs, and the only consumers of a current `index` are the throttled save
and the chunk-top interrupt save. Moving the `splice` *inside* the existing
`if settled || cancelled || throttle.due()` arm would therefore fire it `n/20`
times instead of `n` — roughly 19.7 s → 1 s at 4000 blocks — using the gate
9.5 already built, no redesign. **It was rejected anyway**, because it trades
away the guarantee 9.5 spent a slice establishing: today an interrupt banks the
last *completed block*, and under the gated splice it would bank the last
*saved* watermark, so a Ctrl-C would lose up to `K` blocks instead of one. The
coupling cannot be worked around locally either — `map::Builder::snapshot`
`debug_assert!`s `Mode::Idle`, so the chunk-top check cannot re-derive the
spans mid-block. Phase 7 may still take it, but as a deliberate change to the
interrupt's promise, not as a cleanup.

**Why Phase 7 cares.** The phase's target is a device-bound scan path, and this
is a *CPU* cost inside the scan loop that the 243 MB/s koji baseline the phase
doc opens with cannot see — on a block-rich, byte-poor dump the scan is not
device-bound at all. Two consequences. First, the fix is in the same code the
phase's parallelism plans would rework: keeping the frontier's spans appendable
instead of rebuilt (and `stream::splice` is already flagged above for assuming
coverage is a contiguous prefix), so the two should be decided together.
Second, the throttle's constant `K = 20` is a starting value chosen against
this series; a scan whose per-block cost changes is a scan whose save cadence
changes with it.

**Origin.** Slice 9.5, 2026-08-27. Figures, both series and their commands:
[`measurements.md`](measurements.md), "Per-block cache saving is quadratic in
block count, and so is the map".
