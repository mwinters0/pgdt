# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

## What exists

P1–P3 are complete and were struck at the keystone review, so there is no
per-phase checklist here any more. How the system works is
[`../design/architecture.md`](../design/architecture.md); what is still ahead is
[`../design/roadmap.md`](../design/roadmap.md).

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, and cache-only `info` | working, text output shape provisional; `--json` carries no shape promise at all. **`parse` is the only scanner** — it resumes from a matching cache and banks its progress at `COPY` block boundaries, throttled to ~5% of scan time and saving unconditionally on Ctrl-C (exit 130/143); `info` reports from the cache and never scans |
| Partial reporting | `info` reports a cache from an unfinished scan for as far as it got, with `Scan completion: N% (M bytes)` stated once at the top; `--json` carries the coverage components and per-`COPY`-block type resolution. An interrupted cache is **typed** for every database segment the scan finished — the mapping pass states each database's DDL at that database's first `COPY` block (I1) |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<` range struct `>`, and `List<List<T>>` for a uniformly multi-dimensional array column. Three shapes stay a string, each with its own resolution outcome: an array whose element type is opaque (`box`, a C base type, a shell type, through any chain of domains), an array whose element type is itself an array (I26), and an array column whose values disagree on shape |
| Array shape census | recorded by every mapping pass (`CopyBlock::array_shapes`) and **consumed**: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| Predicate and projection pushdown; per-row-group statistics | not started — P5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign, sparse row index | not started — P7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-28 (the measurement-era backlog's seven entries were reviewed and closed on 2026-08-27; the work they queued — `M12`, `M11`, `M13` under "The out-of-band queue" — has all landed, and `M13`'s re-take moved every warm figure in [`../design/measurements.md`](../design/measurements.md), three of them by a factor. `M16` renamed the doc tree to the `P<k>` phase-identifier scheme and settled what a keystone does to the out-of-band ledger; the sweep it prepares for has **not** run. **P4 and P9 are both complete and wrapped**, so no phase is open: what remains queued is `M17` alone — the committed measurement harness, which absorbed `M14`. Its **harness half landed 2026-08-28** as `scripts/measure.py`, and the sweep has run — but **no figure in `measurements.md` has moved yet**: the fold-in is what remains, per section, out of `runs/measure-20260828T021024/tables.md`. Choosing the next phase is a re-grilling the maintainer has claimed. A phase boundary: an unattended loop stops here.)

## P4 — complete and wrapped

Specified in
[`../design/roadmap-P4-composite-decoding.md`](../design/roadmap-P4-composite-decoding.md);
wrapped 2026-08-27, with the thirteen per-slice notes consolidated into
[`../design/roadmap-P4-composite-decoding-notes.md`](../design/roadmap-P4-composite-decoding-notes.md).
Every slice landed — 4.1 and 4.1.1 (the fixture value shapes, including the
domain-over-`box` array and the zero-field composite), 4.2 (the nested literal
codec), 4.3 (`ColumnBuilder`'s `List`/`Struct` arms), 4.4 (the resolution flip)
with 4.4.1 (the resolved Arrow type on `--verbose`, and the manual), 4.4.2 (the
array-of-array-typed-element refusal), 4.4.3 (the six array-declaration
spellings) and 4.4.4 (the array arm folded into one function), 4.5 and 4.5.1
(the shape census, recorded then consumed), and 4.6 with 4.6.1 (the array
stress data and the phase's measurement work). Six of those thirteen rows were
not in the spec's table: they were split at grilling, split mid-slice, or
earned afterwards from a defect or a review.

How the result works is
[`../design/architecture.md`](../design/architecture.md) — "Type resolution",
"The nested literal codec", "Nested columns: `NestedPlan` travels beside the
`DataType`", "The array shape census" and "What the census decides, and who may
believe it". The wrap moved what the slices learned into it by subject, so the
notes doc holds the phase's negative results, why its slice numbering is not
its landing order, and where its facts were filed.

## P9 — complete and wrapped

Specified in
[`../design/roadmap-P9-partial-reporting.md`](../design/roadmap-P9-partial-reporting.md);
wrapped 2026-08-27, with the per-slice notes consolidated into
[`../design/roadmap-P9-partial-reporting-notes.md`](../design/roadmap-P9-partial-reporting-notes.md).
Every slice landed — 9.1 (`parse` resumes and banks per block), 9.2 (`info`
stops scanning), 9.3 (the coverage line), 9.4 (per-block resolution in
`--json`), 9.5 (the save throttle and the interrupt guard, earned from 9.1's
measurement) and 9.5.1 (metadata stated at every legal boundary, earned from
9.5's verification). How the result works is
[`../design/architecture.md`](../design/architecture.md), "CLI surface" and
"The cache"; the wrap moved what the slices learned into it by subject, so the
notes doc holds only the phase's negative results and where its facts were
filed.

## The out-of-band queue

- **M12, M11, M13 — three out-of-band items in that order**, queued
  2026-08-27 out of the review of the measurement-era backlog. They are three
  rows rather than one because four unrelated edits under one ledger row is
  unreadable later; the order is forced, since each lands on the one before.

  - [x] **`M12` — generator hygiene.** `scripts/generate_perf_data.py` draws
    the microsecond component of its three date/time columns uniformly instead
    of from a shape-coverage list, and
    `pgdump_query-cli/tests/perf_generator_fidelity.rs` asserts `uv` is
    runnable — naming `mise install` — instead of skipping itself. Ledger row
    `M12` in [`../design/roadmap.md`](../design/roadmap.md); the notes are
    [`history/2026-08-27.md`](history/2026-08-27.md), "`M12`: the benchmark
    generator draws a uniform microsecond".

  - [x] **`M11` — the census pre-filter on `memchr2`.**
    `map::Builder::on_row`'s pre-filter is `memchr::memchr2(b'{', b'[', raw)`,
    and the function carries a doc comment naming the two
    [`../design/measurements.md`](../design/measurements.md) sections whose
    census-off column is taken by patching it. Ledger row `M11`; the notes are
    [`history/2026-08-27.md`](history/2026-08-27.md), "`M11`: the census
    pre-filter is `memchr2`". **The swap delivered**: `M13` re-took the figure and
    the pre-filter is 45 ns a row against the scalar loop's 1.03 µs, so the
    deferred "should the census be skippable" question is closed rather than
    reopened.

  - [x] **`M13` — the warm set re-taken on tmpfs, in one session.** Five
    figures were page-cache-warm SSD reads taken before that standing rule
    existed: both census figures, the nested end-to-end table, the per-block
    quadratic table, and the `COPY` path's warm CPU that the scan-throughput
    table cites. All are re-taken and folded into
    [`../design/measurements.md`](../design/measurements.md), whose "The warm
    set" now records the one apparatus they share. Ledger row `M13`; the notes
    are [`history/2026-08-27.md`](history/2026-08-27.md), "`M13`'s figures are
    folded in".

    **Every figure moved, three of them by a factor**, because the tmpfs,
    in-container-timer and glibc rules landed together: the warm `COPY` scan
    is 0.57 s where it was 2.92 s; the census on brace-free rows is +7% where
    it was +39%; the nested `strings` leg is 4.43 s where it was 9.74 s. Two
    readings are **withdrawn** rather than adjusted — that cross-file
    differencing is structurally biased, and that the untyped baseline is
    file-independent — both of which were artifacts the sharper apparatus
    dissolved. The musl sweep is kept at `runs/m13-warm-set-musl.log` as the
    evidence for the libc gap and carries no figure the docs cite.

  - [ ] **`M14` — the cold scan-throughput table, re-taken under one
    apparatus. Absorbed into `M17` on 2026-08-27; the number is spent and is
    never reused.** It is kept here because its scope statement is the
    specification of one of `M17`'s stages, not because it will land on its
    own. Queued 2026-08-27 out of `M13`'s apparatus finding, which
    reaches further than the warm set: every `pgdq` row of that table was
    timed around `nerdctl run` and carries 0.77 s, its floor row is a **host**
    `cat` against container `pgdq` runs, and all of it is a musl binary —
    three apparatus faults in one comparison, which arithmetic cannot
    repair. Its own regime (cold,
    `drop_caches` before every run, on the SSD), its own inputs (the
    large-object and `INSERT` generators regenerated at 3.00 GiB), and the
    floor taken with `dd` inside the same container. **It also carries the two
    census sections' cold rows**, added 2026-08-27: `M13` re-took those
    comparisons warm only, leaving four claims about the cold regime (+1.2%
    and +2.6%, in [`../design/measurements.md`](../design/measurements.md),
    [`../design/architecture.md`](../design/architecture.md) and the P7 inbox)
    sourced to the superseded pre-`M11` apparatus. `M17`'s sweep stages the
    3.00 GiB `COPY` control cold in that container, so the census-off binary
    costs it two extra runs per file. Corrected by subtraction
    the conclusion strengthens rather than moves — the `COPY` path runs 1.03×
    the floor, not 1.2× — so this buys a correct number for a claim that
    already holds, which is why it is queued rather than urgent. Scope and
    the corrected reading are in
    [`../design/measurements.md`](../design/measurements.md), "Scan throughput
    by input shape". **Not** a re-take of koji (0.77 s of 3300 s) or of the
    criterion micros (no container).

  - [x] **`M15` — `whole_file.rs` regenerates its input when the generator
    changes.** The bench regenerates `runs/perf-whole-file.sql` only when the
    file is *missing*, so a checkout that already has one benchmarks pre-`M12`
    bytes forever and silently — and the population that has one is exactly the
    population that will compare a new number against an old one. The fix is to
    hash `scripts/generate_perf_data.py`, store the hash beside the input, and
    regenerate on mismatch — landed as `runs/perf-whole-file.stamp`, with both
    branches exercised by hand (an unchanged generator reuses the input; a
    one-line edit to the generator regenerates it). Ledger row `M15`; the notes
    are [`history/2026-08-27.md`](history/2026-08-27.md), "`M12` armed a
    stale-input trap".

  - [x] **`M16` — the phase-identifier scheme, and what a keystone sweeps.**
    A phase is identified by `P<k>` permanently and its slug is an informal
    caption; slice numbers stay integers, where the order they assert is real.
    The out-of-band ledger is struck at a keystone like the phase docs, leaving
    a watermark of spent numbers. Two phases may be in flight at once given
    disjoint mechanisms and separate checklists. Landed in
    [`../process.md`](../process.md), `CLAUDE.md` and
    [`../design/roadmap.md`](../design/roadmap.md), and the repo was renamed to
    match — `docs/design/roadmap-P<N>-<slug>*.md`, with every open phase's
    inbox gaining a slug. **The keystone sweep itself has not run**: P4's and
    P9's docs were renamed, not struck. Ledger row `M16`; the notes are
    [`history/2026-08-27.md`](history/2026-08-27.md), "Phases are identified by
    `P<k>`".

  - [ ] **`M17` — the measurement harness, and the sweep that fills the doc.**
    **The harness is in; not one table has been folded into the doc.** Contract
    settled across 2026-08-27 and 2026-08-28; the notes are
    [`history/2026-08-27.md`](history/2026-08-27.md) and
    [`history/2026-08-28.md`](history/2026-08-28.md), the second of which also
    says where the next session picks this up.

    **Landed 2026-08-28**: `scripts/measure.py`, which implements all thirteen
    rows of the inventory below as eleven figures; `scripts/test_measure.py`
    (65 stdlib `unittest` cases); `CLAUDE.md`'s command block and read-trigger,
    and the contention and process-group rules its sweep needs;
    `CLAUDE.local.md`'s paths; and
    [`../design/measurements.md`](../design/measurements.md)'s "each section's
    recipe below is the durable record" retargeted at the harness. Verified
    against the recorded figures at reduced scale — the quadratic table's save
    counts come back 503 → 15 / 1003 / 2003 / 4003 and its wall times within
    3–8% of the doc's, and the preamble dump's first `COPY` header at byte
    980,996 of 1,998,741 exactly.

    Also landed, out of the 2026-08-28 grilling: `quoted_by`, the `--check`,
    `--koji-recipe` and `--drift` commands, marker-based section addressing, a
    computed tmpfs budget with a preflight, the cold and warm throughput tables
    sharing one doc section, and
    `pgdump_query-cli/tests/measure_harness.rs`, which runs the harness's own
    95 tests from `cargo test --workspace`. `--koji-recipe` replaced the three
    hand-maintained copies of koji's invocation in `CLAUDE.md` and
    [`../design/measurements.md`](../design/measurements.md); one of the three
    could not have been interrupted cleanly, since it wrapped `parse` in a
    compound `sh -c` that cannot be `exec`'d.

    **`M17` is the harness, and a claim the sweep overturns is not part of
    it.** The out-of-band admission rule is "changes no decision any spec
    records and fits one session"; the harness meets it and the fold-in's claim
    changes do not, so those travel to
    [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md)
    rather than growing this item. The first is filed there already.

    **Not landed**: any table. The first full sweep ran for 36 minutes and
    **lost three of eleven figures** to a staging budget that was over by 6,799
    bytes — its five surviving tables are kept as a drift cross-check, not
    folded in, since the doc's tables must come from one session. The re-run is
    what the fold-in reads. The fold-in is per section: the table, the prose
    recipe it replaces deleted with it, the `<!-- figure: <id> -->` marker, and
    the session stamp. **This box ticks when every row of the inventory below
    is harness-emitted *in the doc*.**

    **One figure will move a claim, not a value.** The cold `INSERT` run reads
    9.77 s and 1.70× the device floor against the recorded 15.13–15.42 s and
    2.7×, because `M7` changed how `--inserts` runs are scanned after that row
    was taken. Under the rule settled the same day, a fold-in may replace a
    value but a changed *conclusion* goes back through grilling — so the `~5×`
    claim in
    [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)
    and [`../design/roadmap.md`](../design/roadmap.md) is not the fold-in's to
    rewrite. The re-run's warm `INSERT` figure decides how far it moves.

    **The fold-in reaches five other files, and that is now declared rather
    than remembered.** Every figure names the documents that repeat its numbers
    (`quoted_by`, printed by `--check` and beside every emitted table):
    [`../design/architecture.md`](../design/architecture.md),
    [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
    [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md),
    [`../design/roadmap.md`](../design/roadmap.md),
    [`../design/roadmap-P4-composite-decoding-notes.md`](../design/roadmap-P4-composite-decoding-notes.md)
    and this file. The P7 inbox alone repeats about fifteen of these figures,
    and `pg-dump-compatibility.md` carries the "~5× the per-byte cost" claim
    that the warm throughput table exists to replace.

    **Why it exists.** `M10`, `M13` and `M14` each re-derived the same
    apparatus from scratch because every re-take so far has been a one-off
    `runs/` script that dies with the session. The recurrence is the target,
    not any one figure: `M17` commits the sweep to `scripts/` so the next
    apparatus change re-runs it instead of rewriting it, and amends
    [`../design/measurements.md`](../design/measurements.md)'s "each section's
    recipe below is the durable record" to point at it. That amendment lands
    *with* the harness, not before.

    **The tables it emits, and their state today.** ✅ marks one already on
    the current footing — the harness has only to *reproduce* those, where the
    rest are re-takes or first takes. The sweep emits **every row** regardless
    (see "the run replaces every table" below), so this column is a statement
    about how much each row is expected to move, not about which ones get run.
    The list is the progress record: the box above ticks when every row is
    harness-emitted, and until then this says which are.

    | # | Table | Stage | State |
    |---|---|---|---|
    | 1 | Scan throughput by input shape (`COPY`, large object, `INSERT`, `dd` floor) | cold SSD | superseded apparatus |
    | 2 | Census on brace-free rows — cold pair | cold SSD | pre-`M11` reading |
    | 3 | Census on array-bearing rows — cold pair | cold SSD | pre-`M11` reading |
    | 4 | Census on brace-free rows — warm pair | warm tmpfs | ✅ |
    | 5 | Census on array-bearing rows — warm pair | warm tmpfs | ✅ |
    | 6 | **Warm** scan throughput by input shape — row 1's twin | warm tmpfs | **never taken** |
    | 7 | Nested end-to-end, three files × two modes | warm tmpfs | ✅ |
    | 8 | Cross-file floor, seed-42 against seed-43 | warm tmpfs | ✅ |
    | 9 | Census attribution, census on/off × two files | warm tmpfs | ✅ |
    | 10 | Per-block quadratic — before/after/save counts | warm tmpfs | ✅ |
    | 11 | Map alone, cache disabled | warm tmpfs | ✅ |
    | 12 | Preamble prepass | — | host, **no container at all** |
    | 13 | Nested decode micro — decode/render/÷copy/÷view | `criterion` | conforming; harness emits it |

    Row 12 runs `/usr/bin/time` around a **host** `pgdq`, so it obeys neither
    the timer nor the cgroup rule; nobody had counted it until 2026-08-27.

    **Thirteen rows are eleven figures**, which is what `uv run measure.py
    --list` names: rows 2 and 4 are the cold and warm rows of *one* table
    (`census-brace-free`), as are 3 and 5 (`census-arrays`), and a table is the
    unit that may not be half re-taken. The other nine map one-to-one —
    `scan-throughput-cold`, `scan-throughput-warm`, `nested-end-to-end`,
    `cross-file-floor`, `census-attribution`, `per-block-quadratic`,
    `map-only`, `preamble-prepass`, `nested-decode-micro`.

    **Row 6 is the reason `M17` was allocated**, and its shape is settled: the
    cold table's twin — same four rows (`COPY` block, large-object region,
    `INSERT` run, `dd` floor), same columns, tmpfs instead of a cold SSD. That
    makes the `INSERT` path's per-byte CPU a division **within one table**
    rather than the cross-regime comparison it has been since it was written,
    which is the defect that allocated `M17`. Its `COPY` row is not a new
    measurement: it is row 4's census-on column, same binary, same command,
    same input.

    **Two things are deliberately not tables.** koji is a byte-for-byte
    regression check on another medium — the harness owns its invocation and
    does not run it. `benches/decoders.rs`'s per-type pairs and
    `benches/whole_file.rs` are **tripwires**, quoting no number, and the
    harness says so rather than inventing a table nobody consumes: "a figure
    with no table shape is a figure nobody has decided how to report" catches
    oversights, and these are a decision.

    **Row 13 comes from `criterion`'s own JSON**, not from scraping console
    output: `target/criterion/<group>/<bench>/new/estimates.json` carries a
    median in nanoseconds, and every cell of that table is a median plus a byte
    count that is a constant in the bench.

    **`M14` is absorbed into it**, settled 2026-08-27 — two overlapping cold
    sweeps is the thing to avoid, and `M14`'s cold table plus the two cold
    census rows are one stage of this. `M14`'s number is **spent, not
    reused**.

    **koji stays out of the run.** It is 784 GB on the HDD, ~54 minutes, a
    different medium and a regression check rather than a throughput figure,
    and it is device-bound at ~33% of one core so the allocator is unlikely to
    move it. Its glibc re-take rides the next koji run. What `M17` does take
    is the **invocation**: the harness owns koji's recipe so the next session
    to run one conforms without re-deriving it.

    **The inputs do not fit, so the sweep is staged.** Control, seed-43
    control, `--composite`, `--arrays`, `INSERT` and large-object is 6 ×
    3.00 GiB = 18 GiB against `/dev/shm`'s 16 G, and the cold stages need the
    SSD rather than tmpfs anyway. `M13`'s sweep already had to delete two
    inputs mid-run for the same reason.

    **One libc: glibc.** Settled 2026-08-27. The sweep does not take a second
    leg — the musl comparison in `measurements.md`'s ninth standing rule is
    the evidence for *why* an SE is not an error bar, not a practice to
    repeat, and the corollary that survives is the measured ~0.5 µs/row floor.

    **It emits the markdown tables, not just a log.** Each figure's section in
    [`../design/measurements.md`](../design/measurements.md) has a table shape,
    and the harness writes that shape so folding in is a paste. Transcription
    is the step that has actually gone wrong — `M13`'s fold-in introduced two
    wrong readings by hand — and the interpretation around each table is prose
    a harness cannot write and should not try to. A figure with no table shape
    is a figure nobody has decided how to report, which is the useful forcing
    function.

    **Python in `scripts/`, run with `uv`.** Every generator there already is,
    and there is no shell script in `scripts/` at all. The specific cost a
    shell harness cannot remove: every sweep so far has ended with a throwaway
    Python parser scraping medians out of a shell log — one for `M13`, another
    for the census-baseline test — and that scraping step is where the
    transcription errors live. A Python harness still shells out per run, but
    times, takes medians and spreads, and emits the tables in-process.

    **Parameterized by environment variable, defaults for this machine.** It
    moves from `runs/` to `scripts/`, so machine facts cannot travel with it:
    the procedure goes in `CLAUDE.md` and the paths stay in `CLAUDE.local.md`,
    the split the project already runs. A harness that has to be rewritten
    after a machine move is the same recurrence in a different disguise.

    **Selection is per figure, never finer.** The fourth standing rule makes a
    *table* atomic — half a table may not be re-taken — and one figure is
    exactly one table, so per-figure selection cannot violate it while
    per-run selection would. Stage grouping (cold-SSD, warm-tmpfs,
    `criterion`) is the natural default because it shares input staging, but
    the selectable unit is the figure. Without this the harness is unusable
    while being written, since a full run is 18 GiB of staging and an hour.

    **The fold-in lands per section, across several commits.** A thirteen-table
    diff is exactly what "never mix high- and low-confidence work in one review
    cycle" forbids, and `M13`'s four-figure fold-in already produced two
    withdrawn readings and three flagged calls.

    **The prose recipes it executes are deleted, not left alongside.** Each of
    `measurements.md`'s thirteen `sh` blocks is tested by one question — does
    the harness execute it? If yes, the block becomes the one-line invocation
    that reproduces that figure. Leaving both is the second-authority problem
    the keystone doctrine is written against, and `M19` proved it is not
    hypothetical: a recipe drifted out of runnability and nothing noticed,
    because nothing executes prose. **Two kinds of block stay**, because the
    harness genuinely does not own them: the census-off **source patch**
    (`return;` at the top of `map::Builder::on_row`, which no harness can
    perform) and generator invocations a reader may want on their own. koji's
    detached recipe was a third until 2026-08-28, when `--koji-recipe` took it.

    **It is tested where a silent error would be worst.** Median, spread and
    table formatting get stdlib `unittest` — `uv run python -m unittest`, no
    dependency added to a repo that has none — because a wrong median produces
    a confidently wrong table, which is the failure `M17` exists to prevent. No
    container-level smoke test: that would need root and a runtime to run the
    suite, a cost this project has consistently refused. The second guard is
    free and already obligatory: the ninth standing rule requires the observed
    per-rep readings beside every median, so each emitted table carries its own
    audit trail and a wrong median is visible against the numbers that produced
    it.

    **The run replaces every table, including the seven already current.** The
    doc differences *across* tables — the census-attribution table is quoted as
    reproducing the census `parse` figures to within 3%, and row 6 exists so
    the `INSERT` ratio becomes a division against row 1 — so a doc spanning two
    sessions reintroduces the fourth standing rule's failure one level up. Seven
    tables moving by the session drift is not churn to avoid; it is evidence
    the drift is real, and the ninth rule already forbids reading a small
    movement as a result.

    **`measurements.md` gains a session stamp** — one line near the top naming
    the sweep's date and the commit it ran against — and per-section apparatus
    notes shrink to the places a section genuinely departs (koji's medium,
    `criterion`'s absence of a container). Stating the shared apparatus once
    and letting a section speak only about its departures is the same argument
    as the standing rules themselves, and it answers "are these figures from
    before or after my change" at a glance, which has been answered by
    archaeology every time so far.

    **Each figure declares the paths that invalidate it**, so the harness can
    say which figures a diff has made stale — the census figures on `map.rs`,
    the nested tables on `nested.rs`/`batch.rs`, the quadratic on the map plus
    the cache, every figure on its generator. This is the *other* half of the
    recurrence and the one the harness alone does not fix: `M11` changed one
    line in `on_row` and invalidated both census figures, `M12` changed a
    generator and armed a stale-input trap that `M15` had to disarm, and in
    both cases nothing announced it — someone noticed. A declaration is the
    same forcing function as the table shape: a figure that cannot say what
    invalidates it is one nobody has thought about. `CLAUDE.md` gains both the
    command and the read-trigger when the harness lands, since a capability
    nobody is told about is one nobody uses.

    **The box ticks only when every table above is harness-emitted**, per the
    rule that a tick meaning "about half" makes every other tick worthless. The
    table is the progress record in the meantime, which says more than a tick
    would — it names *which* are done.

  - [x] **`M18` — musl leaves the apparatus.** Only glibc is measured, so a
    static musl build is an untested configuration and an untested portability
    claim is worse than none. `CLAUDE.md`'s long-running-job section no longer
    offers a musl recipe, [`../design/measurements.md`](../design/measurements.md)'s
    eighth standing rule says musl is not measured and is in no recipe, its two
    koji recipes build the default target and run `postgres:16`, and the P7
    inbox's allocator entry is reframed — musl settled, `jemalloc`/`mimalloc`
    still open. The musl *figures* stay where they are, labelled, because they
    are the evidence for naming glibc and for the ninth rule. Ledger row `M18`;
    the notes are [`history/2026-08-27.md`](history/2026-08-27.md), "Only
    glibc, anywhere". **`postgres:16-alpine` in
    [`../design/postgres-invariants.md`](../design/postgres-invariants.md) and
    the compatibility matrix is untouched** — that is a PostgreSQL image used
    to probe server behaviour, nothing to do with pgdq's libc.

  - [x] **`M19` — the scan-throughput recipe runs, and `--seed` stops calling
    determinism a non-goal.** The documented regeneration command for two of
    that table's three inputs could not execute: it passed `--size-mb` to
    generators whose flag is `--size-gb`, and omitted the required positional
    output path. Fixed, along with the misleading `--seed` help in
    `scripts/generate_large_object_bench.py` and
    `scripts/generate_insert_run_bench.py` — both use `random.Random(seed)`, and
    a seeded run was verified byte-for-byte reproducible, which is the property
    the standing rule "re-take a comparison table whole" depends on. Ledger row
    `M19`; the notes are [`history/2026-08-27.md`](history/2026-08-27.md),
    "A documented recipe that does not run".

- **Order from here**, re-settled 2026-08-27 with `M12`, `M11` and `M13`
  landed: **`M17` alone**, which absorbed `M14`. Nothing else is scheduled
  (`M15` and `M16` each landed the day they were queued; the keystone sweep
  `M16` prepares for is the maintainer's call, not a scheduled item). `M17`
  runs **before** the phase-choice conversation, settled 2026-08-27: leaving
  six of twelve figures under a superseded apparatus, or none at all, is the
  "figures disagreeing about regime" failure the standing rules were written
  from — and a phase that opens on those figures inherits the disagreement.
  No phase is open — 4 and 9 are both wrapped, and everything
  queued ahead of them has landed (M5–M12, the koji wrap run whose durable
  halves are in [`../design/measurements.md`](../design/measurements.md),
  "koji full scan", and
  [`../design/architecture.md`](../design/architecture.md), "CLI surface").

  **Which phase comes next is a separate conversation**, claimed by the
  maintainer on 2026-08-27. `process.md` step 6 re-grills the roadmap before
  the next phase is specified, and four are unspecified (5, 6, 7, 8); numeric
  order is not plan order, since 9 was taken ahead of 5. An unattended session
  does not pick one, so this is where an unattended loop stops whatever remains
  queued behind it.

  The `--disable-triggers` fix is **not** in this order — it is unscheduled, in
  `roadmap.md`'s "Future".

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Nothing is pooled here at present.

- **`M17`, the only queued out-of-band item** — the committed measurement
  harness and the sweep that fills the doc, which absorbed `M14`. Six of its
  twelve figures need re-taking or taking for the first time; see "The
  out-of-band queue" above. Nothing else is scheduled.

## Known gaps

- **A `--disable-triggers` dump loses TOC attribution on every data span**,
  `COPY` and `INSERT` runs alike. I31: `pg_dump` writes `ALTER TABLE … DISABLE
  TRIGGER ALL;` — and `SET SESSION AUTHORIZATION DEFAULT;` ahead of the first
  entry — between each `-- Data for Name:` block and the data, so the comment
  is no longer adjacent to what it heads and closes as its own `Framing` span,
  which clears `governing_toc`. Measured against 16.15, two tables: TOC
  coverage 2/24, the two being the comment blocks. **Accepted, not a
  correctness hazard**: tiling stays byte-exact, the table name comes from the
  `COPY` header or the `INSERT INTO` line, the object census still reads
  `TABLE DATA: 2`, and roles still come off the entry's `Owner:`. What is lost
  is the coverage diagnostic and `Span::toc` on data spans. Opt-in and gated on
  `--data-only`/`--section=data`. Not fixed opportunistically because a fix is
  three coordinated changes, one of them to a decision `architecture.md`
  states — so by `roadmap.md`'s admission rule it is a graded slice, not a
  drive-by. Wanted but unscheduled, with the three changes named:
  [`../design/roadmap.md`](../design/roadmap.md), "Future — wanted,
  unscheduled".

- **An array nested inside a composite** is still decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode` naming the column. Deliberate and permanent as things
  stand: the census is keyed by column and has nowhere to record a shape at
  that depth, so scanning more of the file cannot help. `--schema-mode
  strings`, which the message now names, returns the literal verbatim. A
  *top-level* array column does not reach this — it is retyped from the census
  on any query, cold or full — and an array whose *element type* is an array
  does not either, since resolution refuses that shape outright (I26). Keying the census by path is a roadmap "Future" item and would be
  purely additive.
