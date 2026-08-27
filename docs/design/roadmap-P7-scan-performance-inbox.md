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

**Why P7 cares.** [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md)'s
"Two workloads, two algorithms" splits structure discovery from row
extraction; that split is now real in the code rather than notional, and the
two have measurably different cost profiles. The double read is the obvious
thing to measure and the obvious thing to want back — the deferred fix (carry
the live segment's in-flight spans in the opaque `ResumeToken`, so a single
pass can emit rows again without reintroducing the unmapped hole) is written
up under "Future — wanted, unscheduled" in [`roadmap.md`](roadmap.md). P7
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

**Why P7 cares.** It is `O(blocks × DDL)`, and P7's whole job is
knowing where the scan's time goes. The bound is fine for koji-shaped input
and could stop being fine for a dump with far more, far smaller blocks — which
is a shape worth deciding whether to care about rather than assuming away.
P7's "Measurement discipline" section is the right place to settle it.

**Origin.** Slice 3.2.2, 2026-08-24. See
[`architecture.md`](architecture.md).

---

## An `INSERT`-run scan is CPU-bound at ~5× a `COPY` scan's per-byte cost

**Fact.** Three 3.00 GiB synthetic dumps, same disk, same session, three cold
runs each (a whole-file `pgdq` scan, 512MB-limited container, `drop_caches`
before every run): a `COPY` block scans in 6.68–6.70 s, a large-object region
in 6.61–6.65 s, an `INSERT` run in 15.13–15.42 s, against a 5.73–5.74 s
`cat`-to-`/dev/null` floor for the same files. The first two are within 20% of
the I/O floor; the `INSERT` scan is 2.7× the floor's time. **The "~5×
per-byte CPU" this entry was filed under is not a CPU ratio**: it divides that
cold rate, device included, by the `COPY` path's *warm* CPU, and no warm
`INSERT` figure has ever been taken. The `COPY` side is now 0.57 s per 3.00 GiB
rather than 2.92 s, and bounding the `INSERT` side from the cold table (15.2 s
against a 5.73 s floor) puts the real per-byte ratio near **16–26×**. `M17`
takes the warm `INSERT` `parse` that settles it — **re-check that before using
any number here**. The cause is structural, not incidental:
slice 3.6 gave the large-object region a `crate::scan`-level fast path (lines
skipped unread) but left `INSERT` runs decoding every line into `Event::Line`
and pushing it through `preamble::statement_complete`, folding only the
*spans* into one.

**Why P7 cares.** This is a second scanner-level fast path — the same
mechanism `State::InLargeObjectRegion` already is — and P7 owns scan
performance and the "two workloads, two algorithms" split. It is also the one
place where this project's cost claim is currently false in the direction that
matters: `--inserts` output is a shape the fixture tooling generates routinely,
and a koji-scale 1 TB `--inserts` dump spends tens of minutes of CPU that a
`COPY` dump of the same size does not — "~45 minutes" is the figure the ~5×
gave and is a floor under the same re-take. The design constraint to carry in: an `INSERT` run's end
has no invariant behind it the way `COPY`'s `\.` (I7) and `BLOBS`' `COMMIT;`
(I12) do, so a skip-and-count path needs the string-aware `'`-tracking scan
[`architecture.md`](architecture.md)
("The three regions do not share an end marker") specifies — which P8
Track A's row reader needs anyway.

