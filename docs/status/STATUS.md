# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

<!-- repointed: 3b61c04 --> The marker names the commit `scripts/repoint.py`
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
| CLI `pgdq parse` / `info` / `query`, with `--map`, `--json`, `--detail` and cache-only `info` | working; `parse` scans ahead, resumes, and saves on Ctrl-C; `info` never scans; `query` reads partitioned and prints file order | `pgdump_query-cli/src/main.rs`; D61–D67; [`../manual/dump-inspection.md`](../manual/dump-inspection.md) |
| Partial reporting | `info` reports an unfinished scan's cache with its completion stated once at the top; an interrupted cache is typed for every database segment the scan finished (I1) | D67 |
| Column projection | working, library and CLI; an unprojected column is never decoded unless a filter term names it | `batch.rs`; D28; [`../manual/type-handling.md`](../manual/type-handling.md) |
| The filter expression, three-valued | working; `Expr` is one tree evaluated in SQL's `True`/`False`/`Unknown` domain, reached as `--where` and as repeated `--filter` | `predicate.rs`; D53, D54 |
| The `--where` and `--filter` grammars | working, CLI only; nothing below L4 parses a term | `pgdump_query-cli/src/where_expr.rs`, `main.rs`; D60; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`" and "Writing a filter term" |
| Typed comparison: `=`/`!=` and the four ordering operators | working, library and CLI; equality falls back to text where the register gives no comparison and is refused only where the file says the server's is not a text comparison (a range declaring `canonical`), ordering is refused where the register gives no order, and a special value is a rank rather than a fault | `predicate.rs`, `pgtype.rs`; D55–D58; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings" |
| The comparison register and the declared collation | L2, `comparison_for` in `pgtype.rs`: one `ComparisonPlan` per column, divergence announced per term on its own channel; a stated collation this build does not implement compares bytewise (`KD7`) and an unmodelled scalar's equality is a guess (`KD10`) | D40, D59; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise" |
| Comparison oracle, cross-major differ, register-to-oracle reconciliation | committed under `fixtures/<major>/oracle/` and checked by `predicate.rs`'s unit test, `scripts/oracle_differences.py` and `scripts/oracle_register.py` | D70, D71 |
| ADBC floor oracle and the floor rule | committed under `fixtures/<major>/adbc/` and reconciled by `scripts/floor_mapping.py` | D38 |
| Compressed input (`--source foo.dump.xz`) | working serially and at any `--jobs`; the seek table is cached; a file the budget cannot block-decode streams and says so; gzip, zstd and lz4 are not read (P15, P18) | `io.rs` (`XzSource`), `cache.rs`; D14–D19; [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md) |
| Partitioned replay and the interior split | working; `query` is the replay's consumer and the mapping pass the split's, and each answers what the serial path answers; a sub-stream can pin more than the budget bills (`KD23`) and a window over small blocks over-reads (`KD22`) | `stream.rs`, `leader.rs`; D5, D7, D8, D48, D51, D52 |
| Caller-stated parallelism and its resident allowance | working end to end; an omitted `--jobs` asks the source and an omitted `--memory` the discovered limit, a stated one carved exactly as a discovered limit is; what the workers leave under the margin is what a gathering pass's statistics may hold, and a block that does not fit declines and is re-read only under a larger allowance; what is announced is corrected where it is not delivered; the charge is short on four named terms (`KD21`, `KD24`–`KD26`), a stated allowance is inert on a plain source (`KD32`) and a plain typed `query` does not scale (`KD17`) | `io.rs` (`Parallelism`, `WorkerMemory`, `statistics_allowance`), `gather.rs`; D1–D5, D9–D13, D64, D83, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and `--memory`" |
| Measurement harness | `scripts/measure.py` takes every figure and emits the tables; every rule it enforces is asserted in `scripts/test_measure.py` | [`../design/measurements.md`](../design/measurements.md), "The apparatus" |
| Fixtures | real `pg_dump` output at six majors, regenerated by `scripts/generate_fixtures.py` | D69, D73 |
| Remote input (`--source https://…`), over `object_store` | not started; P14 | D6 |
| Python bindings, DataFusion `TableProvider` | not started; P6 | |
| Device-bound scan performance | settled; parallelism is filed beside its own mechanisms | D10, D29 |
| Per-row-group column statistics | working; `pgdq parse` and the library's `map_file` gather them by default, at any worker count, and persist them in the cache, re-reading a mapped block that lacks what is asked, and say on stderr how much memory they held, `info --detail` reports them per table and column and `--json` exports every group's, and a query — library and `pgdq query` — skips the row groups they rule out, and stops reading a block sorted past the filter's bound, saying after the fact what that left unread, unless told `--statistics none`; under an unstated group size a block's groups merge pairwise past `STATISTICS_GROUP_CAP` and, once it is read, until its median group holds the density minimum, never past a stated maximum, which turns the cap off and re-reads a block once at the finer size its groups predict, each block recording the request that sized it; a block whose statistics the run's allowance cannot hold declines, says so, and is re-read only under a larger one | `statistics.rs`, `gather.rs`, `prune.rs`, `pgdump_query-cli/src/info_statistics.rs`; D19, D34, D54, D67, D75–D82, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics`: what `parse` records for later queries" |
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

