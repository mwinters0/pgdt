# Batches, streams and the leader

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. From mapped blocks to Arrow batches: zero-copy views and
the span that bounds them, the two passes and the seam between them, the partitioned replay, the dynamic filters a
DataFusion plan hands a scan, and the three value modes for what a type cannot hold. `pgdt-query`'s `stream`, `batch`
and `plan`.

### D46 Zero-copy views are top-level `Utf8View` only, and `max_source_span` alone bounds a batch in flight's pins
Every arm but a top-level `Utf8View` copies, and a batch's views pin the chunks they point into, so `max_source_span` is
the one bound on what a batch in flight pins, an emitted one keeping its chunks for its consumer (`KD171`): `max_rows`
and `max_bytes` count selected rows, which a filter makes sparse. A source retaining by partition is charged no span
(`RetainedUnit`), unsound where a query's cut spans several units (`KD23`). Rejected: compacting views past a
selectivity threshold. Evidence: `parallel-peak-rss`.

### D141 A long value is decoded once beside its line: a `bytea` into its column, an escaped text as its own block
A `bytea` is decoded from its field as `Unescaped` reads it, straight into its column's one buffer, and a strict parse
checks one holding none of it; an escaped text past Arrow's largest view block is handed to its column as that block. So
a long `bytea` or escaped text read from a held line has one decoded copy beside the line. Rejected: Arrow's
`BinaryBuilder`, which cannot reserve once built nor abandon a value written in pieces, so a decode into it stages a copy
or grows by doubling; `BinaryView` for `bytea`, a type the floor does not yield (D38); a block per escaped text of any
length, fragmenting a batch's buffers. A batch's buffer growing past a value is its span's, which the replay is charged
(D84). Code: `batch/bytea.rs`, `batch/chunks.rs` (`OWN_BLOCK_PAST`), `value/bytea.rs` (`decode_bytea_into`). Evidence:
`tests/decoded_copies.rs`.

### D153 A nested value is decoded into its builders as the output grammars' walk reads it, its tier tested by a walk of its own
A query's batch reads an array, record, range, multirange or `int2vector` field by `decode_*`'s grammar walked over its
text, or off its bytes unescaped as read, each part appended to its builder as handed over. A tier test (D98) walks it
first, testing each leaf as read, as the census's and the gatherer's do. A refused field's parts stay, the refusal
ending the query. Why: a query holds its line and one decoded copy (D141); a tested container grows as long as any.
Rejected: `decode_*`'s elements, a `Cow` and its text an element; unescaping the field first; builders of our own for
every leaf type, truncating a refused or nulled field's parts, so one walk could test and decode. Code:
`batch/builder.rs` (`append_walked`), `nested/output/walk.rs`, `unrepresentable.rs` (`tier_walked`). Evidence:
`tests/decoded_copies.rs`, `batch/tests.rs`.

### D154 A decode factor is what a column's reserved builders and walk hold per text byte; a query's, its largest
A nested value past its chunk term is a batch of its own, decoded into builders reserved at each buffer's own density;
its column's factor sums the reservations beside what its walk holds, and a query's is the largest beside its terms' key
factors (D152). Why: a buffer doubling past its chunk term holds its outgrown one; a reservation is committed whole, as
the held line is. Rejected: doubling, a batch's (D141); every factor doubled; the densest buffer alone (another
doubles); the largest charged, the unwritten rest free (aborting past admission under a strict overcommit or an
address-space limit); counting parts first to reserve exactly (a second read); summing the two kinds; a factor per type
in pgdt-codec; reserving builders already holding rows, not cutting before the value: arrow's builders reserve only as
built, so growing one copies its rows. Code: `batch/builder/factor.rs`, `RowBatcher::cuts_before`, `ReplayPlan::factor`.
Evidence: `factor/tests.rs`, `tests/decoded_copies.rs`.

### D159 A query's drawn line ends its batch, and a replay returns every draw before it yields
A replay draws a line held past its chunk term as a parse does (D158), charged its bytes and its query's factor of them
(D154). The line's row ends its batch, and every draw is returned before a batch is yielded, a cut before the row
drawing again. Why: a batch holds the line until it is emitted, and a stream held back at a yield, as a merge in file
order holds one, must hold nothing an earlier line waits on; what a plan holds past it is its pool's (UF5). Rejected:
returning it at the next chunk, which the batch outlives, or as its copy's last reference drops, which a plan holding
the batch for later rows deadlocks; one draw for a batch's lines, whose give-up drops decoded rows; cutting only under
a limited ledger, batches varying with the allowance. Code: `stream/replay.rs`, `RowBatcher::holds_drawn_line`.
Evidence: `tests/ledger.rs`.

