# P35.11 notes — the trees designed

The module tree every library crate and `pgdt` now wants, drawn from
[`concepts.md`](concepts.md) and the four reviews under
`runs/seams-review-20261011/` (taken at `2df93720`, the code unchanged since),
with each taste call settled with the maintainer. No code moved. This doc
holds the design until the last cut toward it lands; `concepts.md` records
the tree as it stands, and each cut moves it.

## Across crates

- **Recording a row into a block's census is pgdt-model's.** `census_row`
  leaves pgdt-dump's `map.rs` for `BlockCensusExt::observe_row` in
  pgdt-model's `block.rs`, beside `ArrayShapeExt::observe` and
  `BlockCensusExt::merge`: it is defined over model types and core's
  `split_fields` alone, and both passes that record — dump's mapping, query's
  backfill — sit above model. "What a pass records of a block's rows" is then
  model's record and how a row adds to it, and codec's tier test. No persisted
  shape changes.
- **A seventh crate, `pgdt-mapping`, between {pgdt-dump, pgdt-codec} and
  pgdt-query**: the mapping pass that extends a map with its observers
  attached — `map_forward` and the whole-file map, the back-fill, the
  gatherer and its strict arm, the statistics request, the account, the
  stored contract. The `seams` skill's test passes on every count it reads
  (at `26d0bf23`): `stream/map.rs`, `stream/backfill.rs`, `gather/` and
  `statistics.rs` — about 5,600 production lines — name **no** item of the
  rest of pgdt-query, which names 19 of theirs; they need core, model, io,
  dump, codec, `bytes`, `tracing`, `serde` and Arrow's `DataType`, and shed
  `arrow`'s arrays, `async-stream`, `futures`, `tokio` and `memchr`. Gathering
  reads codec keys, core's unescape and `DataType` and builds no array, so the
  spec's "statistics gathering sits above the codec" still holds, and the
  boundary the dump crate's `internal` existed for — 20 of its 22 items read by
  pgdt-query alone, 11 of 12 production reads from `stream/map.rs` and
  `backfill.rs` — becomes the new crate's. Co-change could not decide it:
  these were single files until 35.6, and seven library commits have landed
  since the carve. The spec's crate list and D68 change with it.
- **The interior split moves to pgdt-mapping with the pass it serves**:
  every caller of `leader.rs`'s items is there, and once `census_row` is
  model's it reads only the scanner's types (`CopyScanner`, `Event`,
  `PieceEndings`, `RowEnding`, `ScanOptions`), io, model and core. "It decodes
  nothing, so it sits by the scanner" placed it while the only alternative was
  the query layer. The preamble's prepass and the tiling diagnostics stay
  dump's: they read the file with the scanner and the DDL grammar, and
  pgdt-query's pass 1 reads the prepass too.
- **The SQL lexer moves to pgdt-dump**, `lex.rs` beside `scan.rs`: `Lexer`,
  `Region`, `LineLex`, `Quote`, `ident_cont`, `standard_conforming_strings`
  have production readers there alone. **A type name's canonical form stays
  in pgdt-core** as `type_name.rs` (`canonical_type_name`,
  `parse_type_name`), read by codec and model — not model itself, whose edits
  ask whether the cache format bumps.

## pgdt-core and pgdt-model

pgdt-core keeps `error.rs`, `instrument/` and `copy.rs` as they are — its
modules are the four things its doc lists — and `lex.rs` becomes
`type_name.rs`, the lexer having gone to pgdt-dump. `copy.rs`'s header and
field codec stay one file, under every limit. pgdt-model gains
`BlockCensusExt::observe_row` in `block.rs` and is otherwise unchanged.

## pgdt-io

The `io/` layer goes: `lib.rs` and `io.rs` were two re-export lists with
nothing between them. The crate root lists the concept map's three concepts —
the byte sources, memory and parallelism, stopping — and the sources' arms
nest under their contract. Sizing keeps the names `parallelism` and `memory`:
"budget" already names datafusion-pgdump's session allowance and the `.xz`
arm's `XzBudget`.

