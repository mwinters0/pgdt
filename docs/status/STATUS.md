# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; where what is built falls short is
[`deficiencies.md`](deficiencies.md), the register every `KD<k>` below
resolves in; dated pickup notes and plan-changing discoveries are in
`history/`.

<!-- repointed: a766233 --> The marker names the commit `scripts/repoint.py`
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
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working; `money` stays text by decision (`KD13`), and a value a typed column cannot hold reads as NULL by default, or its column as its text, the rest typed, or, told to refuse, refuses a query materializing its column at planning | `pgtype.rs`, `resolve.rs`, `decode.rs`; D37–D44; [`../manual/type-handling.md`](../manual/type-handling.md) |
| Full byte-exact file map, every byte in exactly one span, verified over every fixture | working | `map.rs`; D30–D33 |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working; a `--disable-triggers` dump loses data-span attribution (`KD1`) | `map.rs`, `preamble.rs`; D31, D36 |
| Structural cache with source-identity checking and cache-only inspection | working; a cache that cannot be used — another file's, another build's, damaged, or not a pgdt cache — is refused before the dump is read past its first bytes, and `--overwrite-unusable-cache` replaces any but the last; a weak signal — an mtime, or a server's `Last-Modified` and `ETag`, and where a source was fetched from — is advisory between runs unless `--strict-identity` binds the term, and a source that changes under an in-flight read aborts a run that then saves and removes nothing, unless `--strict-identity=none` | `cache.rs`; D18–D22; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--strict-identity`: when a moved file should stop the run" and "When `info` says it cannot answer" |
| Arrays, composites, ranges, multiranges, `int2vector` | typed, decoded and compared structurally; two shapes stay text (`KD3`) and an array inside a composite is decided optimistically (`KD2`) | `nested.rs`, `pgtype.rs`; D39, D41, D45, D58 |
| Array shape census | recorded at the data level by every mapping pass and read back before a query's first batch; a table `parse` recorded at the metadata level holds none, and a query of it reads the table again for one, writing nothing | `map.rs`, `stream.rs`; D35, D43 |
| Unrepresentable count | beside the census, per block and column, in two tiers — past Arrow's format spec, and past the calendar DataFusion displays through — under a calendar the cache records and a build ending elsewhere refuses; shown by `info --detail`; statistics count it per group and keep each view of a group's values apart where it differs — the values the type holds, every value, and those within the calendar; a query reads such a value as NULL for every purpose — its batch, its filters, its pruning and the provider's statistics — in the tiers its front end cannot hold, and says per column how many, unless told to read such a column as its text, compared bytewise by DataFusion and in its declared type's order by `pgdt`, or to refuse, when a query materializing such a column refuses at planning, naming it and how many, whatever its filter keeps (`--unrepresentable`, `PgDumpOptions::unrepresentable`, `pgdump.unrepresentable`, `:unrepresentable=`); in every mode a filter term tells such a value from a NULL the dump holds, `IS [NOT] UNREPRESENTABLE` and in DataFusion `pgdump_unrepresentable(<column>)`, which only a scan answers, DataFusion evaluating it refusing — at planning where the embedder built its session with the guard (`with_unrepresentable_guard`), which registering a dump leaves to it | `unrepresentable.rs`, `index.rs`, `cache.rs`, `gather.rs`, `prune.rs`, `batch.rs`, `resolve.rs`, `stream.rs`, `summary.rs`, `predicate.rs`, `datafusion-pgdump/src/unrepresentable.rs`; D96–D101 |
| CLI `pgdt parse` / `info` / `query`, with `--map`, `--json`, `--detail` and cache-only `info` | working; `parse` scans ahead, resumes, and saves on Ctrl-C; `info` never scans; `query` reads partitioned and prints file order | `pgdt/src/main.rs`; D61–D67; [`../manual/dump-inspection.md`](../manual/dump-inspection.md) |
| Partial reporting | `info` reports an unfinished scan's cache with its completion stated once at the top; an interrupted cache is typed for every database segment the scan finished (I1) | D67 |
| Column projection | working, library and CLI; an unprojected column is never decoded unless a filter term names it | `batch.rs`; D28; [`../manual/type-handling.md`](../manual/type-handling.md) |
| The filter expression, three-valued | working; `Expr` is one tree evaluated in SQL's `True`/`False`/`Unknown` domain, its `IN` a leaf answered per row by one lookup, reached as `--where` and as repeated `--filter` | `predicate.rs`; D53, D54, D101 |
| The `--where` and `--filter` grammars | working, CLI only; nothing below L4 parses a term | `pgdt/src/where_expr.rs`, `main.rs`; D60; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`" and "Writing a filter term" |
| Typed comparison: `=`/`!=` and the four ordering operators | working, library and CLI; equality falls back to text where the register gives no comparison and is refused only where the file says the server's is not a text comparison (a range declaring `canonical`), ordering is refused where the register gives no order, and a special value is a rank rather than a fault; a query may compare as DataFusion compares the emitted value instead, which refuses a nested column and orders one with no plan bytewise | `predicate.rs`, `pgtype.rs`; D40, D55–D58; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings" |
| The comparison register and the declared collation | L2, `comparison_for` in `pgtype.rs`: one `ComparisonPlan` per column, divergence announced on its own channel per term, or per column in a query's semantics; a stated collation this build does not implement compares bytewise (`KD7`) and an unmodelled scalar's equality is a guess (`KD10`) | D40, D59; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise" |
| Comparison oracle, cross-major differ, register-to-oracle reconciliation | committed under `fixtures/<major>/oracle/` and checked by `predicate.rs`'s unit test, `scripts/oracle_differences.py` and `scripts/oracle_register.py` | D70, D71 |
| ADBC floor oracle and the floor rule | committed under `fixtures/<major>/adbc/` and reconciled by `scripts/floor_mapping.py` | D38 |
| Compressed input (`--source foo.dump.xz`) | working serially and at any `--jobs`; the seek table is cached; a file the budget cannot block-decode streams and says so; gzip, zstd and lz4 are not read (P15, P18) | `io.rs` (`XzSource`), `cache.rs`; D14–D19; [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md) |
| Partitioned replay and the interior split | working; `query` is the replay's consumer and the mapping pass the split's, and each answers what the serial path answers; a sub-stream can pin more than the budget bills (`KD23`) and a window over small blocks over-reads (`KD22`) | `stream.rs`, `leader.rs`; D5, D7, D8, D48, D51, D52 |
| Caller-stated parallelism and its resident allowance | working end to end; an omitted `--jobs` asks the source and an omitted `--memory` the discovered limit, a stated one carved exactly as a discovered limit is; a cache's statistics are held off the margin before the workers of a pass holding them are carved — not a `query`'s replay, which holds none — and what the workers leave under it is what a gathering pass's statistics may hold, and a block that does not fit declines and is re-read only under a larger allowance; what is announced is corrected where it is not delivered; the charge is short on three named terms (`KD21`, `KD24`, `KD26`) and describes nothing held on a fourth (`KD25`), a stated allowance cannot raise a plain source's read budget (`KD32`) and `pgdt query` reads its sub-streams one at a time past the first round (`KD57`) | `io.rs` (`Parallelism`, `WorkerMemory`, `statistics_allowance`), `gather.rs`; D1–D5, D9–D13, D64, D83, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and `--memory`" |
| Measurement harness | `scripts/measure.py` takes every figure and emits the tables; every rule it enforces is asserted in `scripts/test_measure.py` | [`../design/measurements.md`](../design/measurements.md), "The apparatus" |
| Fixtures | real `pg_dump` output at six majors, regenerated by `scripts/generate_fixtures.py` | D69, D73 |
| Remote input (`--source https://…`), over `object_store` | working behind the default-off `http` feature: one ranged GET probes, one reader reads, no credential is sent, `file:` resolves to a local path and every other scheme is refused by name; the cache is named after the URL's last segment in the working directory and records the origin it was written for, and every ranged GET after the probe pins the object to the version the probe saw, unless `--strict-identity=none` unbinds it — its entity tag, or its modification time where it stated no tag or a weak one — so a rewrite mid-scan is refused by the server, and a run over a server stating neither is refused before it reads unless `--strict-identity=none`; a fetched `.xz` is read block by block out of windows we fetch, and its cold footer walk is announced rather than refused (`KD36`); where the budget cannot hold one decoded block the fetched arm keeps the window and the handle together, so a block a scan sits inside is fetched and decoded once | `io.rs` (`RemoteSource`, `FetchedXzSource`, `walk_seek_table`, `Origin`), `cache.rs` (`OriginMatch`); D6, D12, D14, D15, D18, D26; RT13–RT18; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Reading a dump over HTTP" |
| DataFusion `TableProvider` | a library, `datafusion-pgdump`: a dump opened through its complete cache, a table it holds at the metadata level refused under a typed schema, naming the `parse` that records it, under the `StrictIdentity` its caller states for it — `STORED AS PGDUMP`'s `pgdump.strict_identity`, or the factory's where a statement states none — and registered as one catalog per database, or one table alone; a scan is the partitioned replay over the held map, projected by name, batched at the session's size and parallel to its `target_partitions` within a budget every scan of the session draws on, under the `pgdump.*` settings a `SET` states — an allowance over the discovered one, a read chunk, a line limit and whether a dynamic filter's rows are evaluated — its count held under a margin that a finite memory-pool limit and each registered dump's resident statistics are billed against, and its budget where the count cannot fall; a filter is pushed `Exact` where the library answers it as DataFusion does and left to DataFusion elsewhere, and a join's, a TopK's or an ungrouped `MIN`/`MAX`'s dynamic filter cuts the sub-streams over the row groups it keeps when the first is polled, bar where that would lose a declared ordering (`KD53`), and is read as each runs, skipping the groups it rules out as the replay reaches them and ending a sorted block past a bound it requires, and, where that setting is on — it is off by default — dropping each row it rejects before decoding it, in every block; the scan's plan node carries the table's rows exactly and each column's NULLs, and its extremes where the map's statistics reach them, an enum's in its labels' text order, a float's zero the one `total_cmp` picks, its distinct values where every group kept a dictionary of the text it emits — a `character` column never — and an integer, `oid` or `Decimal128` column's sum where every group kept one, wrapped as DataFusion's `SUM` wraps, so `COUNT(*)`, `COUNT(<column>)`, `MIN`, `MAX`, `SUM` and a `COUNT(DISTINCT <column>)` beside another count can answer without a row being read, each value its column's type cannot hold read as the scan reads it — as NULL by default, a column read as its text keeping none of its bounds — though an ungrouped `MIN` beside a `MAX` can skip rows a DataFusion filter wrongly rules out (`KD56`) — and each column's Arrow bytes but a typed nested one's — a fixed-width one's and a boolean's from its rows, an enum's from both, and a text or `bytea` one's, or any read as text, from its values' text — so a join weighs its sides by bytes; a filter, or a fetch shorter than the table, makes every one of them an estimate, a pushed filter's rows and bytes bounded by the row groups its pruning kept; a column every partition is proved to emit in Arrow's order — each block recorded sorted alike, no NULL, each block boundary a partition crosses proved from adjacent groups' bounds — is declared that ordering, NULLs where an unqualified `ORDER BY` puts them, so a sort it satisfies is not planned; what a registration finds — the file's findings, each column's resolution and its divergence from PostgreSQL, and each table whose plan refuses, which is not listed — goes to a sink the caller supplies, which then hears what each scan's plan settles when it is planned — a note quoting a budget wrapped with what that budget was carved from and the settings that move it — while the groups a scan's statistics pruned, the groups and rows a dynamic filter pruned and the bytes an early stop left unread are its plan node's metrics; `CREATE EXTERNAL TABLE … STORED AS PGDUMP` registers one table | `datafusion-pgdump/src/`, `stream.rs` (`TablePartitions`), `summary.rs`; D40, D51, D88–D91, D93–D95, D98, D100 |
| SQL shell, `datafusion-cli-pgdump` | working: `datafusion-cli` 55.1.0 with `--dump [NAME=]SOURCE[:strings][:strict-identity=TERMS]`, `STORED AS PGDUMP`, `SET pgdump.*` and `pgdt`'s `--strict-identity` as the default a dump's own overrides, printing each registration's warnings to stderr, and each scan's at planning, built with `pgdump_unrepresentable`'s guard, and an ungrouped aggregate's dynamic filter off unless the environment states it (`KD56`); it never parses | `datafusion-cli-pgdump/src/`; D90; [`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md) |
| Python bindings | not started; P24 | |
| Device-bound scan performance | settled; parallelism is filed beside its own mechanisms | D10, D29 |
| Per-row-group column statistics | working; `pgdt parse` and the library's `map_file` gather them by default, for every column `--statistics-level` leaves at the data level, at any worker count, and persist them in the cache, re-reading a mapped block that lacks what is asked, and say on stderr how much memory they held, `info --detail` reports them per table and column and `--json` exports every group's, and a query — library and `pgdt query` — skips the row groups they rule out, and stops reading a block sorted past the filter's bound, saying after the fact what that left unread, unless told `--statistics none`; statistics stop where the account fills, so a long dump keeps a prefix (`KD33`); under an unstated group size a block's groups merge pairwise past `BLOCK_MAX_ROW_GROUPS` and, once it is read, until its median group holds the density minimum, never past a stated maximum, which turns the cap off and re-reads a block at the finer size its groups predict — a block another request sized being first re-read at the default, so twice in one run — each block recording the request that sized it; a block whose statistics the run's allowance cannot hold declines, says so, and is re-read only under a larger one | `statistics.rs`, `gather.rs`, `prune.rs`, `pgdt/src/info_statistics.rs`; D19, D34, D54, D67, D75–D82, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics-level`: what `parse` records for later queries" |
| `--inserts` row reading; custom, directory and tar archives | not started; P8, and the map already locates `INSERT` runs (`KD9`) | D33 |

**Figures.** [`../design/measurements.md`](../design/measurements.md) carries
the session stamp and its own account of what stands outside it; `cd scripts
&& uv run measure.py --stale` names what is red and why. **Every register
figure but `session-drift` was re-taken at `183a50eb`, with its binaries staged
on tmpfs**, the scan figures at the metadata level and the query figures over a
data-level cache; `session-drift` is the `da05a72` pair's, and `reserve`'s
stated axis is a reading `KD34` names.
What only something other than a sweep clears: `session-drift`, which only a
second sweep on a first's commit re-takes, and the koji section, outside the
register and red, which only a run on the HDD clears. Red with the reason
written down is the standing requirement, not red cleared
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

**Profiles and heap recordings are not figures.** `measure.py --profile-recipe`
and `--heaptrack-recipe` print an instrument sequence and run none of it; what
either produces is a `runs/` artifact with no median, no apparatus gate and no
marker ([`../design/measurements.md`](../design/measurements.md), "What an
instrument can see").

## P28 progress

Spec: [`../design/roadmap-P28-unrepresentable-values.md`](../design/roadmap-P28-unrepresentable-values.md).

- [x] **28.1** The evidence, no product code: every query shape the spec's "Evidence" names, run in every partition order under each mode, statistics and dynamic filters on and off, over majors 13, 16 and 18, one outcome asserted per query, each case recorded failing; [notes](../design/roadmap-P28.1-harness-notes.md)
- [x] **28.10** The extremes, no product code: every typed arm's least and greatest, the special values, `24:00:00`, `interval`'s and nested cases in the `types` fixture at every major; a reconciliation holding each typed `builtin_scalar` arm to an extremes row; the DataFusion-path validity test and the harness over the new rows, each failing value recorded; [notes](../design/roadmap-P28.10-extremes-notes.md)
- [x] **28.2** The metadata and data levels: `StatisticsLevel` and `--statistics-level` with its overrides, the census gated on the level, `info` reporting each table's level, a query's cold semantics over a metadata-level table and the provider's refusal of one; [notes](../design/roadmap-P28.2-levels-notes.md)
- [x] **28.3** The unrepresentable count beside the census, lexical, per block and column, each leaf walked by the declared type, in two tiers, in the cache with the calendar bound it counted under; `info --detail` showing it; `24:00:00` refused by its decoder; count and extremes record held to each other by tier; [notes](../design/roadmap-P28.3-count-notes.md)
- [x] **28.4** Statistics' views: representable bounds, the unrepresentable count, PostgreSQL-order bounds where they differ, and the engine tier's bounds where it is not empty, gathered, cached and read by pruning under each semantics; [notes](../design/roadmap-P28.4-views-notes.md)
- [x] **28.5** The mode option and the typed mode: NULL for every purpose — decode, static and dynamic filters, NULL counts, the provider's statistics — its warning, and the option in `pgdt`, the provider and the shell; [notes](../design/roadmap-P28.5-typed-notes.md)
- [x] **28.6** The refuse mode: by column, at planning, from the map; a timestamp past `i64` microseconds keyed, filling every value's view; [notes](../design/roadmap-P28.6-refuse-notes.md)
- [x] **28.7** The untyped mode: the widening resolution and its comparison in each semantics, "D38"'s clause; the harness green, closing `KD8`; [notes](../design/roadmap-P28.7-untyped-notes.md)
- [x] **28.8** `IS [NOT] UNREPRESENTABLE` and `pgdump_unrepresentable`, "D53" amended; [notes](../design/roadmap-P28.8-predicate-notes.md)
- [x] **28.11** The unrepresentable guard, the embedder's: registering a dump registers `pgdump_unrepresentable` and leaves the session's planning alone; the planning refusal a physical optimizer rule the embedder installs on a `SessionStateBuilder`, the evaluation's refusal naming it, and the shell installing it; "D101" and "RT22" rewritten; [notes](../design/roadmap-P28.11-guard-notes.md)
- [x] **28.9** The figures: the `census-*` figures and the census-off build retired, `statistics-gathering` pricing the data level against the metadata level, `statistics-pruning` without its `uncarried` leg, the bar lifted, and a data-level `parse` profiled; [notes](../design/roadmap-P28.9-figures-notes.md)
- [x] **28.9.1** The query figures over a data-level cache: `query-typed`, `query-strings`, `query-project-*` and `query-where-*` built untimed at the data level and queried with `--statistics none`, their readings with 28.9's; [notes](../design/roadmap-P28.9.1-query-cache-notes.md)

## Not started

- **A CLI-feedback pass** — the `pgdt info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P28 is open**, its checklist above. A dump is readable
  over HTTP, plain and `.xz`, with nothing about the network's speed priced
  (`KD35`, `KD36`). What statistics may hold resident is bounded and their
  coverage is not (`KD33`, `KD34`), both owned by P23, whose sketch in
  [`../design/roadmap.md`](../design/roadmap.md), "P23 — Statistics coverage
  and the resident reserve" holds what it inherits. Every other built
  mechanism — the parallel scan's defaults included: a worker count the
  source recommends, a memory limit discovered and filled under the reserve
  and the margin, and a `parse` saying what it delivered rather than what it
  was asked for — is in [`../design/decisions.md`](../design/decisions.md).
  Eight phases remain sketched — P22, P21, P23, P26, P15, P18, P8, P24,
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

