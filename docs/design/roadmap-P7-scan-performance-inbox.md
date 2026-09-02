# P7 inbox — facts filed for its grilling

Evidence found in earlier phases that P7 (scan performance) will need.
**This is a queue, not a document**: when P7 is grilled, walk every entry,
fold it into [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md)
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

**Why P7 cares.** This is the *one* place the P3 spec's "coverage is
a prefix, by construction" stopped being an observation and became an
assumption in code. That spec section already names P7's device-aware
parallelism as "the plausible future source of interior holes" and argues a
span list (rather than a watermark) is what makes them expressible. It is
right that the *format* allows them — but `splice` does not, and out-of-order
NVMe scanning is exactly what would produce them. Whatever P7 does about
scan ordering has to either keep coverage prefix-shaped or rework `splice`'s
seam rule, and that should be a decision, not a discovery.

**Origin.** 2026-08-24. See
[`architecture.md`](architecture.md),
"What the next slice inherits".

---

## The mapping pass reads the target block twice, and does almost no per-row work

**Fact.** A query is two passes. The mapping pass walks bytes
allocating no `SourceChunk`s, taking no zero-copy views, and building no
batches; all it needs from a block is the extent the `\.` terminator gives it.
The row pass then re-reads the queried block's bytes to build batches. So a
cold query reads the target block twice, and the first read is far cheaper per
byte than the second.

**Amended** (entry below): the mapping pass is no longer
per-row-free at all. Every mapping pass records the array-shape census from
`Event::Row` — `build_index`, `build_map` and `stream::map_forward` under
either `ScanExtent`, a cold `UntilTargetSettled` query included. The first
read is still far cheaper per byte than the second, but "no per-row work" is
now "a brace/bracket pre-filter per row, and a field split on the rows that
pass it".

**Why P7 cares.** [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md)'s
"Two workloads, two algorithms" splits structure discovery from row
extraction; that split is now real in the code rather than notional, and the
two have measurably different cost profiles. The double read is the obvious
thing to measure and the obvious thing to want back — the deferred fix (carry
the live segment's in-flight spans in the opaque `ResumeToken`, so a single
pass can emit rows again without reintroducing the unmapped hole) is written
up under "Future — wanted, unscheduled" in [`roadmap.md`](roadmap.md). P7
is where it should be measured before it is built.

**Origin.** 2026-08-24. Decision in
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

**Why P7 cares.** It is `O(blocks × DDL)`, and P7's whole job is
knowing where the scan's time goes. The bound is fine for koji-shaped input
and could stop being fine for a dump with far more, far smaller blocks — which
is a shape worth deciding whether to care about rather than assuming away.
P7's "Measurement discipline" section is the right place to settle it.

**Origin.** 2026-08-24. See
[`architecture.md`](architecture.md).

---

## An `INSERT`-run scan is CPU-bound at mid-teens times a `COPY` scan's per-byte cost

<!-- deficiency: KD9 -->
**This entry is deficiency `KD9`'s detail** (`../status/STATUS.md`, "Known
deficiencies"), filed here because the analysis is here and this phase is the
destination the index names. Draining this inbox moves the paragraph rather
than deleting it — `scripts/deficiencies.py` fails until it lands somewhere.

**Fact.** The warm figure exists now, and it settles the ratio this entry was
filed under. Three 3.00 GiB synthetic dumps, one sweep, both regimes
(512 MB-limited container, timer inside it, glibc):

| | `COPY` block | large object | `INSERT` run | `dd` floor |
|---|---|---|---|---|
| warm, tmpfs | 0.500 s | 0.464 s | **8.37 s** | 0.275 s |
| cold, SSD | 5.77 s | 5.76 s | 9.85 s | 5.75 s |

**An `INSERT` run costs 16.7× a `COPY` block's per-byte CPU** — 8.37 s against
0.500 s on the same 3.00 GiB — and 30× the warm device floor where the `COPY`
path is 1.8×. The **"~5×" this entry was filed under was never a CPU ratio**:
it divided a cold `INSERT` rate, device included, by the `COPY` path's warm
CPU. A single-regime measurement replaces it, landing below the
16–26× the cold table alone was used to bound it at. Carry it as
**mid-teens, never to three figures**: sweeps under an apparatus the telemetry
witnesses quiet have read 14.4×, 14.5×, 14.6×, 14.9× and 16.7× on the same
binaries and inputs, both legs moving inside the measured session drift. The
`COPY` leg is the one that moves — half a second, memory-bandwidth-bound —
which is why the ratio is carried as a magnitude rather than a value.