**Origin.** Out-of-band item M3, 2026-08-25; the table re-taken cold by `M10`,
2026-08-27, which is where the numbers above come from
([`measurements.md`](measurements.md), "Scan throughput by input shape"). The
original measurement: [`../status/history/2026-08-25.md`](../status/history/2026-08-25.md).

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
3.75× the same query in `strings` mode, against 2.41× for the same file
without those three columns — so the three nested columns account for about
**13.9 µs of every row**, nearly twice what the sixteen scalar columns cost
together. **The two arrays carry all of it**: a third file holding the
composite column and no arrays reads +0.61 µs/row against an instrument whose
own floor is +0.20, which bounds that column at around half a microsecond
(4.6.1, and the entry below on what that subtraction can resolve). The
`nested.rs` literal parse and its render account for only **6.3 µs** of the
13.9; the remaining ~7.6 µs is the Arrow build — 56 per-element
`append_value` calls into child builders, plus list offsets. The
micro also puts the array cost per *element* (78 ns decoding, 28 ns
rendering), which is the shape of one allocation each, since
`ArrayLiteral::elements` is a `Vec<Option<String>>`. **Two targets, then, not
one**: viewing instead of copying attacks the build, and it is the larger
share — but a `Vec<Option<String>>` intermediate is paid before the build is
reached, so a viewing builder that still routes through `decode_array` keeps
the 6.3 µs.

**The prize, measured against the path this phase would widen.** One
`append_view_unchecked` into a borrowed block costs **3.14 ns**, against
~21 ns to copy the same bytes — so the two micro controls bracket the
question: the literal parse is 12.6× a copy and 84× a view for a 4-element
array, 149× and 1225× for a 50-element one. 3.14 ns is a floor rather than the
borrowed arm itself (`push_utf8view_field` also scans the chunk deque and
calls `block_for`), so those view ratios bound the real ones from above. Read
together with the ~10 µs build share above, the shape of the answer is that
**viewing is worth far more than the parse is**, and worth most on long
arrays.

**Origin.** P4 grilling, 2026-08-25; the figures from slice 4.6,
2026-08-27, re-taken by `M10` the same day once the generator declared the
types `pg_dump` writes — the earlier end-to-end ratios were taken with three
of the sixteen scalar columns silently untyped. Decision and its rationale:
[`roadmap-P4-composite-decoding.md`](roadmap-P4-composite-decoding.md),
"Nested elements copy"; figures and commands:
[`measurements.md`](measurements.md), "Nested decode costs what it copies" and
"A typed query over nested columns".

---

## Every mapping pass now does per-row work, and on array-bearing rows it is not free

**Fact.** Slice 4.5 put the array-shape census in `map::Builder::on_row`, fed
from `Event::Row` by `build_index`, `build_map` and `stream::map_forward`;
4.5.1 removed the `ScanExtent::Full` gate, so **a cold query's mapping pass
censuses too**. Every data row of every block any mapping pass maps is now
inspected. A row containing neither `{` nor `[` is rejected after one pass over
its bytes and never split into fields; a row containing either is split by
`copy::split_fields` and every field's first bytes examined.

Both sides are measured, on 3.00 GiB files served from tmpfs to a 512 MB
container, alternating a census and a no-census glibc binary in one session,
six reps each in both orders. On the brace-free `COPY` control — every row
rejected by the pre-filter — the census costs **0.037 s per 3.00 GiB**, 45 ns
per 16-column row: **+7%** of a warm scan. On a file where **every** row
carries an array it costs **1.26 s per 3.00 GiB**, 1.80 µs per 19-column row:
**+270%** warm. Cold from this SSD a 5.73 s device floor hid both at +1.2%
and +2.6%, on the pre-`M11` pre-filter; `M17` re-takes that regime.

**So the census's cost is the field split, not the pre-filter**: 97.5% of it
falls on the rows the pre-filter passes. That inverts the reading this entry
carried until `M11` — a scalar `raw.iter().any(…)` at ~3.8 GB/s made the
pre-filter ~40% of the census's cost even on rows it rejected; `memchr::memchr2`
makes it 2.5%.

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
of koji's shape pays 7% of a memory-resident scan and nothing at all off a
device; a dump whose rows all carry arrays pays nearly 3× the rest of the
scan, on any regime that is not device-bound. A phase that succeeds in making
the scan CPU-bound makes the second case its dominant cost.
`scripts/generate_perf_data.py --arrays --composite` is the input.

