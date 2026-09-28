# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; where what is built falls short is
[`deficiencies.md`](deficiencies.md), the register every `KD<k>` below
resolves in; dated pickup notes and plan-changing discoveries are in
`history/`.

<!-- repointed: a5c23d5 --> The marker names the commit `scripts/repoint.py`
measures the record's growth from; red means a repoint is due
([`../process.md`](../process.md), "Repointing").

## What exists

The phases [`../design/roadmap.md`](../design/roadmap.md)'s index calls
`Struck` are complete and went at keystone reviews; the code is how each
mechanism works, and
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
| Structural cache with source-identity checking and cache-only inspection | working; a cache that cannot be used — another file's, another build's, damaged, or not a pgdt cache — is refused before the dump is read past its first bytes, and `--overwrite-unusable-cache` replaces any but the last; a weak signal — an mtime, or a server's `Last-Modified` and `ETag`, and where a source was fetched from — is advisory between runs unless `--strict-identity` binds the term, and a source that changes under an in-flight read aborts a run that then saves and removes nothing, unless `--strict-identity=none` | `cache.rs`; D18–D22; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--strict-identity`: when a moved file should stop the run" and "When `info` says it cannot answer" |
| Arrays, composites, ranges, multiranges, `int2vector` | typed, decoded and compared structurally; two shapes stay text (`KD3`) and an array inside a composite is decided optimistically (`KD2`) | `nested.rs`, `pgtype.rs`; D39, D41, D45, D58 |
| Array shape census | recorded by every mapping pass and read back before a query's first batch | `map.rs`; D35, D43 |
| CLI `pgdt parse` / `info` / `query`, with `--map`, `--json`, `--detail` and cache-only `info` | working; `parse` scans ahead, resumes, and saves on Ctrl-C; `info` never scans; `query` reads partitioned and prints file order | `pgdt/src/main.rs`; D61–D67; [`../manual/dump-inspection.md`](../manual/dump-inspection.md) |
| Partial reporting | `info` reports an unfinished scan's cache with its completion stated once at the top; an interrupted cache is typed for every database segment the scan finished (I1) | D67 |
| Column projection | working, library and CLI; an unprojected column is never decoded unless a filter term names it | `batch.rs`; D28; [`../manual/type-handling.md`](../manual/type-handling.md) |
| The filter expression, three-valued | working; `Expr` is one tree evaluated in SQL's `True`/`False`/`Unknown` domain, its `IN` a leaf answered per row by one lookup, reached as `--where` and as repeated `--filter` | `predicate.rs`; D53, D54 |
| The `--where` and `--filter` grammars | working, CLI only; nothing below L4 parses a term | `pgdt/src/where_expr.rs`, `main.rs`; D60; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`" and "Writing a filter term" |
| Typed comparison: `=`/`!=` and the four ordering operators | working, library and CLI; equality falls back to text where the register gives no comparison and is refused only where the file says the server's is not a text comparison (a range declaring `canonical`), ordering is refused where the register gives no order, and a special value is a rank rather than a fault; a query may compare as DataFusion compares the emitted value instead, which refuses a nested column and orders one with no plan bytewise | `predicate.rs`, `pgtype.rs`; D40, D55–D58; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings" |
| The comparison register and the declared collation | L2, `comparison_for` in `pgtype.rs`: one `ComparisonPlan` per column, divergence announced on its own channel per term, or per column in a query's semantics; a stated collation this build does not implement compares bytewise (`KD7`) and an unmodelled scalar's equality is a guess (`KD10`) | D40, D59; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise" |
| Comparison oracle, cross-major differ, register-to-oracle reconciliation | committed under `fixtures/<major>/oracle/` and checked by `predicate.rs`'s unit test, `scripts/oracle_differences.py` and `scripts/oracle_register.py` | D70, D71 |
| ADBC floor oracle and the floor rule | committed under `fixtures/<major>/adbc/` and reconciled by `scripts/floor_mapping.py` | D38 |
| Compressed input (`--source foo.dump.xz`) | working serially and at any `--jobs`; the seek table is cached; a file the budget cannot block-decode streams and says so; gzip, zstd and lz4 are not read (P15, P18) | `io.rs` (`XzSource`), `cache.rs`; D14–D19; [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md) |
| Partitioned replay and the interior split | working; `query` is the replay's consumer and the mapping pass the split's, and each answers what the serial path answers; a sub-stream can pin more than the budget bills (`KD23`) and a window over small blocks over-reads (`KD22`) | `stream.rs`, `leader.rs`; D5, D7, D8, D48, D51, D52 |
| Caller-stated parallelism and its resident allowance | working end to end; an omitted `--jobs` asks the source and an omitted `--memory` the discovered limit, a stated one carved exactly as a discovered limit is; a cache's statistics are held off the margin before the workers of a pass holding them are carved — not a `query`'s replay, which holds none — and what the workers leave under it is what a gathering pass's statistics may hold, and a block that does not fit declines and is re-read only under a larger allowance; what is announced is corrected where it is not delivered; the charge is short on three named terms (`KD21`, `KD24`, `KD26`) and describes nothing held on a fourth (`KD25`), a stated allowance cannot raise a plain source's read budget (`KD32`) and a plain typed `query` does not scale (`KD17`) | `io.rs` (`Parallelism`, `WorkerMemory`, `statistics_allowance`), `gather.rs`; D1–D5, D9–D13, D64, D83, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and `--memory`" |
| Measurement harness | `scripts/measure.py` takes every figure and emits the tables; every rule it enforces is asserted in `scripts/test_measure.py` | [`../design/measurements.md`](../design/measurements.md), "The apparatus" |
| Fixtures | real `pg_dump` output at six majors, regenerated by `scripts/generate_fixtures.py` | D69, D73 |
| Remote input (`--source https://…`), over `object_store` | working behind the default-off `http` feature: one ranged GET probes, one reader reads, no credential is sent, `file:` resolves to a local path and every other scheme is refused by name; the cache is named after the URL's last segment in the working directory and records the origin it was written for, and every ranged GET after the probe pins the object to the version the probe saw, unless `--strict-identity=none` unbinds it — its entity tag, or its modification time where it stated no tag or a weak one — so a rewrite mid-scan is refused by the server, and a run over a server stating neither is refused before it reads unless `--strict-identity=none`; a fetched `.xz` is read block by block out of windows we fetch, and its cold footer walk is announced rather than refused (`KD36`); where the budget cannot hold one decoded block the fetched arm keeps the window and the handle together, so a block a scan sits inside is fetched and decoded once | `io.rs` (`RemoteSource`, `FetchedXzSource`, `walk_seek_table`, `Origin`), `cache.rs` (`OriginMatch`); D6, D12, D14, D15, D18, D26; RT13–RT18; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Reading a dump over HTTP" |
| DataFusion `TableProvider` | a library, `datafusion-pgdump`: a dump opened through its complete cache, under the `StrictIdentity` its caller states for it — `STORED AS PGDUMP`'s `pgdump.strict_identity`, or the factory's where a statement states none — and registered as one catalog per database, or one table alone; a scan is the partitioned replay over the held map, projected by name, batched at the session's size and parallel to its `target_partitions` within a budget every scan of the session draws on, under the `pgdump.*` settings a `SET` states — an allowance over the discovered one, a read chunk, a line limit and whether a dynamic filter's rows are evaluated — its count held under a margin that a finite memory-pool limit and each registered dump's resident statistics are billed against, and its budget where the count cannot fall; a filter is pushed `Exact` where the library answers it as DataFusion does and left to DataFusion elsewhere, and a join's, a TopK's or an ungrouped `MIN`/`MAX`'s dynamic filter cuts the sub-streams over the row groups it keeps when the first is polled, bar where that would lose a declared ordering (`KD53`), and is read as each runs, skipping the groups it rules out as the replay reaches them and ending a sorted block past a bound it requires, and, where that setting is on — it is off by default — dropping each row it rejects before decoding it, in every block; the scan's plan node carries the table's rows exactly and each column's NULLs, and its extremes where the map's statistics reach them, an enum's in its labels' text order, a float's zero the one `total_cmp` picks, its distinct values where every group kept a dictionary of the text it emits — a `character` column never — and an integer, `oid` or `Decimal128` column's sum where every group kept one, wrapped as DataFusion's `SUM` wraps, so `COUNT(*)`, `COUNT(<column>)`, `MIN`, `MAX`, `SUM` and a `COUNT(DISTINCT <column>)` beside another count can answer without a row being read — and without raising what reading a column holding a value its Arrow type cannot represent would (`KD8`), bar a sum, which such a column keeps none of — and each column's Arrow bytes but a typed nested one's — a fixed-width one's and a boolean's from its rows, an enum's from both, and a text or `bytea` one's, or any read as text, from its values' text — so a join weighs its sides by bytes; a filter, or a fetch shorter than the table, makes every one of them an estimate, a pushed filter's rows and bytes bounded by the row groups its pruning kept; a column every partition is proved to emit in Arrow's order — each block recorded sorted alike, no NULL, each block boundary a partition crosses proved from adjacent groups' bounds — is declared that ordering, NULLs where an unqualified `ORDER BY` puts them, so a sort it satisfies is not planned; what a registration finds — the file's findings, each column's resolution and its divergence from PostgreSQL, and each table whose plan refuses, which is not listed — goes to a sink the caller supplies, which then hears what each scan's plan settles when it is planned — a note quoting a budget wrapped with what that budget was carved from and the settings that move it — while the groups a scan's statistics pruned, the groups and rows a dynamic filter pruned and the bytes an early stop left unread are its plan node's metrics; `CREATE EXTERNAL TABLE … STORED AS PGDUMP` registers one table | `datafusion-pgdump/src/`, `stream.rs` (`TablePartitions`), `summary.rs`; D40, D51, D88–D91, D93 |
| SQL shell, `datafusion-cli-pgdump` | working: `datafusion-cli` 55.1.0 with `--dump [NAME=]SOURCE[:strings][:strict-identity=TERMS]`, `STORED AS PGDUMP`, `SET pgdump.*` and `pgdt`'s `--strict-identity` as the default a dump's own overrides, printing each registration's warnings to stderr, and each scan's at planning; it never parses | `datafusion-cli-pgdump/src/`; D90; [`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md) |
| Python bindings | not started; P24 | |
| Device-bound scan performance | settled; parallelism is filed beside its own mechanisms | D10, D29 |
| Per-row-group column statistics | working; `pgdt parse` and the library's `map_file` gather them by default, at any worker count, and persist them in the cache, re-reading a mapped block that lacks what is asked, and say on stderr how much memory they held, `info --detail` reports them per table and column and `--json` exports every group's, and a query — library and `pgdt query` — skips the row groups they rule out, and stops reading a block sorted past the filter's bound, saying after the fact what that left unread, unless told `--statistics none`; statistics stop where the account fills, so a long dump keeps a prefix (`KD33`); under an unstated group size a block's groups merge pairwise past `BLOCK_MAX_ROW_GROUPS` and, once it is read, until its median group holds the density minimum, never past a stated maximum, which turns the cap off and re-reads a block at the finer size its groups predict — a block another request sized being first re-read at the default, so twice in one run — each block recording the request that sized it; a block whose statistics the run's allowance cannot hold declines, says so, and is re-read only under a larger one | `statistics.rs`, `gather.rs`, `prune.rs`, `pgdt/src/info_statistics.rs`; D19, D34, D54, D67, D75–D82, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics`: what `parse` records for later queries" |
| `--inserts` row reading; custom, directory and tar archives | not started; P8, and the map already locates `INSERT` runs (`KD9`) | D33 |

**Figures.** [`../design/measurements.md`](../design/measurements.md) carries
the session stamp and its own account of what stands outside it; `cd scripts
&& uv run measure.py --stale` names what is red and why. **Every register
figure was re-taken at `542fdfb`**, the four P23 had held included: three
resolve no reader count the margin could lower, and `reserve`'s stated axis,
which does, is a reading `KD34` names. Two can never be cleared by a sweep at all:
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

## P27 progress

Spec: [`../design/roadmap-P27-dynamic-filters.md`](../design/roadmap-P27-dynamic-filters.md).

- [x] **27.1** The evidence, no product code: the flags-on-against-off result-equality harness over every filter shape the spec's "Evidence" names, an aggregate's included; `measure.py` timing `datafusion-cli-pgdump`, and `dynamic-filter-join`, `dynamic-filter-topk` and the costing input taken on today's code; [notes](../design/roadmap-P27.1-evidence-notes.md)
- [x] **27.2** Receiving and translating: `PgDumpExec` holds every pushed dynamic filter from any producer, visits it in `apply_expressions`, resets it in `reset_state` and prints it in EXPLAIN; the physical-to-library translator, with a generated check that it only loosens; nothing pruned yet; [notes](../design/roadmap-P27.2-receiving-notes.md)
- [x] **27.2.1** A dynamic filter's float comparison with a zero of either sign has no term, the translator keeping every row the filter's producer keeps by its own order and not only every row DataFusion's evaluation of the filter keeps; [notes](../design/roadmap-P27.2.1-float-zero-notes.md)
- [x] **27.3** The library's dynamic-filter trait on the partitioned replay: re-pruning between groups on a moved generation, the early stop at a dynamic bound, and the scan's new metrics; [notes](../design/roadmap-P27.3-re-pruning-notes.md)
- [x] **27.4** The byte cut made once at the first poll, over the groups the static and dynamic filters keep, planning staying at `scan()`, tested to leave a clustered selective join's sub-streams byte-balanced over the groups the dynamic filter keeps; [notes](../design/roadmap-P27.4-first-poll-cut-notes.md)
- [x] **27.5** Row-level evaluation before decode, in every block, its state read at each group entered and each chunk taken, the figures re-taken and what they say filed against "D53"'s **Reopens**, which a set-membership term answers; [notes](../design/roadmap-P27.5-row-level-notes.md)
- [x] **27.6** The set-membership term, `Expr::In`, answering exactly as the `Or` of `=` it replaces by a generated check, and `pgdt --where`'s `in (…)`; [notes](../design/roadmap-P27.6-membership-notes.md)
- [x] **27.7** Both DataFusion translators emit `Expr::In` for `IN`; `dynamic-filter-join` and `dynamic-filter-topk` re-taken and their readings filed; [notes](../design/roadmap-P27.7-translators-notes.md)
- [x] **27.8** The per-row account of the costing input, no product code: its on leg's cost a row attributed among evaluating a row at all, each leaf's decode, each comparison and the `IN` lookup, by a `perf` profile and a per-term reading from an `introspect` build, summing to the measured Δ within its spread, what each shape of removing duplicate decoding saves, and the mechanisms beyond it named; [notes](../design/roadmap-P27.8-per-row-account-notes.md)
- [x] **27.9** The bounds an `IN` implies dropped from row evaluation and the byte cut keying each group's bounds once, every figure the change could move re-taken, and the spec's criterion applied: it fails on the costing input, so row evaluation goes off, delivered by 27.10; [notes](../design/roadmap-P27.9-implied-bounds-notes.md)
- [x] **27.10** The `pgdump.*` switch for row evaluation, taken since 27.9 turned it off: the setting off by default, its manual entry, "D93" rewritten, and the result-equality harness run with it on; [notes](../design/roadmap-P27.10-switch-notes.md)
- [ ] **27.11** The figures price the switch: `dynamic-filter-join` and `dynamic-filter-topk` timing the filter off, on at the default and on with `pgdump.dynamic_filter_rows` set, `--profile-recipe`'s costing pair the default against the rows leg, "D93" rewritten to read that pair, and both figures re-taken

## Not started

- **A CLI-feedback pass** — the `pgdt info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P27 is open**, its checklist above. A dump is readable
  over HTTP, plain and `.xz`, with nothing about the network's speed priced
  (`KD35`, `KD36`). What statistics may hold resident is bounded and their
  coverage is not (`KD33`, `KD34`), both owned by P23, whose sketch in
  [`../design/roadmap.md`](../design/roadmap.md), "P23 — Statistics coverage
  and the resident reserve" holds what it inherits. Every other built
  mechanism — the parallel scan's defaults included: a worker count the
  source recommends, a memory limit discovered and filled under the reserve
  and the margin, and a `parse` saying what it delivered rather than what it
  was asked for — is in [`../design/decisions.md`](../design/decisions.md).
  Nine phases remain sketched — P28, P22, P21, P23, P26, P15, P18, P8, P24,
  in the roadmap table's schedule order; a `P<k>` is an identifier, so the numbers say
  nothing about the order they run in. Each gets its own full grilling when it
  becomes current, and every one that carries an inbox must have it drained as
  part of that grilling.

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

- **`M178`'s pinned `datafusion-cli-pgdump` leg runs 6 runtime workers
  against the unpinned leg's 24.** The call: `measure.stated_threads` places
  a dynamic-filter leg by what it states — `target_partitions` at
  `SWEEP_JOBS` — so it is pinned to one L3 group, and `#[tokio::main]` then
  sizes its runtime off the cpuset (`nproc` 6 in its image). Taken because the
  admitting entry names the dynamic-filter figures as experiment legs and
  places by thread count, and those legs state one. But the same entry keeps
  "every leg discovering its count" unpinned, and the runtime's size is
  discovered, so the two arms of those legs differ by a thread pool as well as
  a placement. Reconsidering means one of: state `TOKIO_WORKER_THREADS` on
  every run of the second program so both arms start the same pool, which
  changes the unpinned leg's shape too; or keep those legs unpinned, leaving
  the experiment's CPU-bound legs to `pgdt`. Either lands before the first
  sitting, which none has yet.