```
lib.rs                      crate doc and API re-exports (absorbs io.rs)
source.rs                   the read contract: ByteRangeSource, RemoteIdentity, WaitPolicy, the default chunk
  source/pool.rs            BufferPool, PooledBuffer: what every source reads into (out of local.rs)
  source/local.rs           LocalFileSource
  source/xz.rs              what both .xz sources share: LiveBlock, piece_of, the short read, the advice, walk_seek_table
    xz/blocks.rs            BlockCache, XzBudget
    xz/file.rs              XzSource
    xz/fetched.rs           FetchedXzSource
  source/origin.rs          Origin, the probe, open and open_local
  source/remote.rs          RemoteSource (http)
partition.rs                Partitioning, PartitionBoundaries, RetainedUnit, PartitionRead, cut
parallelism.rs              Parallelism, WorkerMemory, worker_count
memory.rs                   the allowance and its discovery, the MEMORY_* constants
cancel.rs                   Cancellation
```

`partition` depends on `parallelism` and nothing points back, which removes
the `source` ↔ `parallelism`, `xz` ↔ `blocks` and `local` ↔ `blocks` cycles,
and `fetched` stops reaching into `XzSource`'s file. `origin` → `remote` is a
dispatcher and its `http` arm, and stays. The 2760-line `tests.rs` splits
beside each module, in the order it already runs.

## pgdt-dump

"Preamble" means one thing — the region before the first data, PostgreSQL's
own word — and the statement grammar applied across the whole file is named
for what it parses.

```
lib.rs
scan.rs (+ scan/tests.rs)   the event scanner, ChunkCarry, row endings
ddl.rs                      the statement grammar: classify_statement, StatementShape, parse_connect
  ddl/lex.rs                keyword, quote, paren and literal helpers
  ddl/table.rs              CREATE TABLE, ALTER TABLE references, columns, constraints
  ddl/types.rs              TYPE, DOMAIN, COLLATION, EXTENSION, the ALTER TYPE bodies
  ddl/refs.rs               role and tablespace cross-references
  ddl/metadata.rs           dump_metadata_from_spans and the ALTER TYPE folds
map.rs                      the Builder state machine
  map/statement.rs          StatementScan and statement completion (from preamble.rs)
  map/toc.rs                TOC comment-block lexing
  map/tiling.rs             check_tiling, attach_text, the map's diagnostics (from load.rs)
lex.rs (+ lex/tests.rs)     the SQL lexer (from pgdt-core)
preamble.rs                 the region's prepass: scan_preamble, preamble_only (was load.rs)
cache.rs                    load, save, claim, status
  cache/mode.rs             CacheMode, StrictIdentity
  cache/identity.rs         every identity: SourceIdentity, WeakIdentity, OriginMatch, SourceWatch
  cache/file.rs
```

The cut repoints `measure.py`'s declared files (`load.rs`, `preamble.rs`,
`leader.rs`) and `emitter_register.py`'s reader list.

## pgdt-codec

**By type family, never by role.** Everything PostgreSQL says about one type —
its reader, its renderer, its refusal classifier, its order key — is one
family module under `value/`, and the API namespaces declare their items from
the families by `pub use`. The edges decided it: every classifier's only
caller is the order key; the key reached into the decoders' internals
(`extract_offset`, `parse_time_of_day`, `interval_parts`, `date_days`,
`network_in`, `NumericColumn`, the numeric bounds), each exported `pub(crate)`
for it alone; int's, oid's and macaddr's readers, and macaddr's renderer,
were in `key/order.rs`. Organized by role, those exports stay widened only so
one tree can reach the other.

**The comparison register is its own namespace**, `compare`, beside `key`:
how PostgreSQL orders a value is the register and the keys, and `pgtype`
keeps only a declared type's Arrow mapping. **The strict parse's codec part is
one module**, `grammar`: `compare` imports `TextGrammar` from it, since a
nested plan that cannot be compared records the grammar refusing it, and
nothing imports back.

