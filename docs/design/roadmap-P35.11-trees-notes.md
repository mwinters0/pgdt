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
`BlockCensusExt::observe_row` in `block.rs`.

pgdt-model's files stay, and three items move so its edges run one way —
`block` below `statistics`, both below `span`, `diagnostic` below `span`.
`BlockObserver` answers `BlockGathered` and `IgnoredRefusals`, so it sits
beside them in `statistics.rs`, which reads `block`'s `Unrepresentable`;
`union_census` folds a table's `CopyBlock`s, so it sits in `span.rs`, which
reads `block`'s census; `TilingIssue` is what a diagnostic reports, and moves
to `diagnostic.rs`, which `span`'s `DumpIndex` reads. 35.23 cut it.

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
  source/xz.rs              what both .xz sources share: LiveBlock, piece_of, the short read, walk_seek_table
    xz/blocks.rs            BlockCache, XzBudget, the partition advice
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
the `source` ↔ `parallelism` and `local` ↔ `blocks` cycles, and `fetched`
stops reaching into `XzSource`'s file. `xz_partition_advice` reads
`BlockCache` and `XzBudget` calls it, so it moves beside them into
`xz/blocks.rs`, which removes `xz` ↔ `blocks` — the 35.23 cut. `origin` →
`remote` is a dispatcher and its `http` arm, and stays. The 2760-line `tests.rs` splits
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
cache.rs                    namespace: the cache's doc, declaring its API
  cache/identity.rs         every identity and which bind: SourceIdentity, WeakIdentity, OriginMatch, SourceWatch, StrictIdentity
  cache/file.rs             the file, and what a read of it finds: CacheFile, its envelope and compression shape, CacheStatus
  cache/store.rs            a cache read against its source and written beside it: load, load_offline, claim, save, colocated_path
  cache/mode.rs             CacheMode, CacheLoad: where a caller's cache is, and what it asked of it
```

The cut repoints `measure.py`'s declared files (`load.rs`, `preamble.rs`,
`leader.rs`) and `emitter_register.py`'s reader list.

**`cache.rs` declares and defines nothing**, as an API namespace does: the
operations it defined read `file` and `identity` and `mode` read them, while
`file`'s reader answers `CacheStatus` and `identity`'s `SourceWatch` reads
`StrictIdentity`. So `CacheStatus` with its envelope and shape sits in
`file.rs`, `StrictIdentity` in `identity.rs`, and the operations in
`store.rs`, running `identity` ← `file` ← `store` ← `mode`. 35.23 cut it.

## pgdt-codec

**By type family, never by role.** Everything PostgreSQL says about one type —
its reader, its renderer, its refusal classifier, its key type — is one family
module under `value/`, and the API namespaces declare their items from the
families by `pub use`. The edges decided it: every classifier's only caller is
the order key; the key reaches into the decoders' internals (`extract_offset`,
`parse_time_of_day`, `interval_parts`, `date_days`, `network_in`,
`NumericColumn`, the numeric bounds), each exported `pub(crate)` for it alone;
int's and oid's readers sit in `key/order.rs`. Organized by role, those
exports stay widened only so one tree can reach the other.

**A family reads no `key`.** `OrderKey` holds the families' key types
(`NumericKey`, `Jsonb`, `NetworkKey`), so `key.rs` sits above them and keeps
every function that builds an `OrderKey` — `timetz_key`, `network_key`,
`macaddr_key`, `render_macaddr`, `jsonb_key` beside `field_key` and
`literal_key` — as the dispatch over families; a family keeps its readers and
its key type. `field_key` reads `DateTimeInput` from `value/datetime/grammar.rs`
directly. `first_difference` is `Jsonb`'s order's and the nested keys' both, so
it sits in `value.rs`, which reads no family. `decode.rs` defines `Read` and
`Unread`, which every family reads, and reads no family.

**A declared type's catalog walk is below everything reading one.** `compare`,
`grammar`'s `text_grammar`, `pgtype` and the datetime grammar each read
`find_type`, `array_element`, `domain_terminal`, the typmods and `Position`, and
`pgtype` reads `compare`, so the walk is its own module, `declared`, beneath
all four.

**The comparison register is its own namespace**, `compare`, beside `key`: how
PostgreSQL orders a value is the register and the keys, and `pgtype` keeps a
declared type's Arrow mapping. The built-in table answers both questions
(D40), so `pgtype` reads `compare`'s types; `comparison_for` and its walk read
the table, so they are `compare/walk.rs`, a child the parent never reads.

**The strict parse's codec part is one tree**, `grammar`. `compare`'s plans
carry `TextGrammar` and its walk calls `text_grammar`, while the refusal checks
read the plans 104 times, so `grammar.rs` holds the type's grammar alone and
the checks are its child. `grammar_refuses` and `checked_key` call each other —
an array of `box` — and the checks are `checked_key`'s only callers, so it and
`checked_range` leave `key/order.rs` for `grammar/check.rs`.

```
lib.rs              crate doc, the namespaces, internal
declared.rs         a declared type read against the catalog: split_typmod, builtin_name, find_type, companion_range, Visits, array_element, domain_terminal, domain_not_null, typdelim, is_box, NumericTypmod, IntervalQualifier, Position
decode.rs           namespace: Unread, Read; pub use of each family's decode_* and render_*
key.rs              namespace: ValueKey; OrderKey, Side, field_key, order_key, read_key, literal_key, special_order_key, compare_keys, and every builder of an OrderKey — the dispatch over families
compare.rs          namespace: the comparison register — CompareKind … ComparisonPlan, ComparisonSemantics, NestedCompare, Discrete, collation, the DataFusion divergences
  compare/walk.rs   a declared type's plan: comparison_for, through the catalog and the built-in table