- **An array type this build declines to represent comes back as text with no
  way to ask for more.** Two shapes are in that state: an array whose element
  type is itself an array (`ColumnResolution::NestedArrayElement`) and an
  array column whose values disagree on shape (`VaryingArrayShape`). Neither
  is opaque — both are fully understood, and a representation that is lossless
  for every array (dimensions, lower bounds and elements in one value) would
  cover both. Accepted, not scheduled: it is a roadmap "Future" item, and
  adding it later only ever touches columns these two refusals leave as
  `Utf8View`, so it strictly widens coverage.
- **A type name that needs quoting comes back `Unknown`.** A type or domain
  whose name contains a space, a bracket, or the `ARRAY` keyword is legal, and
  `pg_dump` writes it quoted in both its `CREATE` statement and every column
  declaration using it (I29). `parse_ident` dequotes it into `TypeDef.name`
  while the declaration keeps its quotes, so the lookup never matches: the
  column resolves `Unknown` and stays `Utf8View`, and an array *of* such a type
  resolves `List<Utf8View>` — the array-ness is still read correctly, since the
  `[]` falls outside the quotes. **No spelling is misread**: a column of a type
  named `"x ARRAY"` or `"d[3]"` is not mistaken for an array, because
  `array_element`'s strip helpers bail on the trailing `"`. So the cost is a
  weaker type, never a wrong one, and every value still decodes as the text the
  file holds. Unreachable from any dump whose type names are ordinary
  identifiers, which is every fixture and the koji sample. The fix is the
  roadmap "Future" item "A real type-name tokenizer", and it is strictly
  additive.

