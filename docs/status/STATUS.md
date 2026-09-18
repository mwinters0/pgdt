# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

<!-- repointed: 94220cc --> The marker names the commit `scripts/repoint.py`
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
| Best-effort structural cache with source-identity checking and cache-only inspection | working; the library never replaces cache data automatically, a weak signal is advisory between runs unless `--strict-identity` binds it, and a source that changes under an in-flight read aborts a run that then saves and removes nothing | `cache.rs`; D18–D22; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--strict-identity`: when a moved file should stop the run" |
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
| Remote input (`--source https://…`), over `object_store` | working for a plain dump behind the default-off `http` feature: one ranged GET probes, one reader reads, no credential is sent and every other scheme is refused by name; a remote source is asked for `--dqcache`, a remote `.xz` is refused, and nothing yet catches a remote object rewritten under an in-flight read, all until later slices of P14, which is current ([spec](../design/roadmap-P14-remote-input.md)) | `io.rs` (`RemoteSource`, `Origin`); D6, D12, D14, D26; RT13–RT17; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Reading a dump over HTTP" |
| Python bindings, DataFusion `TableProvider` | not started; P6 | |
| Device-bound scan performance | settled; parallelism is filed beside its own mechanisms | D10, D29 |
| Per-row-group column statistics | working; `pgdq parse` and the library's `map_file` gather them by default, at any worker count, and persist them in the cache, re-reading a mapped block that lacks what is asked, and say on stderr how much memory they held, `info --detail` reports them per table and column and `--json` exports every group's, and a query — library and `pgdq query` — skips the row groups they rule out, and stops reading a block sorted past the filter's bound, saying after the fact what that left unread, unless told `--statistics none`; statistics stop where the account fills, so a long dump keeps a prefix (`KD33`); under an unstated group size a block's groups merge pairwise past `BLOCK_MAX_STATISTICS_GROUPS` and, once it is read, until its median group holds the density minimum, never past a stated maximum, which turns the cap off and re-reads a block once at the finer size its groups predict, each block recording the request that sized it; a block whose statistics the run's allowance cannot hold declines, says so, and is re-read only under a larger one | `statistics.rs`, `gather.rs`, `prune.rs`, `pgdump_query-cli/src/info_statistics.rs`; D19, D34, D54, D67, D75–D82, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics`: what `parse` records for later queries" |
| `--inserts` row reading; custom, directory and tar archives | not started; P8, and the map already locates `INSERT` runs (`KD9`) | D33 |

**Figures.** [`../design/measurements.md`](../design/measurements.md) carries
the session stamp and its own account of what stands outside it; `cd scripts
&& uv run measure.py --stale` names what is red and why. **Every register
figure is red**, the statistics work having moved many declared paths with no
sweep since. Four of them are held deliberately — `reserve`, `rss-attribution`,
`statistics-gathering` and `statistics-pruning`, whose legs resolve fewer
readers now that the margin binds a typed number, and which are re-taken once
P23 settles `MEMORY_RESERVE`. Two more can never be cleared by a sweep at all:
`session-drift`, whose apparatus is the harness itself and which only
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

## P14 progress

Slices of [`../design/roadmap-P14-remote-input.md`](../design/roadmap-P14-remote-input.md),
in order. Each lands its notes doc, its manual text and its register entries in
the same change ([`../process.md`](../process.md), "The loop"); the rows say only
what is delivered.

- [x] **14.1** Cancellation gains an awaitable form beside the polled flag, so a
  read in flight can be dropped rather than waited out (D16). No remote code;
  amends [`../design/decisions.md`](../design/decisions.md), "D26", which
  describes a polled flag.
  [notes](../design/roadmap-P14.1-cancellation-notes.md)
- [x] **14.2** The **origin**: the pre-source probe that answers stored size,
  weak identity and leading magic bytes before any source is built, `cache::claim`
  consuming it instead of a `&Path`, and recognition told what it holds rather
  than reading the path (D2). The local source is its only user.
  [notes](../design/roadmap-P14.2-origin-notes.md)
- [x] **14.3** In-flight identity on the local source: the check at every cache
  save and once at run end, `--strict-identity=time,location,none` with absence
  as a failure, and an abort that saves nothing and deletes nothing (D5,
  D10–D12). `location` is inert until 14.6 gives it an origin.
  [notes](../design/roadmap-P14.3-in-flight-identity-notes.md)
- [x] **14.4** The oracle: an HTTP server inside the test binary serving fixtures
  over loopback, with a knob per misbehaviour — range ignored, ETag changed
  mid-run, short read, mid-body failure, mid-scan 404 — and its own tests proving
  it misbehaves as asked (D6). No subject yet, deliberately.
  [notes](../design/roadmap-P14.4-oracle-notes.md)
