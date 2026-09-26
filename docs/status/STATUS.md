# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; where what is built falls short is
[`deficiencies.md`](deficiencies.md), the register every `KD<k>` below
resolves in; dated pickup notes and plan-changing discoveries are in
`history/`.

<!-- repointed: 3528b07 --> The marker names the commit `scripts/repoint.py`
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
| Best-effort structural cache with source-identity checking and cache-only inspection | working; a cache that cannot be used — another file's, another build's, damaged, or not a pgdt cache — is refused before the dump is read, and `--overwrite-unusable-cache` replaces any but the last; a weak signal — an mtime, or a server's `Last-Modified` and `ETag`, and where a source was fetched from — is advisory between runs unless `--strict-identity` binds the term, and a source that changes under an in-flight read aborts a run that then saves and removes nothing, unless `--strict-identity=none` | `cache.rs`; D18–D22; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--strict-identity`: when a moved file should stop the run" and "When `info` says it cannot answer" |
| Arrays, composites, ranges, multiranges, `int2vector` | typed, decoded and compared structurally; two shapes stay text (`KD3`) and an array inside a composite is decided optimistically (`KD2`) | `nested.rs`, `pgtype.rs`; D39, D41, D45, D58 |
| Array shape census | recorded by every mapping pass and read back before a query's first batch | `map.rs`; D35, D43 |
| CLI `pgdt parse` / `info` / `query`, with `--map`, `--json`, `--detail` and cache-only `info` | working; `parse` scans ahead, resumes, and saves on Ctrl-C; `info` never scans; `query` reads partitioned and prints file order | `pgdt/src/main.rs`; D61–D67; [`../manual/dump-inspection.md`](../manual/dump-inspection.md) |
| Partial reporting | `info` reports an unfinished scan's cache with its completion stated once at the top; an interrupted cache is typed for every database segment the scan finished (I1) | D67 |
| Column projection | working, library and CLI; an unprojected column is never decoded unless a filter term names it | `batch.rs`; D28; [`../manual/type-handling.md`](../manual/type-handling.md) |
| The filter expression, three-valued | working; `Expr` is one tree evaluated in SQL's `True`/`False`/`Unknown` domain, reached as `--where` and as repeated `--filter` | `predicate.rs`; D53, D54 |
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
| DataFusion `TableProvider` | a library, `datafusion-pgdump`: a dump opened through its complete cache, under the `StrictIdentity` its caller states for it — `STORED AS PGDUMP`'s `pgdump.strict_identity`, or the factory's where a statement states none — and registered as one catalog per database, or one table alone; a scan is the partitioned replay over the held map, projected by name, batched at the session's size and parallel to its `target_partitions` within a budget every scan of the session draws on, under the `pgdump.*` settings a `SET` states — an allowance over the discovered one, a read chunk and a line limit — its count held under a margin that a finite memory-pool limit and each registered dump's resident statistics are billed against, and its budget where the count cannot fall; a filter is pushed `Exact` where the library answers it as DataFusion does and left to DataFusion elsewhere; the scan's plan node carries the table's rows exactly and each column's NULLs, and its extremes where the map's statistics reach them but an enum's (`KD45`), a float's zero the one `total_cmp` picks, its distinct values where every group kept a dictionary of the text it emits — a `character` column never — and an integer, `oid` or `Decimal128` column's sum where every group kept one, wrapped as DataFusion's `SUM` wraps, so `COUNT(*)`, `COUNT(<column>)`, `MIN`, `MAX`, `SUM` and a `COUNT(DISTINCT <column>)` beside another count can answer without a row being read — and without raising what reading a column holding a value its Arrow type cannot represent would (`KD8`), bar a sum, which such a column keeps none of — and each column's Arrow bytes but a typed nested one's — a fixed-width one's and a boolean's from its rows, an enum's from both, and a text or `bytea` one's, or any read as text, from its values' text — so a join weighs its sides by bytes; a filter, or a fetch shorter than the table, makes every one of them an estimate, a pushed filter's rows and bytes bounded by the row groups its pruning kept; a column every partition is proved to emit in Arrow's order — each block recorded sorted alike, no NULL, each block boundary a partition crosses proved from adjacent groups' bounds — is declared that ordering, NULLs where an unqualified `ORDER BY` puts them, so a sort it satisfies is not planned; what a registration finds — the file's findings, each column's resolution and its divergence from PostgreSQL, and each table whose plan refuses, which is not listed — goes to a sink the caller supplies, which then hears what each scan's plan settles when it is planned — a note quoting a budget wrapped with what that budget was carved from and the settings that move it — while the groups a scan's statistics pruned and the bytes an early stop left unread are its plan node's metrics; `CREATE EXTERNAL TABLE … STORED AS PGDUMP` registers one table | `datafusion-pgdump/src/`, `stream.rs` (`TablePartitions`), `summary.rs`; D40, D51, D88–D91 |
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

## Not started

- **A CLI-feedback pass** — the `pgdt info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **No phase is open.** A dump is readable
  over HTTP, plain and `.xz`, with nothing about the network's speed priced
  (`KD35`, `KD36`). What statistics may hold resident is bounded and their
  coverage is not (`KD33`, `KD34`), both owned by P23, whose sketch in
  [`../design/roadmap.md`](../design/roadmap.md), "P23 — Statistics coverage
  and the resident reserve" holds what it inherits. Every other built
  mechanism — the parallel scan's defaults included: a worker count the
  source recommends, a memory limit discovered and filled under the reserve
  and the margin, and a `parse` saying what it delivered rather than what it
  was asked for — is in [`../design/decisions.md`](../design/decisions.md).
  Nine phases remain sketched — P27, P22, P21, P23, P26, P15, P18, P8, P24,
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

- **No dump can be looser than its session and keep its in-flight check.**
  `M170`'s per-dump strictness reads `StrictIdentity`'s `FromStr`, whose
  grammar is `time`, `location`, both, or `none`, and has no spelling for
  `ADVISORY` — `pgdt` reaches it by omitting the flag, and an empty value is
  refused. So under `--strict-identity=time` a dump can state `location`,
  `time` or `none`, but not "nothing weak binds, in-flight still checked";
  the workaround is the inverse, an advisory session with the strict dumps
  naming `time`. Built as the row reads rather than widening the grammar
  `pgdt` shares. Reconsidering means a word for it (`advisory`, say) in
  `FromStr`, which `pgdt --strict-identity=advisory` would then accept too.