- **Mapping is O(blocks²), and the save throttle only halved it.** Every
  `CopyEnd` rebuilds `DumpIndex::spans` whole — `map::Builder::snapshot` clones
  the builder's spans, `stream::splice` clones the prefix — so a block-rich,
  byte-poor dump pays quadratic CPU with the cache disabled entirely: 18.6 s
  for 4000 blocks under `query --dqcache none`, against under 10 ms for the
  same bytes in one block. 9.5's throttle removed the other half (45.8 s → 20.1
  s for a 4000-block `parse`), which leaves the map as **93%** of what a
  throttled `parse` now costs at that block count. Accepted for now, not scheduled: the fix is to
  stop rebuilding the span list per block, which is the same code P7's
  parallel-scan plans would rework and which
  [`roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md) already flags
  for assuming coverage is a contiguous prefix — so the two belong in one
  decision. A **cheap** version exists and was weighed: for `parse` nothing
  reads `index.spans` between saves, so gating the splice on the throttle the
  same way the save is gated would cost a few dozen splices instead of `n` (~18.6 s
  → ~1 s at 4000 blocks). It is not taken, because it would make an interrupt
  bank the last *saved* watermark rather than the last *completed block* —
  reversing a guarantee 9.5 established — and `Builder::snapshot` asserts
  `Idle`, so the chunk-top interrupt check cannot re-derive the spans mid-block
  to compensate. The analysis is in the P7 inbox so it is not re-derived. Nothing koji-shaped is affected: 74 blocks over 784 GB pay this
  74 times, and the figure there is +1.5%. Figures and commands:
  [`../design/measurements.md`](../design/measurements.md), "Per-block cache
  saving is quadratic in block count, and so is the map".

- A query stops mapping once its target is settled, so a conflicting
  candidate **past** the stopping point is never seen and
  `Error::AmbiguousTable` is not raised for it — the query returns the
  candidate it found, with no signal that another existed. The stop rule
  (`stream::target_settled`) rules out the two shapes that announce
  themselves: a matching block carrying a partition-root marker (I2), and a
  file containing any `\connect` at all (`pg_dumpall`, concatenation,
  `--create`). What is left undetectable is a file whose *first* segment is a
  plain dump with something concatenated after it — nothing in the prefix says
  so. `ScanExtent::Full` (or a query after `pgdq parse`) gives exact
  detection. Rows are never a union either way, and ambiguity is raised
  *before* any row is emitted rather than partway through one candidate's,
  which is what the previous form of this gap cost. Accepted, not
  scheduled — closing it means abandoning early stopping, which is what makes a
  cold query on a large dump affordable. Filed into
  [`roadmap-P6-embeddable-engine-inbox.md`](../design/roadmap-P6-embeddable-engine-inbox.md) so the
  embedded API's promises get decided against it deliberately.
- `DumpIndex::roles`/`tablespaces` are complete only once `scanned_through`
  reaches the file's size — the same partiality `metadata`'s
  `preamble_complete` already carries, for the same reason: a query that
  stops at its target (`ScanExtent::UntilTargetSettled`, the default) never
  reaches a reference past the stopping point, which is exactly koji's
  `backup` role (granted only in a post-data `GRANT`). `ScanExtent::Full` (or
  a query after `pgdq parse`) gives the complete set.
- An `INSERT` run is folded into one `Data` span, but every line in it is
  still decoded into `Event::Line` and pushed through the statement
  accumulator — unlike the large-object region, which is skipped unread at
  the scanner level. Measured at **~209MB/s cold against ~1.10GB/s for a `COPY`
  dump of the same size on the same disk read page-cache warm**, i.e. about 5×
  the per-byte CPU. **The 5× is not a CPU ratio and is a loose floor**: it
  divides a *cold* `INSERT` rate, device included, by the `COPY` path's *warm*
  CPU — and no warm `INSERT` figure has ever been taken. Bounding it from the
  cold table alone puts the real per-byte ratio near **16–26×**, since the
  `COPY` side is now 0.57 s per 3.00 GiB rather than 2.92 s. `M17` measures the
  warm `INSERT` CPU. Every correction so far has made this path look worse, so
  the gap the fix addresses is larger than the figure says, never smaller. Figures and re-run commands in
  [`../design/measurements.md`](../design/measurements.md). A
  koji-scale 1TB `--inserts` dump therefore spends ~45 minutes of CPU that a
  `COPY` dump of the same size does not. Correctness is unaffected — the map, the tiling and the row counts
  are the same either way. Not scheduled: the fix is a scanner-level
  `INSERT` path, which changes a decision and so needs a slice, filed into
  [`roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md).

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed. **Nothing is
open.** `M17`'s harness half raised five on 2026-08-28 and the grilling the
same day closed all five; `M13`'s fold-in was reviewed on 2026-08-27, where two
of its three calls were reversed and its one new causal claim was tested rather
than argued.