```
lib.rs              crate doc, the namespaces, internal
decode.rs           namespace: Unread, Read; pub use of each family's decode_* and render_*
key.rs              namespace: ValueKey; OrderKey, Side, order_key, read_key, literal_key, special_order_key, compare_keys — the dispatch over families
compare.rs          namespace: the comparison register — CompareKind … ComparisonPlan, ComparisonSemantics, NestedCompare, collation, comparison_for, Discrete, the DataFusion divergences (pgtype/discrete.rs and pgtype/divergence.rs folded in)
pgtype.rs           namespace: a declared type to an Arrow type — TypeOutcome, NestedPlan, Position, builtins, user types, domains, extensions
nested.rs           namespace: the nested literal, one grammar with two scanners (D45) — pub use of value/nested.rs's 18 items, off the crate root
resolve.rs          a COPY header's columns joined against the declared schema (+ resolve/tests.rs, out of line)
grammar.rs          which grammar reads a column's text and whether PostgreSQL refuses a field: TextGrammar, text_grammar, Unchecked, column_grammar, unchecked_positions, field_refused, grammar_refuses, accepted_form, nested_accepted_form — dispatching to each family's classifier
unrepresentable.rs  a field's tier against its Arrow type
value.rs            the families, and what their parsers share
  value/libc.rs     the C library calls PostgreSQL's input functions make, as glibc answers them: is_c_space, HEX_NIBBLE, both strtod models, both strtoints
  value/digits.rs   the digit writers
  value/bool.rs, int.rs (oid with it), float.rs, bytea.rs, uuid.rs, text.rs (char(n)'s typmod), network.rs (inet, cidr, macaddr), geometric.rs, bit.rs
  value/numeric.rs  digits, typmod rounding, render_decimal, NumericKey, numeric_unread, the storage bounds
    numeric/spelling.rs   NumericSpelling, v14 and v16
  value/datetime.rs date, time, timestamp, interval: readers, renderers, the PostgreSQL-epoch forms (was decode/stored.rs), keys
    datetime/grammar.rs   today's datetime_in.rs, moved whole and still exempt
  value/json.rs     json_in, the jsonb parser and its key
  value/nested.rs   arrays, records, ranges: the output scanner, the literal scanner, NestedKey, the range algebra
```

**The nested literal is a namespace**, `nested`, beside `decode`, its 18 items
leaving the crate root; only `resolve`'s items and `UnrepresentableMode` stay
there, as the crate's way in.

`decode/refusal.rs` and `decode/stored.rs`, named for their visibility, are
gone into their families; `key/order.rs`'s six sections go to `numeric`,
`network`, `json`, `nested`, `key.rs` and `grammar.rs`. What each family's
parsers duplicate is left duplicated and side by side — two glibc `strtod`
models, two `strtoint`s, decode's numeric lexers beside `NumericKey::parse`,
`json_in`'s grammar beside jsonb's — since this phase changes no behaviour.

## pgdt-mapping

The mapping pass, whole: a map extended from its frontier with its observers
attached, the pieces of a block and their fold, the back-fill, and what a
pass gathers and what holding it costs. Its parts come from pgdt-query's
`stream/map.rs`, `stream/backfill.rs`, `gather/` and `statistics.rs`, and
pgdt-dump's `leader.rs`.

```
lib.rs                      crate doc; the API
forward.rs                  map_forward, splice, close_copy_block: a map extended from its frontier, each block closed
  forward/save.rs           SaveThrottle: the cache saved on a throttle
whole_file.rs               map_file, build_index, build_map, MapRun: a whole file's map, resumed and saved
backfill.rs                 a mapped block re-read for what it lacks: reread_rows, announce_read_loop, the back-fill
interior.rs                 the interior split: a piece scanned alone, the pieces folded (pgdt-dump's leader.rs)
  interior/region.rs        scan_region, the scheduler
statistics.rs               namespace: what a pass gathers, and what holding it costs
  statistics/request.rs     StatisticsRequest, Selection, Level, Target, StatisticsBackfill
  statistics/account.rs     StatisticsAccount, Term, Charge, StatisticsTerms, StatisticsHeld
  statistics/stored.rs      what a column's statistics are stored against, read by pruning and summaries (out of gather/tracking.rs)
  statistics/gather.rs      the observer a block gets: observer_for, observer_tracking, bounded_columns
    gather/gatherer.rs, column.rs, bounds.rs, order.rs, dictionary.rs, charge.rs
    gather/strict.rs        the strict check, an arm of the block observer
```