## P20 progress

Spec: [`../design/roadmap-P20-statistics-memory.md`](../design/roadmap-P20-statistics-memory.md).

- [x] **20.1** The statistics account — every statistic alive counted, retained, loaded, a parallel window's partitions and a dictionary's second copy — an observer's growth charged whenever it passes a step, a vector or map ahead of its growth, and a fold moving a piece's charge onto its block in one update; printed by every `parse` holding statistics as one status line with the total, the peak and each term's peak, a `query` printing none; reconciled against the `introspect` build's live heap over generated reasonable-width, wide-text and long distinct-text shapes, two-sided within a step an open observer plus the longest row; declining nothing; [notes](../design/roadmap-P20.1-statistics-account-notes.md)
- [x] **20.11** The cache file streamed at both ends: a save encodes into the file through a buffered writer and a load decodes through a buffered reader, the bytes written identical, so neither holds the serialized cache beside the statistics it carries; a save written beside the cache and renamed over it, a kill mid-save leaving the previous cache whole; the account's rustdoc no longer names either term; [notes](../design/roadmap-P20.11-streamed-cache-notes.md)
- [x] **20.2** Koji's row density: a gathering `parse` of koji on the shipped build tracking one narrow column per table, launched detached; a `scripts/` tool with its tests deriving each block's rows-per-group distribution at every `2^n` from `info --json`, over koji and the fixtures, into a `runs/` artifact; the median quantile checked against the spec's registered criterion, which koji does not meet, so the median stands; [notes](../design/roadmap-P20.2-row-density-notes.md)
- [x] **20.3** The per-block length cap: groups under an unstated group size merge pairwise past a judgement constant, a stated `--statistics-group-size` honoured exactly, parallel identical to serial over every fixture; [notes](../design/roadmap-P20.3-length-cap-notes.md)
- [x] **20.4** The density minimum: a block's size chosen at its end from rows per group at the median, default minimum 1,024, the cap's size standing where coarser; `--statistics-min-rows` (0 turns it off), a stated `--statistics-group-size` exact, a power of two and refused beside either row flag; the bounds recorded per block with D34's back-fill rule extended to each; `FORMAT_VERSION` bumped; parallel identical to serial over every fixture; [notes](../design/roadmap-P20.4-density-minimum-notes.md)
- [x] **20.4.1** The minimum's median at the upper middle group, `floor(G/2)+1`-th smallest: the predicate monotone in size, so a block the length cap coarsened reaches no size coarser than the guarantee allows, tested on the odd-tail shape and on a capped block that reached the minimum finer; `row_density.py`'s `choose` and its tests moved with it, 20.2's koji reading re-derived from `koji-info.json`, which moved no verdict; `FORMAT_VERSION` bumped, a cache written at 19 holding sizes this rule would not choose; [notes](../design/roadmap-P20.4.1-upper-middle-group-notes.md)
- [x] **20.5** The stated maximum: `--statistics-max-rows` at the 90th percentile, honoured over the length cap, winning where a block meets neither bound, refused below the minimum; a block whose `2^20` groups break it re-read by the back-fill at the size its rows per group predict, once, saying where it still misses; `FORMAT_VERSION` bumped; [notes](../design/roadmap-P20.5-stated-maximum-notes.md)
- [x] **20.6** `--memory` as the resident allowance, replacing `--parallel-memory`, and the library's `Parallelism` memory carved the same way — `Parallelism::within` is the one carving and `discover_in` calls it; the two-tunable rule's allowlist test, closed three ways and pinning the hardware pair; `MEMORY_RESERVE`'s value unmoved; [notes](../design/roadmap-P20.6-memory-allowance-notes.md)
- [x] **20.7** The decline: the statistics allowance carved after the workers, half of `MemAvailable` where no limit is found; a block that does not fit declines and says so, recorded in the cache with its allowance, back-fill retrying only under a larger one; the margin left against the account, closing `KD28`; [notes](../design/roadmap-P20.7-decline-notes.md)
- [ ] **20.8** The reserve measured: the `introspect` build attributes the remainder above the charge and the statistics account over flagless gathering `parse` and `query`-over-its-cache legs — reasonable-width and allowance-filling wide-text inputs, plain and 24/128 MiB-block `.xz`, across the `reserve` figure's limits; `MEMORY_RESERVE` set to the smallest step whose worst rep leaves the margin, a remainder growing with statistics billed to the query instead, then one blind sitting on the shipped build at that value
- [ ] **20.9** The generated gates in 512m on the shipped build — a wide-text input that must decline and a reasonable-width one that declines nothing — and the re-taken figures at 20.8's reserve: `reserve`, `statistics-gathering` in 512m, and every figure whose arrangement moves with the reserve; `parallel-*`'s two `.xz` legs move in `parallel-peak-rss` alone, their count unchanged and their announced budget not, while its plain typed-`query` leg has an axis again since `M111` and is re-taken with `QUERY_SUBSTREAM_CAP`'s annotation, and its plain `parse` leg, which `M111` does not reach, still resolves the same eight readers at every row above eight and is redesigned rather than re-taken
- [ ] **20.10** Koji: a flagless gathering `parse` in 512m beside the `none` recipe, and one `query` over its cache, launched detached and read by a later session — no large block declines, peak leaves the margin, wall clock against the `none` run; a cap it refutes earns 20.10.1

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P20 is open**; see "P20 progress" above. Every built mechanism — the
  parallel scan's defaults included: a worker count the source recommends, a
  memory limit discovered and filled under the reserve and the margin, and a
  `parse` saying what it delivered rather than what it was asked for — is in
  [`../design/decisions.md`](../design/decisions.md).
  Six phases remain sketched — P21, P14, P6, P15, P18, P8, in the
  roadmap table's schedule order; a `P<k>` is an identifier, so the numbers say
  nothing about the order they run in. Each gets its own full grilling when it
  becomes current, and every one that carries an inbox must have it drained as
  part of that grilling.

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
rather than being deleted. <!-- deficiency-watermark: KD32 -->
**`KD1`–`KD32` are allocated, and nothing at or below `KD32` is reused** — a
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
  *announced* rather than the one delivered, so a `--jobs` well above what the
  read-buffer budget affords holds more than that budget — 1,280 MiB against
  1,024 at `--jobs 24` and a 1 GiB budget on a 128 MiB-block file.
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
  flat in the count and never above the stated budget unless one chunk is. **(c) unowned**;
  promoted by a caller announcing a large chunk, or by a phase reworking
  `WorkerMemory`, which has no count-independent term to bill it with. Detail:
  `pgdump_query/src/io.rs`.