### D84 The batch span is derived from the budget and the count, and spent before the count is cut
`max_source_span` is a ceiling: a partitioned replay derives the span it charges from the budget and the reader count,
narrowing the span before it cuts the count, never below the announced read chunk (`ReadOptions::chunk_size_bytes`, the
unit a batch pins), and its sub-streams read under the span it was solved for. Rejected: a third flag (`roadmap.md`,
"Two tunables fit pgdt to hardware"); pricing a plain reader (`KD25`); a floor above the read chunk, a performance claim
with no batch-size figure behind it; the shipped chunk as the floor, which under a smaller `--chunk-size` declines
readers the announced one seats. Reopens: a floored span measurably slower than an unfloored one. Evidence:
`parallel-scan-throughput`.

### D48 Mapping and replay are separate passes, and `splice` owns the seam
A mapping pass settles a table's blocks and yields no row; replay reads them again, so the map is never behind the rows
and a `ResumeToken` points inside mapped territory. A segment is spliced by extending the *preceding* span, so a blank
line stays with the span before it across a stop. A cold query's pass (`Table::settle`) takes the extent and schema mode
it reads under as arguments. Rejected: a start floor on `Builder`, which made assembly visible in the map; the pass
taking the `QueryOptions` it is planned under, its `Table` planned under others anyway (Strings then Typed refuses,
D35); a value for those two alone.

### D49 One target per query, the early stop conservative, and `ResumeToken` opaque, fingerprinting the query
A `TableSelector` narrows to one `TableName` before replay or `AmbiguousTable`; `target_settled` vetoes a stop on a
partition root (I2) or any `\connect`. A conflict past the stop is unseen (`KD6`). The token's filter is folded by
exhaustive match, operator then operand then children, so an added operator is a compile error where the stamp is taken
rather than a `Hash` derived onto the public predicate types; the batching knobs, `postgres_invalid_values` and
`use_statistics` stay out, no skipped group holding a resumed row.

### D131 A table reads its blocks and every descendant's, projected to its columns; a parent's cold pass runs to the end
`Table` holds its blocks and those of each table `parents` and `partition_of` reach below it, each resolved under its own
DDL and projected by name to the table's columns: its own blocks' set, or with none, the declared columns its first
descendant's block names, every block naming the same of them; a filter or projection naming a descendant's own column
refuses. A table is listed where a block holds its rows or a descendant's. Rejected: one column set across descendants
(a child's own column refusing its parent); listing every declared table (a schema-only dump's reading empty); a cold
pass stopping at the last descendant's block (one may hold none). Code: `dump/descendants.rs`. Evidence:
`tests/descendants.rs` (D130).

### D132 Whose rows a table holds is chosen per read and stated per dump; a cold pass learns no mode
`Table` holds every block either mode reads and each mode's refusal, and `QueryOptions::inheritance` chooses at each
read, so a cold pass settles a parent with its descendants whatever the mode. Read alone, a partitioned table reads none
(I93), under the columns of its descendants' blocks, or with none, of its root's blocks. The provider takes the
mode per dump, as its other modes, its catalog listing a table by what its schema refuses at registration. Rejected: the
pass learning the mode (a `Table` bound to one, unlike a complete map's); `SET pgdump.inheritance` (listed at
registration; two registrations read both modes); the declared columns for an empty read (a generated column no block
names). Code: `dump.rs` (`Table::read`). Evidence: `tests/descendants.rs`, `pgdt`'s `tests/descendants.rs`.