pgtype.rs           namespace: a declared type to an Arrow type — TypeOutcome, NestedPlan, the built-in table, user types, domains, extensions; pub use of Position
nested.rs           namespace: the nested literal, one grammar with two scanners (D45) — pub use of value/nested.rs's 18 items, off the crate root
resolve.rs          a COPY header's columns joined against the declared schema (+ resolve/tests.rs, out of line)
grammar.rs          which grammar reads a column's text: TextGrammar, Geometric, text_grammar
  grammar/check.rs  whether PostgreSQL refuses a field: field_refused, grammar_refuses, checked_key, checked_range, column_grammar, Unchecked, unchecked_positions
  grammar/form.rs   the form a filter's literal has to take: accepted_form, nested_accepted_form
unrepresentable.rs  a field's tier against its Arrow type
value.rs            the families, and first_difference
  value/libc.rs     the C library calls PostgreSQL's input functions make, as glibc answers them: is_c_space, HEX_NIBBLE, both strtod models, both strtoints
  value/digits.rs   the digit writers, the hex pair among them
  value/bool.rs, int.rs (oid with it), float.rs, bytea.rs, uuid.rs, text.rs (char(n)'s typmod), network.rs (inet, cidr, macaddr; NetworkKey), geometric.rs, bit.rs
  value/numeric.rs  digits, typmod rounding, both spellings, render_decimal, NumericKey, numeric_unread, the storage bounds
  value/datetime.rs date, time, timestamp, interval: readers, renderers, the PostgreSQL-epoch forms (was decode/stored.rs), datetime_read, interval_read
    datetime/grammar.rs   today's datetime_in.rs, moved whole and still exempt
  value/json.rs     json_in, the jsonb parser and Jsonb
  value/nested.rs   arrays, records, ranges: the output scanner and the literal scanner
    nested/key.rs   NestedKey, nested_key, compare_nested, the range algebra
```

**`value/numeric.rs` is one file**: `NumericSpelling` reads `NumericColumn`,
`typmod_round`, `radix_to_decimal` and the bounds that the typmod reader beside
it shares, so a `spelling` child would read its parent and be read by it.

**The nested literal is a namespace**, `nested`, beside `decode`, its 18 items
leaving the crate root; only `resolve`'s items and `UnrepresentableMode` stay
there, as the crate's way in.

`decode/refusal.rs` and `decode/stored.rs`, named for their visibility, are
gone into their families; `key/order.rs` goes to `numeric`, `network`, `json`,
`int`, `datetime`, `nested/key.rs`, `grammar/check.rs` and `key.rs`. What each
family's parsers duplicate is left duplicated and side by side — two glibc
`strtod` models, two `strtoint`s, decode's numeric lexers beside
`NumericKey::parse`, `json_in`'s grammar beside jsonb's — since this phase
changes no behaviour.

35.17 cut it.

## pgdt-mapping

The mapping pass, whole: a map extended from its frontier with its observers
attached, the pieces of a block and their fold, the back-fill, and what a
pass gathers and what holding it costs. Its parts come from pgdt-query's
`stream/map.rs`, `stream/backfill.rs`, `gather/` and `statistics.rs`, and
pgdt-dump's `leader.rs`. No moved item reads pgdt-query: `stream/mapping.rs`
(today's `map_query.rs`), pass 1 of a query, stays there and reads down into
`forward` and `backfill`, as `prune`, `summary` and `pgdt` read `stored`.

```
lib.rs                      crate doc; the API
forward.rs                  map_forward, splice, close_copy_block, cancelled_read, report_shortfall: a map extended from its frontier, each block closed
  forward/save.rs           SaveThrottle: the cache saved on a throttle
whole_file.rs               map_file, build_index, build_map, MapRun: a whole file's map, resumed and saved
backfill.rs                 a mapped block re-read for what it lacks: reread_rows, announce_read_loop, merge_ignored, the back-fill
interior.rs                 the interior split: a piece scanned alone, the pieces folded (pgdt-dump's leader.rs)
  interior/region.rs        scan_region, the scheduler, Shortfall
statistics.rs               namespace: what a pass gathers, and what holding it costs
  statistics/request.rs     StatisticsRequest, Selection, Level, Target, StatisticsBackfill
  statistics/account.rs     StatisticsAccount, Term, Charge, StatisticsTerms, StatisticsHeld
  statistics/stored.rs      what a column's statistics are stored against, read by pruning and summaries; Summand (out of gather/tracking.rs)
  statistics/gather.rs      the observer a block gets: observer_for, observer_tracking, bounded_columns, checker
    gather/gatherer.rs      Gatherer, Sizing
    gather/column.rs        ColumnGatherer, GroupState, Observed, and the per-group dictionary (today's dictionary.rs)
    gather/bounds.rs        BoundsGatherer, Refused
    gather/order.rs         RowOrder, ViewOrders
    gather/clip.rs          Clipped: a bytewise value's head, which a bound and a row order both hold (D76)
    gather/charge.rs
    gather/strict.rs        the strict check, an arm of the block observer: FieldCheck, check_row, strict_unchecked
```

`whole_file` reads `forward` and `backfill`, and `backfill` reads `forward`
(`cancelled_read`, `report_shortfall`) and `forward/save`, so the `map` ↔
`backfill` cycle goes and nothing reads back. `merge_ignored` is
`backfill_statistics`'s alone. `interior/region` reads its parent's piece and
fold, and the parent reads nothing of it. `statistics.rs` declares by `pub
use` alone; `request` and `account` read neither each other nor it.

**The strict check stays an arm of the observer**: a gathering observer
carries its `field_checks`, a checker alone bills the statistics account, and
`observer_for` picks gatherer, checker or both. `checker` builds a `Gatherer`
and `gatherer` reads `strict`, so `checker` sits in the `gather` parent beside
`observer_for`, which reads its children and is read by none.

Today's `gather/` is one cycle, and these break it. `Observed` is what
`ColumnGatherer::observe` answers, so it is `column`'s. A refused field is
first met keying it, in `bounds`, which `column` and `gatherer` read, so
`Refused` is `bounds`'. `order` read `bounds` for `Clipped` alone, so
`Clipped` and its clip go to `clip`, which both read. `stored` takes
`Summand`, since `keeps_sums` answers from it and `column` then reads it
downward. **The dictionary is the column group's texts**:
`DictionaryGatherer::observe` reads only `GroupState`'s texts, so it is folded
into `column`.

35.18 cut it.

## pgdt-query

**The predicate is one tree**, its resolution gathered from
`stream/resolve.rs`, and **a group's verdict is the predicate's**: the truth
sets lean on `ResolvedTerm`'s fields, `eval_value` and `wants_equal`
throughout, and touch statistics only through `GroupStatistics`, which
pruning implements. **`stream.rs` is the top of the stream** — pass 1 for a
query, then pass 2 — and no child reads it.

```
lib.rs
options.rs             QueryOptions, ScanExtent: what a query asks for (out of batch.rs)
render.rs              an Arrow value read back as PostgreSQL text: render_field, render_field_into, decode_field (out of batch.rs)
batch.rs               RowBatcher
  batch/chunks.rs      RetainedChunks, SourceChunk, push_utf8view_field: views over the retained read buffers (D46)
  batch/builder.rs     ColumnBuilder, its list and struct parts, the append_* helpers, text_refused
predicate.rs           Expr, Predicate, PredicateOp, Truth, Membership
  predicate/resolve.rs a filter resolved against a block: ResolvedTerm, ResolvedMembership, ResolvedExpr, Lookup, resolve_term, resolve_membership, resolve_expr, resolve_loosened
  predicate/notes.rs   ComparisonNote, the divergences, the refusal reasons
  predicate/eval.rs    a row's three-valued verdict: eval, eval_value, matches, for_rows, Lookup::contains
  predicate/truths.rs  a group's verdict: TruthSet, GroupStatistics, KeyedBounds, the truths methods
prune.rs               which row groups a filter rules out: GroupStatistics for a stored group
summary.rs             a table's totals from its statistics; partition_orders for the planner
stream.rs              pass 1 for a query, then pass 2 (D48): table_stream, read_table (from batch.rs)
  stream/table.rs      the stream's handle: TableStream, StreamShared, BlockingTableIter, table_schema
  stream/partitions.rs TablePartitions, DynamicPartitions, table_stream_partitions
  stream/resume.rs     the token: ResumeToken, InCopyResume, query_fingerprint, hash_expr
  stream/note.rs       PlanNote, EarlyStop
  stream/mapping.rs    pass 1 for a query, over pgdt-mapping: map_for_query, validate_request, render_candidate (was map_query.rs)
  stream/replay.rs     pass 2: the replay loop, row_text, and a block made active — activate, resume_state, snapshot
    replay/plan.rs     ReplayPlan, plan_blocks, TableColumns, resolve_block, refuse_metadata_level, project, PlannedReplay, plan_replay
    replay/segment.rs  a piece of a block: Segment, SegmentEntry, first_row_start
    replay/partition.rs  cut_blocks, plan_partitions
    replay/dynamic.rs  DynamicFilter, DynamicRead, Cut
```

`eval.rs` and `truths.rs` hold `impl` blocks of the types `resolve.rs`
defines, split by role, each method placed by its own move line below; so
eval reads resolve, truths reads both, and resolve reads neither. `Lookup` is
resolve's: `resolve_membership` builds it and `ResolvedMembership` holds it,
and only its `contains` is evaluation.

`table_stream` calls `replay` and `replay` writes into `StreamShared`, which
ends in a `TableStream`; so the entry sits above both in `stream.rs`, with
`read_table`, which drives it, and `StreamShared` sits beside `TableStream`.
`TablePartitions` reads `PlannedReplay` and `plan_replay`, so they are the
replay's plan, below it. `activate` reads `ReplayPlan`, which holds
`Segment`s, and `resume_state` calls `activate`; both, with `snapshot`, are
read by the replay loop alone, so they are its own, and `resume.rs` and
`segment.rs` are the data the plan and the loop read. `DynamicRead` carries
the `Cut` a `DynamicPartitions` makes, so `Cut` sits with it. Today's cycles
(map↔backfill, table↔map_query, table↔plan, table↔replay, table↔segment,
plan↔segment, dynamic↔table_partitions through `Cut`) all fall to this tree
and pgdt-mapping's.

`render.rs` is not the batcher's: reading an array back assembles nothing,
and its readers are `summary`, datafusion-pgdump's filter and `pgdt`'s
printer; it reads `batch/builder.rs`, a field decoded alone being a one-row
column. Pruning and summaries are root siblings, one reader and one consumer
each, not a third crate's `statistics` module holding "what reads them".

`predicate/tests.rs` splits along that tree; the per-type comparison tests
drive `Predicate` resolution, its notes and query's errors, so they stay.
Two oracle tests state properties of the key alone —
`the_key_is_a_total_order_over_every_committed_value` and
`golden_order_is_pinned_to_the_format_version` — and move to pgdt-codec
beside `key`, which dev-depends on pgdt-dump (`preamble_only`) and tokio for
them; `layering.py` reads no dev-dependency. `oracle_register.py:216` names
`predicate/tests.rs` and a test in it, and follows the test.

The 35.19 and 35.20 cuts' moves, projected against `2ea0056b` with no cycle:

```moves
# 35.19: the predicate as one tree
pgdt-query::predicate::ComparisonNote -> pgdt-query::predicate::notes
pgdt-query::predicate::column_divergences -> pgdt-query::predicate::notes
pgdt-query::predicate::compares_as_declared -> pgdt-query::predicate::notes
pgdt-query::predicate::DivergingPosition -> pgdt-query::predicate::notes
pgdt-query::predicate::postgres_divergences -> pgdt-query::predicate::notes
pgdt-query::predicate::datafusion_divergences -> pgdt-query::predicate::notes
pgdt-query::predicate::NOT_MAPPED -> pgdt-query::predicate::notes
pgdt-query::predicate::NESTED -> pgdt-query::predicate::notes
pgdt-query::predicate::NESTED_IN_DATAFUSION -> pgdt-query::predicate::notes
pgdt-query::predicate::NO_ORDER -> pgdt-query::predicate::notes
pgdt-query::predicate::nested_refusal -> pgdt-query::predicate::notes
pgdt-query::predicate::unanswerable_reason -> pgdt-query::predicate::notes

pgdt-query::predicate::ResolvedTerm -> pgdt-query::predicate::resolve
pgdt-query::predicate::ResolvedMembership -> pgdt-query::predicate::resolve
pgdt-query::predicate::ResolvedExpr -> pgdt-query::predicate::resolve
pgdt-query::predicate::resolve_term -> pgdt-query::predicate::resolve
pgdt-query::predicate::resolve_membership -> pgdt-query::predicate::resolve
pgdt-query::predicate::believed_bounds -> pgdt-query::predicate::resolve
pgdt-query::predicate::BelievedStatistics -> pgdt-query::predicate::resolve
pgdt-query::predicate::ComparedTerm -> pgdt-query::predicate::resolve
pgdt-query::predicate::Comparison -> pgdt-query::predicate::resolve
pgdt-query::predicate::NestedComparison -> pgdt-query::predicate::resolve
pgdt-query::predicate::Spellings -> pgdt-query::predicate::resolve
pgdt-query::predicate::equality_comparison -> pgdt-query::predicate::resolve
pgdt-query::predicate::not_null_column -> pgdt-query::predicate::resolve
pgdt-query::predicate::null_refused -> pgdt-query::predicate::resolve
pgdt-query::predicate::named_in -> pgdt-query::predicate::resolve
pgdt-query::predicate::Lookup -> pgdt-query::predicate::resolve
pgdt-query::stream::resolve::resolve_expr -> pgdt-query::predicate::resolve
pgdt-query::stream::resolve::resolve_loosened -> pgdt-query::predicate::resolve
pgdt-query::stream::resolve::column_position -> pgdt-query::predicate::resolve

pgdt-query::predicate::ResolvedTerm::eval -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedTerm::eval_value -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedTerm::field_decode -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedTerm::as_read -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedTerm::is_false -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedTerm::wants_equal -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedMembership::unmatched -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedMembership::eval_value -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedMembership::eval -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedMembership::implies -> pgdt-query::predicate::eval  # read by for_rows alone
pgdt-query::predicate::ResolvedExpr::matches -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedExpr::eval -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedExpr::for_rows -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedExpr::conjuncts -> pgdt-query::predicate::eval
pgdt-query::predicate::ResolvedExpr::reads_fields -> pgdt-query::predicate::eval
pgdt-query::predicate::Lookup::contains -> pgdt-query::predicate::eval

pgdt-query::predicate::ResolvedTerm::truths -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedTerm::truths_keyed -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedTerm::value_truths -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedTerm::bounded -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedMembership::truths -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedMembership::truths_keyed -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedExpr::truths -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedExpr::truths_keyed -> pgdt-query::predicate::truths
pgdt-query::predicate::ResolvedExpr::required_ordering_terms -> pgdt-query::predicate::truths  # a sorted run's stop, off believed bounds
pgdt-query::predicate::TruthSet -> pgdt-query::predicate::truths
pgdt-query::predicate::GroupStatistics -> pgdt-query::predicate::truths
pgdt-query::predicate::KeyedBounds -> pgdt-query::predicate::truths
```

```moves
# 35.20: options, render, batch and the stream
pgdt-query::batch::QueryOptions -> pgdt-query::options
pgdt-query::batch::ScanExtent -> pgdt-query::options

pgdt-query::batch::render_field -> pgdt-query::render
pgdt-query::batch::render_field_into -> pgdt-query::render
pgdt-query::batch::render_array_into -> pgdt-query::render
pgdt-query::batch::render_list_level -> pgdt-query::render
pgdt-query::batch::decode_field -> pgdt-query::render

pgdt-query::batch::RetainedChunks -> pgdt-query::batch::chunks
pgdt-query::batch::SourceChunk -> pgdt-query::batch::chunks
pgdt-query::batch::push_utf8view_field -> pgdt-query::batch::chunks

pgdt-query::batch::ColumnBuilder -> pgdt-query::batch::builder
pgdt-query::batch::ListParts -> pgdt-query::batch::builder
pgdt-query::batch::StructParts -> pgdt-query::batch::builder
pgdt-query::batch::append_array_level -> pgdt-query::batch::builder
pgdt-query::batch::append_nested -> pgdt-query::batch::builder
pgdt-query::batch::append_null -> pgdt-query::batch::builder
pgdt-query::batch::append_range -> pgdt-query::batch::builder
pgdt-query::batch::append_typed -> pgdt-query::batch::builder
pgdt-query::batch::array_depth -> pgdt-query::batch::builder
pgdt-query::batch::builder_len -> pgdt-query::batch::builder
pgdt-query::batch::finish_column -> pgdt-query::batch::builder
pgdt-query::batch::new_column_builder -> pgdt-query::batch::builder
pgdt-query::batch::new_list_parts -> pgdt-query::batch::builder
pgdt-query::batch::new_struct_parts -> pgdt-query::batch::builder
pgdt-query::batch::text_refused -> pgdt-query::batch::builder

pgdt-query::batch::read_table -> pgdt-query::stream
pgdt-query::stream::table::table_stream -> pgdt-query::stream
pgdt-query::stream::segment::StreamShared -> pgdt-query::stream::table

pgdt-query::stream::table::ResumeToken -> pgdt-query::stream::resume
pgdt-query::stream::table::InCopyResume -> pgdt-query::stream::resume
pgdt-query::stream::resolve::query_fingerprint -> pgdt-query::stream::resume
pgdt-query::stream::resolve::hash_expr -> pgdt-query::stream::resume
pgdt-query::stream::resolve::hash_children -> pgdt-query::stream::resume

pgdt-query::stream::table::table_stream_partitions -> pgdt-query::stream::partitions
pgdt-query::stream::table_partitions -> pgdt-query::stream::partitions
pgdt-query::stream::table_partitions::Cut -> pgdt-query::stream::replay::dynamic

pgdt-query::stream::map_query -> pgdt-query::stream::mapping
pgdt-query::stream::resolve::render_candidate -> pgdt-query::stream::mapping

pgdt-query::stream::resolve::row_text -> pgdt-query::stream::replay
pgdt-query::stream::segment::activate -> pgdt-query::stream::replay
pgdt-query::stream::resolve::Active -> pgdt-query::stream::replay
pgdt-query::stream::resolve::Opened -> pgdt-query::stream::replay
pgdt-query::stream::table::resume_state -> pgdt-query::stream::replay
pgdt-query::stream::table::snapshot -> pgdt-query::stream::replay

pgdt-query::stream::plan -> pgdt-query::stream::replay::plan
pgdt-query::stream::resolve::project -> pgdt-query::stream::replay::plan
pgdt-query::stream::table::TableColumns -> pgdt-query::stream::replay::plan
pgdt-query::stream::table::resolve_block -> pgdt-query::stream::replay::plan
pgdt-query::stream::table::refuse_metadata_level -> pgdt-query::stream::replay::plan
pgdt-query::stream::table::PlannedReplay -> pgdt-query::stream::replay::plan
pgdt-query::stream::table::plan_replay -> pgdt-query::stream::replay::plan

pgdt-query::stream::segment -> pgdt-query::stream::replay::segment
pgdt-query::stream::partition -> pgdt-query::stream::replay::partition
pgdt-query::stream::dynamic -> pgdt-query::stream::replay::dynamic
```

## pgdt

One module a subcommand, each owning its flags: `Command::Parse(ParseArgs)`
and its siblings, each `#[derive(Args)]` struct in its subcommand's module, so
a flag's help text sits beside its behaviour. Help output is unchanged
(`tests/help_text.rs`).

**The root dispatches and holds nothing a subcommand reads**: it calls every
subcommand, so an item a subcommand reads in `main.rs` is a cycle. It keeps
`Cli`, `Invocation`, `Command`, `main`, `sql_release` and the native runtime's
setup; flags and helpers that more than one subcommand reads go to `args`,
`parallel`, `source`, `interrupt` and `version`. `commands` is one item today,
and the cut splits it into each subcommand's `run`, so the cut re-projects
once those exist; today's body reads downward only, `parse` reading `info/text.rs`'s
`print_metadata` and `print_diagnostics`.

**`--filter`'s entry is the filter's parent.** `parse_filter_flag` reads
`--where`'s `refuse_where_structure` and the term grammar's `parse_filter`,
and `--where` reads the term grammar for its leaves and its operator lexing
(`filter_op_at`, `in_list`, `TermOp`, `InList`). So the term grammar is the
bottom, `expr.rs` sits above it, and `filter.rs` sits above both. Placed in
`term.rs`, the entry would make term and expr read each other, which is
today's `pgdt` ↔ `where_expr` cycle.

**`info/labels.rs` is what both renderings read**: the labels, and the one
resolution pass (`block_resolutions`) and `block_unchecked` that `print_index`
and `--json` share. `compression_line` is only `print_index`'s, so it goes
in `text.rs`, below `info.rs`'s `report`, which calls both renderings.

```
main.rs                  Cli, Invocation, Command, main, sql_release; runtime setup and dispatch to <cmd>::run
version.rs               version(), RELEASE_VARIABLE, RELEASED, same_str
args.rs                  the flag groups subcommands share: IdentityArgs, OverwriteArgs, ReadFlags, scan_options, the Cli* enums, value parsers
parallel.rs              ParallelArgs, Discovered, QueryPasses, Resolved, parse_jobs, parse_memory
source.rs                Opened, open_with_cache, open_for_scan, unusable_cache_message, naming the source, quoted_name_note
interrupt.rs             InterruptGuard, install_interrupt_guard, die_by
parse.rs                 ParseArgs, run, resume_notice, statistics_request
info.rs                  InfoArgs, run, info_offline, report, completion_line
  info/text.rs           print_index, print_metadata (which parse also calls), DatabaseHeadings, compression_line, the line printers
  info/json.rs           IndexJson and its parts, print_index_json
  info/labels.rs         what both read: arrow_type_label, range_label, type_kind_*, span_summary, block_resolutions, block_unchecked
  info/statistics.rs     today's info_statistics.rs
query.rs                 QueryArgs, run, QueryStatistics, the Slot merge, announce_*, print_batch, projection
  query/filter.rs        --filter's entry: parse_filter_flag, refused where --where reads structure
    filter/term.rs       the term grammar and its operator lexing (from main.rs)
    filter/expr.rs       --where (today's where_expr.rs), its leaves from term.rs
alloc.rs, profile.rs, introspect.rs   unchanged
```

`tests.rs` splits beside the modules it tests.

The 35.22 cut's moves, projected against `2ea0056b` with no cycle:

```moves
pgdt::version -> pgdt::version
pgdt::RELEASE_VARIABLE -> pgdt::version
pgdt::RELEASED -> pgdt::version
pgdt::same_str -> pgdt::version
pgdt::IdentityArgs -> pgdt::args
pgdt::OverwriteArgs -> pgdt::args
pgdt::ReadFlags -> pgdt::args
pgdt::CliSchemaMode -> pgdt::args
pgdt::CliUnrepresentable -> pgdt::args
pgdt::CliInvalidValues -> pgdt::args
pgdt::CliParseInvalidValues -> pgdt::args
pgdt::parse_strict_identity -> pgdt::args
pgdt::parse_chunk_size -> pgdt::args
pgdt::parse_max_line_bytes -> pgdt::args
pgdt::parse_statistics_level -> pgdt::args
pgdt::parse_row_group_size -> pgdt::args
pgdt::scan_options -> pgdt::args      # reads parallel's `Resolved`, one way
pgdt::ParallelArgs -> pgdt::parallel
pgdt::Discovered -> pgdt::parallel
pgdt::QueryPasses -> pgdt::parallel
pgdt::Resolved -> pgdt::parallel
pgdt::parse_jobs -> pgdt::parallel
pgdt::parse_memory -> pgdt::parallel
pgdt::Opened -> pgdt::source
pgdt::open_with_cache -> pgdt::source
pgdt::open_for_scan -> pgdt::source
pgdt::unusable_cache_message -> pgdt::source
pgdt::about_the_source -> pgdt::source
pgdt::naming_the_source -> pgdt::source
pgdt::name_taken_verbatim -> pgdt::source
pgdt::quoted_name_note -> pgdt::source      # `name_taken_verbatim` reads it
pgdt::install_interrupt_guard -> pgdt::interrupt
pgdt::InterruptGuard -> pgdt::interrupt
pgdt::die_by -> pgdt::interrupt
pgdt::resume_notice -> pgdt::parse
pgdt::statistics_request -> pgdt::parse
pgdt::info_offline -> pgdt::info
pgdt::report -> pgdt::info
pgdt::completion_line -> pgdt::info
pgdt::compression_line -> pgdt::info::text
pgdt::print_index -> pgdt::info::text
pgdt::print_metadata -> pgdt::info::text
pgdt::DatabaseHeadings -> pgdt::info::text
pgdt::database_label -> pgdt::info::text
pgdt::print_statistics -> pgdt::info::text
pgdt::print_diagnostics -> pgdt::info::text
pgdt::severity_label -> pgdt::info::text
pgdt::print_cross_references -> pgdt::info::text
pgdt::print_object_kinds -> pgdt::info::text
pgdt::print_map -> pgdt::info::text
pgdt::unchecked_lines -> pgdt::info::text
pgdt::unrepresentable_line -> pgdt::info::text
pgdt::ignored_refusals_line -> pgdt::info::text
pgdt::level_label -> pgdt::info::text
pgdt::IndexJson -> pgdt::info::json
pgdt::BlockResolutionJson -> pgdt::info::json
pgdt::ColumnResolutionJson -> pgdt::info::json
pgdt::print_index_json -> pgdt::info::json
pgdt::block_unchecked -> pgdt::info::labels
pgdt::arrow_type_label -> pgdt::info::labels
pgdt::range_label -> pgdt::info::labels
pgdt::type_kind_summary -> pgdt::info::labels
pgdt::type_kind_label -> pgdt::info::labels
pgdt::span_summary -> pgdt::info::labels
pgdt::resolution_token -> pgdt::info::labels
pgdt::enum_labels -> pgdt::info::labels
pgdt::label_list -> pgdt::info::labels
pgdt::block_resolutions -> pgdt::info::labels
pgdt::info_statistics -> pgdt::info::statistics
pgdt::QueryStatistics -> pgdt::query
pgdt::Slot -> pgdt::query
pgdt::announce_comparisons -> pgdt::query
pgdt::announce_plan_notes -> pgdt::query
pgdt::announce_early_stops -> pgdt::query
pgdt::print_batch -> pgdt::query
pgdt::projection -> pgdt::query
pgdt::parse_filter_flag -> pgdt::query::filter
pgdt::parse_filter -> pgdt::query::filter::term
pgdt::FILTER_OPS -> pgdt::query::filter::term
pgdt::word_at -> pgdt::query::filter::term
pgdt::skip_spaces -> pgdt::query::filter::term
pgdt::distinct_from_at -> pgdt::query::filter::term
pgdt::in_at -> pgdt::query::filter::term
pgdt::TermOp -> pgdt::query::filter::term
pgdt::InList -> pgdt::query::filter::term
pgdt::in_list -> pgdt::query::filter::term
pgdt::filter_op_at -> pgdt::query::filter::term
pgdt::parse_in_list -> pgdt::query::filter::term
pgdt::split_filter_op -> pgdt::query::filter::term
pgdt::FilterSplit -> pgdt::query::filter::term
pgdt::dequote -> pgdt::query::filter::term
pgdt::filter_part -> pgdt::query::filter::term
pgdt::unbalanced_quote -> pgdt::query::filter::term
pgdt::strip_ci_suffix -> pgdt::query::filter::term
pgdt::where_expr -> pgdt::query::filter::expr
```

## datafusion-pgdump

DataFusion's expressions read as the library's filter tree are one concept.
`filter.rs` holds the leaf builders both translations use (`compared`,
`member`, `null_term`, `boolean_term`, `is_float`, `column_index`, and the
literal's text: `literal_text`, `string_value`, `compared_as_text`).
`filter/pushdown.rs` holds the static translation and its `Exact`/`Inexact`
answer; `filter/dynamic.rs` (with its `tests.rs`) holds the dynamic filters.
Each translation reads the leaves and nothing reads back. `literal_text` reads
`compared_as_text`, so that one goes to the parent with it.

**The root declares and defines nothing.** Today `lib.rs` defines `Error`,
`register_dump`, `session_budget`, `describe` and `PgDump::table`. `dump`,
`table`, `report` and `factory` read the first and the last of those, and
`register_dump` and `PgDump::table` read `catalog`, `table` and `report` back.
So each item goes to the module it belongs to:

- **`Error` goes to `error.rs`**, with `table.rs`'s `external`, the library's
  error turned into DataFusion's. `exec` reads `external`, and `table` reads
  `exec`.
- **`PgDump::table` and `describe` go to `table.rs`**: one table on its own,
  the provider a catalog hands out.
- **`register_dump` goes to `catalog.rs`**, a dump registered as catalogs, and
  **`session_budget` to `budget.rs`**: `budget` reads `settings`, and nothing
  points back.
- **`report.rs`'s `impl PgDumpTable` goes to `table.rs`** (`report`,
  `report_plan`). The table's scan calls `report_plan`, and `report_plan` reads
  the table's own fields, so wherever it is defined it is the table's. `report`
  keeps the findings and the sink a table reports to (`Located`, `Reporting`,
  `BudgetedPlanNote`, `RefusedTable`) and `PgDump::report`, and reads `dump`
  and `budget`, never `table`.

The edges then run `catalog` → `table` → `exec`, `report`, `dump` → `budget`
→ `settings`, with everything above `error`, and `factory` beside `catalog`.
`unrepresentable.rs`, a UDF and its guard, stays where it is; every other file
is one DataFusion extension point already. datafusion-cli-pgdump is left
alone: `lib.rs` is upstream's `main.rs`, laid out for the diff.

```
lib.rs              crate doc, the modules, the API's `pub use`
error.rs            Error, external: a refusal, and the library's carried as DataFusion's
dump.rs             PgDump, PgDumpOptions: a dump opened through its complete cache
budget.rs           ScanBudget, its draws and account, session_budget
settings.rs         PgDumpSettings: the `pgdump.` settings a `SET` moves
catalog.rs          PgDumpCatalog, PgDumpSchema, register_dump
table.rs            PgDumpTable, PgDump::table, describe, a table's report and report_plan
exec.rs             PgDumpExec, the replay's partitions, ScanMetrics, StaticFilter
statistics.rs       what the map's statistics tell DataFusion
report.rs           Located, Reporting, BudgetedPlanNote, RefusedTable, PgDump::report
factory.rs          PgDumpTableFactory, register_table_factory
unrepresentable.rs  pgdump_unrepresentable and its guard
filter.rs           the leaf builders both translations use, the literal's text
  filter/pushdown.rs  the static translation: translate, Exact or Inexact
  filter/dynamic.rs   the dynamic filters (+ dynamic/tests.rs)
```

The 35.21 cut's moves, projected against `2ea0056b` with no cycle:

```moves
datafusion-pgdump::Error -> datafusion-pgdump::error
datafusion-pgdump::table::external -> datafusion-pgdump::error
datafusion-pgdump::describe -> datafusion-pgdump::table
datafusion-pgdump::PgDump -> datafusion-pgdump::table          # the root's `impl PgDump { fn table }`
datafusion-pgdump::report::PgDumpTable -> datafusion-pgdump::table   # report's `impl PgDumpTable { report, report_plan }`
datafusion-pgdump::register_dump -> datafusion-pgdump::catalog
datafusion-pgdump::session_budget -> datafusion-pgdump::budget
datafusion-pgdump::pushdown -> datafusion-pgdump::filter::pushdown
datafusion-pgdump::dynamic_filter -> datafusion-pgdump::filter::dynamic
datafusion-pgdump::pushdown::compared -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::member -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::null_term -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::boolean_term -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::is_float -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::column_index -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::literal_text -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::string_value -> datafusion-pgdump::filter
datafusion-pgdump::pushdown::compared_as_text -> datafusion-pgdump::filter
```

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
| Type mapping | codec: `declared.rs`, `pgtype.rs`, `resolve.rs` |
| Values | codec: `decode.rs`, `nested.rs`, `value.rs`, `value/`; query: `render.rs`, an array read back (D74) |
| Comparison semantics | codec: `compare.rs`, `compare/`, `key.rs` (each family's key type in its `value/` module, the nested keys in `value/nested/key.rs`) |
| Arrow batches | query: `options.rs`, `batch.rs`, `batch/` |
| Filtering | query: `predicate.rs`, `predicate/`, `stream/replay/dynamic.rs`; datafusion-pgdump: `filter.rs`, `filter/`, `unrepresentable.rs`; pgdt: `query/filter.rs`, `query/filter/` |
| Strict parse | codec: `grammar.rs`, `grammar/`; mapping: `statistics/gather/strict.rs` |
| Statistics | model: `statistics.rs`; mapping: `statistics.rs`, `statistics/`; query: `prune.rs`, `summary.rs`; datafusion-pgdump: `statistics.rs`; pgdt: `info/statistics.rs` |
| The replay | query: `stream.rs`, `stream/table.rs`, `stream/partitions.rs`, `stream/resume.rs`, `stream/replay.rs`, `stream/replay/` but `dynamic.rs` |
| What a query tells its caller | query: `stream/note.rs`; datafusion-pgdump: `report.rs`; pgdt: `query.rs`'s `announce_*`, a front end's printing |
| The DataFusion provider | datafusion-pgdump: `lib.rs`, `error.rs`, `catalog.rs`, `factory.rs`, `table.rs`, `exec.rs`, `settings.rs` |
| The DataFusion CLI, the binary's build | unchanged |
| The `pgdt` CLI | pgdt: `main.rs`, `version.rs`, `args.rs`, `source.rs`, `parse.rs`, `info.rs`, `info/` but `statistics.rs`, `query.rs` |

## The cuts

One `STATUS.md` row a cut, from 35.13, bottom up so each lands on crates
already designed; each owns whole concepts. The register goes before the
families so the families' keys import from a `compare` that exists; the
mapping crate is carved before query's tree, so query is redrawn over what is
left. 35.17 is the largest, values and order keys being one cluster by the
edges. Each cut moves its concepts' claims in `concepts.md`.

**Each crate's section states its cut's moves**, against `2ea0056b`'s paths,
and `scripts/module_graph.py --design` projects this doc's blocks together to
no cycle but the kept `origin` ↔ `remote`. A cut re-projects its own block
over the tree the cuts before it left, and a cut deleting a cycle's last
member strikes its `OWED` entry. 35.23 moves the items whose cycles no other
cut removes — pgdt-model's three, pgdt-dump's `cache/`, the `.xz` advice — and
goes first, touching the crates below the rest.

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