- **KD25** — the plain source bills `PLAIN_PARTITION_CHUNKS × chunk` a reader
  where the path holds `POOL_DEPTH` chunks flat, and recommends no per-reader
  memory at all, so plain readers are bounded by a charge describing nothing held — 8 MiB
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

- **KD32** — `max_source_span` is solved against the read-buffer budget, which
  D83 leaves at `DEFAULT_MEMORY_BUDGET` on a plain source whatever `--memory`
  states, so a plain `query`'s sub-stream count and batch size are fixed at the
  number D3 picked to decline block decode on an ordinary `.xz`: a stated
  allowance moves neither, and no flag does. **(c) unowned**; promoted with
  `KD25` by a reading of a parallel plain scan on a real device. Detail:
  `pgdump_query/src/stream.rs`.

- **KD14** — peak resident set is flat in dump bytes but grows ~9.9 KB per
  table, three fifths of it live structure the preamble alone pays, so a
  4,000-table `parse` holds **44.2 MiB** against a one-block one's 6.2 MiB.
  **(c) unowned**; promoted by a dump with tens of thousands of tables, nothing
  in hand being one. Detail: `pgdump_query/src/preamble.rs`.

- **KD29** — a flagless `pgdq` run reads its memory limit twice, once for the
  status lines and the statistics allowance and again inside
  `Parallelism::discover_in` for the read-buffer budget, so a limit rewritten
  between the two is announced and carved for statistics as one number while
  the pools are budgeted from another. **(c) unowned**; promoted by a limit
  seen to move inside a run, or by discovery that can take a limit already
  read. Detail: `pgdump_query-cli/src/main.rs`.

- **KD30** — a cache from a build whose persisted shape changed is decoded
  whole before its version is read, so it almost always reads as not a pgdq
  cache rather than as another build's, and `info` sends the user to check the
  path. **(c) unowned**; promoted by a user misled by it, the fix being the
  version read first. Detail: `pgdump_query/src/cache.rs`.

- **KD31** — `attach_text` caps a run's one read at `TEXT_CAP` per span from
  the run's start, so a span following one longer than the cap can be stored
  empty and `truncated` however short it is. **(c) unowned**; promoted by a
  `--map` listing seen to lose a statement's text. Detail:
  `pgdump_query/src/map.rs`.

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

- **The statistics allowance is `margin_allowance(allowance) − budget`, with
  `MEMORY_RESERVE` deliberately not subtracted a second time** (20.7,
  `io::statistics_allowance`). The spec says only "what that arrangement leaves
  under the margin"; the reserve carves the *cap* a worker count is solved
  against, while the margin ceiling already stands `MEMORY_UNPOOLED_BOUND`
  below its fraction of the allowance, so taking both would apply
  `MEMORY_MARGIN_PERCENT` twice — which `margin_allowance`'s own rustdoc
  refuses for the same reason. **What reconsidering changes**: the number a
  gathering `parse` may hold at the reference allocation, and therefore whether
  koji's flagless gather fits it — which is 20.10's criterion and the one thing
  that confirms or moves `STATISTICS_GROUP_CAP`. The phase's own arithmetic
  (["Length is bounded by a constant, per
  block"](../design/roadmap-P20-statistics-memory.md)) puts koji's statistics
  at the cap within a small factor of what this formula leaves there, so the
  margin between "fits" and "declines" is thin and the formula is what sets it.
  A second consequence, from the same two constants: below `5 ×
  (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` the allowance is zero and every
  block declines, so no `parse` under that allocation gathers anything.

