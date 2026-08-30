# Measurements

Every performance figure the design relies on, with the command that
reproduces it. A baseline nobody can re-run is a rumour with a decimal point,
so **a figure that loses its regeneration command should be deleted, not
kept**.

**Session stamp.** Every figure below was taken by `scripts/measure.py` on
2026-08-30, against commit `4c2c3e7`. One sweep, one apparatus — which is what
lets these tables be differenced against each other, and what "are these
figures from before or after my change" is answered by. `uv run measure.py
--stale` reads that commit back and names the figures a diff has invalidated
since. The one exception is `session-drift`, which no sweep can take: it is
derived across the published sweep and a second one taken two minutes later on
the same commit, and it says so in its own section.

All figures are on the hardware `CLAUDE.local.md` describes. Synthetic inputs
are regenerable with `--seed 42` and are **never committed** — they measure
throughput, not correctness, which stays entirely fixture-based.

Eleven standing rules for reading anything below:

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
  brace-free control is **0.537 s** timed by the container's own shell. The
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
  Whether a different allocator should be the shipped default is a P7
  question, filed in
  [`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md).
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
  and a **cross-file per-row difference** to **~0.5 µs/row**. A move inside its
  regime's floor is apparatus. Write it up as *reproduces*, never as a change —
  in this doc, in a history entry, or in an argument about which of two sweeps
  to publish; narrating one manufactures a finding that the next sweep silently
  reverses. Two consequences: a warm table's third decimal carries no
  information across sweeps, and the way to resolve something finer is a
  difference taken **inside** one sweep, not more reps. These three numbers are
  read off "What a session's own drift costs" below and off "The cross-file
  subtraction bottoms out"; a re-derivation of either re-reads this rule, which
  is a cross-reference rather than a `quoted_by` edge because a figure never
  declares the doc it lives in.
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

**Every figure below comes from one sweep of `scripts/measure.py`**, which is
what the session stamp above records. Figures that share a reading share it
rather than measuring it twice: each throughput table's `COPY` row *is* the
census table's census-on column for that regime, and the preamble table's
full-`parse` row *is* the quadratic table's 4000-block "after" column.

**One line, in two regimes: 3.00 GiB inputs read by a `glibc` binary in a
512 MB `postgres:16` container, timed by that container's own `bash`.** Warm
figures read from `/dev/shm`; cold ones read from the SSD with `drop_caches`
before every run, including before the floor. Nothing else builds or runs on
the machine while a sweep does — a `cargo` job across 24 cores moves the
numbers being taken, which is "a koji figure taken while local work ran is not
a figure" one scale down.

**The harness stages the inputs, and the budget is computed.** The full input
set is seven 3.00 GiB files, which does not fit `/dev/shm`, so it stages one
figure's inputs at a time and evicts what no remaining figure wants. The
ceiling is the largest single figure's own inputs plus 10% — 9.90 GiB here —
checked against the filesystem's real free space before the first measurement
rather than discovered twenty minutes into a sweep. Inputs are generated onto
the SSD once and *copied* into tmpfs, both from the host: 3 GiB written from
inside the 512 MB container would be charged to its cgroup and kill it.

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

**A commit can be acknowledged, and then it stops marking a figure stale.**
Coarse `depends` costs something in both directions. The paragraph above weighs
the false negative; the false positive is the one that decays the mechanism. A
change *inside* a declared path that provably moves nothing leaves `--stale`
red until a sweep re-stamps the doc — and a sweep is an hour on a machine that
has to be quiet, so the realistic outcome is that no sweep runs and `--stale`
becomes a light that is always on. A signal that is always on is the same thing
as no signal, which is the decay the register was built against, arriving from
the other side.

So `measure.ACKNOWLEDGED` records commits that touched a declared path without
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

*Rejected:* letting an acknowledgement cover library or harness changes on a
reading of the diff. There is no cheap oracle for those — the only way to know
whether a change to `map.rs` or to the harness's own timing path moved a number
is to take the number — so they stay stale and `--stale` keeps saying so. The
worked case is `session-drift`, which declares `scripts/measure.py` because the
harness *is* the apparatus it measures: the commits that armed the contention
gate are not acknowledgeable, and that figure waits for the sweep pair
`--drift` needs.

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

