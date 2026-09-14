# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

<!-- repointed: 70d479f --> The marker names the commit `scripts/repoint.py`
measures the record's growth from; red means a repoint is due
([`../process.md`](../process.md), "Repointing").

## What exists

P1–P5, P7, P9, P11–P13, P16, P17 and P19 are complete and were struck at keystone
reviews; the code is how each mechanism works, and
[`../design/decisions.md`](../design/decisions.md) is where a session touching
one meets its rejected alternatives. Each row below names the modules, the
`D<k>` entries and the manual section that hold a capability, and nothing here
quotes a number: every figure is in
[`../design/measurements.md`](../design/measurements.md) under its own id.

| Capability | State | Where |
|---|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working | `stream.rs`, `batch.rs`; D46–D50 |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working; `money` stays text by decision (`KD13`), and a typed column cannot hold a special value (`KD8`) | `pgtype.rs`, `resolve.rs`, `decode.rs`; D37–D44; [`../manual/type-handling.md`](../manual/type-handling.md) |
| Full byte-exact file map, every byte in exactly one span, verified over every fixture | working | `map.rs`; D30–D33 |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working; a `--disable-triggers` dump loses data-span attribution (`KD1`) | `map.rs`, `preamble.rs`; D31, D36 |
| Best-effort structural cache with source-identity checking and cache-only inspection | working; the library never replaces cache data automatically | `cache.rs`; D18–D22 |
| Arrays, composites, ranges, multiranges, `int2vector` | typed, decoded and compared structurally; two shapes stay text (`KD3`) and an array inside a composite is decided optimistically (`KD2`) | `nested.rs`, `pgtype.rs`; D39, D41, D45, D58 |
| Array shape census | recorded by every mapping pass and read back before a query's first batch | `map.rs`; D35, D43 |
| CLI `pgdq parse` / `info` / `query`, with `--map`, `--json`, `--detail` and cache-only `info` | working; `parse` is the only scanner, resumes, and saves on Ctrl-C; `info` never scans; `query` reads partitioned and prints file order | `pgdump_query-cli/src/main.rs`; D61–D67; [`../manual/dump-inspection.md`](../manual/dump-inspection.md) |
| Partial reporting | `info` reports an unfinished scan's cache with its completion stated once at the top; an interrupted cache is typed for every database segment the scan finished (I1) | D67 |
| Column projection | working, library and CLI; an unprojected column is never decoded | `batch.rs`; D28; [`../manual/type-handling.md`](../manual/type-handling.md) |
| The filter expression, three-valued | working; `Expr` is one tree evaluated in SQL's `True`/`False`/`Unknown` domain, reached as `--where` and as repeated `--filter` | `predicate.rs`; D53, D54 |
| The `--where` and `--filter` grammars | working, CLI only; nothing below L4 parses a term | `pgdump_query-cli/src/where_expr.rs`, `main.rs`; D60; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`" and "Writing a filter term" |
| Typed comparison: `=`/`!=` and the four ordering operators | working, library and CLI; equality is never refused and falls back to text, ordering is refused where the register gives no order, and a special value is a rank rather than a fault | `predicate.rs`, `pgtype.rs`; D55–D58; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings" |
| The comparison register and the declared collation | L2, `comparison_for` in `pgtype.rs`: one `ComparisonPlan` per column, divergence announced per term on its own channel; a stated collation this build does not implement compares bytewise (`KD7`) and an unmodelled scalar's equality is a guess (`KD10`) | D40, D59; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise" |
| Comparison oracle, cross-major differ, register-to-oracle reconciliation | committed under `fixtures/<major>/oracle/` and checked by `predicate.rs`'s unit test, `scripts/oracle_differences.py` and `scripts/oracle_register.py` | D70, D71 |
| ADBC floor oracle and the floor rule | committed under `fixtures/<major>/adbc/` and reconciled by `scripts/floor_mapping.py` | D38, D72 |
| Compressed input (`--source foo.dump.xz`) | working serially and at any `--jobs`; the seek table is cached; a file the budget cannot block-decode streams and says so; gzip, zstd and lz4 are not read (P15, P18) | `io.rs` (`XzSource`), `cache.rs`; D14–D19; [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md) |
| Partitioned replay and the interior split | working; `query` is the replay's consumer and the mapping pass the split's, and each answers what the serial path answers; a sub-stream can pin more than the budget bills (`KD23`) and a window over small blocks over-reads (`KD22`) | `stream.rs`, `leader.rs`; D5, D7, D8, D48, D51, D52 |
| Caller-stated parallelism and its memory budget | working end to end; an omitted `--jobs` asks the source and an omitted `--parallel-memory` the discovered limit; what is announced is corrected where it is not delivered; the charge is short on four named terms (`KD21`, `KD24`–`KD26`) and a plain typed `query` does not scale (`KD17`) | `io.rs` (`Parallelism`, `WorkerMemory`); D1–D5, D9–D13, D64; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and `--parallel-memory`" |
| Measurement harness | `scripts/measure.py` takes every figure and emits the tables; every rule it enforces is asserted in `scripts/test_measure.py` | [`../design/measurements.md`](../design/measurements.md), "The apparatus" |
| Fixtures | real `pg_dump` output at six majors, regenerated by `scripts/generate_fixtures.py` | D69, D73 |
| Remote input (`--source https://…`), over `object_store` | not started; P14 | D6 |
| Python bindings, DataFusion `TableProvider` | not started; P6 | |
| Device-bound scan performance | complete (P7); parallelism is filed beside its own mechanisms | D10, D29 |
| Per-row-group column statistics | in progress (P10); `CopyBlock::sparse_index` and `column_stats` are reserved `None` | D34 |
| `--inserts` row reading; custom, directory and tar archives | not started; P8, and the map already locates `INSERT` runs (`KD9`) | D33 |