*`M17`'s five were reviewed on 2026-08-28 and **all five stand**, three of them
with the reasoning sharpened.*

- *Sharing the **cold** throughput table's `COPY` row with the census table*,
  where the contract asked only for the warm one. It is one measurement; the
  sharing makes two numbers for it structurally impossible rather than
  something to check.
- *The prose recipes are deleted **per section**, with the table that replaces
  each one* — not all at once with the harness, which would have left the doc
  with neither for any section whose figure had not been emitted. The set that
  never goes is now **two**, not the contract's three: the census-off source
  patch and the generator invocations. `--koji-recipe` took the third.
- *The preamble prepass's invented table shape stands.* Its claim — an
  uncancellable region is milliseconds against a scan of seconds to an hour —
  is a ratio between two rows, which is what a table is for. It was prose only
  because nobody had decided how to report it, which is the oversight the emit
  contract's forcing function exists to catch.
- *`depends` stays narrow, and granularity is **not** the goal.* Replaying
  `--stale` against `M7`'s real commit flags 8 of 11 figures through `map.rs`,
  including the one figure that did move. Since any stale figure forces a whole
  re-sweep, the actionable answer is binary — re-take the doc or don't — and
  one true positive settles it. Per-figure attribution is explanatory colour,
  and an under-declared path costs a false negative only if *no* figure
  declares the changed file.