**The floor is read directionally, and inside a tolerance.** Contention makes a
floor *slower* — that is the entire mechanism it witnesses. So a co-measured
warm floor **above** the standing one by more than the warm resolution floor
(~8%, the rule above) disqualifies the sweep; one **below** it does not, and
neither does any move inside that band. Without both halves the check fires on
drift it cannot tell from contention: a sweep pair taken two minutes apart on
one commit read their `control` floor 5.5% apart in the *fast* direction, with
no contention available to produce it, and that was briefly read as grounds to
reject the quieter of the two.

That case is also why the doc carries the sweep it does. Both sweeps of
2026-08-30 pass the check as now written, and every table they disagree on
disagrees by less than the apparatus resolves — so re-stamping from one to the
other would move numbers without moving information. The published sweep is
therefore the one already folded in, and its softest table says so in its own
apparatus line ("Per-block cache saving is quadratic in block count").

*Rejected:* a **symmetric** tolerance, disqualifying a floor move beyond the
envelope in either direction. A faster floor does mean something about staging
changed, and in that pair the `control` file's warm readings moved with its
floor while the `arrays` file's did not — but that is exactly what a warm
figure being a *ratio against its own co-measured floor* already handles, and
it is reported where it belongs, in the drift figure's own section. Rejecting
a sweep for it would discard eleven good tables over a property the tables
themselves express.

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

## Scan throughput by input shape

Three 3.00 GiB synthetic dumps, a whole-file `pgdq` scan in a 512MB-limited
container, in both regimes. **Cold** is `drop_caches` before every run,
including before the floor, because that is the only regime in which a device
floor means anything: this file fits page cache twice over, so a second read of
it measures RAM. **Warm** is the same files on tmpfs, which is where the CPU
the device hides becomes visible.

<!-- figure: scan-throughput-cold — reproduce with `cd scripts && uv run measure.py --figure scan-throughput-cold` -->

**Every run cold, on the SSD**

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | **5.77 s** (5.76–5.81) | ~559 MB/s | 1.00× the floor's time |
| Large-object region | **5.77 s** (5.77–5.78) | ~558 MB/s | 1.00× the floor's time |
| `INSERT` run | **9.27 s** (9.20–9.28) | ~348 MB/s | **1.61× the floor's time** |
| `dd` → `/dev/null` | **5.76 s** (5.76–5.77) | ~559 MB/s | — |

Apparatus over every run in this table: CPU stall ≤1.09%, I/O stall ≤19.12%, machine ≤4% busy, steal ≤0.00%, busiest core ≥3.83 GHz, ≤62°C.

<!-- figure: scan-throughput-warm — reproduce with `cd scripts && uv run measure.py --figure scan-throughput-warm` -->

**Every run warm, on tmpfs**

| Input | Wall | Rate | Against the floor |
|---|---|---|---|
| `COPY` block | **0.537 s** (0.529–0.570) | ~5999 MB/s | 1.73× the floor's time |
| Large-object region | **0.455 s** (0.446–0.461) | ~7080 MB/s | 1.46× the floor's time |
| `INSERT` run | **7.71 s** (7.71–7.82) | ~418 MB/s | **24.81× the floor's time** |
| `dd` → `/dev/null` | **0.311 s** (0.307–0.315) | ~10358 MB/s | — |

Apparatus over every run in this table: CPU stall ≤0.21%, I/O stall ≤7.88%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.77 GHz, ≤64°C.

Each table's `COPY` row is the census figure's census-on column for that
regime — the same binary, the same command, the same input, not a second
measurement of it.

Every run completes inside the 512 MB cgroup, which is the memory claim this
apparatus can actually make. **It carries no max-RSS figure**: `/usr/bin/time
-f %M` around `nerdctl run` reports the *nerdctl client's* peak, not pgdq's —
it read the same ~40–45 MB for a 2 MB input as for a 3.00 GiB one, four times
what koji's row below records for a 784 GB scan. pgdq's own resident set is the
koji figure, ~9 MiB.

**What this says.** The `COPY` and large-object paths are device-bound to the
point of disappearing into the device: cold, they each spend **1.00×** the
wall-clock of reading the same bytes and doing nothing. Warm, the same scans
cost 0.537 s and 0.455 s against a 0.311 s `dd` floor — so the CPU is there,
and at 559 MB/s the disk simply covers it.