- [x] **14.5** The remote source over a plain dump, end to end:
  `parse`/`query`/`info --source https://…`, the `http` feature and `cmake`
  pinned, URL and `file://` handling with credentials and non-HTTP schemes
  refused by name, the client defaults, the conservative trait answers, the
  public constructor, and network failure as an error naming the URL (D3, D7–D9,
  D14, D15, D17). The oracle gains an eighth knob, a stalled origin, and the
  client deadline and the cancellation race are asserted against it rather than
  left unproducible (D6, D9, D16). Adds the `RT<n>` entries its decisions depend
  on. A remote source is asked for `--dqcache` until 14.6 derives one, and a
  remote `.xz` is refused by name until 14.7.
  [notes](../design/roadmap-P14.5-remote-source-notes.md)
- [ ] **14.6** Remote identity and the remote cache: `SourceIdentity::Remote`,
  the URL-basename default cache path, the origin recorded and reported
  advisorily, the precondition on every ranged GET, and `CacheSourceMismatch`
  naming a source rather than a path (D4, D11, D18, D19).
- [ ] **14.7** Remote `.xz`: the composition, the cold footer walk announced at
  open with its remedies, a `KD<k>` for it owned by the phase that tunes the
  network, and the vetting `xz-seek`'s publication was gated on (D1, D13).

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P14 is open** — remote input over HTTP, its spec
  [`../design/roadmap-P14-remote-input.md`](../design/roadmap-P14-remote-input.md)
  and its checklist above. What statistics may hold resident is bounded and their
  coverage is not (`KD33`, `KD34`), both owned by P23, whose sketch in
  [`../design/roadmap.md`](../design/roadmap.md), "P23 — Statistics coverage
  and the resident reserve" holds what it inherits. Every built mechanism — the
  parallel scan's defaults included: a worker count the source recommends, a
  memory limit discovered and filled under the reserve and the margin, and a
  `parse` saying what it delivered rather than what it was asked for — is in
  [`../design/decisions.md`](../design/decisions.md).
  Seven phases remain sketched — P22, P21, P23, P6, P15, P18, P8, in the
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
rather than being deleted. <!-- deficiency-watermark: KD34 -->
**`KD1`–`KD34` are allocated, and nothing at or below `KD34` is reused** — a
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
  D83 leaves at `DEFAULT_MEMORY_BUDGET` capped by the allowance on a plain
  source whatever `--memory`
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

- **KD31** — `attach_text` caps a run's one read at `SPAN_STORED_TEXT_MAX_BYTES` per span from
  the run's start, so a span following one longer than the cap can be stored
  empty and `truncated` however short it is. **(c) unowned**; promoted by a
  `--map` listing seen to lose a statement's text. Detail:
  `pgdump_query/src/map.rs`.

- **KD33** — statistics stop where the account fills: nothing releases
  `Term::Retained` while the pass gathers forward, so a dump long enough fills
  its allowance partway through and every block after it declines, leaving statistics a prefix of the
  file and a query pruning nothing over the tail. How much is covered depends on
  the allowance the box resolved, which a user cannot predict. **(b) owned by
  P23**, whose remedy is a granularity derived from the dump's length. Detail:
  `pgdump_query/src/gather.rs`.

- **KD34** — `MEMORY_RESERVE`'s 384 MiB does not cover what a run holds above
  its charge and its statistics account: the attribution sitting read a worst
  remainder of 544 MiB on a compressed `query`, and every `wide-xz24` `query`
  leg from 1 GiB up was OOM-killed in every rep on that build. The reserve was
  fixed before statistics existed and has not been read since. **(b) owned by
  P23**, which sets it from those readings and runs the blind gate an
  attribution cannot stand in for. Detail: `pgdump_query/src/io.rs`.

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

- **`Origin::remote_with_read_timeout` is public so that the client deadline can
  be asserted.** `REMOTE_READ_TIMEOUT` is 30 s and the retry schedule around it
  runs to three minutes, so a test at the shipped value cannot produce the
  deadline; 14.5's row asks for it to be asserted rather than left
  unproducible, and a second constructor is what makes the stalled origin
  observable in a fifth of a second. The alternatives were a shipped value
  chosen to be testable, or no assertion. Reconsidering means either dropping
  the constructor and the assertion together, or accepting it as surface P6
  inherits ([`../design/roadmap-P14-remote-input.md`](../design/roadmap-P14-remote-input.md),
  "D9" and "D14").

- **A cancelled remote read ends the run as an error, not as an interrupted
  scan.** The cancellation reaches the source as
  `ByteRangeSource::hint_cancellation`, announced beside the other hints, and a
  dropped request comes back as `Error::ScanCancelled`; `pgdq parse` translates
  that into the interrupted report and a 128+signal exit, so the user sees what
  a local Ctrl-C shows. What it is *not* is `MapStop::Interrupted`, so the run
  makes no final save — the cache holds the scan to its last throttled one. The
  alternative teaches every read loop to read a cancelled read as a graceful
  stop, which is a rework of tested core paths an unattended session should not
  make on a judgement call ([`../process.md`](../process.md), "Working
  unattended"). Reconsidering moves the translation from the CLI into the loops
  and gives a remote `parse` the same final save a local one gets.
