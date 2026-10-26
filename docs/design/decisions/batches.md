# Batches, streams and the leader

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. From mapped blocks to Arrow batches: zero-copy views and
the span that bounds them, the two passes and the seam between them, the partitioned replay, the dynamic filters a
DataFusion plan hands a scan, and the three value modes for what a type cannot hold. `pgdt-query`'s `stream`, `batch`
and `plan`.

### D46 Zero-copy views are top-level `Utf8View` only, and `max_source_span` alone bounds pinned bytes
Every arm but a top-level `Utf8View` copies, and a batch's views pin the chunks they point into until it flushes, so
`max_source_span` is the one bound on pinned bytes: `max_rows` and `max_bytes` count selected rows, which a filter makes
sparse. A source retaining by partition is charged no span (`RetainedUnit`), unsound where a query's cut spans several
units (`KD23`). Rejected: compacting views past a selectivity threshold. Evidence: `parallel-peak-rss`.

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
A projection resolves each name against a block's own columns as a predicate's column does, emitting them in the order
asked; a name the block lacks, or one asked twice, is refused on the request before a byte is read
(`QueryOptions::validate`), so an embedder and `pgdt` refuse alike. Rejected: a repeat kept, as SQL's select list keeps
one (a field feeds one built column, and a batch would carry two fields of one name); `--columns a,b`, a column name
being free to hold a comma, so a list would invent a quoting grammar; `--columns ''` for none, `--no-columns` saying it.
Code: `QueryOptions::validate`, `project`. Evidence: `crates/pgdt-query/tests/projection.rs`.
