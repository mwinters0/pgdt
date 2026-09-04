# Measurements

Every performance figure the design relies on, with the command that
reproduces it. A baseline nobody can re-run is a rumour with a decimal point,
so **a figure that loses its regeneration command should be deleted, not
kept**.

**Session stamp.** Every figure below was taken by `scripts/measure.py` on
2026-09-03, against commit `ba2fc12`. One sweep, one apparatus — which is what
lets these tables be differenced against each other, and what "are these
figures from before or after my change" is answered by. `uv run measure.py
--stale` reads that commit back and names the figures a diff has invalidated
since. **Ten tables stand outside that sweep and each says so in its own
apparatus line**, so a reading taken from one of them and differenced against a
sweep table is a cross-sitting difference and must clear the drift figure
below: `session-drift` itself, which no sweep can take — it is derived across
the published sweep and a second one taken two minutes later on the same commit
— and `allocator`, `per-block-quadratic`, `map-only`, `preamble-prepass`,
`census-brace-free`, `scan-throughput-cold` and `scan-throughput-warm`, each
re-taken after a change that moved it, in two groups that each share readings.
Two stand outside it for the other reason, having not existed when the sweep
ran: `predicate-terms`, the first table here to pass a filter at all, and
`scan-throughput-nvme`, the first taken on a third device class.

All figures are on the hardware `CLAUDE.local.md` describes. Synthetic inputs
are regenerable with `--seed 42` and are **never committed** — they measure
throughput, not correctness, which stays entirely fixture-based.

Twelve standing rules for reading anything below:

- **Every figure is a ratio, never a disk throughput.** Page-cache state
  dominates. A number taken warm on a freshly generated file can be twice what
  the disk delivers to `cat`, which is exactly how the large-object figure was
  once misread. Always take the `cat`-to-`/dev/null` floor for the same file on
  the same disk in the same session, and compare against that.
- **Say which regime, and stay in it for the whole figure.** "In one session"
  is not enough: a run sequence that starts cold and warms up puts each run in
  a different regime, and a difference smaller than the warming trend
  disappears into it. Either `drop_caches` before *every* run or `cat` the file
  first and take every run warm — and when the figure is a difference between
  two binaries, run the pair in both orders, because within a pair the second
  run is the warmer one. The census figures were once re-taken for exactly
  this reason
  ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
- **A figure about parsing CPU must not be taken against a filesystem.** Put
  the input on tmpfs, or otherwise guarantee it is served from memory for every
  run. Device time and background I/O swamp the difference being measured —
  badly on the HDD, where minor unrelated activity moves the reading further
  than any code change under review will — and page-cache residency is an
  assumption, not a guarantee: a multi-gigabyte input can be partly evicted
  between runs by anything else the machine does. This does not apply to a
  figure whose *subject* is the device ("Scan throughput by input shape"
  below), which is taken cold on purpose. **Every warm figure below is on
  tmpfs**, and they were re-taken together in one session rather than drifting
  onto the new footing one at a time — see "The apparatus" below, which every
  figure here shares.
- **Re-take a comparison table whole, in one interleaved sweep.** Never
  difference one row against a figure from another session, and never run a
  multi-file comparison a file at a time. Session-to-session level shifts of up
  to 8.5% happen here on identical binaries and identical inputs, and that is
  the two-hour figure rather than the back-to-back one — measured, in
  "What a session's own drift costs" below — and a
  file-at-a-time sweep maps a session's own drift onto file identity,
  manufacturing a between-file difference that is apparatus. Each rep runs
  every file-and-mode combination in turn; report medians. Two takes of the
  nested table a day apart disagreed by 3.45× against 3.08× for exactly this
  reason, while the per-row differences the design actually consumes barely
  moved
  ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
- **The timer goes inside the container, never around it.** A figure must
  not carry the harness that produced it. `sudo nerdctl run` costs **0.74–0.76
  s** before the binary starts — three runs of a trivial command, opening the
  warm-set sweep below — against which a 3.00 GiB warm `parse` of the
  brace-free control is **0.532 s** timed by the container's own shell. The
  wrapper is *larger than the figure*, and more than twice the smallest row of
  the quadratic table. So the timed command is `bash -c 'time /pgdq …'`, whose
  timer resolves to 1 ms. This does not license running a figure outside the
  container to avoid the cost — the cgroup limit is part of the apparatus, and
  a difference of binaries is not measurable across two different ones.
- **A performance figure is taken with the default `glibc` build, in a glibc
  image.** The allocator is part of what is being measured, the two libcs do
  not agree, and the gap is far larger than the first reading of it suggested.
  On the block-serialization workload it is ~25%: the same
  500-block `parse` runs **0.29 s** glibc against **0.41 s** static musl. On
  the two workloads that move real bytes it is a factor: the same 3.00 GiB
  warm `parse` is **0.57 s** glibc against **1.35 s** musl, and the same
  `--schema-mode strings` query over it **4.43 s** against **7.98 s**. So
  the figures here are `cargo build --release` (no `--target`) run under
  `postgres:16` (Debian bookworm), whose **glibc 2.36 malloc is part of the
  apparatus** and should be named when a figure moves. **musl is not measured
  and is no longer in any recipe** — the comparison above is why glibc is
  named, not an invitation to take a second leg. Debian's `/bin/sh` is
  dash, with no `time`, so the in-container timer is `bash -c 'time …'`.
  Which allocator the shipped binary links against, and what the two
  replacements are worth, is "Which allocator a figure was taken under" below;
  it is also where a figure here being a **CLI** figure is stated, the choice
  being the binary's and never the library's.
- **Never quote a standard error or a *t* from one sweep — give the median and
  the observed spread.** Within-sweep dispersion measures the *reps*, not the
  measurement: the allocator, the stage's position in the session and the
  tmpfs/page state are all held constant inside a sweep, so they contribute
  nothing to an SE and nearly everything to the answer. The demonstration is
  the composite column's per-row share, five interleaved reps on each of two
  builds of the same source: **+0.61 µs/row (t = +4.34) on glibc and
  −1.16 (t = −4.81) on musl** — both "significant", 1.77 µs/row apart, opposite
  signs. That comparison is the *demonstration*, not a practice to repeat —
  **every figure here is glibc**, and the musl leg exists only as the evidence
  for this rule. What follows for a real figure is a corollary rather than a
  second run: a cross-file per-row difference under **~0.5 µs/row** is
  apparatus, that floor is measured rather than assumed (the seed-43 control,
  "The cross-file subtraction bottoms out"), and no number of reps moves it.
  Reasoning:
  [`../status/history/2026-08-27.md`](../status/history/2026-08-27.md).
- **A move smaller than the apparatus resolves is not a finding.** The floor is
  per regime, and each number below is read off "What a session's own drift
  costs" rather than asserted: a **warm absolute compared across sessions**
  resolves to no better than **~8%**, a **cold device-bound one** to **~0.5%**,
  and a **cross-file per-row difference** to **~0.5 µs/row**. The warm number is
  the one with an observation outside it: the pair below moved one file's warm
  tmpfs `dd` floor 15.9%, so ~8% bounds the warm *figures* and not every warm
  reading — the floors themselves move further, which is why each warm table
  co-measures its own, and why a **floor** is judged against a number of its
  own ("The floor is read directionally") rather than against this one. A move
  inside its regime's floor is apparatus. Write it up as *reproduces*, never as a change —
  in this doc, in a history entry, or in an argument about which of two sweeps
  to publish; narrating one manufactures a finding that the next sweep silently
  reverses. Two consequences: a warm table's third decimal carries no
  information across sweeps, and the way to resolve something finer is a
  difference taken **inside** one sweep, not more reps. These three numbers are
  read off "What a session's own drift costs" below and off "The cross-file
  subtraction bottoms out"; a re-derivation of either re-reads this rule, which
  is a cross-reference rather than a `quoted_by` edge because a figure never
  declares the doc it lives in.
- **Two builds of one source can differ by layout, so a stamp-to-stamp move is
  not necessarily a code change.** The `INSERT`-run row moved ~10% in both
  regimes between two stamps against byte-identical input, and the attribution
  is that and nothing else: `release` builds at `b70589f` and at `ba2fc12`
  retire **114.62 G against 114.66 G instructions** for that scan — 0.03% apart
  — and spend **35.8 G against 40.0 G cycles**, with branch misses, cache
  misses, L1-icache misses and frontend stalls flat or *lower* on the slower
  one. The whole difference is inside `preamble::scan_buf`, whose 293
  instructions are byte-identical between the two binaries and differ only in
  address; building both with `-C llvm-args=-align-all-functions=6` collapses
  it to −1.5% and takes both below the faster one. What follows for reading a
  figure: **a move of this size in a hot, tight, branchy loop is not evidence
  of a code change**, a bisect over it lands on whatever commit shifted the
  binary and explains nothing, and the same flag moved the control's `parse`,
  `strings` and `typed` shapes not at all — so this is not a lever, it is the
  instrument's own floor for a *code* comparison across two builds. Two
  consequences the phase pays: the honest way to compare two commits is
  `instructions:u` alongside the wall time, since that number holds still when
  layout moves; and a profile taken with `--profile-recipe`'s frame-pointer
  build reverses the sign of this particular difference, so **a profile is
  read for proportions and never for a wall-time comparison between two
  builds**. The working is beside the mechanism in
  [`architecture.md`](architecture.md), "Bulk regions: one span kind, three
  payloads"; the evidence is
  [`../status/history/2026-09-03.md`](../status/history/2026-09-03.md).
- **A mode difference and a per-column delta are CLI numbers, so neither sizes
  a library change.** Every `query` figure here is a `pgdq query` figure, which
  means `pgdq::print_batch` — the CLI turning each batch back into TSV — is
  inside it, and it is not a rounding error: on the control **79% of the
  `typed` − `strings` gap is that one function**, and a typed query spends
  62.8% of its user time there
  ([`architecture.md`](architecture.md), "Where a scan's time goes"). So the
  two sharpest instruments in this document price decode **plus** the Arrow
  build **plus** the render-back: "A typed query over nested columns…"'s
  per-row differences, and "What a column costs…"'s per-column deltas, whose
  own closing paragraph says `render_field` is included. Read either as what a
  *user of the CLI* pays, which is what they are for. What an **embedder** pays,
  and therefore what a library lever can remove, comes from a profile's shares
  or from a criterion bench (`benches/decoders.rs`, whose `decode` and `render`
  columns are separate for exactly this reason) — **never** from a difference
  taken across two CLI runs. Two lever rows in
  [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md) were sized
  the wrong way before this was written down, and a third was still wrong after
  the first two were corrected, which is why the rule is here rather than in
  each figure's own prose.
- **Long runs are detached.** A koji-scale scan is roughly an hour; see
  `CLAUDE.md`, "Long-running processes", for why waiting on one is expensive
  and what to do instead.
- **koji is one dump *shape*, so a per-block or per-span cost measured only
  there is unmeasured.** It is the project's only real sample and it is
  byte-rich and block-poor — 74 blocks over 784 GB — so a cost that scales with
  block or span count cannot express itself in it at all. The save throttle was
  correctly declined on a koji reading of +1.5% wall and 18 MB written, and the
  same code cost 44 s on 4000 small blocks in a 2 MB file, because every save
  serializes the whole index. `scripts/generate_block_count_bench.py` exists to
  be that other axis; "Per-block cache saving is quadratic in block count"
  below is the figure it takes.

- **A koji figure taken while local work ran is not a figure.** Whether this
  checkout and the sample share a spindle is a machine fact — see
  `CLAUDE.local.md`'s hardware section, which records what the contention costs
  here.

## The apparatus

**Every figure below is taken by `scripts/measure.py`, and the session stamp
above records the sweep that took the doc as a whole.** Figures that share a
reading share it rather than measuring it twice: each throughput table's `COPY`
row *is* the census table's census-on column for that regime, the preamble
table's full-`parse` row *is* the quadratic table's 4000-block "after" column,
and the allocator table's reference column *is* the census and nested tables'
readings of the same three shapes.

**A figure may be re-taken on its own, and its apparatus line is what says so.**
Selection is per figure, so a table can be replaced between sweeps; what a
reader then cannot do is set one sitting's **absolute** beside another's, which
costs the ~8% a warm absolute resolves to across sessions. Three rules keep
that legible, and the third is the one a single-figure re-take is most likely
to skip:

- **An apparatus line names the figures its table was taken with**, so a reader
  can tell at a glance which absolutes may be read together. A table taken
  entirely alone says that, in those words.
- **A table whose *shared* reading was measured alone carries the harness's
  partial-sweep note into the doc**, verbatim in substance. `measure.py` emits
  that note precisely when a borrow could not be satisfied, and dropping it
  publishes a reference column that looks shared and is not.
- **The harness names the closure, not its direct sources.** Re-taking a figure
  that others borrow from drags them too: `census-brace-free` is borrowed by
  both throughput tables, so the honest set behind the allocator table is four
  figures where its direct sources are two. Each figure declares what it
  borrows, so the harness computes that closure rather than a session working
  it out by hand — `--figure` takes what a figure borrows and names the rest
  before the first reading, the partial-sweep note states the whole set to
  re-take, `--list` prints it per figure, and `--check` reports a partial
  sitting the doc still carries. A **deliberate** partial sitting is
  `--figure <id> --alone`, which borrows nothing and emits that note.

**One line, in three regimes: 3.00 GiB inputs read by a `glibc` binary in a
512 MB `postgres:16` container, timed by that container's own `bash`.** Warm
figures read from `/dev/shm`; cold ones read from the SSD with `drop_caches`
before every run, including before the floor; `cold-nvme` ones are that same
discipline against a copy of the input on the NVMe. Nothing else builds or runs
on the machine while a sweep does — a `cargo` job across 24 cores moves the
numbers being taken, which is "a koji figure taken while local work ran is not
a figure" one scale down.

**A regime names a device, and a figure that reads the wrong one still emits a
plausible table.** That is why the three staging areas are three directories
and never one, why `cold-nvme` is its own regime rather than a flag on `cold`,
and why `--stage cold` does not reach a cold-NVMe figure: its absolutes belong
to no cold-SSD sitting. The contention gate carries a row per regime for the
same reason — a regime with no row gates nothing, so a fourth one added without
a row would silently take every reading it was handed.

**The harness stages the inputs, and the budget is computed.** The full input
set is six 3.00 GiB files, which does not fit `/dev/shm`, so it stages one
figure's inputs at a time and evicts what no remaining figure wants. The
ceiling is the largest single figure's own inputs plus 10% — 9.90 GiB here —
checked against the filesystem's real free space before the first measurement
rather than discovered twenty minutes into a sweep. Inputs are generated onto
the SSD once and *copied* into tmpfs, both from the host: 3 GiB written from
inside the 512 MB container would be charged to its cgroup and kill it. The
NVMe copies are made the same way and **kept** rather than evicted — that area
is disk, not RAM, so there is no budget to reclaim and nothing to buy by
re-copying 3 GiB before every sitting. Each copy carries the generator stamp
its source does, so a changed generator replaces it instead of measuring
pre-change bytes forever.

**Each figure's section carries an `<!-- figure: <id> -->` marker**, and that
marker — not the heading — is how the harness addresses it. Headings here are
free to quote a number, and a heading whose number the next sweep moves is
rewritten with it. `uv run measure.py --check` reconciles the markers against
the harness's register and names, for each figure, the other documents that
repeat its numbers; `--stale` names the figures a diff has invalidated.

*Rejected:* sharpening `depends` until it attributes staleness figure by
figure. **Any stale figure forces a whole re-sweep** — selection is per figure,
but a sweep is what the session stamp records — so the actionable output is
binary, re-take the doc or don't, and one true positive settles it. `map.rs`
alone is declared by eight of the twelve figures a sweep takes, which means
most changes mark most of the doc stale and the tool still answers correctly.
Per-figure detail would be explanatory colour, and buying it by narrowing the
declarations trades against the only failure that matters: an under-declared
path costs a false negative when *no* figure declares the changed file. The
same predicate decides whether a sweep was taken against a dirty tree
(`git_head` in `scripts/measure.py`), so the two answers are consistent by
construction rather than by anyone keeping them in step. Evidence:
[`../status/history/2026-08-28.md`](../status/history/2026-08-28.md),
"`--stale`'s job is binary".

**A declared path is matched by prefix, so moving code out of one silently
un-declares it.** `QUERY_CLI` is the worked case, and it is now the directory
`pgdump_query-cli/src/` rather than `main.rs`: naming the one file left every
other module in that crate outside every declaration quoting it, which is a
staleness edge nobody declared — and the crate has three modules, `main.rs`,
`where_expr.rs` and `alloc.rs`. Splitting a crate up is fine; whatever splits
it widens the declaration to the directory in the same change, because the
alternative is a `--stale` that is silent about the file the change is in.

**A commit can be acknowledged, and then it stops marking a figure stale.**
Coarse `depends` costs something in both directions. The paragraph above weighs
the false negative; the false positive is the one that decays the mechanism. A
change *inside* a declared path that provably moves nothing leaves `--stale`
red until a sweep re-stamps the doc — and a sweep is an hour on a machine that
has to be quiet, so the realistic outcome is that no sweep runs and `--stale`
becomes a light that is always on. A signal that is always on is the same thing
as no signal, which is the decay the register was built against, arriving from
the other side.

So `measure.ACKNOWLEDGED` — the register in `scripts/acknowledged.py`, which no
figure declares, so that adding an entry does not mark stale the figure it
excuses — records commits that touched a declared path without
moving a reading: the commit, the figures it excuses, why, and the command that
re-checks it. `--stale` then prints the excuse rather than the figure, so the
acknowledgement is *visible* — an invisible excuse would be the same defect one
level down.

Four properties keep it from becoming a way to wave staleness away:

- **It is per commit, never per path.** Excusing a path would silently cover
  every future change to it. A commit is a fixed diff someone looked at.
- **Every commit touching a path must be excused**, not just one, so a path
  changed by an excused commit and an unexamined one stays stale.
- **An uncommitted path is never excused**, because there is no commit to point
  at and nobody has read the diff — the same predicate `git_head` uses to call
  a sweep unpublishable.
- **An entry lives inside one session stamp's range.** Once the doc is
  re-stamped past the commit, no `--stale` range reaches it and the entry
  excuses nothing; `--check` names spent entries so they are deleted rather
  than kept as sediment.

**The evidence is computed, not asserted.** `uv run measure.py
--verify-additive` regenerates every published figure's inputs at both
revisions and compares them byte for byte, which settles the one class of
staleness that has a cheap oracle: a figure's `depends` names its generator, so
any edit to that script marks it stale, including one that only adds a flag.
It generates at 30 MiB rather than the sweep's 3.00 GiB — the same seed draws
the same row sequence at any size, so a prefix that matches byte for byte is
the row logic matching rather than a coincidence of length — and it verifies
only the inputs a *published* figure is taken on, since an input that exists
for an untaken instrument has no bytes in the doc to be wrong about and may not
be generatable at the older revision at all.

**Reachability is the second mechanical oracle, and it is narrower than
byte-identity.** A change inside a declared path that **no registered command
shape executes** moves no reading, and whether a shape passes a flag is a grep
over `_script` rather than a judgement. It says the changed code did not
*run* — not that it runs identically — so an entry using it names the command
shapes and, where a shape reaches part of the change, says which part and how
often. The worked case is the `--filter` term grammar: it lives in `main.rs`,
which five figures declare, and no command shape passes `--filter` at all.

*Rejected:* letting an acknowledgement cover library or harness changes on a
reading of the diff. Neither oracle above is a diff read: one regenerates bytes
and compares them, the other resolves what a fixed set of commands can reach.
Where neither applies — a change to `map.rs`, or to the harness's own timing
path, that a published command *does* execute — the only way to know whether a
number moved is to take it, so the figure stays stale and `--stale` keeps
saying so. The worked case is `session-drift`, which declares
`scripts/measure.py` because the harness *is* the apparatus it measures.

**A stale figure does not oblige a sweep, and neither does a phase boundary.**
Red is the honest state for a figure whose evidence nobody has taken, and the
requirement is that the reason is *written down* — in `STATUS.md`, naming the
figure and what would settle it — not that the red is cleared. A full sweep is
an hour of a machine that has to be quiet, and taking one at each wrap spends
it on a stamp the next phase invalidates before anyone reads it for a decision.
Sweeps belong to the phase that is *about* performance, which is also the phase
that will re-take every table under its own apparatus. What protects a reader
in the meantime is not freshness but the standing rules below: quote a
magnitude rather than three significant figures, and read a move against the
resolution floor for its regime. Reasoning:
[`../status/history/2026-08-31.md`](../status/history/2026-08-31.md), "A sweep
at every wrap buys a stamp the next phase invalidates".

**Every reading carries a witness to how quiet the machine was.** Two procfile
reads bracket each timed run — PSI's monotonic `total=` stall counters and
`/proc/stat`'s jiffies, `steal` included — and their difference is what the
machine did *during that rep*. Frequency and CPU temperature have no such
counter under `amd-pstate-epp`, so those alone are sampled at 5 Hz and
labelled with how many samples landed inside the window. Each table's
`Apparatus over every run in this table:` line reports the **worst** run, not
the average: a median survives one bad rep, but a reader deciding whether to
trust the number wants the worst the apparatus got. `raw.json` keeps the
per-reading detail.

**Counters, because a warm reading is half a second.** PSI's `avg10` and any
affordable sampling rate both describe a window many times longer than the
thing being measured; only a counter difference covers the window by
construction.

*Rejected:* **normalising a reading against the witnesses.** Dividing by a
"contention factor" needs a model of how contention maps to *this* workload's
slowdown, and that model cannot be a scalar: between the 2026-08-28 sweeps
`dd` (memory-bandwidth-bound) moved +20.1% while the CPU-bound `INSERT` scan
moved +0.4%, so any divisor correcting one over-corrects the other by 20×. A
mis-calibrated divisor emits a *confidently wrong* table, which is the failure
this harness exists to prevent. Contention is therefore grounds to **discard a
reading and take it again** — the discipline `drop_caches` already applies to a
dirty page cache. Control the apparatus; never model it.

**The gate is armed, per regime**: `cpu_busy_pct` 15, `psi_cpu_some_pct` 5 and
`cpu_steal_pct` 2, each roughly 3x the p95 observed over 182 readings, and
`psi_io_some_pct` gated in neither regime — a cold run drops the page cache and
reads 3.00 GiB off the SSD, so it stalls on I/O by construction and one global
limit would fire on every cold reading or none. A reading over a limit is
retaken up to three times; a figure that cannot get a quiet reading fails
loudly rather than publishing one nobody can defend.

**What no counter can see — and it is not only a VM problem.** A neighbour
saturating memory bandwidth appears as neither steal nor PSI: the CPU is
scheduled, nothing stalls on a runqueue, the instructions are simply slower.
The only witness is a co-measured one, which is what the `dd` floor already is.

That is measured, not predicted. A sweep taken on this machine while unrelated
processes read the HDD and ran duckdb queries **passed every gate** — machine
≤13% busy against the limit's 15, no steal, CPU stall ≤0.99% — while its warm
readings ran 5–45% slow against a quiet sweep of the same binaries and inputs.
What moved tracked bandwidth, not clocks: the warm tmpfs `dd` floor **+19–24%**
and the warm `COPY` scan **+45%**, against the cold SSD `dd` floor at **+0.1%**
and the line-skipping large-object path at +0.2% — with the busiest core
*faster* than in the quiet sweep (3.73–4.24 GHz against 3.59–4.09). So the
counters witness CPU contention, the floor witnesses bandwidth, and a sweep is
judged on both. Evidence:
[`../status/history/2026-08-28.md`](../status/history/2026-08-28.md), "A gate
that passes cannot mean a machine that was quiet".

Neither is a normaliser.

**The floor is read directionally, against a threshold of its own, and it
takes two conditions.** Contention makes a floor *slower* — that is the entire
mechanism it witnesses. So the disqualifying observation is a co-measured warm
floor **above** the standing one; one **below** it disqualifies nothing.
Without that half the check fires on drift it cannot tell from contention: the
pair this doc is stamped with read their `control` warm floor **15.9% apart**
in the *fast* direction, with no contention available to produce it, and a
symmetric check would have rejected whichever of the two it happened to see
second.

**The threshold is ~15%, and it is measured on floors rather than on figures.**
The warm resolution floor (~8%) does not serve here: it is read off warm
*figures*, and floors move further than the figures riding on them, so it sits
inside the band drift alone demonstrably produces. The two populations this
threshold has to separate are both measured here, and **they now overlap**:
pure drift moved a warm floor by **15.9%** within the stamped pair and put this
sweep's own `control` floor **22.4% above** the previous stamp's ("What a
session's own drift costs"), against the one witnessed contention episode's
**+19–24%** with the cold floor holding at +0.1% ("A gate that passes cannot
mean a machine that was quiet"). ~15% was the gap between them when only the
13.6% reading existed; it no longer separates them on its own, so it is kept as
a backstop and the conjunct below is what discriminates.

**And the slow move must be shared across the sweep's warm floors.** That is
the measured signature of the thing being witnessed: contention is
machine-wide, so it moved *every* warm floor together, while drift is per-file
on tmpfs — this sweep's `control` floor sits 22.4% above the previous stamp's
while the `arrays` file's sits 6.9% below it, and inside the pair the two moved
−15.9% and +5.0%. One file's floor moving slow on its own is staging luck;
every file's moving slow together is the machine. The number is the backstop,
the shared move is the discriminator, and a sweep is disqualified only when both
hold — which is what clears this one, whose `control` floor is over the
threshold and alone in being.

Two costs come with that, and neither is hidden. A contention episode **milder
than ~15%** now passes the gate — what catches it downstream is that a warm
figure is a ratio against its own co-measured floor, not an absolute. And with
two or three warm files in a sweep, "shared" is a weak test on its own, which
is why it is a conjunct with the number rather than a replacement for it — a
weak test now carrying the separation the number used to, which is the standing
cost of the overlap above.

**Which sweep is published is fixed before either runs**, so this check gates
the published sweep rather than selecting it. The first of the pair is the
publishable one and the second exists so `session-drift` has a second reading;
choosing between them on how they read is exactly the choice the session stamp
exists to remove. Under this stamp that costs something visible and is paid
anyway: the published sweep is the slower-floored of the pair on `control`
(+22.4% against the previous stamp's floor, where its partner sits at +2.9%)
and the faster-floored on `arrays`.

*Rejected:* a **symmetric** tolerance, disqualifying a floor move beyond the
envelope in either direction. A faster floor does mean something about staging
changed, and in this pair each file's warm readings moved with its own floor —
`control` down, `arrays` up. But that is
exactly what a warm figure being a *ratio against its own co-measured floor*
already handles, and it is reported where it belongs, in the drift figure's own
section. Rejecting a sweep for it would discard twelve good tables over a
property the tables themselves express.

*Rejected:* **pinning the CPU governor as part of the apparatus.** The
hypothesis was that an 8× scaling range (0.56–4.67 GHz) under `powersave` was
moving the memory-bandwidth-bound readings. Measured, it is not:
`amd-pstate-epp` is a hardware-managed P-state driver where the governor name
is very nearly cosmetic and the energy-performance preference does the work, so
a busy core boosts to 4.55 GHz under `powersave` and 4.55 GHz under
`performance`, with an identical `dd` median either way (0.270 s over three
runs of a 3 GiB tmpfs read). `measure.py --pin-governor` implements it and is
**off**, because pinning is an apparatus change that would oblige a full
re-sweep in exchange for nothing measurable here. It is kept rather than
deleted because the reasoning is machine-specific: a box on `acpi-cpufreq`
with a genuine `ondemand` governor would show exactly the effect this was
written for.

**Two prose recipes never go**, because the harness genuinely does not own
them: the census-off **source patch**, which no harness should perform, and the
generator invocations a reader may want on their own. koji's was a third until
the harness took it — `uv run measure.py --koji-recipe` prints it now.

## Which allocator a figure was taken under

**The platform allocator — glibc's `malloc` on this apparatus.** The choice is
`pgdump_query-cli`'s and never the library's, so every figure in this document
is a **CLI** figure taken under whatever `pgdq` links against, and an embedder
inherits whatever their own binary chose
([`architecture.md`](architecture.md), "The allocator is the binary's choice").
The harness reads the allocator out of the binary — `pgdq --version` names it —
so the session stamp above cannot go on saying `glibc` after the day the
default changes.

The reference column is the shipped binary itself — never a fourth build of the
same source, since two builds of one source can differ by ~10% from code layout
alone ("Two builds of one source can differ by layout"), which is larger than
the effect measured here. Its three readings are shared with the figures that
already take them where the sitting emits those too, exactly as the census
table's census-on column is shared with the warm throughput table; where it does
not, they are measured here and the table says so.

<!-- figure: allocator — reproduce with `cd scripts && uv run measure.py --figure allocator` -->

| Warm, on tmpfs | `system` — the shipped binary | `jemalloc` | `mimalloc` |
|---|---|---|---|
| `pgdq parse` — structure discovery | **0.481 s** (0.471–0.493) | **0.489 s** (0.475–0.499) — 1.02× | **0.480 s** (0.465–0.511) — 1.00× |
| `query --schema-mode strings` — zero-copy extraction | **4.14 s** (4.13–4.16) | **4.62 s** (4.61–4.69) — 1.11× | **4.12 s** (4.08–4.20) — 0.99× |
| `query --schema-mode typed` | **9.84 s** (9.77–9.99) | **10.41 s** (10.37–10.49) — 1.06× | **9.95 s** (9.83–10.00) — 1.01× |
| `dd` → `/dev/null` — the co-measured floor | **0.288 s** (0.286–0.289) | — | — |

Every leg was asked what it links against before it was timed — `system`,
`jemalloc`, `mimalloc` — so a leg whose build silently dropped its feature
cannot be published as a comparison of two identical binaries.

**Partial sweep**: the reference column was measured here rather than shared
with `census-brace-free` and `nested-end-to-end`, which this sitting did not
emit. Those are the same measurements and the harness says to emit them
together. They were not, deliberately, and the reason is **shelf life** rather
than cost.

*Rejected:* running the honest sitting before folding this table in. The
closure is four figures — `census-brace-free`, `nested-end-to-end` and both
throughput tables, which borrow the census reading in turn — and it costs
**13–27 minutes**, not the hour a first reading assumes; every input it needs
is already staged, and `--figure` takes the whole set in one invocation. So
cost is not what decides it. What decides it is that all four are *already*
stale on `pgdump_query/src/io.rs`, along with every other figure that times a
`pgdq` run: a sitting run today would produce a self-consistent set that the
wrap sweep overwrites inside this phase, buying a reference column with a shelf
life of weeks and buying the **ratios** — the only thing this table is read for
— nothing whatever, since those are within-sitting however the reference was
obtained. What the shortcut costs is what the paragraph below says of *any*
sitting of this table: its absolutes may not be set beside another table's.
That is disclosed here rather than inferred, which is the condition under which
a partial sitting is publishable at all.

Per-rep readings (s):
- `pgdq parse` (system): 0.484, 0.481, 0.493, 0.471, 0.473
- `pgdq parse` (jemalloc): 0.489, 0.475, 0.499, 0.479, 0.492
- `pgdq parse` (mimalloc): 0.484, 0.465, 0.480, 0.511, 0.470
- `query --schema-mode strings` (system): 4.16, 4.14, 4.15, 4.13, 4.14
- `query --schema-mode strings` (jemalloc): 4.61, 4.66, 4.62, 4.69, 4.61
- `query --schema-mode strings` (mimalloc): 4.08, 4.12, 4.17, 4.08, 4.20
- `query --schema-mode typed` (system): 9.82, 9.99, 9.84, 9.86, 9.77
- `query --schema-mode typed` (jemalloc): 10.37, 10.49, 10.40, 10.48, 10.41
- `query --schema-mode typed` (mimalloc): 9.95, 10.00, 9.92, 10.00, 9.83
- `dd` → `/dev/null` (warm): 0.288, 0.286, 0.287, 0.289, 0.288

Apparatus over every run in this table: CPU stall ≤0.27%, I/O stall ≤10.32%,
machine ≤5% busy, steal ≤0.00%, busiest core ≥3.59 GHz, ≤65°C. **Taken in its
own sitting, alone** — no other figure was taken with it, which is why its
reference column is measured rather than shared — not in the `ba2fc12` sweep
the stamp above records, so setting
one of its absolutes beside that sweep's costs the ~8% a warm absolute resolves
to across sessions, while its **ratios** are within-sitting, which is what the
table is for.

**Nothing beats the platform allocator on any of the three shapes, and the two
readings that once said otherwise were both about something else.** This
sitting is the read path's buffer pool
([`architecture.md`](architecture.md), "Execution model and API surface"); the
sitting before it was not, and the difference between them is the whole result:

- **`jemalloc`'s `parse` was 1.87× and is 1.02×.** It was never the allocator
  being slow. All of it was system time — 0.27 s → 0.80 s on the host, with
  user time slightly *lower* and 8% fewer user instructions — and `strace -c`
  said why: 3,161 `madvise` calls against glibc's 50 and mimalloc's 62, over a
  3.00 GiB file read in 3,072 chunks. That was
  `LocalFileSource::read_range`'s per-chunk `vec![0u8; 1 MiB]` being returned to
  the kernel and re-faulted once per chunk. Pool the buffer and the storm, and
  the ranking's largest number with it, is gone.
- **`mimalloc`'s `typed` was 0.96× and is 1.01×.** That 3–4% was the one cell
  of the old table that reproduced its magnitude across two sittings, and it was
  the whole of the case for adopting. It does not survive the buffer pool: the
  spread here (9.83–10.00) sits above the reference's (9.77–9.99) rather than
  below it, and the remaining `parse` and `strings` cells are 1.00× and 0.99×.

**The decision this figure existed to make is therefore made: the platform
allocator stays.** The features stay in the manifest so a later re-take costs
five minutes, and the deadline that bound it — settle before the wrap's sweep
pair, since adopting after it would invalidate a freshly-taken thirteen-table
document with no sweep left to repair it — is discharged rather than deferred
again. The reasoning that would have applied had a leg won is beside the
mechanism ([`architecture.md`](architecture.md), "The allocator is the binary's
choice").

**The old table's legs were nearly published against a fresh reference.**
`ensure_allocator_binary` short-circuited on `runs/pgdq-alloc-<leg>` existing,
and that file outlives a session, so the first re-take timed the *previous*
session's jemalloc and mimalloc binaries against this session's `pgdq` — two
different sources, reported as two allocators, with nothing to notice because a
stale leg still answers `--version` with its own allocator name. The build is
now memoized per **process** rather than per machine, and every leg is built
before the first reading rather than lazily at the rep that wants it. The tell
in a run log: `building the <leg> allocator leg into …` must appear once per
non-reference leg before `rep1`.

## Scan throughput by input shape

Three 3.00 GiB synthetic dumps, a whole-file `pgdq` scan in a 512MB-limited
container, in three regimes. **Cold** is `drop_caches` before every run,
including before the floor, because that is the only regime in which a device
floor means anything: this file fits page cache twice over, so a second read of
it measures RAM. **Warm** is the same files on tmpfs, which is where the CPU
the device hides becomes visible. **Cold on the NVMe** is the same discipline
as the first on a device roughly 4.5× as fast, and it is here for one reason:
it is the only device class we own on which reading the bytes and parsing them
are within a small factor of each other, so it is the only place a readahead,
`fadvise` or chunk-size default can show anything at all.

<!-- figure: scan-throughput-cold — reproduce with `cd scripts && uv run measure.py --figure scan-throughput-cold` -->

**Every run cold, on the SSD**

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | **5.78 s** (5.75–5.83) | ~557 MB/s | 1.01× the floor's time |
| Large-object region | **5.77 s** (5.76–5.77) | ~558 MB/s | 1.00× the floor's time |
| `INSERT` run | **5.87 s** (5.87–5.88) | ~548 MB/s | 1.02× the floor's time |
| `dd` → `/dev/null` | **5.75 s** (5.75–5.75) | ~560 MB/s | — |

Apparatus over every run in this table: CPU stall ≤1.03%, I/O stall ≤17.77%, machine ≤3% busy, steal ≤0.00%, busiest core ≥4.02 GHz, ≤60°C. **Taken in its own sitting**, with the warm table below and the census table they share their `COPY` row with, not in the `ba2fc12` sweep the stamp above records.

<!-- figure: scan-throughput-warm — reproduce with `cd scripts && uv run measure.py --figure scan-throughput-warm` -->

**Every run warm, on tmpfs**

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | **0.532 s** (0.519–0.588) | ~6049 MB/s | 1.76× the floor's time |
| Large-object region | **0.449 s** (0.448–0.464) | ~7174 MB/s | 1.49× the floor's time |
| `INSERT` run | **2.27 s** (2.26–2.27) | ~1422 MB/s | **7.50× the floor's time** |
| `dd` → `/dev/null` | **0.302 s** (0.300–0.305) | ~10666 MB/s | — |

Apparatus over every run in this table: CPU stall ≤0.24%, I/O stall ≤7.00%, machine ≤5% busy, steal ≤0.00%, busiest core ≥4.04 GHz, ≤66°C. **Taken in the same sitting as the cold table above.**

<!-- figure: scan-throughput-nvme — reproduce with `cd scripts && uv run measure.py --figure scan-throughput-nvme` -->

**Every run cold, on the NVMe**

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | **1.406 s** (1.285–1.499) | ~2291 MB/s | 1.10× the floor's time |
| Large-object region | **1.532 s** (1.450–1.828) | ~2103 MB/s | 1.20× the floor's time |
| `INSERT` run | **3.40 s** (3.31–3.45) | ~946 MB/s | **2.66× the floor's time** |
| `dd` → `/dev/null` | **1.281 s** (1.259–1.305) | ~2515 MB/s | — |

Per-rep readings (s):
- `COPY` block (cold-nvme): 1.406, 1.340, 1.499, 1.412, 1.285
- Large-object region (cold-nvme): 1.594, 1.526, 1.828, 1.532, 1.450
- `INSERT` run (cold-nvme): 3.41, 3.36, 3.45, 3.40, 3.31
- `dd` → `/dev/null` (cold-nvme): 1.280, 1.259, 1.292, 1.281, 1.305

Apparatus over every run in this table: CPU stall ≤0.61%, I/O stall ≤10.27%, machine ≤9% busy, steal ≤0.00%, busiest core ≥3.47 GHz, ≤66°C. **Taken entirely alone**, on 2026-09-04 at `e889634`, five reps rather than the other two tables' three — each reading here is a third of a cold SSD one, and what the table is read for is a ratio near 1 where a few percent decides three levers. The harness change that added the regime was uncommitted when it ran, which is what the stamp's "uncommitted changes under a measured path" recorded; no library path was dirty, so the binary is `e889634`'s exactly.

Each cold-SSD and warm table's `COPY` row is the census figure's census-on
column for that regime — the same binary, the same command, the same input, not
a second measurement of it. **The NVMe table borrows nothing**: no census
figure is taken in that regime, so its `COPY` row is its own reading.

Every run completes inside the 512 MB cgroup, which is the memory claim this
apparatus can actually make. **It carries no max-RSS figure**: `/usr/bin/time
-f %M` around `nerdctl run` reports the *nerdctl client's* peak, not pgdq's —
it read the same ~40–45 MB for a 2 MB input as for a 3.00 GiB one, four times
what koji's row below records for a 784 GB scan. pgdq's own resident set is the
koji figure, ~9 MiB.

**What this says.** All three paths are device-bound to the point of
disappearing into the device: cold on the SSD, each spends **1.00–1.02×** the
wall-clock of reading the same bytes and doing nothing. Warm, the same scans
cost 0.532 s, 0.449 s and 2.27 s against a 0.302 s `dd` floor — so the CPU is
there, and at 557 MB/s the disk covers all of it.

**"Device-bound" is a claim about a device, and the NVMe is where it stops
holding for one of the three.** At 2515 MB/s the `COPY` and large-object paths
are still inside the device — **1.10×** and **1.20×** its time — and the
`INSERT` path is not: **2.66×**, which is 2.1 s of a 3.40 s scan spent
somewhere the disk is idle. The ratios are what may be read across the three
tables; the absolutes may not, these having been taken in a different sitting a
day later, on a commit four library changes ahead of the stamp above.

**The one number the I/O-defaults levers are sized against is 1.10×.** Whatever
readahead, `posix_fadvise` or a different chunk size could do, none of them can
put a scan below the time the device takes to deliver the bytes — so on the
fastest disk this project has, the whole prize for overlapping I/O with parsing
is the **0.125 s** by which a cold `COPY` scan exceeds its own floor, **8.9%**
of that scan, and less than that in practice since no scheme overlaps
perfectly. The kernel's own readahead is what has already taken the rest.
Nothing about a slower device changes that arithmetic in the levers' favour:
on the SATA SSD the same subtraction is 1% and on the HDD the scan is device-
bound by a factor of several.

**The `INSERT` path is still a different algorithm, and the warm table is the
only place that shows it.** Warm, an `INSERT` run costs **2.27 s against the
`COPY` path's 0.532 s on the same 3.00 GiB — 4.3× the per-byte CPU**, and
**7.5× the `dd` floor** where the `COPY` path is 1.8×. A `COPY` block's data is
walked and skipped; an `INSERT` run's bytes have to be read quote-aware to
find where each statement ends, because that is the only thing that says where
one row stops ([`architecture.md`](architecture.md), "Bulk regions"). Cold on
the SSD the difference is gone entirely — 1.02× against 1.01× — which is
exactly why these tables are here together rather than one being differenced
against another's regime. **Cold on the NVMe it is back**: 2.66× against 1.10×,
the same algorithms against a device fast enough to stop paying for them.

**Quote it as a small multiple, not to three figures.** The ratio is the
durable half of this table and neither of its legs is: across sweeps the legs
move several percent while the ratio does not, and the same caution that
applied at mid-teens applies here.

**This table is what `KD9` is read off, and the warm row is what keeps the
entry live.** Under the `ba2fc12` stamp the same two rows read **9.19 s warm against
0.558 s — 16.5×** — and **10.87 s cold, 1.89× the floor**. P7's slice 7.5 put
the `INSERT` run's statement scan on raw bytes: no `String` per line, no
statement buffer to re-walk, and a `memchr` pass across the string values that
are most of what an `INSERT` statement is
([`architecture.md`](architecture.md), "Bulk regions"). What is left is partly
a property of the two algorithms — an `INSERT` run's end can only be found by
crossing every byte — and partly two named, untaken cuts, which is why `KD9`
is rewritten to the residual rather than struck. **The cold-SSD row was never
the evidence that the residual is free, and the NVMe table is the evidence that
it is not**: 7.8 took it, and an `INSERT` scan there costs **2.66× the device's
own time** where the `COPY` path costs 1.10×. A user on ordinary SATA storage
pays nothing for the residual and a user on NVMe pays most of the scan for it,
which is the reading that settles what the entry is about. The claim is
corrected wherever it is repeated —
[`pg-dump-compatibility.md`](pg-dump-compatibility.md),
[`roadmap.md`](roadmap.md) and [`architecture.md`](architecture.md), which
`--check` names as this table's consumers along with
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md), whose lever
table records the stake the phase *committed* to and is left alone
([`../process.md`](../process.md), "Progress lives in STATUS, never in the
spec").

**Both legs of the earlier ratio drifted between stamps and the ratio did
not**, which is the reading that says to quote the multiple rather than the
seconds. The `COPY` leg went 0.500 → 0.558 s across two stamps and the
`INSERT` leg 8.37 → 9.19, for 16.7× against 16.5×; ~10% of the `INSERT` move
was attributed to code layout rather than to code, and the rule that came out
of it is "Two builds of one source can differ by layout" below. The row's
earliest reading is the same story one stamp further back: it read 15.13–15.42 s
cold when first taken on 2026-08-25, and `2eb51f4` changed how an `--inserts`
dump's runs are scanned in between, absorbing the `Data for` comment into the
run.

Regenerate the three inputs, for a reader who wants them without the harness:

```sh
cd scripts
uv run generate_perf_data.py          --size-mb 3072 --seed 42 /path/to/copy_control.sql
uv run generate_large_object_bench.py --size-gb 3    --seed 42 /path/to/large_object.sql
uv run generate_insert_run_bench.py   --size-gb 3    --seed 42 /path/to/insert_run.sql
```

All three are deterministic under `--seed`, which is what lets these tables be
re-taken whole rather than re-measured against different bytes.

The `COPY` control is 814,362 rows of 16 columns, 3,956 bytes each, and holds
no `{` or `[` in any data row — see "The census on brace-free rows" below for
why that is a contract rather than an accident. The `INSERT` generator writes
one `INSERT INTO public.bench_inserts VALUES (…);` per line under an ordinary
`TABLE DATA` TOC comment, with an apostrophe doubled the way `pg_dump` writes
one in ~15% of them, so the statement scan's quote tracker is genuinely
exercised.
The large-object generator is `LOBBUFSIZE`-chunked to match real `pg_dump`.

Both tables come from one command:

```sh
cd scripts && uv run measure.py --figure scan-throughput-cold --figure scan-throughput-warm
```

`parse` is the only command that reads the dump
([`architecture.md`](architecture.md), "CLI surface"), and the cache it must
write goes to the container's ephemeral layer — a few hundred KB against 3 GiB
read.

**The large-object skip is the measurement that justifies it.** Completing a
3 GiB region inside a 512 MB limit, at the same rate the `COPY` path walks the
same kind of bytes and a fifth of what the `INSERT` path costs, is the
"skipped, not walked" evidence: walking it statement by statement would mean
one span *and one stored text string* per `lowrite` call, hundreds of
thousands of them.

## What the read chunk size is worth

<!-- figure: chunk-size — reproduce with `cd scripts && uv run measure.py --figure chunk-size` -->

One `pgdq parse` of the 3.00 GiB `COPY` control at six read chunk sizes, in all
three regimes, differing in nothing but the number `--chunk-size` carries.
`scan::DEFAULT_CHUNK_SIZE` is the shipped constant and the ratio column is
against its row. The figure exists to decide one of P7's three I/O defaults and
to bound the other two.

**Nine reps, not the throughput tables' three or five.** The cold-NVMe `COPY`
row above spreads about 15% of its median over five reps, against a total
envelope for all three I/O levers of 8.9%. An instrument that cannot resolve a
lever cannot report that the lever is worth nothing — it can only report that it
saw nothing, which is a different sentence.

| Chunk | Warm, tmpfs | Cold, SATA SSD | Cold, NVMe |
|---|---|---|---|
| 64 KiB | **0.611 s** (0.552–0.659) · 1.53× | **5.79 s** (5.77–5.80) · 1.00× | **1.643 s** (1.516–1.823) · 1.18× |
| 256 KiB | **0.445 s** (0.422–0.505) · 1.12× | **5.78 s** (5.76–5.79) · 1.00× | **1.423 s** (1.329–1.489) · 1.03× |
| 1 MiB *(default)* | **0.399 s** (0.386–0.445) · 1.00× | **5.78 s** (5.76–5.79) · 1.00× | **1.387 s** (1.323–1.594) · 1.00× |
| 4 MiB | **0.400 s** (0.375–0.423) · 1.00× | **5.78 s** (5.77–5.80) · 1.00× | **1.455 s** (1.357–1.754) · 1.05× |
| 8 MiB | **0.386 s** (0.370–0.397) · 0.97× | **5.79 s** (5.78–5.85) · 1.00× | **1.599 s** (1.544–1.692) · 1.15× |
| 16 MiB | **0.825 s** (0.807–0.941) · 2.07× | **5.83 s** (5.81–5.87) · 1.01× | **1.897 s** (1.821–1.915) · 1.37× |

Per-rep readings (s):
- 64 KiB (warm): 0.564, 0.659, 0.569, 0.553, 0.615, 0.644, 0.618, 0.552, 0.611
- 256 KiB (warm): 0.434, 0.445, 0.455, 0.422, 0.462, 0.463, 0.440, 0.505, 0.445
- 1 MiB (warm): 0.386, 0.409, 0.399, 0.389, 0.445, 0.413, 0.394, 0.387, 0.428
- 4 MiB (warm): 0.378, 0.399, 0.400, 0.384, 0.401, 0.417, 0.406, 0.423, 0.375
- 8 MiB (warm): 0.388, 0.388, 0.381, 0.375, 0.391, 0.397, 0.386, 0.370, 0.372
- 16 MiB (warm): 0.818, 0.853, 0.920, 0.807, 0.941, 0.826, 0.825, 0.822, 0.809
- 64 KiB (cold): 5.80, 5.78, 5.79, 5.79, 5.79, 5.79, 5.80, 5.77, 5.78
- 256 KiB (cold): 5.78, 5.79, 5.77, 5.76, 5.78, 5.79, 5.79, 5.78, 5.78
- 1 MiB (cold): 5.76, 5.77, 5.78, 5.79, 5.78, 5.78, 5.79, 5.76, 5.78
- 4 MiB (cold): 5.78, 5.78, 5.77, 5.80, 5.78, 5.79, 5.78, 5.77, 5.78
- 8 MiB (cold): 5.78, 5.79, 5.79, 5.85, 5.79, 5.79, 5.80, 5.78, 5.78
- 16 MiB (cold): 5.82, 5.83, 5.81, 5.84, 5.84, 5.82, 5.84, 5.81, 5.87
- 64 KiB (cold-nvme): 1.566, 1.698, 1.516, 1.661, 1.726, 1.823, 1.643, 1.524, 1.555
- 256 KiB (cold-nvme): 1.423, 1.423, 1.329, 1.427, 1.489, 1.426, 1.472, 1.354, 1.356
- 1 MiB (cold-nvme): 1.594, 1.425, 1.323, 1.407, 1.387, 1.452, 1.354, 1.362, 1.336
- 4 MiB (cold-nvme): 1.754, 1.420, 1.374, 1.455, 1.501, 1.515, 1.540, 1.417, 1.357
- 8 MiB (cold-nvme): 1.692, 1.589, 1.550, 1.608, 1.627, 1.599, 1.593, 1.639, 1.544
- 16 MiB (cold-nvme): 1.897, 1.895, 1.821, 1.909, 1.915, 1.913, 1.897, 1.915, 1.855

Apparatus over every run in this table: CPU stall ≤1.15%, I/O stall ≤41.78%, machine ≤14% busy, steal ≤0.00%, busiest core ≥3.60 GHz, ≤67°C. **Taken entirely alone**, on 2026-09-04, against `8712756` plus the working-tree change that added the flag and this figure — which is what the stamp's "uncommitted changes under a measured path" records, and which is unavoidable for a figure whose instrument is the change being measured.

**The default is the fastest row, and nothing is within reach of beating
it.** On the one device class where a chunk size can show anything, 1 MiB is
the fastest median in the table. Its two neighbours are not distinguishable
from it — 256 KiB at 1.03× and 4 MiB at 1.05×, both inside the reps' own spread
— and everything further out is clearly slower: 1.18× at 64 KiB, 1.15× at
8 MiB, 1.37× at 16 MiB, each with a spread that does not reach the default's.
So the chunk-size lever is not worth "at most 8.9%" — it is worth **nothing**,
because no value measured beats the one already shipped and the ones that could
have are ties. The constant stays at 1 MiB and
`--chunk-size` is a tuning escape hatch for a device unlike these three, not a
knob with a win behind it.

**The SATA SSD reads 1.00× at every size, which is the point of having it in
the table.** Nothing can be won there — the device is the whole cost — but
something could have been *lost*, and this is what says a default chosen on the
NVMe does not cost the other classes anything. The one exception is small and
in the same direction as everywhere else: 16 MiB is 1.01×.

**A deeper read is not a faster one, and that is what settles `fadvise`.** The
16 MiB row is a 16 MiB synchronous read issued while the parser is idle —
a far deeper prefetch than `POSIX_FADV_SEQUENTIAL`'s doubled window, and one
the kernel is told about rather than has to infer. It is the **slowest** row
cold on the NVMe, and 8 MiB is slower than 1 MiB too. Cold time does not fall
with request depth on any device here, so the kernel's own readahead has
already taken what there was to take and a hint asking for more has nothing to
win ([`architecture.md`](architecture.md), "Execution model and API surface",
where both schemes are refused).

**16 MiB doubles the warm scan, and that is the read path's pool ceiling
rather than the chunk size.** `io::BufferPool` keeps nothing above 8 MiB, so at
16 MiB every chunk is a fresh `vec![0u8; len]` — the `calloc` the pool exists
to remove, back once per chunk. Warm, where nothing hides it, that is 0.386 s
→ 0.825 s, **2.07×**, against the 8 MiB row immediately above it. It is the
same cost the buffer pool was landed to remove, re-entering through a knob, and
it is why the sweep brackets the ceiling rather than stopping at it.

**Small chunks cost CPU, not I/O.** 64 KiB is 1.53× warm and 1.18× cold on the
NVMe, and 1.00× on the SATA SSD — the per-chunk work (a syscall, a pool
take/give, a carry check) paid 16× as often, which the slower device hides
entirely and the faster one does not.

## The census on brace-free rows costs a few percent of a warm scan

<!-- figure: census-brace-free — reproduce with `cd scripts && uv run measure.py --figure census-brace-free` -->

The census walks every data row of every block any mapping pass maps — a cold
query's included, since a mapped block always carries one
([`architecture.md`](architecture.md), "The array shape census"), so it is a
change to the scan hot path. The 3.00 GiB `COPY` control the generator writes
by default — 814,362 rows of 16 columns, 3,956 bytes each on `--seed 42`: no
`{` or `[` in any data row, so every row is rejected by the census's own pre-filter
after one pass over its bytes and no row is ever split into fields. That is
deliberately the koji shape — koji's six array columns are entirely NULL — and
it is the case worth knowing the price of, since it is what a `pgdq parse`
over a real dump mostly does.

Census on is the working tree; census off is the same tree with one line
added, so nothing but the census differs between the two binaries (below).

Six reps each, the pair run in both orders; medians, with the full spread
beside them.

| | Census off | Census on | Δ |
|---|---|---|---|
| cold, on the SSD | **5.78 s** (5.77–5.84) | **5.78 s** (5.75–5.83) | **+0.002 s, +0%** |
| warm, on tmpfs | **0.484 s** (0.475–0.509) | **0.532 s** (0.519–0.588) | **+0.048 s, +10%** |

Apparatus over every run in this table: CPU stall ≤1.01%, I/O stall ≤19.44%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.77 GHz, ≤59°C. **Taken in its own sitting**, with the two scan-throughput tables whose `COPY` row is this table's census-on column, not in the `ba2fc12` sweep the stamp above records — it was re-taken because those two were, and its census-on column *is* their `COPY` row.

Both binaries complete inside the 512 MB cgroup; no max-RSS figure is quoted,
for the reason under the scan-throughput table. `dd` → `/dev/null` on the same
file in the same container: **0.304 s** warm and **5.75 s** cold, so the
census-off scan is already within 1.6× of what the kernel charges to hand over
the bytes.

**What this says.** The pre-filter is very nearly free on the shape a real
dump mostly has: **0.048 s per 3.00 GiB of brace-free rows**, 60 ns per
16-column row of 3,956 bytes. That implies tens of GB/s, which is above what
this machine's DRAM will give one core — so the reading is not a
memory-bandwidth figure at all: the pre-filter re-walks bytes the scanner has
just walked, out of cache, and `memchr2` is fast enough that what is left is
the loop, not the bytes.

**Cold, the census is invisible**: +0.002 s on a 5.78 s scan, one part in
2,900, well inside the run-to-run spread of the two legs it is the difference of
(5.77–5.84 against 5.75–5.83). Earlier stamps read the same row at +0.011 s, at
+0.004 s and at −0.008 s — a *negative* census cost, which is the plainest
possible statement that the effect is smaller than the noise it sits in. That row is not a separate finding —
it is the same CPU cost, hidden behind a device delivering 557 MB/s. Which one
a user sees is decided by whether the bytes are already resident.

**Read the warm Δ against the instrument, not to three figures.** "What a
session's own drift costs" below puts warm sub-second readings several percent
apart between two sweeps of identical binaries and inputs — and both legs of
this Δ are such readings, moving independently — so +10% is "a few percent to a
tenth", and the +6%, +7%, +8%, +9% and +11% other sweeps read are the same
measurement rather than a change. Six sweeps have read it and every one is
positive, which is the finding; **the heading names no number** because that
number is the half the instrument does not hold still, and `measure.py`'s
own heading string for this figure still carries the 8% one sweep read; this stamp's own reading is
+10%, which is inside that band and not a change.

**It was not always.** The scalar `raw.iter().any(…)` loop that preceded
`memchr2` ran at ~3.8 GB/s and cost 1.03 µs per row — +39% as recorded and
**+63% reconstructed**, once the 0.77 s container wrapper that sat in both legs
is taken out ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
That figure is what argued for the swap, and it is why the deferred question
of a *skippable* census is now closed rather than open: tens of nanoseconds per
row is not a cost worth a knob.

So the census's cost is effectively **one tier, not two**: it is paid by the
rows that pass the pre-filter (next section), and the pre-filter itself is
2% of what those rows cost. It is unconditional either way
([`architecture.md`](architecture.md), "The array shape census") — the
alternative is a query that cannot retype its array columns without a second
pass. What the figure does *not* license is calling it exactly zero — but nor does
it rest on the two spreads separating, which under this stamp they do not:
census-off's warm readings span 0.504–0.592 against census-on's 0.548–0.583,
one wide leg swallowing a narrow one. What carries it is the pairing — six reps
each, run in both orders — and six sweeps reading the Δ positive; the earlier
reading that said zero came from taking the pair while the page cache was still
filling.

**The control's brace-freeness is a contract, not an accident.** The same
generator writes array columns behind `--arrays` and a composite behind
`--composite`, and those flags exist precisely so its *default* output stays
what this figure and the scan-throughput table above were taken on. Anything
that puts a `{` or `[` into the default rows invalidates both.

**The census-off binary is a source patch, which no harness performs.** Build
it once, by hand: put a bare `return;` as the first statement of `pub(crate) fn
on_row` in `map.rs` — the pre-filter and everything after it, and nothing else
— then

```sh
cargo build --release -p pgdump_query-cli          # default target: glibc
cp target/release/pgdq runs/pgdq-nocensus          # then revert map.rs
```

and the harness takes it from there, staging both regimes, interleaving the
pair, reversing the order halfway and taking the floor in the same container:

```sh
cd scripts && uv run measure.py --figure census-brace-free
```

**Never redirect stderr inside a timed command.** Some shells route `time`'s
own report through the timed command's redirection, so a `2>/dev/null` meant
to hide the binary's chatter deletes the figure and leaves a labelled run with
no number under it. The harness's own tests assert that none of its commands
does this; the trap is recorded because a hand-run one still can.

## The census on array-bearing rows more than triples a warm scan

<!-- figure: census-arrays — reproduce with `cd scripts && uv run measure.py --figure census-arrays` -->

The other side of the figure above: a 3.00 GiB dump where **every** row holds
an array, so the census's pre-filter passes on all of them and every field of
every row is split out and inspected. Generated by the same script with
`--arrays --composite`, so the file differs from the control in exactly the
three stress columns — `v_int_array` (3–5 elements), `v_int_array_long` (50)
and `v_comp` (a two-field composite). 699,962 rows, 4,602 bytes each, 19
columns.

Census on is the working tree; census off is the same tree with one line
added, so nothing but the census differs between the two binaries (below).
Six reps each, the pair run in both orders; medians, with the full spread.

| | Census off | Census on | Δ |
|---|---|---|---|
| cold, on the SSD | **5.77 s** (5.76–5.78) | **5.85 s** (5.83–5.85) | **+0.080 s, +1%** |
| warm, on tmpfs | **0.476 s** (0.469–0.503) | **1.522 s** (1.493–1.592) | **+1.045 s, +219%** |

Apparatus over every run in this table: CPU stall ≤1.08%, I/O stall ≤20.63%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.61 GHz, ≤60°C.

Both binaries complete inside the 512 MB cgroup; no max-RSS figure is quoted,
for the reason under the scan-throughput table. The `dd` floor for this file in
this container is 0.299 s warm and 5.75 s cold, so warm census-off is within
1.6× of it and census-on is 5.1× it.

**What this says.** The census costs **1.045 s per 3.00 GiB of array-bearing
rows** — 1.49 µs per 19-column row — which is **+219%** on a scan reading from
memory, i.e. the census does a bit over twice the work the rest of the scan
does on this shape. Cold it is **+1%**, hidden behind the device
exactly as the brace-free case is. Both rows are the same CPU; which one a user
sees is decided by whether the bytes are already resident.

**The pre-filter is a few tens of nanoseconds of that 1.49 µs** — 36 ns as the
previous section read it under the `ba2fc12` stamp and 60 ns as it reads it
now, the same census-off binary running the pre-filter on every row of both
files, which is what licenses the subtraction. That is a **cross-sitting**
subtraction since this table was not re-taken beside that one, and it survives
being one because the conclusion does not turn on which value is used:
splitting the row into fields and running `observe` over all 19 of them — the
work the pre-filter exists to avoid — is **96–98% of the census's whole
cost**, and the pre-filter is what keeps the brace-free case off that path. The census is
unconditional either way (`architecture.md`, "The array shape census") — the
alternative is a query that cannot retype its array columns without a second
pass.

**The field-splitting half is the same on either libc**, which is what says it
is CPU rather than allocator: the musl leg of the same sweep read 1.29 s
census-off against 2.56 s census-on, a Δ of 1.27 s against glibc's 1.26 s,
while its *absolute* legs were 2.5× higher.

The same census-off binary the previous section builds, on the same apparatus:

```sh
cd scripts && uv run measure.py --figure census-arrays
```

The input alone, for a reader who wants it without the harness:

```sh
cd scripts && uv run generate_perf_data.py --arrays --composite \
  --size-mb 3072 --seed 42 /dev/shm/pgdq/arrays.sql
```

*Rejected:* a `no-census` cargo feature, so this reproduces as a flag instead
of a source edit. Neither crate declares a `[features]` section today, and the
first one a project adds sets the precedent for what features are for — here,
a build in which `architecture.md`'s "the census is unconditional" is untrue,
serving a comparison taken about once a phase. The escape if the patch-and-
revert ever bites is to drop the comparison, not to gate it: the absolute
figures (tens of ns/row rejected, 1.49 µs/row inspected) are what
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md) actually consumes, and
the census-off column exists to establish it once.

## Nested decode costs what it copies, and an element is an allocation

<!-- figure: nested-decode-micro — reproduce with `cd scripts && uv run measure.py --figure nested-decode-micro` -->

`benches/decoders.rs`'s `nested` group, criterion medians. **Two controls,
because there are two questions.**

- **Copy** — `String::from` over the same byte count: 20 ns at 49 bytes,
  24 ns at 601, 20 ns at 42. A nested value has no borrowed arm
  (`crate::batch::append_nested`), so this isolates what the *parse* costs on
  top of the copy it cannot avoid, which is the variable P7 is choosing
  over.
- **View** — one `append_view_unchecked` into a block the builder does not
  own, which is what `push_utf8view_field` does for an unescaped text field:
  **2.55 ns**, from `text_view_x1024`'s median ÷ 1024. Length-independent,
  which is the point of a view. This is what a user comparing a text column
  against an array column actually pays.

| Literal | Bytes | `decode` | `render` | ÷ copy | ÷ view |
|---|---|---|---|---|---|
| `integer[]`, 4 elements | 49 | 290 ns | 223 ns | **26.0×** | **202×** |
| `integer[]`, 50 elements | 601 | 3.85 µs | 1.48 µs | **226.4×** | **2092×** |
| two-field composite | 42 | 190 ns | 178 ns | **18.9×** | **145×** |

**Both control figures are read with a caveat.** `text_view_x1024` reports
1024 appends and must be divided — timing one append through
`iter_batched_ref` gave ~12.6 ns against a harness floor that `bool/decode`
puts at ~1.1 ns, so three quarters of it was criterion. And 2.55 ns is a
*floor* on the borrowed arm rather than the borrowed arm itself:
`push_utf8view_field` also scans the chunk deque with `find_map` and calls
`block_for`. So the `÷ view` column bounds the real ratio **from above**.

**The `÷ copy` column for the 50-element row is the least stable number in the
table**, because its denominator is: the 601-byte copy control has read 24, 42,
41, 24 and 24 ns over five sweeps, which put that ratio at 122× two stamps back
and 226× here while `decode` itself moved 11%. Read the `decode`
and `render` columns, which are what the design consumes; treat `÷ copy` as
the order of magnitude it establishes.

**What this says.** Cost is per *element*, not per byte: the two array lengths
differ only in element count, and the slope between them is **77 ns per
element** decoding and **27 ns per element** rendering. That is the shape of
one allocation per element, which is what `ArrayLiteral::elements` being a
`Vec<Option<String>>` buys — every element is its own `String`. A composite
sits where its field count says it should: two fields, and it costs about what
a four-element array does.

Comparing a composite against an array of the same *byte* count is therefore
meaningless; comparing them per element is the only reading these three rows
support.

```sh
cd scripts && uv run measure.py --figure nested-decode-micro
```

It reads criterion's own `estimates.json` rather than scraping the console,
because a rounded ratio is how a table acquires a number nobody can reproduce.
The bench alone is `cargo bench -p pgdump_query --bench decoders -- nested`.

## A typed query over nested columns costs 13.5 µs a row more than a string one

<!-- figure: nested-end-to-end — reproduce with `cd scripts && uv run measure.py --figure nested-end-to-end` -->

The end-to-end half of the figure above: what the per-element cost actually
costs a user. The control is within a file — `--schema-mode strings` resolves
every column to `Utf8View` and takes the zero-copy path, so the `typed` run
differs from it by decode plus Arrow build plus typed render and nothing else.
**That third term is the CLI's, and it is the largest of the three** — 79% of
the control's gap — so a difference in this table sizes what a CLI user pays
and not what a library lever can remove; see "A mode difference and a
per-column delta are CLI numbers" above.

**This table no longer attributes cost to a particular column.** That was the
job of the further 3.00 GiB dumps holding fewer of the nested columns, and
"What a column costs: five projection widths over one file" below does it
instead, over identical rows of one file. What the three files are still for is
the finding underneath them: the untyped baseline is not file-independent, and
the census is why.

Three inputs on tmpfs, output to `/dev/null`. **One interleaved sweep**: five
reps, each rep running both modes on all three files in turn, so the slow
upward drift across a long session lands on every row equally rather than on
whichever file went first. Medians of five:

| File | Rows | `strings` | `typed` | `typed` − `strings` | Ratio |
|---|---|---|---|---|---|
| control — 16 scalar columns | 814,362 | 4.42 s | 10.36 s | **7.30 µs/row** | 2.35× |
| `--composite` — the same 16 plus one composite | 803,995 | 4.42 s | 10.94 s | **8.11 µs/row** | 2.47× |
| `--arrays --composite` — the same 16 plus three nested | 699,962 | 5.36 s | 19.93 s | **20.82 µs/row** | 3.72× |

Apparatus over every run in this table: CPU stall ≤0.15%, I/O stall ≤1.51%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.61 GHz, ≤64°C.

Every run completes inside the 512 MB cgroup; no max-RSS figure is quoted, for
the reason under the scan-throughput table.

**The per-row difference is the figure; the ratio is derived and does not
travel.** A ratio carries that session's `strings` leg in its denominator, and
that leg is apparatus-sensitive far beyond the session drift measured below:
the same three files read 9.74 / 9.76 / 9.56 s on a page-cache-warm SSD with a
musl binary and 0.77 s of wrapper, against 4.42 / 4.42 / 5.36 s here. That
alone moved the nested ratio 3.08× → 3.72× while the per-row difference moved
15.1 → 13.5 µs. Quote a ratio only against the sweep it came from; the design
consumes the differences.

**What this says.** Typing the 16 scalar columns costs **7.3 µs per row**;
typing those plus the three nested ones costs **20.8 µs per row**. So three
nested columns — 19% more columns — cost **13.5 µs of every row**, nearly
twice what all sixteen scalar columns together cost. **The two array columns
carry essentially all of it** — 13.21 µs against the composite column's 0.98,
which the projection table below reads directly rather than by differencing two
files.

Each per-row figure is that file's own `typed` minus its own `strings`, which
is what makes the subtraction legitimate: whatever the untyped baseline is
worth on a given file — and the three files hold different row counts at the
same byte count — it cancels out of that file's own difference, and would not
cancel out of a cross-file ratio.

**The untyped baseline is not file-independent, and the cause is the census —
measured, not inferred.** The control and the `--composite` file read within
1% of each other, which is this instrument's own drift; the `--arrays
--composite` file reads **21% above both**.
Its rows are the only ones carrying a `{`, so they are the only ones the
mapping pass's array-shape census splits into fields. Running the same query
with the **census-off** binary settles it:

<!-- figure: census-attribution — reproduce with `cd scripts && uv run measure.py --figure census-attribution` -->

| | control | `--arrays --composite` | gap |
|---|---|---|---|
| census on | 4.426 s | 5.300 s | **+0.874 s** |
| census off | 4.396 s | 4.269 s | **−0.127 s** |

Apparatus over every run in this table: CPU stall ≤0.18%, I/O stall ≤1.53%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.79 GHz, ≤62°C.

**With the census gone the gap does not merely close, it reverses**: +0.874 s
becomes **−0.127 s** on a file carrying 14% *fewer* rows than the control, so
the census accounts for the whole of it and what is left is a residue with the
sign a residue is free to have. Measured this way the census reproduces the
`parse` figures two sections above closely on both files: +1.031 s on the
arrays file against +1.045, and +0.030 s on the control against the +0.030 the
brace-free table read under the `ba2fc12` stamp this figure was taken in (it
reads +0.048 in its own later sitting) — a cross-check on a different command,
off a different pair of legs. The census-off
gap has read +0.036 s and −0.058 s under earlier stamps; all three are inside
what the drift figure below says this instrument can resolve, and all three say
the same thing.

So a `strings` leg is a scan plus a census whose price depends on the data's
shape, not a flat per-byte floor. Both rows come from the same query loop
below, run with each binary in turn:

```sh
cd scripts && uv run measure.py --figure census-attribution
```

Earlier sweeps put all three baselines within 2% and
read that as evidence the untyped path was byte-driven; at 9.6 s legs a 1 s
difference was inside the spread, and it is not at 4.4 s.

The micro above covers 6.2 µs of that 13.5 µs (decode plus render for a
4-element array, a 50-element array and a two-field composite). The remaining
~7.3 µs is the Arrow build the micro does not reach: 56 per-element
`append_value` calls into the child builders, plus the list offsets. **The
literal parse is the smaller half of nested decoding**, which is the fact
P7 needs before deciding what to do about nested values always copying.

### The cross-file subtraction bottoms out at about half a microsecond a row

**This figure is the instrument's own calibration, and that is all it is now.**
It once carried the composite column's cost; "What a column costs" below reads
that within one file, so what is left here is the number the standing rules are
read off — what a *cross-file* per-row difference can resolve at all. Two
readings from the same sweep, the quantity and the control on the instrument
that measures it:

<!-- figure: cross-file-floor — reproduce with `cd scripts && uv run measure.py --figure cross-file-floor` -->

| Reading | Reps | Paired median | Per-rep readings |
|---|---|---|---|
| composite column's share — control against `--composite` | 5 | **+0.79 µs** | +0.73, +0.78, +0.79, +0.88, +1.06 |
| **the instrument's own floor** — control against a second control (`--seed 43`, same 16 columns) | 6 | **+0.01 µs** | −0.35, −0.04, −0.02, +0.03, +0.04, +0.06 |

Apparatus over every run in this table: CPU stall ≤0.15%, I/O stall ≤1.75%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.69 GHz, ≤63°C.

The second row is the control on the *instrument*: two files that differ only
in their random seed should differ by zero, and instead they span −0.35 to
+0.06 µs per row. **In this sweep the two rows do not overlap** — every
composite rep sits above every floor rep, by 0.67 µs — and that is still not a
resolution, because sweeps of the same binaries and inputs have read the same
composite share at +0.31, +0.39, +0.60, +0.62, +0.69, +0.72, +0.79 and +0.99 µs:
takes of one quantity **0.7 µs/row apart**, which is wider than the quantity itself.
It is also why the standing rule against quoting a
standard error forbids quoting an interval here — an earlier draft put these at
+0.61 ± 0.14 against +0.20 ± 0.18 and made the separation look like a result.

**The within-file reading is what settles the quantity, and it lands inside
this bound.** Projecting the composite column in and out of one file puts it at
**+0.98 µs/row** ("What a column costs" below), against eight cross-file takes
spanning +0.31 to +0.99 — at that range's upper edge, and inside it. So the cross-file apparatus was not *wrong* about the
composite column — it was imprecise by exactly the amount its own floor row
says, which is the strongest statement available that the floor row measures
the instrument rather than the data.

*Retracted:* the reading that this subtraction is *biased* rather than merely
imprecise. A musl-built leg of the same sweep put the composite file
**1.16 µs/row below** the control on the same five reps — a negative cost,
which adding a decoded column cannot produce — and that was read as a
structural confound in differencing two files of different row length. The
glibc leg reverses the sign on the same inputs and the same reps, so what was
being measured was the allocator, not the instrument. The floor stands at
roughly ±0.5 µs/row; the confound does not. This pair is also the
demonstration behind the standing rule against quoting a standard error.

*Superseded, and now by a reading rather than a plan:* `composite-isolated`,
an instrument built to resolve the composite column by declaring one column two
ways over byte-identical rows. It was held out of every sweep because column
projection was going to do the same isolation with no second file at all, and
that is what "What a column costs" now does — the same rows, the same bytes,
five widths of one file. It was deleted from the register unpublished, and its
apparatus went with it: `generate_perf_data.py --weak-composite`, the
`composite_text` input and the `perf_generator_fidelity.rs` case holding the
pair byte-identical. Keeping the generator support after deleting the register
entry is the one outcome that is wrong either way — `measure.UNTAKEN` exists so
that a built-and-unrun instrument is *named* rather than latent, so an
instrument with generator support and no entry is exactly the thing that list
was written against.

**`typed` and `strings` agree byte for byte on all three inputs**, as they do
on what `pg_dump` writes (`architecture.md`, "CLI surface") — so `cmp` on the
two outputs is a valid smoke test here, and
`pgdump_query-cli/tests/perf_generator_fidelity.rs` asserts it on small
generated files: the control, `--composite` alone and `--arrays --composite`,
which are the three flag settings a published figure is taken on, so the
generator cannot drift back out of that agreement.

```sh
cargo build --release -p pgdump_query-cli          # default target: glibc
D=/dev/shm/pgdq                                     # generate from the HOST
(cd scripts &&
 uv run generate_perf_data.py --size-mb 3072 --seed 42 $D/control.sql &&
 uv run generate_perf_data.py --composite --size-mb 3072 --seed 42 \
   $D/composite.sql &&
 uv run generate_perf_data.py --arrays --composite --size-mb 3072 --seed 42 \
   $D/arrays.sql)
for i in 1 2 3 4 5; do for f in control composite arrays; do for m in strings typed; do
  echo "### $f $m rep$i"; sudo nerdctl run --rm \
    -m 512m --memory-swap 512m \
    -v "$PWD/target/release/pgdq:/pgdq:ro" \
    -v "$D/$f.sql:/dump.sql:ro" \
    postgres:16 bash -c \
    "time /pgdq query --source /dump.sql --table public.perf --dqcache none \
       --schema-mode $m >/dev/null"
done; done; done
```

Three 3.00 GiB inputs is 9 GiB of `/dev/shm`; the floor reading needs a fourth
(`--seed 43`), so drop `composite` and `arrays` before generating it. It is
otherwise the same loop, six reps, over `control` and `control43`.

## What a column costs: five projection widths over one file

<!-- figure: projection-widths — reproduce with `cd scripts && uv run measure.py --figure projection-widths` -->

One file, read five ways. Every column list is a *subsequence* of the file's
own column order and a superset of the row above it, so the difference between
two adjacent rows is the cost of exactly the columns they differ by — over
identical rows of an identical file, in one interleaved sweep of six reps.
Neither the cross-file subtraction floor above nor the census's file-dependent
untyped baseline enters, which is what makes this the *replacement* for the
cross-file apparatus rather than one more figure beside it.

| Projection | Median | Per row | Δ per row against the row above | What that buys |
|---|---|---|---|---|
| 0 — `--no-columns` | 2.22 s | 3.18 µs | — | the replay floor: no decode, no build, no render |
| 1 — `v_smallint` | 2.31 s | 3.30 µs | **+0.13 µs** | one cheap scalar, above the floor |
| 16 — every scalar | 10.05 s | 14.36 µs | **+11.03 µs** | the other 15 scalars |
| 17 — the scalars and `v_comp` | 10.76 s | 15.38 µs | **+0.98 µs** | **the composite column alone** |
| 19 — every column | 19.99 s | 28.56 µs | **+13.21 µs** | **the two array columns alone** |

One file — `--arrays --composite`, 699,962 rows of 19 columns — read 5 ways,
warm and typed, through the CLI. Per-row differences are paired rep by rep and
then taken as a median.

Per-rep readings (s):
- 0 — `--no-columns`: 2.28, 2.24, 2.24, 2.19, 2.21, 2.19
- 1 — `v_smallint`: 2.35, 2.31, 2.31, 2.31, 2.35, 2.29
- 16 — every scalar: 10.05, 10.05, 10.10, 9.99, 10.03, 10.24
- 17 — the scalars and `v_comp`: 10.82, 10.68, 10.82, 10.80, 10.69, 10.73
- 19 — every column: 19.94, 19.98, 19.86, 20.03, 20.08, 19.99

Apparatus over every run in this table: CPU stall ≤0.17%, I/O stall ≤3.78%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.59 GHz, ≤67°C.

**The zero-column row is why the table is worth publishing rather than
arithmetic.** A zero-column projection is `COUNT(*)`: the block is still read,
every row still walked and field-counted, the predicate still evaluated, and
nothing is decoded, built or rendered. That is **3.18 µs of every row** — 2.22 s
against this file's own warm `dd` floor of 0.299 s, so 7.4× the cost of handing
the bytes over — and it is the floor every row above is read against.
The same file in `--schema-mode strings`, which builds all 19 columns as
zero-copy views, costs 5.36 s; typed and complete it costs 19.99 s.

**A scalar column is cheap and an array column is not, by an order of magnitude
and more.** One `smallint` costs 0.13 µs a row. The other fifteen scalars cost
11.03 µs between them, ~0.74 µs each. The composite costs **0.98 µs**. The two
array columns — a 3–5 element `integer[]` and a 50-element one — cost
**13.21 µs between them**, which is 93% of what all three nested columns cost
together and more than every scalar column in the table.

**It agrees with the two instruments it replaces, and it is sharper than
either.** The three nested columns sum to 14.19 µs here against the 13.5 µs
`typed` − `strings` reads across a file boundary; the composite column's
0.98 µs sits inside the +0.31 to +0.99 range eight cross-file sweeps have read
for it, where the cross-file floor alone is ±0.5 µs. The micro figure above
accounts for 5.84 µs of the arrays' 13.21 (decode plus render for both
literals), leaving the Arrow build — 56 per-element `append_value` calls and
the list offsets — as the larger half, which is the same split the end-to-end
figure reports.

The projections are spelled on the CLI, which is what makes this an end-to-end
figure — `render_field` included — rather than a library-internal one:
`--no-columns` for the zero-column row and a repeated `--column <name>` for the
rest ([`architecture.md`](architecture.md), "Projection"). **So a per-column
delta here does not size a library change**; see "A mode difference and a
per-column delta are CLI numbers" above, which is the rule this paragraph is
the reason for.

```sh
cd scripts && uv run measure.py --figure projection-widths
```

The input alone, for a reader who wants it without the harness, is the same
`--arrays --composite` file the census and nested figures are taken on:

```sh
cd scripts && uv run generate_perf_data.py --arrays --composite \
  --size-mb 3072 --seed 42 /dev/shm/pgdq/arrays.sql
```

*Rejected: folding these rows into "A typed query over nested columns".* A
figure is exactly one whole table, and adding rows to a published one would
silently restate a number under a heading that does not claim it.

## What a filter term costs, and how much of it is the walk to its field

<!-- figure: predicate-terms — reproduce with `cd scripts && uv run measure.py --figure predicate-terms` -->

One file, read six ways, and **the only table in this document that passes a
filter at all.** The terms are OR'd and every one of them is false, so `Or`
evaluates all of them on every row and nothing survives — which is what makes
the difference between two rows here the predicate and nothing downstream of
it, the decode, the Arrow build and the render being identically absent from
all six. Two axes: how many terms, and how far into the row each one reaches.

| Predicate | Median | Per row | Δ per row against the row above | What that buys |
|---|---|---|---|---|
| 1 term, 13th column | 0.984 s | 1.21 µs | — | the base: one term, thirteen fields in |
| 2 terms, 13th column | 0.996 s | 1.22 µs | **+0.09 µs** | one more term at that depth |
| 3 terms, 13th column | 1.095 s | 1.34 µs | **+0.12 µs** | one more |
| 5 terms, 13th column | 1.236 s | 1.52 µs | **+0.16 µs** | two more — the five-way disjunction |
| 5 terms, 1st column | 1.014 s | 1.24 µs | **-0.27 µs** | **the walk those five terms pay** |
| 1 term, 1st column | 0.871 s | 1.07 µs | **-0.13 µs** | four of those five terms, walk-free |

One file — the brace-free control, 814,362 rows of 16 columns — read 6 ways, warm and `--schema-mode strings`, through the CLI. Every term is an equality against a literal no value of the column can equal, so every row is walked, every term is evaluated, and no row is decoded, built or rendered. Per-row differences are paired rep by rep and then taken as a median.

As written:
- 1 term, 13th column: `--where 'v_bool=zzz1'`
- 2 terms, 13th column: `--where 'v_bool=zzz1 OR v_bool=zzz2'`
- 3 terms, 13th column: `--where 'v_bool=zzz1 OR v_bool=zzz2 OR v_bool=zzz3'`
- 5 terms, 13th column: `--where 'v_bool=zzz1 OR v_bool=zzz2 OR v_bool=zzz3 OR v_bool=zzz4 OR v_bool=zzz5'`
- 5 terms, 1st column: `--where 'id=zzz1 OR id=zzz2 OR id=zzz3 OR id=zzz4 OR id=zzz5'`
- 1 term, 1st column: `--where 'id=zzz1'`

Per-rep readings (s):
- 1 term, 13th column: 0.986, 0.870, 0.887, 0.981, 1.152, 1.067
- 2 terms, 13th column: 1.136, 0.954, 0.943, 0.948, 1.039, 1.272
- 3 terms, 13th column: 1.085, 1.104, 1.071, 1.019, 1.239, 1.270
- 5 terms, 13th column: 1.207, 1.265, 1.184, 1.164, 1.492, 1.368
- 5 terms, 1st column: 1.006, 0.878, 1.021, 0.920, 1.135, 1.293
- 1 term, 1st column: 0.869, 0.837, 0.873, 0.840, 1.161, 1.012

Apparatus over every run in this table: CPU stall ≤0.34%, I/O stall ≤39.87%, machine ≤15% busy, steal ≤0.00%, busiest core ≥4.22 GHz, ≤71°C.

**A term's cost is mostly the walk to its field, and the depth is what says
so.** A term against the control's thirteenth column crosses thirteen field
boundaries and one against its first crosses one. The two five-term rows are the
same five terms at those two depths, and they differ by **0.27 µs a row**: 18%
of what the deep one costs, on a query that decodes nothing. The two one-term
rows put the walk-free term at **0.033 µs**, against 0.09–0.12 for a deep one.

**This table was taken before the terms shared one split, and it is what sized
that change.** Each term walked from the front of the row on its own here, so
five deep terms crossed sixty-five boundaries; a row's boundaries are now found
once and read by every term and by `push_row` alike
([`architecture.md`](architecture.md), "Predicates"). The figure is therefore
stale in the register's sense and in fact, and 7.12's sweep re-takes it; what
the sharing measured, on the deterministic instrument and over five more shapes
than this table has, is
[`roadmap-P7.7.1-shared-field-split-notes.md`](roadmap-P7.7.1-shared-field-split-notes.md).

**Every row of this table is a rejected row, which is deliberate: it is where
the lever it sized looks worst.** `RowBatcher::push_row` walks the whole row
whatever the projection is, so on a row that survives the filter the whole-row
walk already happens and sharing it costs less. Selectivity is therefore the
axis the sharing turns on, and a figure on which nothing survives sits at its
pessimal end — which is what made the losses read off this table **upper
bounds**, and the shape gives up an absolute a reader recognises to get a
conservative one.

*Rejected: a filter every row satisfies.* It would have given that absolute,
and the **depth** axis is what defeats it rather than the term count. An
all-true conjunction does not short-circuit, and every shape here puts its N
terms on one column, so the 2% of rows dropped as `Unknown` is flat across the
term axis. The depth axis needs two columns of equal NULL-ness at different
offsets and this file has none: `id` is its only NOT NULL column and it sits at
depth 1, so the depth pair would differ by 2% of the rows *emitted* — and where
a filter keeps everything the emit dominates, so that 2% would swamp a walk
difference worth 18% of a zero-emit query. It is an obstruction in the input,
not in the predicate language; a NULL-free deep column would answer it.

**This sitting was taken with another session resident on the machine**, which
the gate held to its 15%-busy limit rather than excluded. The last two reps
drift upward across every row — the raw readings above are what says so — and
the paired difference behind the 0.27 µs ranges 0.08 to 0.39 s across the six.
So read the **ordering and the magnitude** off this table, not the third
decimal; the deterministic corroboration is `runs/measure-7.7.tsv`, retired
user instructions on the host, which is immune to what else the machine was
doing and puts the same walk at 49% of a five-term query's instructions.

*Why this is published rather than re-taken.* This document's standing rule is
that a figure taken while local work ran is not a figure, and the rule exists
because contention corrupts a number silently — nobody reading the table
afterwards can tell. Three things make that inapplicable here rather than
waived: the per-rep readings are published above, so the drift is visible in the
evidence itself; the caveat states the precision the table supports; and the
claim anything else cites — that the walk is 49% of a five-term query — rests on
the deterministic instrument, not on this wall clock. What the rule forbids is
an unauditable number, and this one is auditable. **The wall table is therefore
not to be quoted at its stated precision elsewhere** until 7.12's sweep re-takes
it with every other figure.

```sh
cd scripts && uv run measure.py --figure predicate-terms
```

The input alone, for a reader who wants it without the harness, is the same
brace-free control the census and throughput figures are taken on:

```sh
cd scripts && uv run generate_perf_data.py --seed 42 \
  --size-mb 3072 /dev/shm/pgdq/control.sql
```

*Rejected: a conjunction of terms every row satisfies, so that every row
survives.* It would give an absolute a user recognises — a filter that keeps
everything over a projection that builds everything — and it cannot be built
here: every column of this file but `id` carries 2% NULLs, so an N-term
conjunction is `Unknown` on 1 − 0.98^N of the rows and drops them, which moves
the emit cost the whole subtraction depends on holding constant. Confining the
terms to `id` fixes that and leaves one depth, which is the axis the table
exists for. The all-false disjunction has neither problem: `Or` evaluates every
child and keeps nothing, at either depth.

*Rejected: `--schema-mode typed`.* The typed `=` decodes the literal against the
column's own type once per block, so `zzz1` on an `integer` column is
`Error::PredicateValueDecode` before the first row. Picking literals that decode
would put a per-type comparison cost inside every delta, which is 7.10's row and
not this one's.

## What a session's own drift costs, measured rather than asserted

<!-- figure: session-drift — reproduce with `cd scripts && uv run measure.py --drift <sweep> <sweep>` -->

The instrument measured against itself. The sweep this doc is stamped with and
a second one on the same commit ran **about three minutes apart** — back to
back, with a 120 s settle between them — on identical inputs and identical
binaries, and all 53 readings are common to both. **Both carry the per-reading
telemetry every table above reports.** The point is not any row but the shape:

| | Drift between two sweeps, three minutes apart |
|---|---|
| median absolute, over 53 readings | **0.9%** |
| largest | **15.9%** |
| cold **device-bound** readings (5–6 s) | **≤0.4%** |
| the one cold reading with real CPU in it (`INSERT`) | **0.8%** |
| warm readings | **−15.9% to +7.0%**, median absolute 1.4% |

**Drift is not a single number, and the gap between the sweeps is one of its
terms.** A pair two hours apart read a median absolute 5.6% and a largest 8.5%;
this pair, three minutes apart, reads 0.9% and 15.9%. The register carries no
gap parameter, so all of these are the same figure taken at two intervals, and
**the two-hour median is the one to plan against** — it is what "another
session" and "a comparison re-taken next week" actually cost. What the short
interval buys is a cleaner view of the rest: at three minutes almost everything
reproduces, so the readings that still move name themselves. The *largest* move
does not follow the interval at all: this short pair's 15.9% is above anything
the two-hour pair produced, and it is a floor reading rather than a figure —
which is the case for co-measuring the floor with every warm table.

Three of them move, and each says something different:

- **Nothing device-bound does.** Every cold 5–6 s reading is inside 0.4%, and
  the cold `INSERT` scan — the one with real CPU riding on the device — is
  0.8%, where at two hours it drifted 8.1%. The device is the clock, and over
  three minutes even the CPU riding on it holds still.
- **The `control` file's warm `dd` floor moved −15.9%, and its warm readings
  moved with it** (−13.1% on the census-off leg, −8.3% on the `COPY` scan),
  while the `arrays` file's floor moved +5.0% and its readings +6.8% and +0.8%.
  The same effect at 13.6% was the largest mover in the previous pair and at
  5.5% in the one before, and it is per-file on tmpfs rather than machine-wide:
  the clearest statement available that a warm figure is a ratio against its own
  co-measured floor rather than an absolute. The two takes of that floor inside
  one sweep pair differ from each other too — −15.9% in the census table,
  −14.5% in the throughput table — which is the same reading, measured twice,
  disagreeing by more than most figures move.
- **The smallest readings in the doc moved most, in relative terms and least in
  absolute.** The quadratic table's one-block control moves −10.0% on the
  throttled build and +7.0% on the unthrottled one — half a millisecond and two
  milliseconds. A reading of a few tens of milliseconds through a container is
  near the resolution of the whole apparatus; the row exists to hold the byte
  count fixed against the block count, and it is read as "milliseconds", never
  to three figures.

What is left after those is the part a counter cannot see — memory layout,
cache and TLB luck — which is why the remedy is to re-take a comparison whole
rather than to correct a reading. `map-only`, the third mover two pairs ago at
3.7–7.3% under a witnessed CPU episode, reproduces here inside 1.4% with every
table in both sweeps at ≤6% machine-busy.

So a *difference* between two warm legs of the same sweep is worth more than
either leg's absolute value across sweeps, which is what the standing rule
"re-take a comparison table whole" already required and this is the measurement
behind it. It is also the calibration for the standing rule against quoting a
standard error: a cross-file per-row difference under ~0.5 µs/row is apparatus,
and seven sweeps agree on that floor (−0.17, −0.11, −0.05, +0.01, +0.07, +0.07
and +0.08 µs/row) far better than any of them agrees on the leg it came from.

**The resolution floor in this doc's standing rules is read off this table**,
so a re-derivation here re-reads that rule — the marker mechanism addresses this
section, not a rule at the top of the same file.

Both sweeps are `runs/measure-*` directories; the table is computed from their
`raw.json`, never transcribed.

## koji full scan — the regression check

The 784GB real sample (`CLAUDE.local.md` has the path). Roughly an hour on the
HDD; run it detached per `CLAUDE.md`.

| | |
|---|---|
| `COPY` blocks | 74 |
| Rows | 19,575,829,920 |
| Bytes accounted for | 784,019,857,152 |
| RSS | flat, ~9 MiB (untyped scan) |
| `UnterminatedCopyBlock` | none |

**The `.dqcache` a run leaves behind dies at the next cache-format bump**, and
those are free and frequent pre-1.0 (`architecture.md`, "The cache"). Treat the
koji cache as a byproduct of a scan run for another reason, never as an asset:
the cache left by the 2026-08-25 run was unreadable within days. Nothing plans around keeping one alive — inspecting koji at all
(`pgdq info`, with or without `--source`) is available only between a scan and
the next bump, and regaining it costs the full ~54-minute scan.

**The regression check is byte-for-byte identity, not throughput**: every
block's header/data/terminator/end offset must match the previous run. That
identity over a change touching only what happens *between* blocks is what the
check is for.

`lock_monitor.activity` — whose row data contains a literal `COPY … TO
stdout;` substring — must parse as one correct block. That is the case that
motivated line-anchored detection.

**These are musl-build figures**, taken before the glibc rule above and under
a static-binary container recipe that no longer exists. The scan is
device-bound at ~33% of one core, so the allocator is unlikely to move them —
but the next koji run takes them on the glibc build, and until one does they
are not comparable to the figures above. koji is deliberately **outside** the
sweep: a different medium, ~54 minutes, and a regression check rather than a
throughput figure. The harness owns the *invocation* — `uv run measure.py
--koji-recipe` — so the next run conforms without re-deriving the recipe.

**Throughput, re-measured clean.** A 2026-08-25 re-run on an uncontended disk
(container `pgdq-koji`, `runs/koji-throughput-scan.log`) reproduced the same
74 blocks/19,575,829,920 rows/784,019,857,152 bytes, and measured 54m9.97s
wall-clock — **~241 MB/s**, against the original 2026-08-22 baseline's ~243
MB/s. The two independent runs agree to within ~1%, which is inside the noise
this kind of wall-clock measurement carries; no `cat`-to-`/dev/null` floor was
taken for this file specifically (impractical at 784GB on the HDD for a
one-point confirmation), but the close agreement between two runs that were
each I/O-bound at ~33% of one core is itself the evidence that the 2026-08-24
run's ~110 MB/s figure was the outlier, caused by the concurrent restore
documented below, not a real regression. Command and container recipe are in
`CLAUDE.md`.

**Per-block cache persistence costs ~1.5%, and the throttle leaves it
untouched.** `pgdq parse` serializes the whole cache at a `CopyEnd` watermark,
so a koji scan writes it 74 times where the build before it wrote it once. The
self-tuning throttle below never fires on this shape — koji's blocks are ~45 s
apart and its saves cost well under a second, so the "20x the last save's own
cost" bar is cleared every time — which is the regime it was designed not to
change. Container
`pgdq-koji-9.1`, launched 2026-08-26T04:21:17Z, `runs/koji-9.1-scan.log`,
`exit=0`:

| | |
|---|---|
| Wall | 3300 s |
| Rate | ~238 MB/s |
| Against the no-per-block-save run above (3250 s) | +50 s, +1.5% |
| Final cache | 247,380 bytes |
| Bytes the saves wrote | under 74 x 247,380 = 18.3 MB |
| Amplification against 784 GB read | ~2.3e-5 |

The +1.5% sits inside the run-to-run spread the two baseline scans above
already show (~241 against ~243 MB/s), and the byte bound is an over-estimate
twice over: every save is charged the *final* cache size, which only the last
block's save actually pays. **This figure is why the throttle is not an
interval**: at koji scale there is nothing to save, and any constant chosen to
help the block-rich shape below would have had to be checked against this one.

This run agreed with the one above on the block list, the per-block row counts
and the byte total. It did **not** re-run the byte-for-byte offset identity
check, which needs `--verbose`; that check runs on its own.

```sh
cd scripts && uv run measure.py --koji-recipe
```

**That recipe supersedes the one this run used**, which wrapped `parse` in a
compound `sh -c` to echo its own exit status and elapsed seconds. A compound
command cannot be `exec`'d, so `sh` stays as PID 1 and swallows the stop
signal — the run above could not have been interrupted cleanly. The harness
emits the `exec` form and takes both numbers from `nerdctl inspect`, which
reports them whether a run finished or was signalled.

The comparison's other half is the throughput row above: the same recipe on a
build predating the per-block save, so reproducing the *delta* means checking
out one of each.

**Stop-and-resume costs nothing measurable, and reproduces the cache
byte-for-byte.** The P9 wrap run (`runs/koji-wrap.sh`, 2026-08-27) parsed
koji cold, signalled it 1200 s in, reported the partial cache, then resumed the
same command to completion:

| | |
|---|---|
| Interrupted at | byte 19,867,623,920 of 784,019,857,152 (2%) |
| Interrupted cache | 109,916 bytes, loadable, `Scan completion: 2%` |
| Resumed leg | 764,152,233,232 bytes in 3302 s, **~231 MB/s** |
| Final cache | 247,380 bytes, **byte-identical** to the straight-through run's |
| Blocks / rows / bytes | 74 / 19,575,829,920 / 784,019,857,152 — the same figures |

The resumed leg's ~231 MB/s against the straight-through run's ~238 MB/s is
**not** a like-for-like scan comparison: it omits the file's opening 19.9 GB,
which is `public.archive_rpm_components` and row-dense rather than byte-dense.
What it does establish is that resuming carries no detectable cost — the same
device, the same order of magnitude, with a stop and a cache reload in between.

The interrupted leg's own throughput (~16.5 MB/s) measures nothing about pgdq:
a `cargo build`/`test`/`clippy` cycle ran on the same HDD throughout. The
resumed leg, uncontended over the same file, is what says so.

**The byte-identical cache is the run's real product**, and it is a property,
not a figure: a scan stopped inside a hundred-gigabyte block and resumed
produces the same structural record — span for span — as an uninterrupted one.
`architecture.md`, "CLI surface", states it. The fixture-scale version is
`pgdump_query/tests/map_file.rs`'s
`a_cancelled_map_file_reports_it_and_banks_what_it_scanned`, which asserts the
resumed *index* equals an eager scan's; koji is where the same property is
checked on the serialized cache, at a scale no fixture reaches.

The run was driven by `runs/koji-wrap.sh`, which is **gitignored** — it
hardcodes one machine's dump path and nothing in the repo consumes its output
(`CLAUDE.md`, "Long-running processes"). The sequence itself is not lost with
it: the harness prints it, machine paths filled in, and a test holds the two
legs to the identical command that resuming depends on:

```sh
cd scripts && uv run measure.py --koji-recipe --wrap
```

**The stop exercises the `SIGINT` arm, not `SIGTERM`**, and no flag on
`nerdctl run` changes that: the `postgres` images set `STOPSIGNAL SIGINT` and
`nerdctl stop` sends what the image label says, so `--stop-signal SIGTERM` is
accepted and ignored (`CLAUDE.md`). Exit **130**, not 143. Both arms reach the
same guard; the `SIGTERM` one is reached directly with `nerdctl kill -s
SIGTERM`, and at fixture scale by the CLI's own tests.

**Neither the exit codes nor the identity check are asserted by the script** —
it logs the expected value beside the observed one and a reader compares. That
is deliberate for a run whose whole point is to be read by a later session, but
it means "the log says done" is not the same as "the checks passed".

## The preamble prepass is bounded by the schema, not by the dump

<!-- figure: preamble-prepass — reproduce with `cd scripts && uv run measure.py --figure preamble-prepass` -->

`pgdq parse` and every cold query open with `index::scan_preamble`, which reads
from byte 0 to the first `COPY` header. It is the one region that ignores
`ScanOptions::cancel` (see `architecture.md`, "`parse` resumes, and saves as it
goes"), so "bounded by its own length" is the claim that has to hold.

**koji: 63,333 bytes of 784,019,857,152** — 0.00000008 of the file. The whole
uncancellable region is one read.

```sh
LC_ALL=C grep -m1 -b -a -E '^COPY .* FROM stdin;' /path/to/koji.dump | cut -c1-80
```

**The most preamble-heavy shape available.** A 4000-table dump is 49% preamble
by bytes — the first `COPY` header sits at 980,996 of 1,998,741 — so this is
the worst case the generators can build:

| | Wall |
|---|---|
| `parse --preamble-only`, 4000-table dump | **0.045 s** |
| full `parse` of the same file | 0.113 s |

Apparatus over every run in this table: CPU stall ≤0.26%, I/O stall ≤11.18%, machine ≤3% busy, steal ≤0.00%, busiest core ≥3.65 GHz, ≤63°C. **Taken in the same sitting as the quadratic table below**, whose 4000-block "after" column the second row is.

The second row is not a second measurement: it is the quadratic table's
4000-block "after" column. **The pair is no longer a ratio worth quoting** —
that scan went from 20.75 s to 0.113 s when the map's per-block rebuild moved
behind the save throttle's gate, so 461× became 2.5× without the prepass
changing at all. The ratio was never the claim. What has to hold is that the
uncancellable region is *milliseconds*: 45 ms here, on the most preamble-heavy
shape the generators can build, half of whose bytes are preamble — and 63,333
bytes of one read on koji, against a scan of an hour.

```sh
cd scripts && uv run measure.py --figure preamble-prepass
```

Both rows are now taken inside the container like every other figure; the 40 ms
this section used to quote was `/usr/bin/time` around a host `pgdq`, obeying
neither the timer rule nor the cgroup one.

So the region grows with the *schema* — table count and DDL size — and not with
the data, which is what makes an immediate Ctrl-C during it a non-issue on a
local file. The remote case is not covered by these numbers: 63 KB is still one
ranged GET that can hang, and that is a P6 decision
(`roadmap-P6-embeddable-engine-inbox.md`).

## Per-block cache saving is quadratic in block count, and so is the map

<!-- figure: per-block-quadratic — reproduce with `cd scripts && uv run measure.py --figure per-block-quadratic` -->

The regime koji cannot show: **block-rich and byte-poor** — a schema with
thousands of tables, or one partitioned table with a daily leaf over a decade.
Four columns, three rows per table, the schema section then the data section,
which is how `pg_dump` orders a plain dump:

```sh
cd scripts
for n in 500 1000 2000 4000; do
  uv run generate_block_count_bench.py --blocks $n --out /dev/shm/pgdq/r$n.sql
done
```

`parse` on each, cache removed first, inputs on tmpfs, both columns in one
interleaved sweep — two passes, the pair run in both orders, medians below.
"Before" is `b726f6b`, the commit preceding `SaveThrottle`; "after" is the
working tree. The cache is written to the tmpfs directory too, mounted into
the container, so no run writes to the container's own layer.

**"Before" is a whole-commit comparison, not a throttle-isolating one.** The
two builds differ in everything that landed after `b726f6b` — the save
throttle, and then the gate that put the map's own rebuild behind it — so the
column says what those two eras bought together and must not be differenced
against a later change. What isolates a mechanism is the census-off method
above — one line, one rebuild — and what the parallel-scan phase consumes is
the map's own quadratic below, which needs no historical build at all.

| blocks | dump | final cache | before | after | saves before → after |
|---|---|---|---|---|---|
| 1 (control) | 2.0 MB | 1 KB | 0.005 s | 0.005 s | 4 → 5 |
| 500 | 242 KB | 319 KB | 0.623 s | 0.015 s | 503 → 5 |
| 1000 | 484 KB | 640 KB | 2.49 s | 0.029 s | 1003 → 5 |
| 2000 | 973 KB | 1.3 MB | 10.89 s | 0.056 s | 2003 → 5 |
| 4000 | 1.9 MB | 2.5 MB | 44.70 s | 0.113 s | 4003 → 5 |

Per-rep readings (s):
- 1 (control) — before: 0.004, 0.005; after: 0.005, 0.004
- 500 — before: 0.631, 0.615; after: 0.015, 0.015
- 1000 — before: 2.49, 2.49; after: 0.031, 0.028
- 2000 — before: 10.98, 10.80; after: 0.056, 0.056
- 4000 — before: 44.84, 44.56; after: 0.111, 0.114

Apparatus over every run in this table: CPU stall ≤0.31%, I/O stall ≤7.04%, machine ≤7% busy, steal ≤0.00%, busiest core ≥4.06 GHz, ≤66°C. **Taken in its own sitting**, with the `map-only` and `preamble-prepass` tables that share its readings, not in the `ba2fc12` sweep the stamp above records.

Every run is 99% CPU at every point: the cost is *serializing* the index, not
writing it. **The control is the table's first row** — the same byte count in
**one** `COPY` block — and it is what makes the rest readable as a series
rather than a curve: it holds the bytes fixed so the only variable left is the
block count, which is how the 4000-block figure can be called three orders of
magnitude above the scan it protects rather than merely large.

**The harness takes that row rather than a reader running it by hand**, because
a control nothing runs is one that goes stale without anyone noticing.
`scripts/test_measure.py` asserts that every declared input is consumed by some
figure, which is the check that catches one sitting unread.

**The "after" column no longer quadruples per doubling — it doubles**, which
is the shape a scan of a file twice as long should have. Two mechanisms
produced that between them and they are not separable here, because the second
changed the input to the first: the save throttle skips a save unless 20× the
last save's own duration has elapsed, and the gate it opens is now also what
decides when `stream::splice` rebuilds the map
([`architecture.md`](architecture.md), "`parse` resumes, and saves as it
goes"). Removing the per-block rebuild shortened the scan, and a shorter scan
earns fewer saves under a rule that is a ratio against elapsed time — so the
save count falls to **5** at every size rather than tracking the block count at
`1/K` of it. What is left of the original quadratic is the rebuild *itself*,
which is O(blocks) each time it runs and still runs per block wherever the gate
does not close — and the gate cannot close on a cache that costs nothing, so
with the cache **disabled entirely** the map is O(blocks²) exactly as it was:

```sh
# maps to EOF (the table never matches) and never saves
/pgdq query --source /dump.sql --table public.nosuchtable --dqcache none
```

<!-- figure: map-only — reproduce with `cd scripts && uv run measure.py --figure map-only` -->

| blocks | 1000 | 2000 | 4000 |
|---|---|---|---|
| map only, no saving | 1.004 s | 3.98 s | 19.07 s |

Per-rep readings (s):
- 1000 blocks: 1.004, 1.014, 0.989
- 2000 blocks: 4.19, 3.98, 3.92
- 4000 blocks: 18.14, 19.07, 19.08

Apparatus over every run in this table: CPU stall ≤0.19%, I/O stall ≤5.19%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.60 GHz, ≤68°C. **Taken in the same sitting as the quadratic table above.**

**This table is what says the gate above did nothing here**, which is why it
was re-taken alongside it rather than assumed: 1.004 / 3.98 / 19.07 s against
the previous stamp's 1.012 / 4.56 / 19.03 s. `--dqcache none` makes
`cache::CacheMode::save` a no-op, so the throttle has no cost to amortize, its
gate never closes, and the map is rebuilt at every `CopyEnd` exactly as before.
The 2000-block row is the one that moves between sittings — +9% at the previous
stamp, −13% here, against ~0.2% for the other two — and the sizes, which are
what this figure is for, have never been in question.

So the two tables bracket the same mechanism from either side. **With a cache,
the map's rebuild is gone**: 0.113 s at 4000 blocks against the 20.75 s the
same command cost at the previous stamp, when it spliced per block — a
cross-sitting difference, and the only kind this document permits, since 184×
is two orders of magnitude past the 8.5% a session's own drift reaches.
**Without a cache it is the whole cost**: 19.1 s for the same file. Against the
*unthrottled* `b726f6b` build the split is ~26 s of saving against ~19 s of
mapping, which is the pair the "before" column and this table make. What is left is `KD5`
([`../status/STATUS.md`](../status/STATUS.md), "Known deficiencies") — the
rebuild is still a whole-list clone, so it is only ever as cheap as the gate is
closed.

Save counts come from `strace -f -e trace=open,openat` filtered to the cache
path (`std::fs::write` opens once per save; the first is the load's miss).
**Trace both calls**, not `openat` alone: glibc uses `openat` and musl uses
`open`, so tracing one of them silently reports zero saves against the other
libc. The counting stage runs on the host and is untimed, so `strace`'s
overhead reaches no figure. Reproducing the "before" column means building the
commit that precedes the throttle — `git worktree add <dir> <commit>` and a
release build there, the same two-binary method the census figure above uses.

## Decoder and whole-file benchmarks

`criterion`, `harness = false`. A regression tripwire for per-byte CPU cost,
not an optimization campaign.

```sh
cargo bench -p pgdump_query
```

- `benches/decoders.rs` — one `decode`/`render` pair per mapped type family.
- `benches/whole_file.rs` — one warm-cache end-to-end `SchemaMode::Typed`
  scan. It warms the page cache before measuring, deliberately: the figure is
  meant to move when decode cost moves, not when the disk is busy.

Its input comes from `generate_perf_data.py` (default `--size-mb 256`, sized
to fit page cache): one wide table, one column per family the decoders cover,
plus `v_long_text` (very long values) and `v_escaped` (high escape density).
Its escaping is a Python reimplementation of `copy::encode_field` (I15), since
the script has no Rust runtime to call into.

`decoders.rs`'s `nested` group is the one group that is a ratio rather than a
tripwire — see "Nested decode costs what it copies". The generator's array and
composite stress columns are behind `--arrays` and `--composite`, and
`whole_file.rs` passes neither: that bench's input stays the brace-free
control, the same shape the scan-throughput and census figures were taken on.
`whole_file.rs` regenerates `runs/perf-whole-file.sql` when it is missing
**or** when `runs/perf-whole-file.stamp` disagrees with a hash of
`generate_perf_data.py` and the bench's size constant, so a change to the
generator is picked up without anyone remembering to delete the input.
