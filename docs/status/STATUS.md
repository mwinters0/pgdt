# Status

What is built, right now. Rewritten in place as state changes. The decisions the
code cannot explain are [`../design/decisions.md`](../design/decisions.md);
what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; where what is built falls short is
[`deficiencies.md`](deficiencies.md), the register every `KD<k>` below
resolves in; dated pickup notes and plan-changing discoveries are in
`history/`.

<!-- repointed: 69b10b42 --> The marker names the commit `scripts/repoint.py`
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
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working | `stream/`, `batch.rs`; D46–D50 |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working; `money` stays text by decision (`KD13`), a `bytea_output = escape` dump's `bytea` renders `hex` (D66), and a value a typed column cannot hold reads as NULL by default, or its column as its text, the rest typed, or, told to refuse, refuses a query materializing its column at planning; a field PostgreSQL refuses fails a data-level parse keying it, at the first — an enum's undeclared label where the preamble read its labels exactly (`KD87`), and a NULL a `NOT NULL` refuses — and a query decoding it, an enum's label only under an ordering filter or one comparing an array or composite holding it (`KD88`), unless told to ignore such fields, when a parse goes on past each, its cache recording them per column for a later refusing parse tracking the column to fail on, and a query reads a float past its range as the parse rounds it, the largest of its sign or zero (`KD108`), a `varchar(n)` past its length as written and a NULL a `NOT NULL` refuses as NULL; told to be strict, a parse checks every field of a type it reads, re-reading each block its cache holds that no strict parse checked, and a strict parse and `info --detail` name under each `COPY` block what it does not check, every other listing counting the blocks | `pgtype.rs`, `resolve.rs`, `decode.rs`, `datetime_in.rs`, `grammar.rs`, `gather/strict.rs`; D37–D44, D103; [`../manual/type-handling.md`](../manual/type-handling.md) |
| Full byte-exact file map, every byte in exactly one span, verified over every fixture | working | `map.rs`, `map/`; D30–D33 |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working; a `--disable-triggers` dump loses data-span attribution (`KD1`) | `map/`, `ddl/`, `preamble.rs`; D31, D36 |
| Structural cache with source-identity checking and cache-only inspection | working; a cache that cannot be used — another file's, another build's, damaged, or not a pgdt cache — is refused before the dump is read past its first bytes, and `--overwrite-unusable-cache` replaces any but the last; a weak signal — an mtime, or a server's `Last-Modified` and `ETag`, and where a source was fetched from — is advisory between runs unless `--strict-identity` binds the term, and a source that changes under an in-flight read aborts a run that then saves and removes nothing, unless `--strict-identity=none` | `cache.rs`, `cache/`; D18–D22; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--strict-identity`: when a moved file should stop the run" and "When `info` says it cannot answer" |
| Arrays, composites, ranges, multiranges, `int2vector` | typed, decoded and compared structurally; two shapes stay text (`KD3`), and an array inside a composite is decided optimistically (`KD2`) | `nested.rs`, `pgtype.rs`, `compare.rs`; D41, D43, D45, D58 |
| Array shape census | recorded at the data level by every mapping pass and read back before a query's first batch; a table `parse` recorded at the metadata level holds none, and a typed query of it reads the table again for one, writing nothing, where a replay over a map the caller holds (`TablePartitions`) refuses it | `map.rs`, `stream/`; D35, D43 |
| Unrepresentable count | beside the census, per block and column, in two tiers — past Arrow's format spec, and past the calendar DataFusion displays through — under a calendar the cache records and a build ending elsewhere refuses; shown by `info --detail`; statistics count it per group and keep each view of a group's values apart where it differs — the values the type holds, every value, and those within the calendar; a query reads such a value as NULL for every purpose — its batch, its filters, its pruning and the provider's statistics — in the tiers its front end cannot hold, and says per column how many, unless told to read such a column as its text, compared bytewise by DataFusion and in its declared type's order by `pgdt`, or to refuse, when a query materializing such a column refuses at planning, naming it and how many, whatever its filter keeps (`--unrepresentable`, `PgDumpOptions::unrepresentable`, `pgdump.unrepresentable`, `:unrepresentable=`); in every mode a filter term tells such a value from a NULL the dump holds, `IS [NOT] UNREPRESENTABLE` and in DataFusion `pgdump_unrepresentable(<column>)`, which only a scan answers, DataFusion evaluating it refusing — at planning where the embedder built its session with the guard (`with_unrepresentable_guard`), which registering a dump leaves to it | `unrepresentable.rs`, `model/block.rs`, `cache/`, `gather/`, `prune.rs`, `batch.rs`, `resolve.rs`, `stream/`, `summary.rs`, `predicate.rs`, `crates/datafusion-pgdump/src/unrepresentable.rs`; D96–D103 |
| CLI `pgdt parse` / `info` / `query`, with `--map`, `--json`, `--detail` and cache-only `info`; `pgdt sql`, the SQL shell below | working; `parse` scans ahead, resumes, and saves on Ctrl-C; `info` never scans; `query` reads partitioned and prints file order | `crates/pgdt/src/main.rs`; D61–D67; [`../manual/dump-inspection.md`](../manual/dump-inspection.md) |
| Partial reporting | `info` reports an unfinished scan's cache with its completion stated once at the top; an interrupted cache is typed for every database segment the scan finished (I1) | D67 |
| Column projection | working, library and CLI; an unprojected column is never decoded unless a filter term names it | `batch.rs`; D28; [`../manual/type-handling.md`](../manual/type-handling.md) |
| The filter expression, three-valued | working; `Expr` is one tree evaluated in SQL's `True`/`False`/`Unknown` domain, its `IN` a leaf answered per row by one lookup, reached as `--where` and as repeated `--filter` | `predicate.rs`; D53, D54, D101 |
| The `--where` and `--filter` grammars | working, CLI only; nothing below query parses a term | `crates/pgdt/src/where_expr.rs`, `main.rs`; D60; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`" and "Writing a filter term" |
| Typed comparison: `=`/`!=` and the four ordering operators | working, library and CLI; equality falls back to text where the register gives no comparison and is refused only where the file says the server's is not a text comparison (a range declaring `canonical`), ordering is refused where the register gives no order, and a special value is a rank rather than a fault; a query may compare as DataFusion compares the emitted value instead, which refuses a nested column and orders one with no plan bytewise | `predicate.rs`, `key.rs`, `compare.rs`; D40, D55–D58; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings" |
| The comparison register and the declared collation | the codec, `comparison_for` in `compare.rs`: one `ComparisonPlan` per column, divergence announced on its own channel per term, or per column in a query's semantics; a stated collation this build does not implement compares bytewise (`KD7`) and an unmodelled scalar's equality is a guess (`KD10`) | D40, D59; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise" |
| Comparison oracle, cross-major differ, register-to-oracle reconciliation; the value oracle | committed under `fixtures/<major>/oracle/` and checked by `predicate/tests.rs`, `scripts/oracle_differences.py` and `scripts/oracle_register.py`; `values.tsv` beside them, the server's reading of every typed value of the `types` schema, which every typed read of its flag sets is held to and every typed arm has a case in (`scripts/value_oracle.py`) | D70, D71, D73 |
| Emitter register | `pg_dump`'s and `pg_dumpall`'s literals and options, read out of the functions pgdt's readers consume — each keyword a reader recognises mapped to a listed function or `mise run check` fails — into `fixtures/<major>/emitters.tsv` by `scripts/emitter_register.py --extract`, and the value forms a setting or a typmod selects, hand-listed beside the extraction, all joined against the fixtures; a row no fixture reaches is exempt with a reason the record resolves or it fails `mise run check`, and a fixture exposing a filed defect is held failing by `crates/pgdt-query/tests/known_failures.rs` | `scripts/emitter_register.py`; D71 |
| ADBC floor oracle and the floor rule | committed under `fixtures/<major>/adbc/` and reconciled by `scripts/floor_mapping.py` | D38 |
| Compressed input (`--source foo.dump.xz`) | working serially and at any `--jobs`; the seek table is cached; a file the budget cannot block-decode streams and says so; gzip, zstd and lz4 are not read (P15, P18) | `source/xz/file.rs` (`XzSource`), `cache.rs`; D14–D19; [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md) |
| Partitioned replay and the interior split | working; `query` is the replay's consumer and the mapping pass the split's, and each answers what the serial path answers; a sub-stream can pin more than the budget bills (`KD23`) and a window over small blocks over-reads (`KD22`) | `stream/partition.rs`, `stream/replay.rs`, `leader.rs`; D5, D7, D8, D48, D51, D52 |
| Caller-stated parallelism and its resident allowance | working end to end; an omitted `--jobs` asks the source and an omitted `--memory` the discovered limit, a stated one carved exactly as a discovered limit is; a cache's statistics are held off the margin before the workers of a pass holding them are carved — not a `query`'s replay, which holds none — and what the workers leave under it is what a gathering pass's statistics may hold, and a block that does not fit declines and is re-read only under a larger allowance; what is announced is corrected where it is not delivered; the charge is short on four named terms (`KD21`, `KD24`, `KD26`, `KD111`) and describes nothing held on a fourth (`KD25`), a stated allowance cannot raise a plain source's read budget (`KD32`) and `pgdt query` reads its sub-streams one at a time past the first round (`KD57`) | `parallelism.rs` (`Parallelism`, `WorkerMemory`), `memory.rs` (`statistics_allowance`), `gather/gatherer.rs`; D1–D5, D9–D13, D64, D83, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and `--memory`" |
| Versioned binary releases | `pgdt` alone, for Linux on x86-64 and arm64: built in one Debian-stable image pinned by digest, whose glibc is the floor a release promises and whose build the register times; a dispatched workflow drafts a Release, an annotated tag publishes it immutable, its body the annotation, each archive holding generated third-party notices and attested, a checksum file beside them; macOS is not shipped (P34) | `release/Dockerfile`, `scripts/release.py`, `scripts/release_ci.py`, `scripts/debian_window.py`, `.github/workflows/release-*.yml`, `.claude/skills/release/`; D13, D104, D105; [`../manual/installing.md`](../manual/installing.md) |
| Measurement harness | `scripts/measure.py` takes every figure and emits the tables; every rule it enforces is asserted in `scripts/test_measure.py` | [`../design/measurements.md`](../design/measurements.md), "The apparatus" |
| Fixtures | real `pg_dump` output at six majors, regenerated by `scripts/generate_fixtures.py` | D69, D73 |
| Remote input (`--source https://…`), over `object_store` | working behind the default-off `http` feature: one ranged GET probes, one reader reads, no credential is sent, `file:` resolves to a local path and every other scheme is refused by name; the cache is named after the URL's last segment in the working directory and records the origin it was written for, and every ranged GET after the probe pins the object to the version the probe saw, unless `--strict-identity=none` unbinds it — its entity tag, or its modification time where it stated no tag or a weak one — so a rewrite mid-scan is refused by the server, and a run over a server stating neither is refused before it reads unless `--strict-identity=none`; a fetched `.xz` is read block by block out of windows we fetch, and its cold footer walk is announced rather than refused (`KD36`); where the budget cannot hold one decoded block the fetched arm keeps the window and the handle together, so a block a scan sits inside is fetched and decoded once | `source/remote.rs` (`RemoteSource`), `source/xz/fetched.rs` (`FetchedXzSource`), `source/xz.rs` (`walk_seek_table`), `source/origin.rs` (`Origin`), `cache/identity.rs` (`OriginMatch`); D6, D12, D14, D15, D18, D26; RT13–RT18; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Reading a dump over HTTP" |
| DataFusion `TableProvider` | a library, `datafusion-pgdump`: a dump opened through its complete cache, a table it holds at the metadata level refused under a typed schema, naming the `parse` that records it, under the `StrictIdentity` its caller states for it — `STORED AS PGDUMP`'s `pgdump.strict_identity`, or the factory's where a statement states none — and registered as one catalog per database, or one table alone; a scan is the partitioned replay over the held map, projected by name, batched at the session's size and parallel to its `target_partitions` within a budget every scan of the session draws on, under the `pgdump.*` settings a `SET` states — an allowance over the discovered one, a read chunk, a line limit and whether a dynamic filter's rows are evaluated — its count held under a margin that a finite memory-pool limit and each registered dump's resident statistics are billed against, and its budget where the count cannot fall; a filter is pushed `Exact` where the library answers it as DataFusion does and left to DataFusion elsewhere, and a join's, a TopK's or an ungrouped `MIN`/`MAX`'s dynamic filter cuts the sub-streams over the row groups it keeps when the first is polled, bar where that would lose a declared ordering (`KD53`), and is read as each runs, skipping the groups it rules out as the replay reaches them and ending a sorted block past a bound it requires, and, where that setting is on — it is off by default — dropping each row it rejects before decoding it, in every block; the scan's plan node carries the table's rows exactly and each column's NULLs, and its extremes where the map's statistics reach them, an enum's in its labels' text order, a float's zero the one `total_cmp` picks, its distinct values where every group kept a dictionary of the text it emits — a `character` column never — and an integer, `oid` or `Decimal128` column's sum where every group kept one, wrapped as DataFusion's `SUM` wraps, so `COUNT(*)`, `COUNT(<column>)`, `MIN`, `MAX`, `SUM` and a `COUNT(DISTINCT <column>)` beside another count can answer without a row being read, each value its column's type cannot hold read as the scan reads it — as NULL by default, a column read as its text keeping none of its bounds — though an ungrouped `MIN` beside a `MAX` can skip rows a DataFusion filter wrongly rules out (`KD56`) — and each column's Arrow bytes but a typed nested one's — a fixed-width one's and a boolean's from its rows, an enum's from both, and a text or `bytea` one's, or any read as text, from its values' text — so a join weighs its sides by bytes; a filter, or a fetch shorter than the table, makes every one of them an estimate, a pushed filter's rows and bytes bounded by the row groups its pruning kept; a column every partition is proved to emit in Arrow's order — each block recorded sorted alike, no NULL, each block boundary a partition crosses proved from adjacent groups' bounds — is declared that ordering, NULLs where an unqualified `ORDER BY` puts them, so a sort it satisfies is not planned; what a registration finds — the file's findings, each column's resolution and its divergence from PostgreSQL, and each table whose plan refuses, which is not listed — goes to a sink the caller supplies, which then hears what each scan's plan settles when it is planned — a note quoting a budget wrapped with what that budget was carved from and the settings that move it — while the groups a scan's statistics pruned, the groups and rows a dynamic filter pruned and the bytes an early stop left unread are its plan node's metrics; `CREATE EXTERNAL TABLE … STORED AS PGDUMP` registers one table | `crates/datafusion-pgdump/src/`, `stream/table_partitions.rs` (`TablePartitions`), `summary.rs`; D40, D51, D88–D91, D93–D95, D98, D100 |
| SQL shell, `pgdt sql` | working: `datafusion-cli` 55.1.0 with `--dump [NAME=]SOURCE[:strings][:unrepresentable=MODE][:postgres-invalid-values=MODE][:strict-identity=TERMS]`, `STORED AS PGDUMP`, `SET pgdump.*` and `pgdt`'s `--strict-identity` as the default a dump's own overrides, printing each registration's warnings to stderr, and each scan's at planning, built with `pgdump_unrepresentable`'s guard, and an ungrouped aggregate's dynamic filter off unless the environment states it (`KD56`); it never parses | `crates/datafusion-cli-pgdump/src/` (a library), `crates/pgdt/src/main.rs`; D90; [`../manual/pgdt-sql.md`](../manual/pgdt-sql.md) |
| Python bindings | not started; P24 | |
| Device-bound scan performance | settled; parallelism is filed beside its own mechanisms | D10, D29 |
| Per-row-group column statistics | working; `pgdt parse` and the library's `map_file` gather them by default, for every column `--statistics-level` leaves at the data level, at any worker count, and persist them in the cache, re-reading a mapped block that lacks what is asked, and say on stderr how much memory they held, `info --detail` reports them per table and column and `--json` exports every group's, and a query — library and `pgdt query` — skips the row groups they rule out, and stops reading a block sorted past the filter's bound, saying after the fact what that left unread, unless told `--statistics none`; statistics stop where the account fills, so a long dump keeps a prefix (`KD33`); under an unstated group size a block's groups merge pairwise past `BLOCK_MAX_ROW_GROUPS` and, once it is read, until its median group holds the density minimum, never past a stated maximum, which turns the cap off and re-reads a block at the finer size its groups predict — a block another request sized being first re-read at the default, so twice in one run — each block recording the request that sized it; a block whose statistics the run's allowance cannot hold declines, says so, and is re-read only under a larger one | `statistics.rs`, `gather/`, `prune.rs`, `crates/pgdt/src/info_statistics.rs`; D19, D34, D54, D67, D75–D82, D85; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics-level`: what `parse` records for later queries" |
| `--inserts` row reading; custom, directory and tar archives | not started; P8, and the map already locates `INSERT` runs (`KD9`) | D33 |