The cause is structural, not incidental: the large-object region has a
`crate::scan`-level fast path (lines skipped unread) but left `INSERT` runs
decoding every line into `Event::Line` and pushing it through
`preamble::statement_complete`, folding only the *spans* into one.

Correctness, tiling and row counts are unaffected: the cost is CPU spent
decoding lines whose spans are folded into one either way.

**Why P7 cares.** This is a second scanner-level fast path — the same
mechanism `State::InLargeObjectRegion` already is — and P7 owns scan
performance and the "two workloads, two algorithms" split. It is also the one
place where this project's cost claim is currently false in the direction that
matters: `--inserts` output is a shape the fixture tooling generates routinely,
and a koji-scale 1 TB `--inserts` dump spends CPU a `COPY` dump of the same
size does not. Scaled from the warm figures, 1 TB of `INSERT` runs is **~40
minutes of CPU** against the `COPY` path's **~3** — the same order the old ~5×
gave, arrived at from a single regime this time rather than by dividing across
two. The design constraint to carry in: an `INSERT` run's end
has no invariant behind it the way `COPY`'s `\.` (I7) and `BLOBS`' `COMMIT;`
(I12) do, so a skip-and-count path needs the string-aware `'`-tracking scan
[`architecture.md`](architecture.md)
("The three regions do not share an end marker") specifies — which P8
Track A's row reader needs anyway.

**The cold `INSERT` reading also moved, and not because of the apparatus**:
15.13–15.42 s when first taken, 9.85 s now. `2eb51f4` changed how an
`--inserts` dump's runs are scanned in between — absorbing the `Data for`
comment into the run. Attributing the difference needs a build from before that
commit and a second cold table, which nothing yet requires —
the warm figure above is what P7 actually consumes.

**Origin.** Out-of-band item M3, 2026-08-25; the table re-taken cold by `M10`,
2026-08-27; re-taken again under the committed harness on 2026-08-28 and on
2026-08-30
([`../status/history/2026-08-30.md`](../status/history/2026-08-30.md)),
which is where the movement above comes from
([`measurements.md`](measurements.md), "Scan throughput by input shape"). The
original measurement: [`../status/history/2026-08-25.md`](../status/history/2026-08-25.md).
**Contingent on** nothing else changing `scan.rs`, `copy.rs` or `map.rs`:
`uv run measure.py --stale` names this figure when one of them does.

---

## Nested Arrow values are built by copying, and a large share of them are viewable