**The `INSERT` path is a different algorithm, and the warm table is what says
so.** Warm, an `INSERT` run costs **7.71 s against the `COPY` path's 0.537 s on
the same 3.00 GiB — 14.4× the per-byte CPU**, and **25× the `dd` floor** where
the `COPY` path is 1.7×. Every line is decoded into `Event::Line` and pushed
through the statement accumulator ([`architecture.md`](architecture.md), "Bulk
regions") where a `COPY` block's data is walked and skipped. Cold, the same
difference is compressed to 1.61× the floor, because the device hides most of
it — which is exactly why the two tables are here together rather than one
being differenced against the other's regime.

**This replaces the "~5× per-byte cost" claim, which was never a CPU ratio.**
That number divided a *cold* `INSERT` rate, device included, by the `COPY`
path's *warm* CPU. The honest figure is the one above, from a single regime:
**14.4×**, below the 16–26× the previous cold table was used to bound it at.
**Quote it as mid-teens, not to three figures**: sweeps of the same binaries
and inputs have read it anywhere between 14.4× and 16.2×, and the four that
agree at 14.4–14.9× are the four taken under an apparatus witnessed quiet by
the telemetry each table now carries. Both of the ratio's legs move inside the
envelope the drift figure below measures, and the `COPY` leg — half a second,
memory-bandwidth-bound — is the one that moves. Correctness, tiling
and row counts are unaffected; the fix is a scanner-level `INSERT` path, and
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md)
holds it. The number is corrected wherever it is repeated —
[`pg-dump-compatibility.md`](pg-dump-compatibility.md),
[`roadmap.md`](roadmap.md), [`architecture.md`](architecture.md) and
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md),
all four of which `--check` names as this table's consumers.
**What a ratio this size changes about P7's priorities is not**: that is a
decision rather than a value, and it is open
(`../status/STATUS.md`, "Decisions worth another look").

**The cold `INSERT` row has itself moved, and not because of the apparatus.**
It read 15.13–15.42 s when it was taken on 2026-08-25 and reads 9.27 s now;
`2eb51f4` changed how an `--inserts` dump's runs are scanned in between —
absorbing the `Data for` comment into the run. Attributing the difference needs
a build from before that commit and a second cold table, which nothing yet
requires.

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
one in ~15% of them, so the accumulator's quote tracker is genuinely exercised.
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
same kind of bytes and a sixteenth of what the `INSERT` path costs, is the
"skipped, not walked" evidence: walking it statement by statement would mean
one span *and one stored text string* per `lowrite` call, hundreds of
thousands of them.

## The census on brace-free rows costs 8% of a warm scan

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
| cold, on the SSD | **5.78 s** (5.76–5.81) | **5.77 s** (5.76–5.81) | **-0.008 s, -0%** |
| warm, on tmpfs | **0.499 s** (0.487–0.519) | **0.537 s** (0.529–0.570) | **+0.038 s, +8%** |

Apparatus over every run in this table: CPU stall ≤1.10%, I/O stall ≤20.33%, machine ≤4% busy, steal ≤0.00%, busiest core ≥3.64 GHz, ≤62°C.

Both binaries complete inside the 512 MB cgroup; no max-RSS figure is quoted,
for the reason under the scan-throughput table. `dd` → `/dev/null` on the same
file in the same container: **0.311 s** warm and **5.75 s** cold, so the
census-off scan is already within 1.6× of what the kernel charges to hand over
the bytes.

**What this says.** The pre-filter is very nearly free on the shape a real
dump mostly has: **0.038 s per 3.00 GiB of brace-free rows**, 47 ns per
16-column row of 3,956 bytes. That implies ~85 GB/s, which is well above what
this machine's DRAM will give one core — so the reading is not a
memory-bandwidth figure at all: the pre-filter re-walks bytes the scanner has
just walked, out of cache, and `memchr2` is fast enough that what is left is
the loop, not the bytes.

**Cold, the census is invisible**: −0.008 s on a 5.77 s scan — a *negative*
reading, which is the plainest possible statement that the effect is smaller
than the noise it sits in. It is inside
the run-to-run spread of the floor itself. That row is not a separate finding —
it is the same CPU cost, hidden behind a device delivering 559 MB/s. Which one
a user sees is decided by whether the bytes are already resident.