The `map` ↔ `backfill` cycle goes: what `backfill` imported from `map` (the
save throttle, `cancelled_read`, the shortfall reports) sits in `forward` and
`whole_file`, and `whole_file` calls `backfill` downward. **The strict check
stays an arm of the observer**: a gathering observer carries its
`field_checks`, a checker alone bills the statistics account, and
`observer_for` picks gatherer, checker or both, so beside gathering is where
its edges run one way.

## pgdt-query

**The predicate is one tree**, its resolution gathered from
`stream/resolve.rs`, and **a group's verdict is the predicate's**: the truth
sets lean on `ResolvedTerm`'s fields, `eval_value` and `wants_equal`
throughout, and touch statistics only through `GroupStatistics`, which
pruning implements.

```
lib.rs
options.rs             QueryOptions, ScanExtent: what a query asks for (out of batch.rs)
render.rs              an Arrow value read back as PostgreSQL text: render_field, render_field_into, decode_field (out of batch.rs)
batch.rs               RowBatcher
  batch/chunks.rs      RetainedChunks: views over the retained read buffers (D46)
  batch/builder.rs     ColumnBuilder, column_of
predicate.rs           Expr, Predicate, PredicateOp, Truth, Membership
  predicate/resolve.rs a filter resolved against a block: resolve_term, resolve_membership, resolve_expr, resolve_loosened
  predicate/notes.rs   ComparisonNote, the divergences, the refusal reasons
  predicate/eval.rs    a row's three-valued verdict: eval, eval_value, matches, for_rows, Lookup
  predicate/truths.rs  a group's verdict: TruthSet, GroupStatistics, KeyedBounds, the truths methods
prune.rs               which row groups a filter rules out: GroupStatistics for a stored group
summary.rs             a table's totals from its statistics; partition_orders for the planner
stream.rs              pass 1 for a query, then pass 2 (D48)
  stream/table.rs      TableStream, table_stream, BlockingTableIter, table_schema, read_table (from batch.rs)
  stream/partitions.rs TablePartitions, DynamicPartitions, table_stream_partitions, Cut
  stream/resume.rs     ResumeToken, InCopyResume, resume_state, snapshot, query_fingerprint, hash_expr
  stream/note.rs       PlanNote, EarlyStop
  stream/mapping.rs    pass 1 for a query, over pgdt-mapping: map_for_query, validate_request (was map_query.rs)
  stream/replay.rs     pass 2: the replay loop, row_text
    replay/plan.rs     ReplayPlan, plan_blocks, TableColumns, resolve_block, refuse_metadata_level, project
    replay/segment.rs  Segment, StreamShared, activate
    replay/partition.rs  cut_blocks, plan_partitions
    replay/dynamic.rs  DynamicFilter, DynamicRead
```

`render.rs` is not the batcher's: reading an array back assembles nothing,
and its readers are `summary`, datafusion-pgdump's filter and `pgdt`'s
printer. `read_table` moving to `stream/table.rs` breaks the `batch` ↔
`stream` cycle; `TableColumns` in `replay/plan.rs` is imported downward by
`table_schema`. Pruning and summaries are root siblings, one reader and one
consumer each, not a third crate's `statistics` module holding "what reads
them". Today's cycles (map↔backfill, table↔map_query, table↔plan,
table↔replay, table↔segment, plan↔segment, dynamic↔table_partitions through
`Cut`) all fall to this tree.