**Fact.** P4 builds every `List`/`Struct` value by copying, including the
`List<Utf8View>` that an array of a string-ish element type resolves to. A good
share of those elements could be views instead: `copy::decode_field` returns a
borrow of the read chunk whenever the field carries no COPY escapes, and `"` is
not in COPY TEXT's escape set (I15), so an ordinary quoted array literal
(`{"a,b","c d"}`) arrives borrowed and each element's content is a contiguous
sub-slice `append_view_unchecked` could point at. Only an element whose text
contains `\` — which forces COPY escaping and makes the whole field owned — or
one needing unescaping has to be copied.

**Why P7 cares.** It owns the zero-copy path and its three sharp edges
(chunk retention in the deque, `StringViewBuilder` block-index invalidation on
every flush, the straddling-field case), and widening that path into a
*recursive* builder means honouring all three at every level of `List` and
`Struct` nesting. P4 deliberately declined to do that so a new family's
correctness would not ride on the most delicate machinery in the codebase, and
left its own array measurement as an honest copying baseline to improve on. The
measurement to take first is that baseline against a viewing variant on the
array-heavy stress section — the gap is what says whether the nesting
complexity is worth it.

**The copying baseline exists, and it says the parse is the smaller half.**
On a 3.00 GiB dump whose every row carries a 4-element array, a 50-element
array and a two-field composite, `pgdq query --schema-mode typed` costs
3.70× the same query in `strings` mode, against 2.35× for the same file
without those three columns — so the three nested columns account for about
**13.2 µs of every row**, nearly twice what the sixteen scalar columns cost
together. **The two arrays carry all of it, and that is now read directly
rather than differenced across files**: projecting the columns in and out of
one file puts the two arrays at **+12.98 µs/row** against the composite
column's **+0.77** (the projection entry below). The
`nested.rs` literal parse and its render account for **6.7 µs** of the
13.2; the remaining ~6.5 µs is the Arrow build — 56 per-element
`append_value` calls into child builders, plus list offsets. The
micro also puts the array cost per *element* (88 ns decoding, 28 ns
rendering), which is the shape of one allocation each, since
`ArrayLiteral::elements` is a `Vec<Option<String>>`. **Two targets, then, not
one**: viewing instead of copying attacks the build, and it is the larger
share — but a `Vec<Option<String>>` intermediate is paid before the build is
reached, so a viewing builder that still routes through `decode_array` keeps
the 6.7 µs.

**The prize, measured against the path this phase would widen.** One
`append_view_unchecked` into a borrowed block costs **2.95 ns**, against
19 ns to copy 49 bytes and 24 ns to copy 601 — so the two micro controls
bracket the question: the literal parse is 26.0× a copy and 171× a view for a
4-element array, 246.2× and 1967× for a 50-element one. 2.95 ns is a floor
rather than the borrowed arm itself (`push_utf8view_field` also scans the chunk
deque and calls `block_for`), so those view ratios bound the real ones from
above. The copy control is itself the least stable number in that table — it
has read 24, 42, 41 and 24 ns at 601 bytes across four sweeps, which alone put
the `÷ copy` ratio at 122× in one sweep and 246× in the next — so read the
`÷ copy` ratios as an order of magnitude. Read together with the ~6.5 µs build share
above, the shape of the answer is that **viewing is worth far more than the
parse is**, and worth most on long arrays.

**Origin.** 2026-08-25; the figures 2026-08-27, re-taken the same day once the generator declared the
types `pg_dump` writes — the earlier end-to-end ratios were taken with three
of the sixteen scalar columns silently untyped. Decision and its rationale:
[`architecture.md`](architecture.md), "Nested columns: `NestedPlan` travels
beside the `DataType`" (nested values always copy); figures and commands:
[`measurements.md`](measurements.md), "Nested decode costs what it copies" and
"A typed query over nested columns".

---

## Every mapping pass now does per-row work, and on array-bearing rows it is not free

**Fact.** pgdq puts the array-shape census in `map::Builder::on_row`, fed
from `Event::Row` by `build_index`, `build_map` and `stream::map_forward`;
There is no `ScanExtent::Full` gate on it, so **a cold query's mapping pass
censuses too**. Every data row of every block any mapping pass maps is now
inspected. A row containing neither `{` nor `[` is rejected after one pass over
its bytes and never split into fields; a row containing either is split by
`copy::split_fields` and every field's first bytes examined.

Both sides are measured, on 3.00 GiB files served from tmpfs to a 512 MB
container, alternating a census and a no-census glibc binary in one session,
six reps each in both orders. On the brace-free `COPY` control — every row
rejected by the pre-filter — the census costs **0.039 s per 3.00 GiB**, 48 ns
per 16-column row: **+8%** of a warm scan. On a file where **every** row
carries an array it costs **1.123 s per 3.00 GiB**, 1.61 µs per 19-column row:
**+225%** warm. Cold from this SSD the device floor hides both entirely, at
+0% and +1% — measured in the same sweep, so the two regimes are one
apparatus.

**So the census's cost is the field split, not the pre-filter**: 97% of it
falls on the rows the pre-filter passes. That inverts the reading this entry
carried until `M11` — a scalar `raw.iter().any(…)` at ~3.8 GB/s made the
pre-filter ~40% of the census's cost even on rows it rejected; `memchr::memchr2`
makes it 3%.

**Why P7 cares.** The double-read entry above recorded that the mapping
pass did no per-row work; that is no longer true of any mapping pass, and the phase's
device-bound targets are set against a scan loop that has since grown a
per-row stage. Two specific consequences: a parallel or reordered scan has to
carry the census with whatever unit it splits the file into (it accumulates
per block and is finalized at `CopyEnd`), and any decision to widen the census
— per-path keying, or the per-row-group statistics `RowGroupStats` reserves —
lands on the same per-row stage, and the array-bearing figure above is the
baseline to measure it against. A third consequence the figures add: **the
census's visibility is decided by the data's shape, not by the code**. A dump
of koji's shape pays 9% of a memory-resident scan and nothing at all off a
device; a dump whose rows all carry arrays pays about 2.5× the rest of the
scan, on any regime that is not device-bound. A phase that succeeds in making
the scan CPU-bound makes the second case its dominant cost.
`scripts/generate_perf_data.py --arrays --composite` is the input.

**Origin.** 2026-08-26; the array-bearing figure 2026-08-27, both re-taken by
that day's warm-set sweep, which is what re-priced the pre-filter. See
[`architecture.md`](architecture.md), "The array shape census";
[`measurements.md`](measurements.md), "The census on array-bearing rows".


---

## What each column costs is now a within-file reading, at five projection widths

**Fact.** `pgdq query --column`/`--no-columns` exists, and
[`measurements.md`](measurements.md)'s "What a column costs: five projection
widths over one file" reads one 3.00 GiB `--arrays --composite` file five ways,
warm and typed, over identical rows:

| Projection | Median | Per row | Δ against the row above |
|---|---|---|---|
| 0 — `--no-columns` | 2.30 s | 3.28 µs | — |
| 1 — `v_smallint` | 2.39 s | 3.41 µs | +0.13 µs |
| 16 — every scalar | 9.59 s | 13.71 µs | +10.30 µs |
| 17 — the scalars and `v_comp` | 10.17 s | 14.52 µs | **+0.77 µs** |
| 19 — every column | 19.25 s | 27.50 µs | **+12.98 µs** |

**The zero-column row is the replay floor** — the block read, every row walked
and field-counted, the predicate evaluated, nothing decoded or built: 3.28 µs a
row, 2.30 s of the 19.25 s a complete typed read costs.

**Why P7 cares.** Three things, and the first is a method rather than a number.

**Every "what does this one thing cost" question this phase will ask can now be
asked within one file.** The campaign's questions — what viewing instead of
copying saves on `List<Utf8View>`, what a sparse row index costs per block —
are all of that shape, and the cross-file subtraction they would otherwise use
has a ±0.5 µs/row floor (the entry below). A projection difference has neither
that floor nor the census's file-dependent untyped baseline: same file, same
rows, same bytes. The roadmap's rule that a slice committing to a measurement
names its instrument is much cheaper to satisfy with this one.

**The 12.98 µs the two array columns cost is the number the viewing work is
worth against**, and it is a within-file reading rather than the cross-file
estimate the nested entries above were built on. The composite column's 0.77 µs
lands inside the +0.31 to +0.99 range seven cross-file sweeps read for it,
which is the cross-file instrument being imprecise rather than wrong.

**The replay floor is what a device-bound target is measured against.** 2.30 s
per 3.00 GiB warm is what replay costs before any column is built, so a phase
that makes the scan CPU-bound is working against that floor and not against
zero. It is also the figure that moves if `push_row`'s walk is ever changed:
`projection-widths` is the one query figure declaring the scan path as well as
the decode path, precisely because its top row is an absolute rather than a
difference.

**Origin.** `P5.7`, 2026-08-30. Figure, commands and apparatus:
[`measurements.md`](measurements.md), "What a column costs". The mechanism:
[`architecture.md`](architecture.md), "Projection".

---

## Mapping is O(blocks²) after the save throttle, and the remaining half is the span splice

<!-- deficiency: KD5 -->
**This entry is deficiency `KD5`'s detail** (`../status/STATUS.md`, "Known
deficiencies"), filed here because the analysis is here and this phase is the
destination the index names. Draining this inbox moves the paragraph rather
than deleting it — `scripts/deficiencies.py` fails until it lands somewhere.

**Fact.** `pgdq parse` serializes the **whole** cache at a `CopyEnd`
watermark, throttled (`SaveThrottle`: skip a save unless 20x the last save's own
duration has elapsed) — which holds 4000-block saves to 108 rather than 4003,
and a 46.4 s scan to 20.3 s. The series is **still** 4x per doubling, because a
second cost has the same shape: every `CopyEnd` rebuilds `DumpIndex::spans`
whole — `map::Builder::snapshot` clones the builder's span vector, then
`stream::splice` clones the prefix and concatenates — so the map alone is
O(blocks²) with the cache disabled entirely (18.8 s for 4000 blocks under
`query --dqcache none`, against 5 ms for the same bytes in one block).
**The map is most of a throttled `parse` at 4000 blocks** — 18.8 s of 20.3 —
so the splice is what is left to close, not the cache. koji cannot show either
half: 74 blocks over 784 GB.

**The throttle is self-tuning, which matters for reading its counts.** `K` is
a ratio against the last save's own duration, so a faster machine, libc or
allocator saves *fewer* times rather than the same number more cheaply — the
4000-block count reads 108 on glibc against 110 on musl and 195 on the SSD-warm
session that first recorded it. A save count is a property of the apparatus,
not only of `K`.

**The cheap fix, and what it costs — decided against, so the phase does not
re-derive it.** `index.spans` is rebuilt at every `CopyEnd`, but for `pgdq
parse` nothing reads it between saves: `target` is `None`, so `target_settled`
never runs, and the only consumers of a current `index` are the throttled save
and the chunk-top interrupt save. Moving the `splice` *inside* the existing
`if settled || cancelled || throttle.due()` arm would therefore fire it a few
dozen times instead of `n` — roughly 18.8 s → 1 s at 4000 blocks — using the
gate the throttle already built, no redesign. **It was rejected anyway**,
because it trades away the interrupt guard's central guarantee: today an
interrupt banks the last *completed block*, and under the gated splice it would bank the last
*saved* watermark, so a Ctrl-C would lose up to `K` blocks instead of one. The
coupling cannot be worked around locally either — `map::Builder::snapshot`
`debug_assert!`s `Mode::Idle`, so the chunk-top check cannot re-derive the
spans mid-block. P7 may still take it, but as a deliberate change to the
interrupt's promise, not as a cleanup.

**Why P7 cares.** The phase's target is a device-bound scan path, and this
is a *CPU* cost inside the scan loop that the 243 MB/s koji baseline the phase
doc opens with cannot see — on a block-rich, byte-poor dump the scan is not
device-bound at all. Two consequences. First, the fix is in the same code the
phase's parallelism plans would rework: keeping the frontier's spans appendable
instead of rebuilt (and `stream::splice` is already flagged above for assuming
coverage is a contiguous prefix), so the two should be decided together.
Second, the throttle's constant `K = 20` is a starting value chosen against
this series; a scan whose per-block cost changes is a scan whose save cadence
changes with it.

**Origin.** 2026-08-27, with the save throttle. Figures, both series and their commands:
[`measurements.md`](measurements.md), "Per-block cache saving is quadratic in
block count, and so is the map".

---

## Attributing a cost to one column by differencing two generated files bottoms out at ~0.5 µs/row

**Fact.** Attributing one column's cost by differencing two generated files
that differ by that column — subtracting their per-row `typed` − `strings`
figures — has a floor, and this is it. Column projection now answers that
question within one file (the entry above), so what this figure is for is the
calibration itself: any *cross-file* per-row difference this campaign takes is
read against this number. Two 3.00 GiB files holding the *same*
sixteen columns and differing only in their RNG seed (`--seed 42` against
`--seed 43`) disagree by a paired median of **+0.08 µs/row**, spanning −0.02 to
+0.29 over six interleaved reps, when they should disagree by zero; the file
differing by one composite column reads a paired median of **+0.60 µs/row**
over five reps, whose per-rep readings (+0.51 to +0.80) clear the floor's in
this sweep, while other sweeps of the same quantity read +0.31, +0.39, +0.62,
+0.69, +0.72 and +0.99 — takes of one quantity 0.7 µs/row apart, which is wider
than the floor. The within-file reading settles the quantity at +0.77, inside
that whole range.
Anything under roughly **±0.5 µs/row** out of this instrument is apparatus. Two
contributors are known and neither is removable within it: the files hold
different row counts at the same byte size, so a per-byte component does not
normalize away per row; and a slow upward drift
across a long session lands on whichever file is measured later, which is why
the runs interleave files rather than running them in blocks.

**A third contributor is present and *is* removable — the census, measured.**
A `strings` leg is a mapping pass plus a row pass, and the mapping pass runs
the array-shape census, whose cost depends on whether the rows carry a `{`.
The `--arrays --composite` file's `strings` leg is 27% above the control's;
with the **census-off** binary on the same two files the gap all but vanishes,
from +1.128 s to +0.036 s — 97% of it is the census, on a file carrying 14%
*fewer* rows than the control. So a cross-file subtraction that changes the
*brace-bearingness* of the rows compares two different baselines, and only the
per-file `typed` − `strings` difference cancels it. The same run reproduces
the census's own cost on a different command to within 3% of the `parse`
figure on the arrays file, and within a factor of two on the control, where
the comparison is two sub-0.1 s readings off 4.1 s legs. Earlier sweeps could
not see this: at 9.6 s legs a 1 s difference was inside the spread.

**And the floor cannot be tightened by taking more reps** — see the next
entry, which is the same instrument measured on two apparatuses and is the
reason no confidence interval quoted here means what it looks like.

**Why P7 cares.** It is an entire performance campaign, and the questions
it will ask — what viewing instead of copying saves on `List<Utf8View>`, what
a sparse row index costs per block, what a parallel scan wins — are mostly of
the form "what does this one thing cost", against inputs from the same
generator. A campaign that reads a 0.3 µs/row difference as a result will
report noise as a win. **The remedy is the projection instrument above**, not a
sharper cross-file one: it needs no second file, so nothing has to be
normalized away and nothing has to be held byte-identical.

*The other remedy was tried and retired.* A pair of files byte-identical in the
data section and differing only in one column's declared type — real type in
one, `text` in the other — removes the same normalization, and it was built:
`generate_perf_data.py --weak-composite`, the `composite-isolated` figure and a
fidelity test asserting the two data sections matched. It was never run, and
`P5.7` deleted the whole apparatus once projection landed. Its real cost, named
at the 2026-08-27 review, is why it is not worth rebuilding: the knob makes the
generator write a declaration `pg_dump` would not, which is the opposite of what
`M10` corrected it to do, so it needed an explicit exemption from
`pgdump_query-cli/tests/perf_generator_fidelity.rs` rather than just a flag.

**Two standing rules came out of the same review**, both in
[`measurements.md`](measurements.md)'s preamble and both binding on this
campaign. A figure about parsing CPU is taken with its input on **tmpfs**, not
page-cache warm off a filesystem — device time and background I/O swamp the
difference being measured, worst on the HDD, and page-cache residency is an
assumption rather than a guarantee. And a comparison table is re-taken **whole,
in one interleaved sweep**, never differenced against a figure from another
session and never run a file at a time: the `strings` leg alone moved from
9.74 s to 4.43 s on an identical input once the apparatus was fixed, which
dwarfs every result this campaign will chase.

**One bullet of this phase's own spec is now wrong, and the drain must fix
it.** [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md)'s
"Measurement discipline" says to benchmark "from tmpfs **or a warm page
cache**"; the standing rule "a figure about parsing CPU must not be taken
against a filesystem" forbids the second half outright, for the
reason above. The spec was left unamended deliberately — it records intent
from before the evidence existed, and editing it to match a doc written
afterwards is what the spec/notes split exists to prevent — so the correction
lives here and lands when this inbox is drained. Two more of that section's
bullets are superseded rather than wrong: "track bytes/second and CPU%" and
the `criterion`-plus-whole-file pairing both predate the nine rules that now
say how a figure is taken at all.

**Superseded as a limit, not as a fact:** the floor is a property of
differencing files of *different row lengths*, and the entry above
("The instrument that resolves a composite column's cost is built and unrun")
is the instrument that removes it — same rows, one column declared two ways.
This entry still governs every figure taken the old way.

**Origin.** 2026-08-27, which tried to separate the composite
column's end-to-end share from the arrays' and found the share below the
floor; re-taken the same day. Figures, the floor reading and the commands:
[`measurements.md`](measurements.md), "A typed query over nested columns" and
its "The cross-file subtraction bottoms out" subsection.

---

## Per-rep SE is not an uncertainty estimate here — two apparatuses gave t>4 in opposite directions

**Fact.** `M13`'s sweep ran the same five interleaved reps of the same
comparison on two builds of the same source. The composite column's per-row
share came out:

| Build | Composite − control | The same-shape control (seed 43 − seed 42) |
|---|---|---|
| glibc | **+0.61 µs/row** (sd 0.31, SE 0.14, **t = +4.34**) | +0.20 (SE 0.18, t = +1.06) |
| static musl | **−1.16 µs/row** (sd 0.54, SE 0.24, **t = −4.81**) | −0.07 (SE 0.23, t = −0.31) |

**These are the only *t* values quoted anywhere in this repo, and the musl leg
they come from is the only one kept — both exist as this demonstration rather
than as results** — [`measurements.md`](measurements.md)'s standing rule
against quoting a standard error now forbids quoting one for a figure. Both legs are
"significant" past any threshold anyone would set, they disagree by
**1.77 µs/row**, and the sign inverts — including the sign of the reading's
distance from its own floor (+0.41 glibc, −1.09 musl). The floor is not
significant on either leg, so it cannot rescue either. And a third scale sits
under both: the **identical** control file, same binary, read 7.595 µs/row in
the sweep's `S5` stage and 7.435 in `S6` seven minutes later — 0.16 µs/row of
drift between two stages of one sweep, which is the size of the floor and 39%
of the glibc effect.

**What that establishes.** Within-sweep dispersion measures the *reps*, not the
*measurement*. Everything that actually moves these numbers — the allocator,
the stage's position in the session, tmpfs and page state — is held constant
inside a sweep, so it contributes nothing to the SE and nearly everything to
the answer. A tight SE says one apparatus is repeatable. It says nothing about
how near the number is to the truth, and this instrument's between-apparatus
spread is an order of magnitude wider than its within-sweep spread.

**Why this phase cares.** It is an entire performance campaign whose questions
are mostly "what does this one thing cost", and it will generate dozens of
differences of exactly this size. Three rules follow, and the first is the one
that costs something:

- **The floor is the uncertainty estimate, and it is measured.** A cross-file
  per-row difference under ~0.5 µs/row is apparatus, established by the
  seed-43 control rather than assumed, and no number of reps moves it. The
  two-libc comparison above is the *demonstration* that an SE cannot stand in
  for that floor — it is not a practice to repeat, since every figure this
  project keeps is glibc.
- **Read per-rep SE as a repeatability check, not an error bar.** A *wide* SE
  means the sweep is broken and should be re-run; a narrow one licenses
  nothing.
- **A result whose sign flips between apparatuses is not a small result.** It
  is a question the instrument cannot address, and the answer is a better
  instrument (here: two files byte-identical in their data section, differing
  only in a declared type — named in the entry above and still unbuilt), not
  more reps.

**Origin.** Both legs of `M13`'s sweep, 2026-08-27
(`runs/m13-warm-set.log`, `runs/m13-warm-set-musl.log`, stages `S5` and `S6`),
after the musl leg alone was briefly read as evidence that cross-file
differencing is structurally *biased* — a reading the glibc leg withdrew.
[`../status/history/2026-08-27.md`](../status/history/2026-08-27.md),
"`M13`'s figures are folded in".

## The scanner's read path costs more than its parser on a memory-resident file

**Fact.** A whole-file `pgdq parse` of the 3.00 GiB brace-free control, served
from tmpfs inside a 512 MB container and timed by the container's own shell,
runs **0.53 s: 0.19 s user, 0.33 s sys** (census-off build; the working tree
adds 0.04 s). `dd if=… of=/dev/null bs=4M` over the same file in the same
container reads it in **0.33 s**, of which 0.33 s is `sys`. So the scan's
kernel time is exactly the cost of handing the bytes over, and its user-space
work — the `COPY` grammar, the map, the census pre-filter — is **36% of its
own elapsed time**. Measured 2026-08-27 by `M13`'s sweep
(`runs/m13-warm-set.log`, stages `S2b` and `S3`), on the tree carrying `M11`'s
`memchr2` pre-filter.

**The musl reading of this was much worse and was an allocator artifact.** The
same stages built against musl read 1.29 s with **1.10 s of `sys`** against
the same 0.30 s `dd` — 3.6× `dd`'s whole wall clock inside the kernel, which
read as a read-path problem and is not one. Whatever musl's allocator does
with the scanner's buffers shows up as kernel time; glibc's does not.

**Why this phase cares.** The campaign's target is a device-bound local read
path, and its planned work — SIMD structure discovery, zero-copy row
extraction, bulk UTF-8 validation — is all *user-space* work. On the one
regime where the device is not the constraint, that work is 36% of elapsed
time and the kernel's read is the other 64%, so it sets the ceiling on what
any parser change can show: on this input, deleting all user-space work
entirely would take 0.53 s to 0.33 s. It also says the ceiling is *libc-
dependent*, which is the next entry.

## The allocator is worth choosing deliberately, and glibc is the assumed target

**Fact.** The same binary source, built against two libcs, differs by ~25% on
the block-serialization stages of the benchmark set — a 500-block `parse` runs
0.29 s on the default glibc build against 0.41 s static musl — and **by a
factor on everything that moves real bytes**. Both legs of `M13`'s sweep, same
inputs, same reps, same container limits:

| Stage | glibc | musl |
|---|---|---|
| 3.00 GiB warm `parse` (working tree) | 0.57 s | 1.35 s |
| the same, `--schema-mode strings` query | 4.43 s | 7.98 s |
| the same, `--schema-mode typed` query | 10.66 s | 19.08 s |
| 4000-block `parse` | 20.1 s | 32.8 s |

The 2.4× on the warm `parse` is almost entirely **kernel** time (0.33 s of
`sys` against 1.10 s for an identical 0.33 s `dd` floor), so it is not a
parsing difference at all. Measured 2026-08-27.

**musl is settled and is not the open question.** Every performance figure is
taken with the default glibc build in a glibc image
([`measurements.md`](measurements.md)), and as of 2026-08-27 musl is gone from
the container recipes too — only glibc is measured, so a static musl build is
an untested configuration. What stays open is a *different* axis: whether pgdq
should select an allocator explicitly rather than inheriting the platform's.

**Why this phase cares.** A campaign aiming at a device-bound local read path
inherits an allocator it never chose, and a **1.8–2.4× swing between two stock
ones** is larger than everything on this phase's list put together. Three
things to settle here: whether pgdq should select an allocator explicitly
(`jemalloc`, `mimalloc`) rather than inheriting glibc's — which on this
evidence is a bigger lever than any parser change, and cheaper; whether any
figure this phase produces is quoted without naming the allocator that
produced it; and
whether the row-emission path — which allocates per nested element
(`architecture.md`, "The nested literal codec") — is where an allocator change
would actually be felt, since the block-serialization workload above is not
the one users pay per row.

---

## A predicate's per-row cost is no longer bounded by "a conjunction with cheap leading terms"

**Fact.** Every filter term walks the row itself —
`predicate.rs`'s `ResolvedTerm::eval` does
`split_fields(raw_row).nth(self.index)` — so a query with *n* terms splits each
row up to *n* times. That was "a real cost only for a conjunction whose leading
terms nearly always pass" while a filter was an `AND` list short-circuiting on
the first failure.

A filter is now a boolean expression tree carrying `OR` and `NOT`, and a
disjunction short-circuits on the first term that *succeeds* — so the shape
that costs *n* walks is the ordinary one, not the pathological one. The
typed-predicates phase deliberately does not fix it: the fix is a rework of an
already-tested core path, and it must not share a review cycle with a change
whose worst bug is a silently wrong row set.

Two fixes were considered and neither was chosen here: splitting each row's
field offsets once into a reusable buffer that every term indexes into, and
collecting at block-resolve time the set of field indices any term needs, then
gathering just those in one walk.

**Why this phase cares.** It owns the row path and the zero-copy work that
touches this same field splitting, so a field-offset buffer built here serves
both — and building it in either place separately means building it twice.

**Origin.** P11 grilling, 2026-08-31. The walk itself, and the cost model that
makes this the phase's own statement of what it did not do, is
[`architecture.md`](architecture.md), "Predicates" — "each term walks the row
itself, so a five-way disjunction is up to five walks per row".

---

## A seekable compressed source hands this phase parallel discovery for free

**Fact.** This phase's "Parallelism" section splits the problem in two:
row extraction parallelizes trivially once boundaries are known, while
*discovery* is hard, because from a cold start you cannot tell whether a random
offset is inside a `COPY` block — which is why the speculative scheme is
prototype-and-measure work rather than a plan.

A seekable compressed source removes exactly that half. An `.xz` stream's index
gives every block's uncompressed offset before a byte is decoded, so workers can
be handed real, self-contained ranges rather than speculative ones. Measured on
the koji `.xz` (31,150 self-contained streams, ~24 MiB uncompressed each): one
core decodes ~446 MB/s of plaintext, four concurrent per-stream decodes reach
~1.48 GB/s at 397% CPU, and `xz`'s own `-T8` on that file gains nothing, its
threaded decoder parallelising blocks within a stream where each stream holds
one. Numbers and method: `roadmap-P13-compressed-input-inbox.md`, "xz decodes at
~446 MB/s of plaintext per core" — probes, not figures.

**Why this phase cares.** P13 lands the decompressing source and deliberately
does *not* take parallel decode, leaving it here rather than duplicating this
phase's machinery. So this phase inherits a second parallelism case with a
different bottleneck — CPU-bound at ~450 MB/s a core where the plain path is
device-bound at ~240 MB/s on the same HDD — and a different unit of work: a
compressed block rather than a byte range resynced to the next LF. Its
"be device-aware" rule needs a second axis, since the right worker count for a
compressed source is set by cores and decode rate, not by
`/sys/block/<dev>/queue/rotational`.

**Origin.** 2026-09-02, sketching P13.

**Contingent on** P13 landing first, which is this table's order. If it slips
behind this phase, the entry becomes a constraint on defaults rather than a
case to implement.