**Read the warm Δ against the instrument, not to three figures.** "What a
session's own drift costs" below puts warm sub-second readings several percent
apart between two sweeps of identical binaries and inputs — and both legs of
this Δ are such readings, moving independently — so +8% is "roughly a tenth",
and the +7%, +9%, +10% and +11% other sweeps read are the same measurement
rather than a change.

**It was not always.** The scalar `raw.iter().any(…)` loop that preceded
`memchr2` ran at ~3.8 GB/s and cost 1.03 µs per row — +39% as recorded and
**+63% reconstructed**, once the 0.77 s container wrapper that sat in both legs
is taken out ([`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)).
That figure is what argued for the swap, and it is why the deferred question
of a *skippable* census is now closed rather than open: 47 ns per row is not a
cost worth a knob.

So the census's cost is effectively **one tier, not two**: it is paid by the
rows that pass the pre-filter (next section), and the pre-filter itself is
3% of what those rows cost. It is unconditional either way
([`architecture.md`](architecture.md), "The array shape census") — the
alternative is a query that cannot retype its array columns without a second
pass. What the figure does *not* license is calling it exactly zero: the two
spreads do not overlap, and the earlier reading that said zero came from
taking the pair while the page cache was still filling.

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
| cold, on the SSD | **5.78 s** (5.76–5.80) | **5.84 s** (5.83–5.84) | **+0.062 s, +1%** |
| warm, on tmpfs | **0.504 s** (0.481–0.534) | **1.607 s** (1.583–1.627) | **+1.103 s, +219%** |

Apparatus over every run in this table: CPU stall ≤1.18%, I/O stall ≤21.48%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.62 GHz, ≤62°C.

Both binaries complete inside the 512 MB cgroup; no max-RSS figure is quoted,
for the reason under the scan-throughput table. The `dd` floor for this file in
this container is 0.312 s warm and 5.76 s cold, so warm census-off is within
1.6× of it and census-on is 5.2× it.

**What this says.** The census costs **1.103 s per 3.00 GiB of array-bearing
rows** — 1.58 µs per 19-column row — which is **+219%** on a scan reading from
memory, i.e. the census does a bit over twice the work the rest of the scan
does on this shape. Cold it is **+1%**, hidden behind the device
exactly as the brace-free case is. Both rows are the same CPU; which one a user
sees is decided by whether the bytes are already resident.

**The pre-filter is 47 ns of that 1.58 µs** (previous section, same
apparatus and the same census-off baseline to within 1%). So splitting the
row into fields and running `observe` over all 19 of them — the work the
pre-filter exists to avoid — is **97% of the census's whole cost**, and the
pre-filter is what keeps the brace-free case off that path. The census is
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
figures (47 ns/row rejected, 1.58 µs/row inspected) are what
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md) actually consumes, and
the census-off column exists to establish it once.

## Nested decode costs what it copies, and an element is an allocation

<!-- figure: nested-decode-micro — reproduce with `cd scripts && uv run measure.py --figure nested-decode-micro` -->

`benches/decoders.rs`'s `nested` group, criterion medians. **Two controls,
because there are two questions.**

- **Copy** — `String::from` over the same byte count: 20 ns at 49 bytes,
  42 ns at 601, 20 ns at 42. A nested value has no borrowed arm
  (`crate::batch::append_nested`), so this isolates what the *parse* costs on
  top of the copy it cannot avoid, which is the variable P7 is choosing
  over.
- **View** — one `append_view_unchecked` into a block the builder does not
  own, which is what `push_utf8view_field` does for an unescaped text field:
  **2.96 ns**, from `text_view_x1024`'s median ÷ 1024. Length-independent,
  which is the point of a view. This is what a user comparing a text column
  against an array column actually pays.

| Literal | Bytes | `decode` | `render` | ÷ copy | ÷ view |
|---|---|---|---|---|---|
| `integer[]`, 4 elements | 49 | 294 ns | 232 ns | **26.9×** | **178×** |
| `integer[]`, 50 elements | 601 | 3.64 µs | 1.50 µs | **122.2×** | **1740×** |
| two-field composite | 42 | 206 ns | 189 ns | **20.2×** | **133×** |

**Both control figures are read with a caveat.** `text_view_x1024` reports
1024 appends and must be divided — timing one append through
`iter_batched_ref` gave ~12.6 ns against a harness floor that `bool/decode`
puts at ~1.1 ns, so three quarters of it was criterion. And 2.96 ns is a
*floor* on the borrowed arm rather than the borrowed arm itself:
`push_utf8view_field` also scans the chunk deque with `find_map` and calls
`block_for`. So the `÷ view` column bounds the real ratio **from above**.

**The `÷ copy` column for the 50-element row is the least stable number in the
table**, because its denominator is: the 601-byte copy control has read 24 ns
in one sweep and 42 and 41 ns in the two that followed, which swings that
ratio between 122× and 221× while `decode` itself moves 2%. Read the `decode`
and `render` columns, which are what the design consumes; treat `÷ copy` as
the order of magnitude it establishes.

**What this says.** Cost is per *element*, not per byte: the two array lengths
differ only in element count, and the slope between them is **73 ns per
element** decoding and **28 ns per element** rendering. That is the shape of
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

## A typed query over nested columns costs 13.1 µs a row more than a string one

<!-- figure: nested-end-to-end — reproduce with `cd scripts && uv run measure.py --figure nested-end-to-end` -->

The end-to-end half of the figure above: what the per-element cost actually
costs a user. Two controls, on two axes. Within a file, `--schema-mode
strings` resolves every column to `Utf8View` and takes the zero-copy path, so
the `typed` run differs from it by decode plus Arrow build plus typed render
and nothing else. Across files, further 3.00 GiB dumps holding fewer of the
nested columns are what attribute the difference to a particular one — `pgdq
query` has no column projection, so there is no within-file way to ask.

Three inputs on tmpfs, output to `/dev/null`. **One interleaved sweep**: five
reps, each rep running both modes on all three files in turn, so the slow
upward drift across a long session lands on every row equally rather than on
whichever file went first. Medians of five:

| File | Rows | `strings` | `typed` | `typed` − `strings` | Ratio |
|---|---|---|---|---|---|
| control — 16 scalar columns | 814,362 | 4.20 s | 9.99 s | **7.10 µs/row** | 2.38× |
| `--composite` — the same 16 plus one composite | 803,995 | 4.17 s | 10.44 s | **7.80 µs/row** | 2.50× |
| `--arrays --composite` — the same 16 plus three nested | 699,962 | 5.17 s | 19.34 s | **20.24 µs/row** | 3.74× |

Apparatus over every run in this table: CPU stall ≤0.15%, I/O stall ≤2.66%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.83 GHz, ≤65°C.

Every run completes inside the 512 MB cgroup; no max-RSS figure is quoted, for
the reason under the scan-throughput table.

**The per-row difference is the figure; the ratio is derived and does not
travel.** A ratio carries that session's `strings` leg in its denominator, and
that leg is apparatus-sensitive far beyond the session drift measured below:
the same three files read 9.74 / 9.76 / 9.56 s on a page-cache-warm SSD with a
musl binary and 0.77 s of wrapper, against 4.20 / 4.17 / 5.17 s here. That
alone moved the nested ratio 3.08× → 3.74× while the per-row difference moved
15.1 → 13.1 µs. Quote a ratio only against the sweep it came from; the design
consumes the differences.

**What this says.** Typing the 16 scalar columns costs **7.1 µs per row**;
typing those plus the three nested ones costs **20.2 µs per row**. So three
nested columns — 19% more columns — cost **13.1 µs of every row**, nearly
twice what all sixteen scalar columns together cost. **The two array columns
carry essentially all of it**: adding the composite column alone moves the
per-row figure by **+0.70 µs** (paired median over the five reps +0.72),
against a cross-file instrument whose own floor reads −0.05 — so ~5% of the
three columns' cost, and a bound rather than a resolution (below).

Each per-row figure is that file's own `typed` minus its own `strings`, which
is what makes the subtraction legitimate: whatever the untyped baseline is
worth on a given file — and the three files hold different row counts at the
same byte count — it cancels out of that file's own difference, and would not
cancel out of a cross-file ratio.

**The untyped baseline is not file-independent, and the cause is the census —
measured, not inferred.** The control and the `--composite` file read within
1% of each other, which is this instrument's own drift; the `--arrays
--composite` file reads **23–24% above both**.
Its rows are the only ones carrying a `{`, so they are the only ones the
mapping pass's array-shape census splits into fields. Running the same query
with the **census-off** binary settles it:

<!-- figure: census-attribution — reproduce with `cd scripts && uv run measure.py --figure census-attribution` -->

| | control | `--arrays --composite` | gap |
|---|---|---|---|
| census on | 4.161 s | 5.153 s | **+0.992 s** |
| census off | 4.162 s | 4.104 s | **-0.058 s** |

Apparatus over every run in this table: CPU stall ≤0.15%, I/O stall ≤3.31%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.76 GHz, ≤62°C.

**The gap does not merely vanish with the census gone, it reverses**: +0.992 s
becomes **−0.058 s**, so the census accounts for the whole of it and then some —
on a file carrying 14% *fewer* rows than the control, which is why what
survives is a residue rather than a cost. Measured this way the census
reproduces the `parse` figures two sections above to within 5% on the arrays
file (+1.049 s against +1.103); on the control it reads −0.001 s against
+0.038 — a cross-check on a different command, and the control's disagreement
is two sub-0.1 s differences read off 4.2 s legs, which is what the drift
figure below says this instrument can resolve.

So a `strings` leg is a scan plus a census whose price depends on the data's
shape, not a flat per-byte floor. Both rows come from the same query loop
below, run with each binary in turn:

```sh
cd scripts && uv run measure.py --figure census-attribution
```

Earlier sweeps put all three baselines within 2% and
read that as evidence the untyped path was byte-driven; at 9.6 s legs a 1 s
difference was inside the spread, and it is not at 4.2 s.

The micro above covers 6.1 µs of that 13.1 µs (decode plus render for a
4-element array, a 50-element array and a two-field composite). The remaining
~7.0 µs is the Arrow build the micro does not reach: 56 per-element
`append_value` calls into the child builders, plus the list offsets. **The
literal parse is the smaller half of nested decoding**, which is the fact
P7 needs before deciding what to do about nested values always copying.

### The cross-file subtraction bottoms out at about half a microsecond a row

**One composite column sits at the edge of what this instrument can resolve.**
Two readings of the same quantity, from the same sweep, plus the control on
the instrument itself:

<!-- figure: cross-file-floor — reproduce with `cd scripts && uv run measure.py --figure cross-file-floor` -->

| Reading | Reps | Paired median | Per-rep readings |
|---|---|---|---|
| composite column's share — control against `--composite` | 5 | **+0.72 µs** | +0.49, +0.68, +0.72, +0.74, +0.81 |
| **the instrument's own floor** — control against a second control (`--seed 43`, same 16 columns) | 6 | **−0.05 µs** | −0.29, −0.17, −0.08, −0.03, +0.19, +0.41 |

Apparatus over every run in this table: CPU stall ≤0.16%, I/O stall ≤2.20%, machine ≤6% busy, steal ≤0.00%, busiest core ≥3.59 GHz, ≤64°C.

The second row is the control on the *instrument*: two files that differ only
in their random seed should differ by zero, and instead they span −0.29 to
+0.41 µs per row. **In this sweep the two rows do not overlap** — every
composite rep sits above every floor rep, by 0.08 µs — and that is still not a
resolution, because sweeps of the same binaries and inputs have read the same
composite share at +0.31, +0.39, +0.62, +0.69, +0.72 and +0.99 µs: takes of one
quantity **0.7 µs/row apart**, which is wider than the quantity itself. It is
also why the standing rule against quoting a
standard error forbids quoting an interval here — an earlier draft put these at
+0.61 ± 0.14 against +0.20 ± 0.18 and made the separation look like a result.
The micro puts the column at 0.40 µs of decode plus render before any Arrow
build, which an end-to-end reading has to exceed and does.

What the figure supports is therefore still a **bound**: the composite column
costs **of the order of a microsecond of every row end to end, ~5% of the
13.1 µs the three nested columns cost together**. That is the answer to "which
of the three columns is the cost" — the arrays, by an order of magnitude.
Whether a sweep that separates the two rows retires the bound is a question for
the maintainer, not for a fold-in (`../status/STATUS.md`, "Decisions worth
another look").

*Retracted:* the reading that this subtraction is *biased* rather than merely
imprecise. A musl-built leg of the same sweep put the composite file
**1.16 µs/row below** the control on the same five reps — a negative cost,
which adding a decoded column cannot produce — and that was read as a
structural confound in differencing two files of different row length. The
glibc leg reverses the sign on the same inputs and the same reps, so what was
being measured was the allocator, not the instrument. The floor stands at
roughly ±0.5 µs/row; the confound does not. This pair is also the
demonstration behind the standing rule against quoting a standard error.

**Built, not taken: the instrument that would resolve it.** Two files whose
data sections are **byte-identical**, one declaring `v_comp` as
`public.perf_comp` and the other as `text`, differ only in whether that one
column is decoded — same rows, same bytes, so the per-row normalization
carrying the floor above disappears, and the same pair read in `strings` mode,
where neither decodes the column, is its own zero control.

`generate_perf_data.py --weak-composite` writes the weak declaration and
`measure.py --figure composite-isolated` takes the reading; the pair is held
valid by `perf_generator_fidelity.rs`, which compares the two data sections
byte for byte, and by the figure refusing to divide if they stop sharing a row
count. **Nothing has run it under a quiet apparatus**, so this section still
reads the composite column's cost as the bound above, and `--list` carries the
instrument under "built, not taken" until someone needs the number badly enough
to take it. Whoever does publishes it the way any figure is published — with
the sweep that carries the doc's session stamp.

**`typed` and `strings` agree byte for byte on all three inputs**, as they do
on what `pg_dump` writes (`architecture.md`, "CLI surface") — so `cmp` on the
two outputs is a valid smoke test here, and
`pgdump_query-cli/tests/perf_generator_fidelity.rs` asserts it on small
generated files, the control and both nested flag combinations that back a
figure, so the generator cannot drift back out of that agreement.

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

## What a session's own drift costs, measured rather than asserted

<!-- figure: session-drift — reproduce with `cd scripts && uv run measure.py --drift <sweep> <sweep>` -->

The instrument measured against itself. The sweep this doc is stamped with and
a second one on the same commit ran **about two minutes apart** — back to back,
with a 120 s settle between them — on identical inputs and identical binaries,
and all 48 readings are common to both. **Both carry the per-reading telemetry
every table above reports.** The point is not any row but the shape:

| | Drift between two sweeps, two minutes apart |
|---|---|
| median absolute, over 48 readings | **0.4%** |
| largest | **7.3%** |
| cold **device-bound** readings (5–6 s) | **≤0.3%** |
| the one cold reading with real CPU in it (`INSERT`) | **0.3%** |
| warm readings | **0.0–7.3%**, median 0.6% |

**Drift is not a single number, and the gap between the sweeps is one of its
terms.** A pair two hours apart read a median absolute 5.6% and a largest 8.5%;
this pair, two minutes apart, reads 0.4% and 7.3%. The register carries no gap
parameter, so both are the same figure taken at two intervals, and **the
two-hour number is the one to plan against** — it is what "another session" and
"a comparison re-taken next week" actually cost. What the short interval buys
is a cleaner view of the rest: at two minutes almost everything reproduces, so
the readings that still move name themselves.

Three of them move, and each says something different:

- **Nothing device-bound does.** Every cold 5–6 s reading is inside 0.3%,
  including the cold `INSERT` scan — which at two hours drifted 8.1%, like the
  warm reading a third of it is. The device is the clock, and over two minutes
  even the CPU riding on it holds still.
- **The `control` file's warm `dd` floor moved 5.5%, and its warm readings
  moved with it** (−3.3% on the `COPY` scan, −5.5% on the census-off leg),
  while the `arrays` file's floor moved +1.9% and its readings barely at all.
  That is a per-file effect on tmpfs, not a machine-wide one, and it is the
  clearest statement available that a warm figure is a ratio against its own
  co-measured floor rather than an absolute.
- **`map-only` moved 3.7–7.3%, and here a counter did see it.** The first
  sweep's last two tables ran at ≤12% and ≤9% machine-busy against ≤5%
  everywhere else in both sweeps — under the armed gate, published, and
  visible in that table's apparatus line. Those three readings are the softest
  in this doc.

What is left after those is the part a counter cannot see — memory layout,
cache and TLB luck — which is why the remedy is to re-take a comparison whole
rather than to correct a reading.

So a *difference* between two warm legs of the same sweep is worth more than
either leg's absolute value across sweeps, which is what the standing rule
"re-take a comparison table whole" already required and this is the measurement
behind it. It is also the calibration for the standing rule against quoting a
standard error: a cross-file per-row difference under ~0.5 µs/row is apparatus,
and five sweeps agree on that floor (−0.17, −0.11, −0.05, +0.07 and +0.07
µs/row) far better than any of them agrees on the leg it came from.

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
| full `parse` of the same file | 21.03 s |

Apparatus over every run in this table: CPU stall ≤0.33%, I/O stall ≤12.01%, machine ≤9% busy, steal ≤0.00%, busiest core ≥4.12 GHz, ≤65°C.

The second row is not a second measurement: it is the quadratic table's
4000-block "after" column. What the pair has to establish is that an
uncancellable region is *milliseconds* against a scan of seconds to an hour,
and at 467× it clears that by more than any apparatus difference could take
away.

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
two builds differ in everything that landed after `b726f6b`, not only in the
save throttle, so the column says what the throttle era bought and must not be
differenced against a later change. What isolates a mechanism is the
census-off method above — one line, one rebuild — and what the P7 inbox
consumes is the map's own quadratic below, which needs no historical build at
all.

| blocks | dump | final cache | before | after | saves before → after |
|---|---|---|---|---|---|
| 1 (control) | 2.0 MB | 1 KB | 0.026 s | 0.005 s | 4 → 5 |
| 500 | 242 KB | 315 KB | 0.626 s | 0.291 s | 503 → 16 |
| 1000 | 484 KB | 632 KB | 2.45 s | 1.101 s | 1003 → 27 |
| 2000 | 973 KB | 1.2 MB | 10.65 s | 4.74 s | 2003 → 51 |
| 4000 | 1.9 MB | 2.5 MB | 47.18 s | 21.03 s | 4003 → 108 |

Apparatus over every run in this table: CPU stall ≤0.25%, I/O stall ≤8.85%, machine ≤5% busy, steal ≤0.00%, busiest core ≥3.61 GHz, ≤68°C.

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

**The throttle does exactly what it was designed to do, and the series still
quadruples per doubling.** Saves fall well under the `1/K` bound — visible in
the last column, and *self-tuning*: the throttle skips a save unless 20× the
last save's own duration has elapsed, so a faster machine or libc saves fewer
times, not the same number faster. The ~26 s of saving at 4000 blocks becomes
under a second of a 21 s scan. What is left is a *second* quadratic with the same
shape and a different cause: every `CopyEnd` clones the whole span list
(`map::Builder::snapshot`, then `stream::splice` over the prefix), so the map
is O(blocks²) with the cache **disabled entirely**:

```sh
# maps to EOF (the table never matches) and never saves
/pgdq query --source /dump.sql --table public.nosuchtable --dqcache none
```

<!-- figure: map-only — reproduce with `cd scripts && uv run measure.py --figure map-only` -->

| blocks | 1000 | 2000 | 4000 |
|---|---|---|---|
| map only, no saving | 1.174 s | 4.75 s | 20.86 s |

Apparatus over every run in this table: CPU stall ≤0.53%, I/O stall ≤11.63%, machine ≤12% busy, steal ≤0.00%, busiest core ≥4.04 GHz, ≤69°C.

**This table was taken under a witnessed CPU episode** — 12% machine-busy
against ≤5% for every other table in the sweep, under the armed gate's 15 but
well above the sweep's own baseline; the `preamble-prepass` table beside it,
taken in the same window, reads ≤9%. The reps here spread 7–11% because of it,
and that spread is within-sweep, so it measures these reps rather than the
apparatus. The sizes are what the figure is for and they are not in question;
the third decimal is. The second sweep read these three 4–7% lower, which is
*not* additional evidence — it is inside the warm resolution floor, and a move
that size across sweeps says nothing either way.

So at 4000 blocks the map is **all but a fraction of what a throttled `parse`
costs** — 20.9 s of 21.0 s — and the cache is what is left. That remainder is
under a second and cannot be resolved more finely: it is the difference of two
~21 s readings taken in different figures, which the drift figure puts several
percent apart. Against the *unthrottled* build the split is 26 s of saving
against 21 s of mapping.
Closing the second half means not rebuilding the span list per block;
it is filed in [`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md), because it
is a change to how the map is assembled rather than to when it is written.

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