**Figures.** [`../design/measurements.md`](../design/measurements.md) carries
the session stamp and its own account of what stands outside it; `cd scripts
&& uv run measure.py --stale` names what is red and why. Two things are red on
purpose: `session-drift`, whose apparatus is the harness itself and which only
a second sweep run back to back with a first re-takes, and the koji section,
outside the register, which only a run on the HDD clears. Red with the reason
written down is the standing requirement, not red cleared
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

**Profiles and heap recordings are not figures.** `measure.py --profile-recipe`
and `--heaptrack-recipe` print an instrument sequence and run none of it; what
either produces is a `runs/` artifact with no median, no apparatus gate and no
marker ([`../design/measurements.md`](../design/measurements.md), "What an
instrument can see").

## P10 progress

Spec: [`../design/roadmap-P10-row-group-statistics.md`](../design/roadmap-P10-row-group-statistics.md).

- [ ] **10.1** Fixture shapes — sorted, reversed, unsorted, constant and all-null columns; float specials; `numeric` `1.5`/`1.50`; `C` and default-collated text; low- and over-64-cardinality columns; a value past `N` and the read chunk — and `--max-line-bytes` on `parse` and `query`
- [ ] **10.2** Row-free comparison and plan-time filter resolution, behaviour-preserving
- [ ] **10.3** The truth-set evaluator, property-tested against the row evaluator
- [ ] **10.4** Serial gathering and persistence: the L1 observer, the statistics types, shared ownership, `SparseRowIndex` struck, `FORMAT_VERSION` and the golden-order test, `parse --statistics` default on and `--statistics-group-size`, the leader declining while statistics are requested, existing figures on `--statistics none`, coarse reserve and resident bumps
- [ ] **10.5** Reporting in `info --detail` and `--json`
- [ ] **10.6** Parallel gathering, identical to serial over every fixture
- [ ] **10.7** Back-fill of blocks lacking the requested statistics
- [ ] **10.8** The pruning consumer: segment gaps, the `PlanNote`, `query --statistics none`, the generated pruned-equals-unpruned check
- [ ] **10.9** Early stop on a column sorted over its block
- [ ] **10.10** Figures `statistics-gathering` and `statistics-pruning`

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P10 is open**; see "P10 progress" above. Every built mechanism — the
  parallel scan's defaults included: a worker count the source recommends, a memory limit discovered
  and filled under the reserve and the margin, and a `parse` saying
  what it delivered rather than what it was asked for — is in
  [`../design/decisions.md`](../design/decisions.md).
  Six phases remain sketched — P20, P14, P6, P15, P18, P8, in the roadmap
  table's schedule order; a `P<k>` is an identifier, so the numbers say nothing
  about the order they run in. Each gets its own full grilling when it becomes
  current, and every one that carries an inbox must have it drained as part of
  that grilling.

## Known deficiencies

The deficiency register. Every known deficiency carries a stable `KD<k>`,
allocated on discovery and never reused, and **one line here**: what it costs,
its stance, and the `.rs` file carrying its `deficiency: KD<k>` marker. **The
comment at that marker is the detail** — it sits on the mechanism, so the
session editing the mechanism reads it without being sent anywhere. This is an
index, not the document.

Three stances, because these are not one kind of thing and the difference
decides whether anyone should act. **(a)** a consequence of a deliberate
tradeoff, never to be worked. **(b)** a defect with a known fix and a named
destination. **(c)** a defect with a known fix and no owner — a legitimate
resting state, said in those words, naming whatever would promote it. A
limitation whose remedy the user already has today is not here at all: it is a
property of how the system works, and it lives beside its mechanism with no
identifier.

A coverage statement is not a deficiency:
[`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)'s
`Unsupported` and `Untested` rows are scope and evidence, and earn a `KD<k>`
only by naming one.

An entry is struck by the change that closes its last part, not at a phase
boundary, and a part closing into a *property* migrates beside its mechanism
rather than being deleted. <!-- deficiency-watermark: KD26 -->
**`KD1`–`KD26` are allocated, and nothing at or below `KD26` is reused** — a
number the index below does not carry is a struck entry, not a typo. That
watermark is what keeps a `KD<k>` in an old commit message resolvable, and the
marker beside it is what a citation resolves against; the names of the struck
entries went at the keystone, `git log` being what answers *when*.

Where a `(b)` entry's owning phase has been sliced, the entry names the slice
and the slice names the entry, so landing one re-reads the other and a re-slice
is obliged to re-target. The two directions are asymmetric: a checklist line is
a **record**, so its `KD<k>` is a citation that may name a struck entry and may
not name a number nobody allocated; an entry is **present tense**, so it may
name only live slices, and naming a ticked one is an error — closing a part
rewrites the entry in the change that ticks the box. A `(b)` stance also needs
its destination to exist: an entry owned by a phase the roadmap's index calls
`Complete` or `Struck`, or does not list, drops to `(c) unowned` unless a phase
actually absorbs it.

`cd scripts && uv run deficiencies.py` reconciles this index against the code
markers — one apiece, in the file the entry names, and none anywhere under
`docs/` — against the slice checklist above and against the roadmap's phase
index, and fails on any of them. An entry owned by a phase with no checklist
yet names no slice and is not asked to. That last read is pinned at both ends:
a phase carrying a checklist is `Current` in the index and a `Current` phase
carries one, so a wrap that dropped the checklist and left the state, or a
slicing that wrote the checklist and left it, fails here rather than reading as
a phase nobody has sliced.

- **KD1** — a `--disable-triggers` dump loses TOC attribution on every data
  span, `COPY` and `INSERT` alike (I31), costing the coverage diagnostic and
  `Span::toc`. **(c) unowned**; promoted by a dump in hand whose data spans
  need attribution. Detail: `pgdump_query/src/map.rs`.

- **KD2** — an array nested inside a composite is decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode`. **(c) unowned**; promoted by a schema that holds one,
  the per-path census being deferred on frequency. Detail:
  `pgdump_query/src/resolve.rs`.

- **KD3** — two array shapes come back as text with no way to ask for more,
  `NestedArrayElement` and `VaryingArrayShape`, though both are fully
  understood. **(c) unowned**; promoted by a caller whose arrays are matrices
  or scientific data, for whom a string is the wrong answer. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD4** — a type name that needs quoting resolves `Unknown` (I29): a weaker
  type, never a wrong one. **(c) unowned**; promoted by a dump whose type names
  are not ordinary identifiers, which neither any fixture nor koji is. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD5** — a map rebuild is still a whole-list clone, so mapping is O(blocks²)
  wherever the save throttle's gate does not close it — which is every
  `--dqcache none` scan, since a no-op save leaves nothing to amortize: 19.3 s
  for 4000 blocks. **(c) unowned**; promoted by a dump with thousands of blocks
  scanned under `--dqcache none`. Detail: `pgdump_query/src/stream.rs`.

- **KD6** — a conflicting table past a query's stopping point is never seen, so
  `Error::AmbiguousTable` is not raised for it and the query returns the
  candidate it found. **(b) owned by P6**, where what the embedded API promises
  is decided. Detail: `pgdump_query/src/batch.rs`.

- **KD7** — a column that *states* a collation this build does not implement is
  compared bytewise, so the row set is not the server's: under `<`/`>` always,
  and under `=`/`!=` where the dump declares it `deterministic = false` (I42);
  the fix is a comparison per named collation, up to a provider version. **(c)
  unowned**; promoted by [`../design/roadmap.md`](../design/roadmap.md)'s
  Future item "collation-aware comparison", intent without a phase. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD8** — a typed column cannot hold `infinity`, `-infinity` or `NaN`, nor —
  on an `interval` — a time part past `2562047:47:16.854775807`, so
  materializing one raises `Error::FieldDecode` and there is no typed way to
  read the value. **(c) unowned**; promoted by whichever phase takes typed
  materialization, which is where the choice between a null, a sentinel and the
  error belongs. Detail: `pgdump_query/src/decode.rs`.

- **KD9** — an `INSERT` run costs **4.9×** a `COPY` scan's per-byte CPU warm
  and **2.63×** the device's own time cold on NVMe against 1.06×, and two cuts
  against that remainder are known and untaken. **(b) owned by P8**, whose
  Track A row reader extends the very scan both cuts are in; the cold-NVMe
  figure confirmed the entry where it might have retired it. Detail:
  `pgdump_query/src/preamble.rs`.

- **KD10** — a column whose declared type this build models no comparison for
  answers `=`/`!=` bytewise, which is not the server's answer for the geometric
  types (`box_eq` compares areas), so the row set is wrong; ordering is refused
  outright, and the announcement misses a type reached through a container
  (`box[]`). **(c) unowned**; promoted by a dump whose queried columns are
  geometric or hold a `money`-shaped extension type. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD13** — `money` is below the ADBC floor: the driver answers `int64` and we
  answer `Utf8View`, because `cash_out` renders through the monetary locale and
  `pg_dump` sets `lc_monetary` nowhere, so the file cannot say which locale
  wrote a value. **(a) deliberate tradeoff** — closing it means guessing a
  locale or asking for one, which the bar refuses for every other type. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD17** — a plain typed `query` is flat at 1.02× across the whole `--jobs`
  axis: the sub-streams it plans never run concurrently, total CPU staying
  under one core. The named suspect — `POOL_DEPTH` clamping the chunk pool —
  moved no cell by more than 0.8% in a probe build that lifts it, so what
  serializes them is unidentified. **(c) unowned**; promoted by a phase that
  takes up plain-source extraction throughput, since no defaults change reaches
  it. Detail: `pgdump_query/src/stream.rs`.

- **KD20** — a block-decoding worker decodes its **successor's block as well as
  its own**, nothing sharing the two, so a parallel compressed scan does about
  twice the decode work and its speedup is capped near half the reader count.
  **(c) unowned**; promoted by a phase taking up compressed scan throughput,
  and the fix left is an in-flight map, a wider cut having been measured and
  refused. Detail: `pgdump_query/src/io.rs`.

- **KD21** — the block pool's slot ceiling follows the worker count a caller
  *announced* rather than the one delivered, so a `--jobs` well above what
  `--parallel-memory` affords holds more than the stated budget — 1,280 MiB
  against 1,024 at `--jobs 24 --parallel-memory 1g` on a 128 MiB-block file.
  **(c) unowned**; promoted by a phase reworking the block pool's sizing rule.
  Detail: `pgdump_query/src/io.rs`.

- **KD22** — the leader cuts a window of `workers × partition_bytes` from a
  `COPY` block's start and drains every piece before merging, so a dump of
  blocks much smaller than that window is read and parsed two orders of
  magnitude over, worse at every worker added and reached with no flag typed.
  **(c) unowned**; promoted by a phase taking up leader scheduling. Detail:
  `pgdump_query/src/leader.rs`.

- **KD23** — a `pgdq query` sub-stream can pin several decoded blocks where the
  budget bills one: a query partition is cut over a whole `CopyBlock` rather
  than through the leader's window, so `BOUNDARIED_PARTITION_UNITS` does not
  bound it and a held batch's `max_source_span` reaches up to four of koji's 24
  MiB blocks. **(c) unowned**; promoted by a phase that takes up query-path
  memory, the repair reversing a recorded decision either way. Detail:
  `pgdump_query/src/stream.rs`.

- **KD24** — the chunk pool's free list is billed nowhere, so a compressed
  source's charge is short by `⌊budget/chunk⌋.clamp(1, POOL_DEPTH)` chunks — 4
  MiB at the shipped chunk, 64 MiB against 16 billed at `--chunk-size 16m`,
  flat in the count and never above the stated budget. **(c) unowned**;
  promoted by a caller announcing a large chunk, or by a phase reworking
  `WorkerMemory`, which has no count-independent term to bill it with. Detail:
  `pgdump_query/src/io.rs`.

- **KD25** — the plain source bills `PLAIN_PARTITION_CHUNKS × chunk` a reader
  where the path holds `POOL_DEPTH` chunks flat, and recommends no count at
  all, so plain readers are bounded by a charge describing nothing held — 8 MiB
  billed against 4 held at the shipped chunk, and unbounded above a budget of
  `8 MiB × jobs`. **(c) unowned**; promoted by a reading of a parallel plain
  scan on a real device, which is that path's own reopening condition. Detail:
  `pgdump_query/src/io.rs`.

- **KD26** — no charge bills a compressed source's seek table, the only
  unbilled term in the account that grows with the file rather than the count —
  3.33 MiB on koji's download against 4.11 KiB on the fixtures the bound was
  read off. **(c) unowned**; promoted by a source whose index is not small
  beside `MEMORY_UNPOOLED_BOUND`, or by a phase reworking `WorkerMemory`, which
  has no per-source term to bill it with. Detail: `pgdump_query/src/io.rs`.

- **KD14** — peak resident set is flat in dump bytes but grows ~9.9 KB per
  table, three fifths of it live structure the preamble alone pays, so a
  4,000-table `parse` holds **44.2 MiB** against a one-block one's 6.2 MiB.
  **(c) unowned**; promoted by a dump with tens of thousands of tables, nothing
  in hand being one. Detail: `pgdump_query/src/preamble.rs`.

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

- **Whether `pgdq` should cap its own glibc arenas.** It does not:
  `MALLOC_ARENA_MAX` is the operator's setting, and nothing in the binary calls
  `mallopt(M_ARENA_MAX, …)`. The refusal's recorded reason was mechanical — the
  arenas exist before the resolved worker count is known, so a cap set then
  removes none — and it assumed runtime worker threads that allocate at
  startup. Under the CLI's `current_thread` runtime the readers are
  blocking-pool threads spawned on demand after the arrangement is resolved, so a
  cap set at resolution would bound the arenas of every thread that decodes, and
  that reason no longer holds. **The decision to make:** keep the refusal on a
  reason that survives (the `allocator` figure is taken uncapped; a cap trades
  resident bytes for allocator contention on exactly the parallel shapes), or
  grill an in-binary cap keyed to the resolved count. Reconsidering it changes
  the manual's `MALLOC_ARENA_MAX` advice and what the `reserve` figure's arena
  legs are for. Found by the keystone review that struck P19
  ([`history/2026-09-13.md`](history/2026-09-13.md), "P19 is struck").