**Origin.** Slices 4.5 and 4.5.1, 2026-08-26; the array-bearing figure from
slice 4.6, 2026-08-27; both figures re-taken by `M10` and then by `M13`'s
warm-set sweep, 2026-08-27, which is what re-priced the pre-filter. See
[`architecture.md`](architecture.md), "The array shape census";
[`measurements.md`](measurements.md), "The census on array-bearing rows".


---

## Mapping is O(blocks²) after the save throttle, and the remaining half is the span splice

**Fact.** `pgdq parse` used to serialize the **whole** cache at every `CopyEnd`
watermark. Slice 9.5 throttled that (`SaveThrottle`: skip a save unless 20x the
last save's own duration has elapsed), which cut 4000-block saves from 4003 to
103 and 45.8 s to 20.1 s. The series is **still** 4x per doubling, because a
second cost has the same shape: every `CopyEnd` rebuilds `DumpIndex::spans`
whole — `map::Builder::snapshot` clones the builder's span vector, then
`stream::splice` clones the prefix and concatenates — so the map alone is
O(blocks²) with the cache disabled entirely (18.6 s for 4000 blocks under
`query --dqcache none`, against under 10 ms for the same bytes in one block).
**The map is now 93% of a throttled `parse` at 4000 blocks**, so the splice is
what is left to close, not the cache. koji cannot show either half: 74 blocks
over 784 GB.

**The throttle is self-tuning, which matters for reading its counts.** `K` is
a ratio against the last save's own duration, so a faster machine, libc or
allocator saves *fewer* times rather than the same number more cheaply — the
4000-block count reads 103 on glibc against 110 on musl and 195 on the SSD-warm
session that first recorded it. A save count is a property of the apparatus,
not only of `K`.

**The cheap fix, and what it costs — decided against, so the phase does not
re-derive it.** `index.spans` is rebuilt at every `CopyEnd`, but for `pgdq
parse` nothing reads it between saves: `target` is `None`, so `target_settled`
never runs, and the only consumers of a current `index` are the throttled save
and the chunk-top interrupt save. Moving the `splice` *inside* the existing
`if settled || cancelled || throttle.due()` arm would therefore fire it a few
dozen times instead of `n` — roughly 18.6 s → 1 s at 4000 blocks — using the gate
9.5 already built, no redesign. **It was rejected anyway**, because it trades
away the guarantee 9.5 spent a slice establishing: today an interrupt banks the
last *completed block*, and under the gated splice it would bank the last
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

**Origin.** Slice 9.5, 2026-08-27. Figures, both series and their commands:
[`measurements.md`](measurements.md), "Per-block cache saving is quadratic in
block count, and so is the map".

---

## Attributing a cost to one column by differencing two generated files bottoms out at ~0.5 µs/row

**Fact.** `pgdq query` has no column projection, so the only way to say what
one column costs end to end is to run the same query on two generated files
that differ by that column and subtract their per-row `typed` − `strings`
figures. That subtraction has a floor. Two 3.00 GiB files holding the *same*
sixteen columns and differing only in their RNG seed (`--seed 42` against
`--seed 43`) disagree by a paired median of **+0.15 µs/row**, spanning −0.31 to
+0.92 over six interleaved reps, when they should disagree by zero; the file differing by one composite
column reads a paired median of **+0.59 µs/row** over five reps, whose per-rep
readings (+0.29 to +1.03) overlap the floor's (−0.31 to +0.92) across most of
their width. Anything under roughly **±0.5 µs/row** out of this instrument is
apparatus. Two contributors are known and neither is removable
within it: the files hold different row counts at the same byte size, so a
per-byte component does not normalize away per row; and a slow upward drift
across a long session lands on whichever file is measured later, which is why
the runs interleave files rather than running them in blocks.

**A third contributor is present and *is* removable — the census, measured.**
A `strings` leg is a mapping pass plus a row pass, and the mapping pass runs
the array-shape census, whose cost depends on whether the rows carry a `{`.
The `--arrays --composite` file's `strings` leg is 24% above the control's;
with the **census-off** binary on the same two files the gap **inverts**, from
+1.115 s to −0.075 s — the arrays file becoming slightly cheaper, as its 14%
lower row count should give. So a cross-file subtraction that changes the
*brace-bearingness* of the rows compares two different baselines, and only the
per-file `typed` − `strings` difference cancels it. The same run reproduces
the census's own cost on a different command to within 3% of the `parse`
figures. Earlier sweeps could not see this: at 9.6 s legs a 1 s
difference was inside the spread.

**And the floor cannot be tightened by taking more reps** — see the next
entry, which is the same instrument measured on two apparatuses and is the
reason no confidence interval quoted here means what it looks like.

**Why P7 cares.** It is an entire performance campaign, and the questions
it will ask — what viewing instead of copying saves on `List<Utf8View>`, what
a sparse row index costs per block, what a parallel scan wins — are mostly of
the form "what does this one thing cost", against inputs from the same
generator. A campaign that reads a 0.3 µs/row difference as a result will
report noise as a win. The remedy, when a figure that sharp is actually
needed, is an instrument whose two files are **byte-identical in the data
section** and differ only in DDL — declare the column under test as its real
type in one file and as `text` in the other, so the same rows are decoded two
ways with the same row count and the same bytes. That needs a generator knob
that writes a deliberately weaker declaration; it was named and not built,
since nothing yet needs the resolution. Reviewed 2026-08-27 and still not
built, with its real cost now named: the knob makes the generator write a
declaration `pg_dump` would not, which is the opposite of what `M10` corrected
it to do, so it needs an explicit exemption from
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
cache**"; the third standing rule forbids the second half outright, for the
reason above. The spec was left unamended deliberately — it records intent
from before the evidence existed, and editing it to match a doc written
afterwards is what the spec/notes split exists to prevent — so the correction
lives here and lands when this inbox is drained. Two more of that section's
bullets are superseded rather than wrong: "track bytes/second and CPU%" and
the `criterion`-plus-whole-file pairing both predate the nine rules that now
say how a figure is taken at all.

**Origin.** Slice 4.6.1, 2026-08-27, which tried to separate the composite
column's end-to-end share from the arrays' and found the share below the
floor; re-taken by `M13`'s sweep the same day. Figures, the floor reading and
the commands:
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

**These are the only *t* values quoted anywhere in this repo, and they are the
demonstration rather than a result** — [`measurements.md`](measurements.md)'s
ninth standing rule now forbids quoting one for a figure. Both legs are
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

- **A figure the design depends on gets two apparatuses, quoted as a range.**
  "Depends on" means a ratio the roadmap cites or a bound an inbox entry
  consumes; a tripwire or an orientation figure may have one and says so. The
  second apparatus is cheap here — the same sweep script against a musl build
  is a second libc for the price of a rebuild.
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

## The allocator is worth choosing deliberately, and glibc is only the measurement baseline

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

**The decision taken was about measurement only**: every performance figure is
now taken with the default glibc build in a glibc image
([`measurements.md`](measurements.md)), so figures are comparable to each
other. Nothing has been decided about what the *shipped* binary should use.

**Why this phase cares.** A campaign aiming at a device-bound local read path
inherits an allocator it never chose, and a **1.8–2.4× swing between two stock
ones** is larger than everything on this phase's list put together. Three
things to settle here: whether pgdq should select an allocator explicitly
(`jemalloc`, `mimalloc`) rather than inheriting the platform's — which on this
evidence is a bigger lever than any parser change, and cheaper; whether any
figure this phase produces is quoted without naming the allocator that
produced it; and
whether the row-emission path — which allocates per nested element
(`architecture.md`, "The nested literal codec") — is where an allocator change
would actually be felt, since the block-serialization workload above is not
the one users pay per row.