### D134 A compressed chunk refuses wherever the DDL's walk reaches it; a chunk's cold pass runs to the end
A read refuses for each chunk the catalog marks compressed among the table and the descendants the DDL declares, so a
chunk the file lacks, which a restore lacks too, refuses nothing. In a TimescaleDB database a cold pass settling a table
with an ancestor runs to the end, any such table possibly a chunk (TS1) whose catalog blocks follow it. Rejected: the
catalog's own chunk list (a dump excluding the chunks refusing over rows its restore lacks); a chunk's pass stopping at
its block on `pg_dump`'s name order (any other file, or a chunk's schema sorting first, reading compressed rows short,
silently); a cold chunk read refused (its remedy reads as far); every table of the database to the end (a plain
table's cold read whole). Code: `dump/hypertables.rs` (`Verdict`), `dump/settle.rs` (`reads_descendants`, `KD161`).
Evidence: `tests/timescaledb.rs`.

### D135 A hypertable's findings are the file's diagnostics and each read's notes both
A root's own rows, absent chunks and an unreadable catalog are properties of the file (D19), what `pgdt info` and a
dump's drain hear, and each is also a `Warning` `PlanNote` on a read it bears on, so `pgdt query`, which prints of the
file's findings only its cache's identity (D21), says it where the answer differs from the server's; `ReadAsNull` is the
precedent. One function counts each for both. Rejected: per read only (`pgdt info` silent); `pgdt query` printing the
file's findings, every hypertable's, and none on a cold partial map; an unreadable catalog's note on a hypertable read
alone, its answer its root's rows either way. Code: `dump/hypertables.rs` (`Verdict::notes`), `timescaledb/findings.rs`.
Evidence: `tests/timescaledb.rs`.

### D136 What refuses a read is one finding of the file per root or hypertable, and no read's note
A partition root its partitions' rows are written through, and a hypertable with compressed chunks the file holds, are
each one `Warning` finding (D19), derived in the model by what the query layer's refusals read, so `pgdt info --json`
and a dump's drain name a root's leaf partitions where every block's TOC entry names one, and each compressed chunk the
DDL declares, or any the catalog names, no DDL held. No plan note: the read refuses, its error saying why, where D135's
notes are for a read that succeeds unlike the server's. Rejected: a clause on each parent's line in `pgdt info` (no leaf
named, and `info` reaching the query layer's rules); a finding per refusing table. Code: `span.rs`
(`DumpIndex::through_roots`), `timescaledb/findings.rs` (`compressed_chunks`), `dump/descendants.rs`. Evidence:
`tests/descendants.rs`, `tests/timescaledb.rs`.

### D137 TimescaleDB is read from its Apache source alone, its catalog by column name at any release; no chunk is decoded
pgdt's TimescaleDB code is written from its Apache-licensed `src/` and `sql/`, its docs and the generator's dumps, never
`tsl/`: a later clean-room decoder needs that held from the start. The catalog is read by column name at any release,
its recipe chosen by release or shape (TS3), so a release keeping what pgdt reads needs no pgdt release and one lacking
it refuses its hypertables. Rejected: refusing a release past the newest pin; plain inheritance with a warning over an
unreadable catalog; detection without the catalog (`compressed_data` columns); decoding chunks in tree (Timescale
License codecs, US 10,936,562) or never; `--no-timescaledb`, nothing to switch off. Reopens: `roadmap.md`'s Future item
"Compressed TimescaleDB chunks, decoded from a clean room". Code: `timescaledb/version.rs`, `dump/hypertables.rs`.
Evidence: TS2, TS3.

### D51 A segment's offsets are search bounds, and a resync is a real read
Sub-streams are contiguous byte-balanced runs cut through blocks in file order, so concatenating them *is* the serial
replay. A cut's offsets are search bounds, a row straddling one belonging to the earlier piece, and a piece resyncs by a
real read to a line start, never handing the scanner a mid-row byte, since a value can end in `\.` (I7 covers line
starts). Rejected: cutting at block boundaries, sparing a declared ordering its proof across them
(`summary::partition_orders`) at every table's balance.

### D52 The worker is fused, and the earliest failing piece is the error
A piece reads its range and parses each read in `spawn_blocking`; `scan_piece` does no I/O. Rejected: decode and parse
pools over a channel (the ratio is a property of the command); speculative splitting ahead of the leader; cross-block
pipelining. Pieces drain in file order so the same truncated file names the same byte. A failure only reading finds is
the lowest-indexed failed sub-stream's, after the rows before it; a resolution refusal comes from the plan (D54). Across
a DataFusion query's partitions the first refusal wins, each true; rejected: ordering them.

### D93 A dynamic filter's rows are evaluated only under `pgdump.dynamic_filter_rows`, off by default; its state is read at each group entered and each chunk
Off still prunes groups, cuts the sub-streams and stops a sorted block; the setting states intent, which "Two tunables"
does not refuse, and is the one way to the unclustered join's win. On, rows are evaluated in every block, less the
bounds an `IN` beside them implies outside any `Not`. The state is read at each group entered and each chunk. Rejected:
on by default (`dynamic-filter-join`'s costing row); DataFusion's flag as the switch (pruning goes with it); evaluating
only where statistics delimit groups (`KD33`'s tail); reading per row, per batch or at group entry alone; ceasing where
nothing is rejected (an explicit request, D77); dropping a static filter's implied bounds, which raise. Reopens: that
row's rows leg within its on leg's spreads (`KD55`); the per-chunk check priced.

### D95 A dynamic filter enters by `TablePartitions::under` alone, and the byte cut is made once per handle, at the first poll
A replay may drop any row a state it read rejects, so the filter is no `QueryOptions` field: the one entry point that
never resumes takes it (D77's reasoning). Planning stays at `scan()`; the first sub-stream polled cuts them all over the
groups the static filter and the state then keep, so D51's balance holds over what is read, at the plan's count; the
planned cut stands where nothing is ruled out or an order would be lost (`KD53`). Rejected: the cut held by
`TablePartitions`, outliving the filter a reset discards; the count re-planned (read at planning); a partition reading
its filter through the plan node, whose clones a reset node shares; seeking the scanner in place at a skip. Code:
`stream::DynamicPartitions`.

### D98 The typed mode nulls what its front end cannot hold, tested before decode where the block's count says it holds one
Under `UnrepresentableMode::Null` a value past the tiers the query's semantics cannot hold — the format spec's under
PostgreSQL's, the calendar's too under DataFusion's — is NULL to the batch, every filter leaf and statistics (D97); a
nested value holding one is NULL whole. It is tested lexically (D96) before decoding, only in a column whose block
counts one or carries no count. Rejected: nulling what fails to decode (an engine-tier value decodes; unparsable text
must still refuse); a tier option beside the semantics, two fields for one fact; testing every block. Reopens: another
front end, with semantics of its own; `arrow-cast` displaying past `chrono` (`upstream.md`, "UF2"). Code:
`UnrepresentableRead`.

### D99 The refuse mode refuses at planning, per materialized column, from the map's count over every block
Under `UnrepresentableMode::Refuse` a plan refuses the first column it materializes — projects, or hands DataFusion —
whose blocks count a value in the tiers its semantics cannot hold (D96, D98), before a row is read: over every block,
not the groups a filter keeps, which survive by statistics, and statistics never change an answer. A column only a
filter the library answers reads is not materialized, and compares every value in PostgreSQL's order (D56), so no read
refuses one. Rejected: refusing where a read reaches one (timing decides); over the kept groups; a read-time refusal
beside it, the count being exact (D96). Code: `stream::replay::plan::materialized_unrepresentable`. Evidence:
`a_selected_special_value_still_cannot_be_materialized`.

### D100 The untyped mode widens a column per table from the map's count, and compares it in each semantics
Under `UnrepresentableMode::Text` a column whose blocks count, over every one, a value in the tiers its semantics cannot
hold (D98) is `Utf8View`, settled once per table as the census is (D43). It compares in its widening's semantics alone:
in PostgreSQL's by its declared plan (D56), so `pgdt --where` keeps its order; in DataFusion's bytewise, so a pushed
filter stays `Exact` (D88); a key asked in the other is `None`. Rejected: text everywhere (an unmapped column's
refusals); per block (two blocks of one table typed apart); the declared plan in both semantics. Code:
`resolve::read_as_text`, `TableColumns`. Evidence: `the_untyped_mode_compares_its_text_column_in_each_semantics_order`.

### D103 The value modes are one option each for every column, fixed where a dump is opened, apart from the schema mode
`QueryOptions::unrepresentable` and `::postgres_invalid_values` are each one option for every column, on every surface
(a `pgdt query` flag, `PgDumpOptions`, a `pgdump.*` table option, a shell suffix), fixed where a dump is opened: the
text mode changes a schema at `PgDumpTable::build`. The invalid-values mode is a parse's too. An ignoring parse records
per column what it went past and keeps no statistic a read could contradict, so a `Default` parse tracking such a column
fails over the cache; `Strict`, a parse's alone, re-reads each block not marked checked: a verdict is the dump's, not
the gathering run's. Rejected: a record per block; a `SET`-able setting; a third `SchemaMode`; recording the mode;
refusing such a cache (D20); `Strict` trusting the record. Defaults: `Null` (D38); `Default`, as a restore.

### D119 A projection is the request's names, each once, in its order; the CLI repeats `--column`
A projection resolves each name against the table's columns, then its block's as a predicate's column does, emitting
them in the order asked; one asked twice is refused on the request (`QueryOptions::validate`) and one the table lacks
when the read is planned, before a row is replayed, so an embedder and `pgdt` refuse alike. Rejected: a repeat kept, as
SQL's select list keeps one (a field feeds one built column, and a batch would carry two fields of one name); `--columns
a,b`, a column name being free to hold a comma, so a list would invent a quoting grammar; `--columns ''` for none,
`--no-columns` saying it. Code: `QueryOptions::validate`, `project`. Evidence: `crates/pgdt-query/tests/projection.rs`.