- *"Taken against a dirty tree" is that same predicate applied to `git
  status`*, so the two are consistent by construction rather than by
  agreement: an uncommitted doc or harness cannot move a reading and does not
  mark a sweep unpublishable.

*The census attribution was reviewed on 2026-08-27 and **tested rather than
argued**; it holds.* `M13`'s fold-in asserted in three docs that the
`--arrays --composite` file's `strings` leg is 24% above the control's because
the mapping pass's census splits its rows. The same query with the census-off
binary, five interleaved reps on both files
(`runs/m13-census-baseline.sh` → `.log`), inverts the gap from **+1.115 s to
−0.075 s** — the arrays file becoming slightly cheaper, as its 14% lower row
count should give. The census accounts for more than the whole gap, and its
cost measured through `query` reproduces the `parse` figures to within 3%,
which is a cross-check nobody asked for. All three docs now state it flatly.

*`M13`'s fold-in made three calls with no maintainer present; all three were
reviewed on 2026-08-27 and **two were reversed**.*

- *The deleted P7 inbox entry is **restored, with a different fact in it**.*
  "Cross-file differencing … is bias rather than noise" was written from the
  musl leg (composite column at −1.16 µs/row, impossible) and read as a
  structural confound; the glibc leg reverses the sign to +0.61 on the same
  inputs and reps, so deleting it was right and the rule folded in its place —
  "survive a change of allocator" — was too weak. The sharper fact is that
  **both legs are confident and they disagree by 1.77 µs/row**: t = +4.34 and
  t = −4.81 on the same quantity, against a same-shape floor that is not
  significant on either leg (t = +1.06, −0.31), and against 0.16 µs/row of
  drift between two stages of one sweep on the *identical* file. So per-rep SE
  measures the reps, not the measurement, and the campaign should quote ranges
  across apparatuses rather than confidence intervals from one. That is now
  its own entry in
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md).
- *The census sections' **cold** rows go into the sweep (`M14` at the time, now `M17`).* Dropping them left four
  claims about the cold regime (+1.2%, +2.6%) with no displayed measurement,
  which is worse than either keeping or deleting them outright. All four are
  now labelled as the pre-`M11` scalar-loop reading, and the sweep — which
  already stages the 3.00 GiB control cold in the same container — takes the
  census-off binary through two more runs per file and brings the rows back.