**Figures.** [`../design/measurements.md`](../design/measurements.md) carries
the session stamp and its own account of what stands outside it; `cd scripts
&& uv run measure.py --stale` names what is red and why. **Every register
figure but `session-drift` and `release-profile` was taken in one sitting at
`3a34f062`, on the mimalloc build made in the release image under Cargo's default profile, before D105 made it fat LTO, and run in
`debian:trixie` under its glibc, with its binaries staged on tmpfs**, the scan figures at the
metadata level and the query figures over a data-level cache.
`session-drift` is the `da05a72` pair's, `release-profile` its own sitting's at `2df93720`, and `reserve`'s 128 MiB-block `rule`
cells are a reading `KD34` names.
What a sweep does not re-take: `session-drift`, which only a second sweep on
a first's commit does, and the koji section, outside the register and red,
which only a run on the HDD clears. Red is the resting
state, and a red figure is re-taken before anything reasons from it
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

**Profiles and heap recordings are not figures.** `measure.py --profile-recipe`
and `--heaptrack-recipe` print an instrument sequence and run none of it; what
either produces is a `runs/` artifact with no median, no apparatus gate and no
marker ([`../design/measurements.md`](../design/measurements.md), "What an
instrument can see").

## P35 progress

Spec: [`../design/roadmap-P35-seams.md`](../design/roadmap-P35-seams.md).

- [x] **35.1** The limits and the skill: `scripts/file_limits.py` with its tests, a meter in `check.py`, the `seams` skill, the rule in `process.md`, and the baseline report over the tree — [notes](../design/roadmap-P35.1-file-limits-notes.md)
- [x] **35.2** Inline test modules moved to a file beside their module, in every file over a limit; every script and citation that named a test by its old path repointed — [notes](../design/roadmap-P35.2-tests-beside-notes.md)
- [x] **35.3** The cycles inside `pgdump_query` broken in place — the cancellation token and chunk default down to `io`, the catalog, span, block and statistics types gathered into a `model` module, the index loader lifted out of `index`, `CACHE_FORMAT_VERSION` into `model` — and `tests/layering.rs` carrying the crate assignment, passing — [notes](../design/roadmap-P35.3-model-notes.md)
- [x] **35.7** The ordering keys and the field-grammar checks lifted out of `predicate.rs` into a module of their own, assigned to `pgdt-codec` in `tests/layering.rs` — [notes](../design/roadmap-P35.7-keys-notes.md)
- [x] **35.8** The `release-profile` figure built — Cargo's default, thin LTO, and fat LTO with one codegen unit, each leg built from the tree measured and asked what it is before it is timed — and its first sitting taken on the one-crate tree — [notes](../design/roadmap-P35.8-release-profile-notes.md)
- [x] **35.4** `pgdt-core`, `pgdt-model`, `pgdt-io`, `pgdt-dump`, `pgdt-codec` and `pgdt-query` carved under `crates/`, `pgdump_query` a facade over them, every integration test moved to the lowest crate whose API it exercises, `io`'s unit tests that drive a scan or a stream moved whole to the lowest crate holding what they use, and `pgdt-codec` dev-depending on `pgdt-dump` for its tests that parse DDL, of which there are none; every shared version in `[workspace.dependencies]`, the `upgrade-deps` skill pointed there — [notes](../design/roadmap-P35.4-carve-notes.md)
- [x] **35.9** The `release-profile` figure's second sitting, on the split tree, and the shipped build's release profile decided from the two in a `D<k>`: fat LTO in one codegen unit, D105 — [notes](../design/roadmap-P35.9-second-sitting-notes.md)
- [x] **35.10** Every forced-public item reachable only through its crate's `internal`: every library module private, each API namespace an embedder names a public module of named re-exports, and each method a crate above calls that is not API behind an extension trait `internal` exports — [notes](../design/roadmap-P35.10-internal-notes.md)
- [x] **35.10.1** Each crate's API sorted by the spec's definition ("What this phase does not do"): what an embedder of the crate would call by its purpose stays API, the rest goes to `internal`, and each API item has one path — 35.10 having taken "public before the carve" for API — [notes](../design/roadmap-P35.10.1-api-sort-notes.md)
- [x] **35.10.3** 35.10.1's sort corrected: `ValueKey` made `pgdt-codec`'s API in a `key` namespace, `compare` no longer test-gated, its other methods kept, while `OrderKey`, `NestedKey`, the functions building them and the refusal checks stay in `internal`; and the per-block statistics re-read restored to `pgdt-query`'s API — `gather_block_statistics`, `BlockReread`, `bounded_columns`, `StatisticsBackfill`, `StatisticsRequest::backfill` an inherent method again, `pgdt-model`'s `BlockGathered` with them — and `pgdt_query::internal`, then empty, removed — [notes](../design/roadmap-P35.10.3-sort-corrected-notes.md)
- [x] **35.10.2** Each API namespace its file's own module made public, what it holds that is not API moved to private child modules at the file's seam and re-exported by `internal`, and no library file loaded by `#[path]` — the layout the spec names ("What this phase does not do"), over 35.10.1's sort — [notes](../design/roadmap-P35.10.2-namespaces-notes.md)
- [x] **35.5** The facade retired; every member under `crates/`; `tests/layering.rs` become `scripts/layering.py`, reading the graph from `cargo metadata`, refusing an embedder's `internal` path or `timed!`/`row_evaluated!`, a codec `arrow_buffer` item but `i256`, a `#[path]` on a library module, and an API item re-exported at its crate's root beside its namespace; scripts, docs, D68 and D74, `CLAUDE.md`'s commands and the graft index repointed to the new paths — [notes](../design/roadmap-P35.5-facade-retired-notes.md)
- [x] **35.6** The seams round: every file `file_limits.py` names cut at its seams or exempted with its reason, the tree green — [notes](../design/roadmap-P35.6-seams-round-notes.md)
- [x] **35.11** Every crate's module tree designed, no code moved, as the next version of `docs/design/concepts.md`: pgdt-codec by type family, its register a `compare` namespace and the strict grammar one module; a seventh crate, `pgdt-mapping`, for the mapping pass and statistics gathering; the SQL lexer in pgdt-dump; `pgdt` a module a subcommand; the cuts sliced below — [notes](../design/roadmap-P35.11-trees-notes.md)
- [x] **35.11.1** 35.11's design checked against the module graph: a `moves` block for every remaining cut in its notes, projected by `scripts/module_graph.py --design`, and each cycle it reports — the five `OWED` entries no cut removes among them — designed out or put to the maintainer to keep or owe; no code moved — [notes](../design/roadmap-P35.11.1-graph-notes.md)
- [x] **35.13** The SQL lexer to pgdt-dump, a type name's canonical form kept in pgdt-core as `type_name.rs`, and `census_row` to pgdt-model as `BlockCensusExt::observe_row` — [notes](../design/roadmap-P35.13-lexer-census-notes.md)
- [x] **35.14** pgdt-io's tree: the `io/` layer gone, the sources under `source/` with `.xz`'s shared parts above its arms, `partition.rs`, and `tests.rs` split beside each module — [notes](../design/roadmap-P35.14-io-tree-notes.md)
- [x] **35.15** pgdt-dump's tree: `ddl/` for the statement grammar, `preamble.rs` for the region's prepass, `map/`, and every identity in `cache/identity.rs` — [notes](../design/roadmap-P35.15-dump-tree-notes.md)
- [x] **35.16** pgdt-codec's comparison register out of `pgtype` into a `compare` namespace, `discrete` and `divergence` folded in, and the strict parse's codec part into `grammar.rs` — [notes](../design/roadmap-P35.16-compare-grammar-notes.md)
- [ ] **35.23** The cycles no other cut removes: pgdt-model's `BlockObserver` to `statistics.rs`, `union_census` to `span.rs` and `TilingIssue` to `diagnostic.rs`; pgdt-dump's `cache.rs` declaring, its operations in `cache/store.rs`, `CacheStatus` in `file.rs` and `StrictIdentity` in `identity.rs`; and `.xz`'s partition advice into `xz/blocks.rs`
- [ ] **35.17** pgdt-codec by type family: `value/` holding each family's reader, renderer, classifier and key type, `key.rs` the dispatch building an `OrderKey`, `declared.rs` the catalog walk beneath the register, `compare/walk.rs`, `grammar/`'s checks and forms, `key/order.rs` dissolved, `nested` a namespace, and the two key-only oracle tests moved from pgdt-query
- [ ] **35.18** `pgdt-mapping` carved between {pgdt-dump, pgdt-codec} and pgdt-query, its tree as designed, the interior split from pgdt-dump with it; D68, `layering.py`, `measure.py` and the integration tests its API covers moved
- [ ] **35.19** pgdt-query's predicate as one tree — resolution, notes, evaluation, truth sets — its tests split along it
- [ ] **35.20** pgdt-query's `options.rs`, `render.rs`, `batch/` and `stream/`, the replay nested under `stream/replay/`
- [ ] **35.21** datafusion-pgdump's `filter/`: the leaf builders in its parent, the static and dynamic translations its children; and `lib.rs` declaring, `Error` in `error.rs` and what else it defines moved below its readers
- [ ] **35.22** `pgdt` a module a subcommand, each owning its flags as a `#[derive(Args)]` struct, `--filter`'s and `--where`'s grammar one `query/filter/`, `tests.rs` split beside them
- [ ] **35.12** The closing seams round: no file past any limit but an exemption, and every crate read for the `seams` skill's "Antipatterns"

## Not started

- **A CLI-feedback pass** — the `pgdt info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P35 is the open phase**, its checklist above. A dump is readable over HTTP, plain and `.xz`, with
  nothing about the network's speed priced
  (`KD35`, `KD36`). What statistics may hold resident is bounded and their
  coverage is not (`KD33`, `KD34`), both owned by P23, whose sketch in
  [`../design/roadmap.md`](../design/roadmap.md), "P23 — Statistics coverage
  and the resident reserve" holds what it inherits. Every other built
  mechanism — the parallel scan's defaults included: a worker count the
  source recommends, a memory limit discovered and filled under the reserve
  and the margin, and a `parse` saying what it delivered rather than what it
  was asked for — is in [`../design/decisions.md`](../design/decisions.md).
  Eleven phases remain sketched — P33, P32, P34, P22, P21, P23, P26, P15, P18, P8, P24,
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

- **M221's fold read the downstream-build rule in wall clock, and filed no
  obligation.** The `release-profile` sitting at `2df93720`
  ([`../design/measurements.md`](../design/measurements.md), "What link-time
  optimization buys the shipped build") holds the default leg within 3% of
  `release-fat` in wall clock on `pgdt sql` and on `typed`, and not on
  `strings`, where its one low rep meets fat's cell; in user time, what a
  codegen change moves, it trails by more on all three. The fold read the
  rule ([`../design/roadmap.md`](../design/roadmap.md), "A downstream build is
  within 3% of ours") as holding: the figure's column is wall clock, the wait
  a user feels; a gap the cells do not resolve is small by the same rule; and
  every gap that widened since `6b759eaf` widened by the fat leg moving, the
  default leg unmoved, by the size fat has swung against thin between
  neighbouring trees, so layout rather than lost inlining is the candidate.
  Reading the rule in user time, or `strings`' gap as resolved, owes the
  rule's remedy instead: `#[inline]` on the hot cross-crate calls of the
  `pgdt sql` and `strings` paths, admitted as an out-of-band row, the figure's
  next sitting its check.

- **35.16 put the strict parse's three API types in `compare`.**
  `TextGrammar`, `Geometric` and `Unchecked` were `pgtype`'s API, and 35.11's
  design moves them to `grammar.rs`, a private module, naming no namespace to
  declare them. `pgdt_codec::compare` now does (`pub use`): the register's
  `NestedCompare::Uncomparable` carries a `TextGrammar`, and `Unchecked` is
  what `unchecked_positions` reads off a plan — so an embedder reading
  `StrictUnchecked`'s reasons names `compare::Unchecked`. A `grammar`
  namespace was the alternative, but its non-API functions, most of the file,
  would then need a private child to keep `internal` their only path.
  Reconsidering moves three `pub use` lines and every `compare::Unchecked`,
  `compare::TextGrammar` path above the codec, and adds a namespace to
  `layering.py` if it is `grammar`
  ([`../design/roadmap-P35.16-compare-grammar-notes.md`](../design/roadmap-P35.16-compare-grammar-notes.md)).