`predicate/tests.rs` splits along that tree; the per-type comparison tests
drive `Predicate` resolution, its notes and query's errors, so they stay.
Two oracle tests state properties of the key alone —
`the_key_is_a_total_order_over_every_committed_value` and
`golden_order_is_pinned_to_the_format_version` — and move to pgdt-codec
beside `key`, which dev-depends on pgdt-dump (`preamble_only`) and tokio for
them; `layering.py` reads no dev-dependency. `oracle_register.py:216` names
`predicate/tests.rs` and a test in it, and follows the test.

## pgdt

One module a subcommand, each owning its flags: `Command::Parse(ParseArgs)`
and its siblings, each `#[derive(Args)]` struct in its subcommand's module, so
a flag's help text sits beside its behaviour. Help output is unchanged
(`tests/help_text.rs`).

```
main.rs                  Cli, Invocation, main; runtime setup and dispatch to <cmd>::run
version.rs               version(), RELEASED, same_str
args.rs                  the flag groups subcommands share: IdentityArgs, OverwriteArgs, ReadFlags, the Cli* enums, value parsers
parallel.rs              ParallelArgs … QueryPasses and their displays
source.rs                Opened, open_with_cache, open_for_scan, unusable_cache_message, naming the source
interrupt.rs             InterruptGuard, die_by
parse.rs                 ParseArgs, run, resume_notice, statistics_request
info.rs                  InfoArgs, run, info_offline, report
  info/text.rs           print_index, print_metadata (which parse also calls), DatabaseHeadings
  info/json.rs           IndexJson and its parts, print_index_json
  info/labels.rs         arrow_type_label, range_label, type_kind_*, span_summary
  info/statistics.rs     today's info_statistics.rs
query.rs                 QueryArgs, run, the Slot merge, announce_*, print_batch, projection
  query/filter.rs        the filter's two front-end spellings, one grammar
    filter/term.rs       --filter's term grammar (from main.rs)
    filter/expr.rs       --where (today's where_expr.rs), its leaves from term.rs
alloc.rs, profile.rs, introspect.rs   unchanged
```

`tests.rs` splits beside the modules it tests.

## datafusion-pgdump