- *One sweep with no second session **stands**.* The re-take is a single
  session, but the second *apparatus* is the cross-check that matters and it
  exists: the musl leg agrees on every difference where the allocator cancels
  (census on array-bearing rows, Δ 1.27 s against 1.26 s) on absolute legs
  2.5× apart. Where the two legs disagree, the entry above says so rather than
  averaging them.

*`M13`'s quadratic table was going to run on the host to dodge container
startup; the maintainer reversed that on 2026-08-27** — startup must not reach
the figures, and the figures are still produced inside the container. The
sweep now times every stage with the container's own shell, which is a
standing rule in [`../design/measurements.md`](../design/measurements.md), and
the whole warm set was re-taken under it. **The finding behind it is not
confined to `M13`**: `nerdctl run` costs 0.77 s, so every container figure in
that doc taken by `/usr/bin/time` around it — the cold scan-throughput table
included — carries 0.77 s it should not.

*The quadratic table's "before" column was reviewed on 2026-08-27 and
**stands**, with what it is limited to now stated in the table.* It is
re-taken from a rebuilt `b726f6b` rather than carried over, so the table is one
apparatus throughout; the limit is that the two builds differ in everything
from 9.5 onward, not only in the throttle, so the column records what the
throttle era bought and may not be differenced against a later change.
Isolating a mechanism is what the census-off method is for.

*The max-RSS column was reviewed on 2026-08-27 and **deleted**.*
`/usr/bin/time -f %M` around `nerdctl run` reports the nerdctl client's peak,
not pgdq's: it read the same ~40–45 MB for a 2 MB input as for a 3.00 GiB one,
against ~9 MiB for koji's 784 GB scan in the same doc. What the apparatus can
honestly claim is that every run completes inside a 512 MB cgroup, and that is
what `measurements.md` now says. A real instrument — a `VmHWM` poller inside
the container — is a busy loop that would distort the timings it rode along
with, so it would have to be its own untimed stage, and nothing consumes a
number that sharp.