DataFusion's expressions read as the library's filter tree are one concept:
`filter.rs` holds the leaf builders both translations use (`compared`,
`member`, `null_term`, `boolean_term`, `is_float`, the literal's text),
`filter/pushdown.rs` the static translation and its `Exact`/`Inexact`
answer, `filter/dynamic.rs` (with its `tests.rs`) the dynamic filters.
`unrepresentable.rs`, a UDF and its guard, stays where it is; every other
file is one DataFusion extension point already. datafusion-cli-pgdump is
left alone: `lib.rs` is upstream's `main.rs`, laid out for the diff.

## Tests

Each cut module's tests move beside it, split where its tests file mirrors
the cut, and each integration test moves to the lowest crate whose API it
exercises — `pgdt-mapping/tests/` takes those `pgdt-mapping`'s API alone
covers. An inline test module moves out of line only where its file is cut
or past a soft limit — `resolve.rs`, and `scan.rs` as the lexer arrives
beside it; one in a file the design leaves whole is already beside what it
tests. The differential tests' LCG stays duplicated between codec's and
query's `differential.rs`: a dozen lines, and sharing it would cost a feature
or a crate edge.

## The concept map's next version

Each concept's claims as the design draws them. Every **Not in one place**
line in today's map falls to it; a concept still spanning crates does so
along the layering, each part's reason given.

| Concept | Claims |
|---|---|
| The byte sources | io: `source.rs`, `source/` |
| Memory and parallelism | io: `partition.rs`, `parallelism.rs`, `memory.rs`; datafusion-pgdump: `budget.rs`; pgdt: `parallel.rs`, the flags a front end spells |
| Stopping | io: `cancel.rs`; namespace-init; pgdt: `interrupt.rs`, the binary's signal |
| Failure | core: `error.rs` |
| Self-measurement | core: `instrument/`; pgdt: `introspect.rs`; peak-rss |
| The `COPY` format | core: `copy.rs` |
| SQL lexing | dump: `lex.rs`; core: `type_name.rs`, read by codec and model too |
| The scanner | dump: `scan.rs` |
| The file map | model: `span.rs`, `diagnostic.rs`; dump: `map.rs`, `map/` |
| The mapping pass | mapping: `forward.rs`, `forward/`, `whole_file.rs`, `backfill.rs`, `interior.rs`, `interior/`; query: `stream/mapping.rs`, pass 1 of a query, which reads the replay's resume token |
| The declared schema | model: `catalog.rs`; dump: `ddl.rs`, `ddl/`, `preamble.rs` (the region's prepass) |
| The cache | dump: `cache.rs`, `cache/`; datafusion-pgdump: `dump.rs` |
| What a pass records of a block's rows | model: `block.rs`; codec: `unrepresentable.rs` |
| Type mapping | codec: `pgtype.rs`, `resolve.rs` |
| Values | codec: `decode.rs`, `nested.rs`, `value.rs`, `value/`; query: `render.rs`, an array read back (D74) |
| Comparison semantics | codec: `compare.rs`, `key.rs` (each family's key in its `value/` module) |
| Arrow batches | query: `options.rs`, `batch.rs`, `batch/` |
| Filtering | query: `predicate.rs`, `predicate/`, `stream/replay/dynamic.rs`; datafusion-pgdump: `filter.rs`, `filter/`, `unrepresentable.rs`; pgdt: `query/filter.rs`, `query/filter/` |
| Strict parse | codec: `grammar.rs`; mapping: `statistics/gather/strict.rs` |
| Statistics | model: `statistics.rs`; mapping: `statistics.rs`, `statistics/`; query: `prune.rs`, `summary.rs`; datafusion-pgdump: `statistics.rs`; pgdt: `info/statistics.rs` |
| The replay | query: `stream.rs`, `stream/table.rs`, `stream/partitions.rs`, `stream/resume.rs`, `stream/replay.rs`, `stream/replay/` but `dynamic.rs` |
| What a query tells its caller | query: `stream/note.rs`; datafusion-pgdump: `report.rs`; pgdt: `query.rs`'s `announce_*`, a front end's printing |
| The DataFusion provider, the DataFusion CLI, the binary's build | unchanged |
| The `pgdt` CLI | pgdt: `main.rs`, `version.rs`, `args.rs`, `source.rs`, `parse.rs`, `info.rs`, `info/` but `statistics.rs`, `query.rs` |

## The cuts

One `STATUS.md` row a cut, from 35.13, bottom up so each lands on crates
already designed; each owns whole concepts. The register goes before the
families so the families' keys import from a `compare` that exists; the
mapping crate is carved before query's tree, so query is redrawn over what is
left. 35.17 is the largest, values and order keys being one cluster by the
edges. Each cut moves its concepts' claims in `concepts.md`.

## Negative results

- **Statistics apart from the rest of pgdt-query is not a crate**: the edges
  run both ways, about 10 items one way and 21 the other.
- **pgdt-codec's values group** (`decode/`, `nested.rs`, `datetime_in.rs`) is a
  one-way lower group that would shed `arrow-schema`, `arrow-buffer` and
  `serde`, but it changes with the rest of the codec as often as alone (29
  commits both, 22 it alone, 26 the rest alone), so it stays; and pgdt-dump's
  DDL grammar, a leaf, changed alone in 8 of 32 commits — too weak to split.
- **No merge**: "pub that nothing outside the pair reads" fires for
  codec+query (40 of 43 `internal` items read by query alone) and dump+query,
  and co-change is high for both (66% and 69% of the smaller crate's
  commits, sweeps excluded, over 321 commits mapped back across the carve);
  D74's reasons for both boundaries stand, and dump's surface is what
  `pgdt-mapping` takes over.
- **Two of the query review's cycles were not there**: `stream/resolve.rs`
  imports no sibling, and `dynamic`'s cycle runs through `Cut`, in
  `table_partitions.rs`, not through `table.rs`.
- **The per-type comparison tests are not codec's**: they drive `Predicate`
  resolution, its notes and query's errors; only two oracle tests are the
  key's alone.