*4.6.1's re-taken table and the ratios that moved were reviewed on
2026-08-27, and the swing is **neutralized by construction rather than
investigated**.* Reconstructing both sessions' raw legs settled it: the shift
is common-mode within a file — `M10`'s `strings`/`typed` read 8.80/19.89 s
(control) and 8.04/27.74 s (nested) against this sweep's 9.74/20.55 and
9.56/29.45 — so the ratio moved because the drifting baseline sits in its
denominator, while the per-row differences the design consumes did not. Three
things changed instead of a diagnosis. The per-row difference is now the
headline of [`../design/measurements.md`](../design/measurements.md)'s nested
section and the ratio a derived column; two standing rules were added there —
a parsing-CPU figure is taken with its input on **tmpfs**, never page-cache
warm off a filesystem (device time and background I/O swamp the difference,
worst on the HDD, and page-cache residency is an assumption), and a comparison
table is re-taken **whole in one interleaved sweep**, never differenced across
sessions or run a file at a time. And `M10`'s explanation of its own 9%
between-file baseline gap — "the untyped path is partly per-row" — is
**retracted**, though the gap itself is real and now explained: `M13`'s sweep
puts the control and `--composite` legs 0.03% apart and the
`--arrays --composite` leg 24% above both, and a census-off run inverts that
gap to −0.075 s — so it is the mapping pass's array-shape census on the only
file whose rows carry a `{`, and in no reading a per-row property of the
untyped path. Both rules are also in
[`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
since that campaign is where they bite.

*The composite column's unmeasured end-to-end share was reviewed on
2026-08-27, and the tick **stands** — the promise is corrected, not the
measurement.* Nothing consumes a figure that sharp, and the bound already
answers the question the phase asked: which of the three nested columns is the
cost, and it is the arrays by an order of magnitude. The sharper instrument
stays unbuilt, with its real cost now named rather than left as "a scope
decision": a generator knob that declares `v_comp` as `text` makes the
generator write a declaration `pg_dump` would not — the opposite of what `M10`
corrected it to do — so it needs an explicit exemption from
`perf_generator_fidelity.rs`, not just a flag. What is fixed is the wording
that promised a share: 4.6.1's spec row in
[`../design/roadmap-P4-composite-decoding.md`](../design/roadmap-P4-composite-decoding.md)
now says the deliverable is whatever that instrument resolves, and the P7
inbox entry carries the exemption. **Generalized into
[`../design/roadmap.md`](../design/roadmap.md)'s standing rule** "A slice row
that commits to a measurement names its instrument", which now states that a
named instrument is satisfied by whatever it resolves, a bound included, and
that the instrument's floor is part of the deliverable — the row is unsatisfied
only if the instrument was not built or not run. That closes the opposite
failure to the one the rule was written for: an unattended session building an
unbudgeted second instrument to rescue a null reading.

*The census's 39% cost on brace-free rows was reviewed on 2026-08-27, and the
answer is to **make the pre-filter cheap, not to make it skippable**.* A knob
would re-introduce exactly what 4.5.1 deleted (`Builder::censusing`) and would
have to be plumbed to a caller who cannot know whether a later query will want
to retype an array column. But the 0.84 s per 3.00 GiB is a hand-rolled scalar
two-comparison byte loop at `map.rs:1269` running at ~3.8 GB/s, and `memchr`
is already a direct dependency (`pgdump_query/Cargo.toml:12`): `memchr2(b'{',
b'[', raw).is_some()` is a one-line, SIMD swap. Queued as out-of-band **M11**
below. The knob question was *deferred, not closed*, pending the re-take —
which came back at **45 ns a row, +7%**, so it is now closed: the census
stays unconditional, and the pre-filter is 2.5% of what it costs on the rows
it does not reject.

*`M10`'s date/time fractions were reviewed on 2026-08-27: the choice is
**wrong in principle, and the correction rides the next re-take** rather than
buying one.* The list was chosen for the rendered shapes it produces, which is
a **correctness** goal met elsewhere — `fixtures/` already carries all five
(`00:00:00`, `.000001`, `.123456`, `.85312` for the trim, `.999999`). Against
real data the list is badly unrepresentative: 300,000 rows of koji's
`task.create_time` hold **zero** values with no fractional part and 9.9%
ending in a zero digit, against the list's 37.5% and 50%. But the stake is
below the instrument: three of sixteen columns, ~7 bytes shorter per
no-fraction render, is 0.2% of a 3,943-byte row, and the decoder costs about
the same either way — while changing it re-takes five figures. So
`FRACTIONS` in `scripts/generate_perf_data.py` switches to a uniform
microsecond draw as **`M12`**, landing ahead of `M13`'s re-take so the figures
are taken on the final bytes; the comment that currently justifies the list
records at that point that shape coverage belongs to the fixtures.

*The census figures' source-edit recipe was reviewed on 2026-08-27 and
**stands**, with the recipe made discoverable from the code.* Regenerating
either figure means putting a bare `return;` at the top of
`map::Builder::on_row` and rebuilding, which isolates exactly the census but
breaks silently if that function is renamed, split, or loses the pre-filter —
and nothing in the source points back at the doc. `on_row` gains a doc comment
naming the two [`../design/measurements.md`](../design/measurements.md)
sections taken by patching it, the way
[`../design/architecture.md`](../design/architecture.md)'s "`Event` is the
scanner's contract" records a constraint at the site that would break it. It
lands with `M11`, which edits that function anyway. Still not a cargo feature:
the existing *Rejected:* paragraph holds, and a feature is a shipped knob for
a measurement.

*The scan-throughput table's whole re-take was reviewed on 2026-08-27 and
**needed no ruling*** — re-taking two rows the maintainer had not queued was
forced by the floor row being page-cache-contaminated, and correct
re-measurement is not a decision to weigh in on. The practice it exemplifies
is now two standing rules in
[`../design/measurements.md`](../design/measurements.md). Its one residual —
that the section cites the `COPY` path's warm CPU (2.92 s), a
page-cache-warm filesystem read the new tmpfs rule now excludes — is covered
by **`M13`**, which re-took the whole warm set on tmpfs in one session: the
`COPY` path's warm CPU is 0.57 s, not 2.92 s.

*The fidelity guard's `uv` skip was reviewed on 2026-08-27 and **reversed**.*
The skip guards an environment that does not exist: `mise.toml` pins exactly
one tool, `uv = "latest"`, and there is no CI, so the suite runs in one place
where `uv` is present — while covering the file's only test, so when it does
fire the suite goes green with its only generator guard entirely absent. Its
recorded reason, that such a checkout "should not report a failure it cannot
act on", is false here: the remedy is `mise install`, which is the point of
pinning tools with `mise` at all — a CI run is one step from having everything
the suite needs. `have_uv()`'s skip becomes an assertion naming that remedy,
riding with `M12`. Generalized into
[`../design/roadmap.md`](../design/roadmap.md)'s standing rules as "A test may
assume the tools `mise` pins", which also states the exception that gates the
other way: a machine-local resource `mise` cannot pin, per `CLAUDE.local.md`'s
koji-replica rule.


*4.6's generator-fidelity entry was reviewed on 2026-08-27, settled as
scheduled work with its scope corrected — the finding was three infidelities,
not one — and **landed the same day as `M10`**.*
`scripts/generate_perf_data.py` declared `time`/`timestamp`/`timestamptz`
where `pg_dump` writes the long spellings, never trimmed fractional seconds
where PostgreSQL does, and filled a `real` column with a float64 `repr()`.
The first two were **coupled** — correcting the spellings alone would have
sent three columns that never decoded through a decoder that re-renders them
differently from the file — and the third was what made `typed` and `strings`
disagree on this input (`v_bytea`, which looked like the culprit, is faithful;
`pg_dump` writes the doubled backslash too). The consequence was a validity
problem rather than a fidelity complaint: a benchmark for the typed path
measured 13 of 16 columns while its table said 16. The fix was not 4.6's — it
changes the default output's bytes and so re-takes the figures taken on them.
The findings and the coupling are in
[`history/2026-08-27.md`](history/2026-08-27.md), "The perf
generator is not the pg_dump shape it claims"; what landed is `M10` in
[`../design/roadmap.md`](../design/roadmap.md)'s out-of-band ledger.

*4.4.4's refusal order was reviewed on 2026-08-27 and **stands**, with its
recorded reason replaced.* It was queued as "kept as a counterfactual" — the
order is dead, since the opaque test matches a bare type name where the array
test matches that name with bounds appended, so no input reaches both. What
settles it is that **both refusals answer `Utf8View`**: the order can only ever
pick a *label*, never a type, so keeping it costs nothing and the hypothetical
defence it was written with ("what must win if the two ever overlap") is not
the argument. `resolve_array` and
[`../design/architecture.md`](../design/architecture.md), "Type resolution",
now say that instead, and both name the alternative — testing opaqueness
recursively through the element's own array levels, which would make an array
over a domain whose base is `box[]` answer `OpaqueElementType`. That is a
behaviour change buying a better diagnostic on a shape `pg_dump` cannot write
(I21), so it is not scheduled.

*M7's two entries were reviewed on 2026-08-27.* The **unconditional
absorption** **stands**, with its reason upgraded from "it would put an
unexplainable asymmetry between the `COPY` and `--inserts` paths" to a claim
about the only producer that can reach the shape: `pg_dump` cannot emit a
header-less comment block immediately before an `INSERT INTO`, but a
hand-written dump can, and there absorbing gives one `Data` span where gating
would give a `Framing` span plus a `Data` span — the same trade
`on_copy_start`'s `Mode::Comment` arm already makes. The symmetry is a
consequence of that call, not the argument for it; the durable half is in
[`../design/architecture.md`](../design/architecture.md), "Bulk regions: one
span kind, three payloads". The **corrected verification figure** needed no
ruling: 30/47, 30/47, 31/48 and 6/21 all reproduce on today's tree, so the
entry is simply closed. What the grilling turned up on the way is the
`--disable-triggers` gap below, and a **third** wrong reason recorded for M6's
asymmetry. Reasoning: [`history/2026-08-27.md`](history/2026-08-27.md).

*M6's boundary-signal asymmetry was reviewed on 2026-08-27 and **stands**, with
its recorded reason replaced.* The queued description of the
`TOC_PREFIX_STATS` drive-by was "one prefix plus the object-census kind". It
turned out to be two functions with **different** answers:
`parse_toc_header_line` reads all three of `_printTocEntry()`'s prefixes, while
the boundary signal `looks_like_toc_name_line` accepts `"Statistics for "` and
still refuses `"Data for "`. Grilled on 2026-08-27, and the first
reason recorded for it was wrong: it said a data entry heads a `COPY` block
that arrives as its own event, which is false under `--inserts`. The real
constraint is `Builder::on_copy_start` — it reads the pending `TocHeader` out
of its `Mode::Comment` arm, and its `Mode::Statement` arm pushes a separate
span and passes `None`, so accepting `"Data for "` here would make **every
`COPY` block in the file lose its TOC entry**. The asymmetry is forced, not
chosen. Pinned by `a_statistics_entry_is_one_attributed_span` and
`a_statistics_entry_parses_and_opens_a_span` in `map.rs`; the durable half is
in [`../design/architecture.md`](../design/architecture.md), "TOC enrichment".
What the grilling turned up on the way is the `--inserts` attribution gap,
closed the same day as **M7**. *M5's placement was reviewed the same day and
**stands**, also with a replaced reason*: not "the library error is shared"
(`require_enabled` has two call sites, both in `main.rs`) but that the remedy
interpolates the user's own `--source` path, which `Error::CacheDisabled` does
not have and should not take a `PathBuf` to get — unlike `Error::FieldDecode`,
which names a static flag string. Reasoning:
[`history/2026-08-27.md`](history/2026-08-27.md).

*9.5.1's three entries were reviewed on 2026-08-27.* The **prepass running
even when the cancel flag is already set** **stands**, now with numbers behind
"bounded by its own length": koji's preamble is 63,333 bytes of 784 GB, and the
most preamble-heavy shape available (4000 tables, 49% preamble by bytes) maps
in 0.04 s — both in [`../design/measurements.md`](../design/measurements.md),
"The preamble prepass is bounded by the schema". The **two
`MetadataNotScanned`s** are **both kept, and the record corrected**: grilling
found the entry's own claim false, since `stream::resolve_block` is private and
all three call sites read `metadata` after the mapping pass, so a carried
`ResumeToken` cannot reach it either — `Error::MetadataNotScanned` is
unreachable through every public entry point, while
`ColumnResolution::MetadataNotScanned` stays reachable because `resolve_columns`
is public. The guard is kept as what stands between a future reordering and a
wrongly-typed row, and is now pinned by a unit test in `stream.rs` rather than
left as untested defence. The **`pg_dumpall` fixture** entry is **reversed**:
`edge_cases/dumpall.sql` gains a second data-carrying database rather than
leaving the recurring boundary verified only against a hand-concatenated file
`pg_dump` never wrote — queued under "Not started". Reasoning:
[`history/2026-08-27.md`](history/2026-08-27.md).

*9.5's two-granularity guard entry was reviewed on 2026-08-27 and **stands**,
restated as a principle.* The amendment is right — chunk granularity alone
leaves a block-rich dump unresponsive for its whole scan — and the finding
underneath it was that a spec *enumerating* check points is what let the case
through. Both the spec and `architecture.md` now say the rule is "read the flag
at every point the loop can cheaply reach", with the two current sites as its
instances, and both record the two limits it does not remove: the flag is read
before `read_range`, not during it, and `scan::scan`/`scan_preamble` ignore it
on purpose, because a stop there could not be told from reaching the first
`COPY` header and would cache a truncated preamble as complete. The remote-I/O
half of that is filed into
[`../design/roadmap-P6-embeddable-engine-inbox.md`](../design/roadmap-P6-embeddable-engine-inbox.md).
Reasoning: [`history/2026-08-27.md`](history/2026-08-27.md).

*Three P9 entries were reviewed on 2026-08-26.* The **save-throttle**
entry is **reversed**: the quadratic regime was measured, it costs 44s on a
4000-block dump, and closing it is slice **9.5** rather than a P7
question. The **`parse` types every `\connect`ed database** entry is
**reversed in the direction of agreement**: the recomputation moves into
`map_forward` so a cold query and a warm one answer alike — an out-of-band
change, since the divergence it removes was never a decision anyone took. The
**coverage-line** entry **stands**: the text line prints unconditionally, since
its absence would leave a user unsure rather than reassured, and `--json`
carries the components without a rendered line, because a caller can divide.
Reasoning: [`history/2026-08-26.md`](history/2026-08-26.md).

*The three remaining P9 entries were reviewed on 2026-08-26 and all three
**stand**.* The diagnostics recompute is 3 ms for koji's 833-span cache — a
full `info --dqcache` run, load and render included — so the O(spans) cost is
below process startup; the figure is in
[`../design/architecture.md`](../design/architecture.md), "The cache".
`info --dqcache none` stays an error, with the message to name `pgdq parse
--dqcache <path>` as its remedy the way `Error::FieldDecode` names
`--schema-mode strings` (out-of-band, riding with 9.5). And `v_empty_enum`
needed no ruling at all: fixture expansion is a standing rule, so additions
under it stop being logged here — flagging each one dilutes a section meant for
decisions.

*4.4.3's normalization-placement entry was reviewed on 2026-08-26 and is
**settled by refactor**.* Grilling established that both placements produce
identical outcomes on every input — normalization can only add an array level,
and no normalized terminal is ever the literal name `box` or a `TypeDef.name` —
so the choice was free and the real finding was that two predicates walking the
domain chain separately is what made placement a question. Slice **4.4.4** folds
the array arm into one function; the reasoning is in
[`history/2026-08-26.md`](history/2026-08-26.md), and the policy it was decided
under is `roadmap.md`, "Refactor when the shape stops fitting".

*4.4.2's `integer[][]` entry was reversed by 4.4.3* — the spelling resolves as
`integer[]` does again, and the refusal it was flagging now has exactly one
DDL shape behind it (I26).

*4.5.1's two entries were reviewed on 2026-08-26.* The
`MAX_ARRAY_DIMS` verdict **stands** — a run past `MAXDIM` is not evidence, so
the column keeps its optimistic type and the row surfaces as a `FieldDecode`;
the durable half is in [`../design/architecture.md`](../design/architecture.md),
"The array shape census", and the scan-time diagnostic it does *not* raise is
now a roadmap "Future" item. The spec/manual divergence is **resolved by
amending the spec**: its "What the manual must say" section now describes one
path, matching the census section the same reversal rewrote. Reasoning:
[`history/2026-08-26.md`](history/2026-08-26.md).
