# P36 — The embedder API

The library's public surface is reshaped so that each crate reads as a surface
designed for whoever embeds it ([`decisions.md`](decisions.md), "D106"). Rough
edges are smoothed, and the orchestration that every embedder repeats is moved
into the library. No user-visible capability is added.

This file says what the phase will do and why; how it lands is up to its slices.
Progress lives in `STATUS.md`'s checklist, never here. Grilled
2026-10-13.

## Why now

P35 drew the crate boundaries and sorted each crate's items into API and
`internal` (D106). It did not reshape what that sort left public. Architecture
and API move together: the crates are now where they will stay for a while,
so this is the moment to make their surfaces fit them. **Pre-1.0 nothing here
is promised** ([`roadmap.md`](roadmap.md), "Pre-1.0: no compatibility
obligations"). The phase smooths rough edges and binds no API as permanent.
Had the survey found nothing obvious, the phase would have been struck until
the API had iterated further; it found plenty (below).

## Scope

- **The library crates only.** The `pgdt` CLI's flags, its `--json` shape and
  `pgdt sql`'s `--dump` spellings are out of scope. They wait on the
  CLI-feedback pass ([`../status/STATUS.md`](../status/STATUS.md), "Not
  started"). A library reshape that changes a flag's behaviour changes the
  flag to match, and nothing more.
- **Judged:** the seven `pgdt-*` libraries, and `datafusion-pgdump`, each as
  a surface an outside embedder uses. `datafusion-cli-pgdump` is plumbing for
  one binary, `pgdt sql`. It changes only where a library reshape forces it.
- **Reshape, and lift what embedders repeat.** Items are renamed, narrowed,
  merged, demoted to `internal` or regrouped, and KD115–KD117 close. Beyond
  that, orchestration moves into the library under one condition: at least
  two embedders (`pgdt`, `datafusion-pgdump`) write it out today, or a
  non-DataFusion binding ([`roadmap.md`](roadmap.md), "P24 — Python bindings")
  would have to copy it out of `pgdt`. Anything else stays where it is. This
  is not a facade in D106's sense: nothing is re-exported, and the library
  gains the entry point the embedders were assembling.
- **The surface leads.** A reshape that the API needs carries through into
  the module tree and the crate graph wherever it leads, D68's graph
  included ([`roadmap.md`](roadmap.md), "The public surfaces lead; the
  architecture follows").
- **Every change follows the seams discipline**
  ([`../process.md`](../process.md), "Seams"). Each concept the phase adds or
  moves is placed on the concept map in this spec, and the slice that lands
  it updates [`concepts.md`](concepts.md). Every file stays under
  `scripts/file_limits.py`'s limits; a reshape that would push a file past
  one designs the tree around it in the same slice. Every module edge stays
  one-way under `scripts/module_graph.py`. A **Not in one place** line that a
  reshape gathers is struck in the slice that gathers it.

## The rough edges found

Four read-only surveys, taken at `d7ce860d`: `pgdt-query` with
`datafusion-pgdump`; `pgdt-io`, `pgdt-dump` and `pgdt-mapping`; `pgdt-core`,
`pgdt-model` and `pgdt-codec`; and the calls a non-DataFusion embedder makes
to open, map, load, list, stream and summarise. Below are the facts they
found, grouped by cluster. The line numbers are as of that commit. Each
cluster's disposition is decided in the grilling.

- **A. Nothing in the library holds "a dump opened through its cache".** The
  sequence is: claim the cache, open the source, map `Recognized::Mismatch`
  to `CompressionContradicted`, open the `SourceWatch` *before* the load,
  load, refuse on strict identity or add the advisory diagnostics, then
  return a verdict. It is written out three times: `pgdt/src/source.rs:126-169`,
  `pgdt/src/info.rs:139-147,183-232`, and `datafusion-pgdump/src/dump.rs:97-170`.
  `CacheMode::load` cannot serve as the shared version because it merges
  `Valid` and `Incomplete`. `TablePartitions::plan` takes the source, the
  index and the watch as separate arguments, and nothing ties them to one
  open. `table_schema` and `table_summary` document that a map must be
  complete and do not check it; only `plan` checks. The metadata-level
  refusal and the database list exist only in the adapter. `claim` decodes
  the cache, and `load` decodes it again.
- **B. Resolving the arrangement is the embedder's job.** That means turning a
  stated or discovered allowance into a worker count, a budget and a
  statistics allowance. `pgdt/src/parallel.rs:230-270,459-468` does it, and
  `datafusion-pgdump/src/budget.rs:217-222` does it differently (`KD112`).
  `Parallelism` has six positional constructors.
  `within_shared(jobs, memory, allowance, drawn, held)` takes three adjacent
  `u64`s. The fallback `memory_bytes().unwrap_or(DEFAULT_MEMORY_BUDGET)` is
  copied in three places. `GatherOptions::statistics_allowance_bytes` must
  equal `statistics_allowance(allowance, budget)`, which only `pgdt`
  computes, and `StatisticsRequest::backfill` takes the same number a second
  time.
- **C. Some options are stated twice, or set and then ignored.** The mapping
  pass reads `ScanOptions.parallelism`; a replay reads
  `QueryOptions.parallelism`. `pgdt query` passes them different values. Only
  the mapping pass reads `QueryOptions.database` and `scan_extent`, and
  `plan`, `table_schema` and `table_summary` ignore them. `max_rows` appears
  both in `QueryOptions` and in `TablePartitions::stream`. `scan` and
  `preamble_only` ignore `ScanOptions.cancel`.
- **D. KD115's pattern recurs: the caller must pass back values that belong
  together.**
  - Back-filling a block passes the block and its metadata to three calls,
    then applies the `BlockReread` by hand (`pgdt-mapping/src/backfill.rs`).
  - `table_summary(index, table, resolved, semantics, reading)` and
    `column_divergences(resolved, semantics)` take parts of one
    `QueryOptions`.
  - `render_field` and `decode_field` take a `NestedPlan` that the caller
    indexes out of `ResolvedSchema.plans`.
  - `ResolvedSchema` is six parallel `pub` vectors that the caller matches up
    by index.
- **E. The stream family grew by accretion.** It is `read_table`,
  `table_stream`, `table_stream_partitions`, `BlockingTableIter`,
  `TableStream`, `TablePartitions`, `DynamicPartitions` and `ResumeToken`. No
  production embedder calls `read_table`, `table_stream`,
  `BlockingTableIter` or `ResumeToken`. `TableStream`'s schema stays empty
  until the first poll, while `TablePartitions` knows it before any poll.
  Resume works only serially: it re-decodes the cache and may write to it.
  The file-ordered merge that reports the earliest error first (D52) exists
  only in `pgdt/src/query.rs:309-417`.
- **F. A table is named three ways.** A `&str` pattern plus
  `QueryOptions.database`; an exact `&TableName`; and `PgDump::table`'s
  three `Option<&str>`, which re-implement the matching. On the model side,
  `DumpIndex::blocks_for(&str)` sits beside `blocks_of(&TableName)`.
- **G. The cache namespace has overlapping entry points.**
  - Free `load`, `load_offline` and `save` sit beside the `CacheMode`
    methods of the same names, and the free `save` skips the `SourceWatch`
    check.
  - `CacheStatus::Valid` and `Incomplete` carry the same six fields.
  - `CacheMode::Offline` turns misuse into a runtime `CacheModeMismatch`.
  - `CacheMode::resolve` reads the path `"none"` as disabled, which is a CLI
    convention inside the library.
  - `cache::Unusable` re-exports the core's type, giving it two paths.
- **H. Library errors name `pgdt`'s flags.** `pgdt-core/src/error.rs` tells
  the reader about `--dtcache none`, `--strict-identity`, `--database`,
  `--schema-mode strings` and "run `pgdt parse`". `datafusion-pgdump` passes
  these messages through to DataFusion users, and for the metadata level it
  had to write its own replacement. `Unrepresentable` is the exception: it
  names its remedy in library terms.
- **I. Items at a crate's root are plumbing, test seams or tuning
  constants.**
  - pgdt-io: the `_in` discovery variants and `MEMORY_RESERVE`,
    `MEMORY_UNPOOLED_BOUND` and `MEMORY_MARGIN_PERCENT` (D11 made the `_in`
    seam public); `walk_seek_table`.
  - pgdt-dump: the chunked scanner protocol, which crates above copy four
    times (`scan.rs`, `forward.rs`, `backfill.rs`, `replay.rs`);
    `check_tiling`, called only by tests; `dump_metadata_from_spans`, whose
    precondition is unchecked; `attach_text`.
  - pgdt-mapping: `build_map`, which is `build_index().spans`; the
    strict-unchecked types and `bounded_columns`, which sit at the root
    though the `statistics` namespace claims them.
  - pgdt-core: `RowSplit`, `RawRow`, `validated_prefix`, and the
    `introspect`-only `instrument` namespace.
  - pgdt-query: `Truth`, which nothing takes; `decode_field`, used by one
    test.
  - datafusion-pgdump: `PgDumpTableOptions`, which duplicates
    `PgDumpOptions`; `PgDumpUnrepresentable`; `PgDumpTableFactory` beside
    `register_table_factory`.
  - Across crates: `xz_seek::SeekTable`, from the vendored and unpublished
    crate, appears in `KnownCompression::Xz` and
    `ByteRangeSource::seek_table`.
- **J. Model fields can be read past the view that interprets them.**
  - `ColumnStatistics`' `pub` bounds, NULL counts and unrepresentable counts
    sit beside methods that apply a statistics view. Reading them raw
    silently picks one view, and `ColumnBounds.every` and `displayable` are
    sparse.
  - `CopyBlock.array_shapes` and `.unrepresentable` are kept in step only by
    `set_census`.
  - The database name is persisted on both `Span` and `CopyBlock`.
  - `UnrepresentableExt::add` and `DiagnosticExt`'s constructors are
    internal while the fields they set are `pub`, so the API/`internal`
    split there separates nothing.
  - `StatisticsRequest`'s field `min_rows` holds what was stated, while its
    method `min_rows()` returns the resolved value.
- **K. The codec's keys (KD116, KD117).**
  - `ValueKey` covers scalars only.
  - Comparing two keys of different kinds panics through the API
    (`pgdt-codec/src/key.rs:479`, `unreachable!`).
  - There is no constructor for a literal.
  - The caller has to unwrap the `ComparisonPlan` itself.
  - `NestedKey`, `nested_key` and `compare_nested` already exist.
  - `OrderKey`, `Side` and their functions are `pub` at `key::`.
  - `ComparisonPlan::orders` warns callers against matching its variant.
  - `NestedCompare` returns unnamed tuples.
  - No embedder uses the `grammar` namespace, and `decode` is a grab-bag.
- **L. Names that collide or disagree.**
  - `pgdt_core::decode_field`/`encode_field` handle COPY escaping, while
    `pgdt_query::decode_field` turns text into Arrow.
  - `TableName::qualified` versus `CopyHeader::qualified_name`.
  - `Predicate { op, value: Option<String> }` can be built as `IsNull` with
    a value, or `Eq` with none.
- **M. DataFusion session prerequisites are left to the embedder.** Two are
  documented: turn off the aggregate dynamic filter (`KD56`) and build the
  session `with_unrepresentable_guard`. `install()` does neither. The
  adapter's `Located` is private, so a structured sink that downcasts a
  finding loses its subject. `PgDumpTable`'s scans report nothing until
  `report(subject, sink)` is called.

These were checked and look fine: `Origin`, `OriginProbe`, `Recognized`,
`KnownCompression`; `CacheClaim` as a type; `StrictIdentity`; `SourceWatch`;
`MapRun`; `StatisticsSelection` and `StatisticsTarget`; `PlanNote` and
`PlanLever`; `DynamicFilter` and `DynamicPartitions`; `Expr`, `Membership`
and `PredicateOp`; `EarlyStop` and `BudgetAccount`; `register_dump`;
`ScanBudget`; `Error` as a whole (D74); `Unusable` and `RowEndingRefusal`;
the catalog types; `Finding`, `DiagnosticSink` and `drain`; `SchemaMode` and
`UnrepresentableMode`; and the contents of every `internal` module.

## The opened dump

This closes cluster A, and with it most of C, F and G. **Two layers, each in
the crate that already owns its part.**

- **`pgdt-dump`'s `cache` namespace owns the open protocol.** A `CacheMode`
  opens an `Origin`. Internally it claims the cache, opens the source,
  refuses a compression the cache contradicts, takes the `SourceWatch`
  before the load, loads, and refuses on strict identity or attaches the
  advisory identity diagnostics. It returns the source and its watch,
  together with what the cache holds: complete, incomplete (with its index),
  or missing. An unusable cache is a refusal. Each of these steps stops being
  a separate entry point an embedder sequences by hand. That includes the
  free `load` and `save` beside `CacheMode`'s own, `save` without the
  watch's check, and `Valid` and `Incomplete` carrying the same fields twice.
  The cache is decoded once, not once by the claim and again by the load.
- **`pgdt-query` gains a `Dump`, complete by construction.** It is built
  either from a complete open, or by a mapping pass run to the file's end.
  It owns the source, the index and the watch, so they can't come from
  different opens. It answers `tables()` and `databases()`, and `table(…)`
  resolves one selector into a table. That table owns what it plans over,
  sharing the source, the watch and the DDL and holding its own blocks, and
  gives a schema, a summary and a plan
  ([history, 2026-10-19](../status/history/2026-10-19.md), "The `Dump`'s
  table owns its blocks; `info` reads the cache"). Completeness, which `table_schema` and `table_summary`
  document today and do not check, becomes a property of the type. D90 then
  reads as "the provider takes a `Dump`".
- **`datafusion-pgdump`'s `PgDump` is a `Dump` plus its modes and its budget
  holds.** Its open sequence, its database list and its metadata-level
  pre-check move down. `pgdt query` opens through the same two layers;
  `pgdt info`, which reports the cache itself, partial or source-less, and
  reads no table, opens through the first alone. A cold `pgdt query` maps first, then plans over the `Dump` the
  mapping pass completes, exactly as a warm one does.

**A cold query keeps its early stop** (`ScanExtent::UntilTargetSettled`, the
default today), since this phase changes no behaviour. Only a pass run to the
file's end builds a `Dump`. A pass that stops once its target is settled
yields the same owning table a `Dump` hands back, for that table alone.
Either pass is one the caller runs, and so it takes the cancellation (The
small fixes); the plan and its streams take none.

Consequences for the other clusters:
- **C.** A plan over a `Dump` takes no `ScanOptions`, so the provider stops
  setting a parallelism that nothing reads. Each pass's options carry only
  what that pass reads: the mapping pass's arrangement belongs to it, and the
  replay's to the plan.
- **F.** A table is named one way: a selector that `Dump::table` resolves
  once.
- **D.** `table_summary`'s and `column_divergences`' argument pairings become
  methods on the table, read from the options it was planned under.

**Placement** ([`concepts.md`](concepts.md), "The cache"):
- **pgdt-dump**'s part of "The cache" widens to "opening a source through
  its cache". The open needs the source and the persisted map, both of which
  sit at or below `pgdt-dump`.
- **pgdt-query** takes a new part: "the complete dump an embedder reads
  tables through". Schema resolution, summaries and the replay are query's,
  so the handle that offers them is query's too.
- **datafusion-pgdump**'s `dump.rs` narrows to the modes and the budget
  holds.

## The arrangement

This closes cluster B. **One resolution in `pgdt-io`.** Its inputs are:
- a stated worker count, if any;
- an allowance, stated or discovered;
- the source's recommendation;
- what statistics already hold;
- what has been drawn.

It returns an arrangement: the `Parallelism`, the statistics allowance that
`GatherOptions` and `StatisticsRequest::backfill` take today by hand, and
where the allowance came from. That last is the adapter's `AllowanceOrigin`,
moved down. **`Parallelism` itself still carries no provenance**, so D64
holds as written. The six positional constructors shrink to three: the
resolution, `Serial`, and a form for a caller that states everything. The
copied `memory_bytes().unwrap_or(DEFAULT_MEMORY_BUDGET)` becomes a method.
`pgdt`'s `parallel.rs` and the adapter's `ScanBudget` both call the
resolution. That gathers what the concept map's "Memory and parallelism"
holds today in three crates.

**A gathering pass takes the arrangement, never its two halves.**
`map_file` and the block back-fill both take the `ReadOptions`, the
`Arrangement` and the invalid-values mode, as three arguments in that
order, reading the parallelism and the statistics allowance out of the one
carve. `GatherOptions` goes. `resolve` stays the arrangement's only
constructor, so no caller can pair a parallelism with an allowance not
carved from it. Reasoning: [history, 2026-10-18](../status/history/2026-10-18.md),
"The mapping pass takes the arrangement".

**D11 is rewritten** to "limit discovery takes a root; the seam is
`internal`'s". The `_in` variants and the tuning constants (`MEMORY_RESERVE`,
`MEMORY_UNPOOLED_BOUND`, `MEMORY_MARGIN_PERCENT`) move to `pgdt-io`'s
`internal`, since only tests and `pgdt` reach them. The embedder API keeps
discovery without a root, and the resolution.

**`KD112` is not closed here.** Whether the provider should split by the
source's recommendation is a change of behaviour, not a reshape, so the
entry stays `(c) unowned`. The shared resolution makes it a one-place change
when someone takes it up.

## Errors and their remedies

This closes cluster H. **The library states the condition, and names the
remedy in its own terms**: the API item or the concept, such as "a mapping
pass at the data level" or `SchemaMode::Strings`, never `pgdt`'s flags or
commands. `Unrepresentable` already does this. Each embedder adds its own
spelling when it reports the error. `pgdt` does so in its reporting layer,
under a test pinning that each refusal still names its flag. The adapter
does so in its own `Error`, which then needs no hand-written
`MetadataLevel` duplicate.

Refused: a structured `Remedy` value carried by the error. It would be a
second taxonomy beside the variants, which already tell an embedder every
case it needs to match on.

[`concepts.md`](concepts.md)'s "The cache" carries a **Not in one place**
line: why a cache cannot be used is worded twice, once by the core's `Error`
and once by `pgdt info`, and the two disagree. That line is struck by the
slice that makes `info` report the library's wording with its own spelling
added.

## Streams over a table

This closes cluster E. **One plan per table**, `Table::plan(options)`. Its
schema and plan notes are known before any poll. It offers:
- **its partitions**, as `TablePartitions` does today. `Serial` means one
  partition, and a dynamic filter enters through `under` (D95);
- **one stream in file order**, which merges the partitions and reports the
  earliest failing piece's error (D52). This lifts what only `pgdt query`'s
  merge does today, because a non-DataFusion binding would have to copy it;
- **resume, on the serial stream only.** It resumes over the `Dump` in hand,
  without re-decoding the cache or writing to it. D49 and the standing rule
  that `ResumeToken` exposes no fields ([`roadmap.md`](roadmap.md), "Four
  decisions that keep later phases additive") are kept.

**`BlockingTableIter` stays**, as the synchronous adapter over any of these
streams; P24's binding is its natural user. **`read_table` goes**: no
embedder uses push mode, and it returns the resolved schema only once the
stream has drained. **The cold path is one entry**: the mapping pass
produces a `Dump`, and the plan follows. The `&str` pattern and
`QueryOptions.database` give way to the selector. **`max_rows` leaves
`QueryOptions`** and becomes the stream's argument alone.

**Placement**: all of this is [`concepts.md`](concepts.md)'s "The replay",
in pgdt-query's `stream/`. The file-ordered merge is a new part of that
concept. `pgdt`'s copy of it goes.

## A key for every value

This closes `KD116` and `KD117`.
- **`ValueKey` is built from a column's plan**:
  `ValueKey::of(plan, semantics, text)`, plus a constructor for a filter
  literal, read in its type's input grammar (D55). The caller no longer
  unwraps `CompareKind` or asks for `datafusion_order()` itself.
- **A nested value has a key.** A hidden variant wraps `NestedKey`, so an
  array, record, range or multirange is ordered without its representation
  showing. `NestedKey` gains a heap size.
- **Two keys compare exactly where PostgreSQL resolves `a < b` without
  consulting a setting**, with the server's answer, whatever plans they were
  read under: `int2` against `int8`, two `numeric` scales, an integer against
  a `numeric`, a float or an `oid`. Every other pair answers `None` and never
  panics: a `date` against an `integer`, a special against another type's
  value, a `macaddr` against a `macaddr8` (the server finds that operator
  ambiguous), and a pair it orders only under `TimeZone`. There is still no
  `Ord` (D56).
- **In DataFusion's semantics, the rule is DataFusion's**: two keys compare
  where its comparison coercion relates their emitted Arrow types without
  consulting session config, with its answer. Keys read in two semantics
  never compare. `ValueKey::compare` relates keys of one order; keys of two
  are answered in `datafusion-pgdump`, by DataFusion's own coercion and
  cast, a cast DataFusion refuses answering `None`. The codec reproduces no
  arrow-cast (history, 2026-10-17, "DataFusion's cross-order answer is
  DataFusion's own").
  - **A key read in DataFusion's semantics carries its column's emitted
    Arrow type**, built from the resolved column: its order does not
    determine the type DataFusion coerces. The type is never read to order
    a key. A key whose value the emitted type cannot hold (D96) answers
    `None` against another order.
  - **One `datafusion-pgdump` function answers every pair** of keys read in
    DataFusion's semantics, a pair of one order through `ValueKey::compare`.
  - **DataFusion is its oracle**: a test runs `a < b` over two fixture
    tables' columns in a session, pair by pair, a query error meaning
    `None`.
- **A nested key follows the same rule**: the server relates two
  containers only where the types beneath them are one — each leaf's, a
  range's at every level, a composite's below the top, a domain's where it
  is an element's or a field's — and then their leaves as it relates two
  scalars, so `numeric[]` compares at two scales. Two records compare only
  at one arity. Which slots hold a value never decides it (history,
  2026-10-17, "A nested key names its types at every level").
- **A `ValueKey` carries the order it was read in**: its semantics, its kind
  with what does not change the order folded away, a `numeric`'s scale, an
  enum's qualified type name and labels, whether a text is `character
  varying`, a nested key's order and types at every level. `compare` checks it first. The key the gatherer and the summary
  hold, `internal`'s, carries none: nothing in the workspace compares across
  plans (history, 2026-10-16, "36.4.1's calls reviewed: the order's identity
  moves to `ValueKey`").
- **KD117**: `OrderKey`, `Side` and the functions that build and compare
  them move to a `pub(crate)` child, and `internal` alone re-exports them.

**Placement**: [`concepts.md`](concepts.md)'s "Comparison semantics", in
pgdt-codec's `key`; DataFusion's answer across two orders in
`datafusion-pgdump`, the only library linking DataFusion's coercion (D74
keeps arrow-cast out of the codec).

## Demotions: D106 applied as written

This closes cluster I. **An item at a crate's root that only tests or the
crates above reach moves to `internal` or `pub(crate)`. Where it duplicates
another item, it is deleted.** No item is ruled on separately; the slice
lists what it moved. Examples:
- `build_map` and `check_tiling`;
- `RowSplit`, `RawRow` and `validated_prefix`;
- `Truth` and `walk_seek_table`;
- `PgDumpTableOptions`, folded into `PgDumpOptions`;
- `PgDumpUnrepresentable`;
- the strict-unchecked types, which move under the `statistics` namespace
  that claims them.

Three items get a ruling of their own:
- **pgdt-dump's scanner stays API.** Reading a dump's `COPY` events without
  mapping it is what an embedder of "the parsers" would call. The chunked
  protocol that four sites in the crates above write out (carry, passes,
  line limit) becomes one `ChunkedScanner` beside it, and all four call
  that. This places the protocol under [`concepts.md`](concepts.md)'s "The
  scanner", in one place.
- **pgdt-core's `instrument` namespace stays API.** Its hooks are already
  `internal`'s; what it holds is what a binary built with `introspect` reads,
  and a counting allocator, which only a binary installs, calls `allocated`
  and `freed` (D106).
- **`xz_seek::SeekTable` stops showing through the API.** Today it surfaces
  in `KnownCompression::Xz` and `ByteRangeSource::seek_table`. It becomes an
  opaque `pgdt-io` type, because the crate it comes from is vendored and
  unpublished.

## Values passed back together

This closes `KD115` and the rest of cluster D.
- **Back-filling a block is one call.** It takes the source, the
  arrangement, the request, the metadata and `&mut CopyBlock`. It computes
  the column mask itself, takes the statistics allowance from the
  arrangement, and applies what it read to the block. `bounded_columns`,
  `StatisticsBackfill` and `BlockReread` become `pub(crate)`.
- **`ResolvedSchema` is one record per column**, holding the Arrow field, the
  `NestedPlan`, the note and the comparison plan, read through accessors. It
  no longer has six parallel `pub` vectors, and the copy of each column's
  resolution in its note goes.
- **A `ValueKey` is built from the column record.** `ValueKey::of` and
  `ValueKey::literal` take it in place of the schema and an index. The record
  carries the semantics a widened column was read as text under (D100), and
  a key asked for in the other semantics answers `None`, closing `KD121`
  (history, 2026-10-17, "36.4.5's call reviewed: a key's resolution names its
  semantics").
- **Rendering takes the column record.** `render_field`,
  `render_field_into` and the text-to-Arrow reader no longer take a
  `NestedPlan` the caller indexed out, so `pgdt` stops carrying the plans
  beside every batch.

## Model fields a view interprets

This closes cluster J. **Plain data keeps its `pub` fields.** A field
becomes private only where a method gives it its meaning:
- **`ColumnStatistics` and `ColumnBounds`** are read only through the methods
  that take a statistics view.
- **`CopyBlock`'s census is one `Option<BlockCensus>`**, replacing two fields
  that only `set_census` keeps in step.
- **A block's database is stored once**, on its span, as D34 asks.
- **`Diagnostic` and `Unrepresentable`** take private fields with getters.
  Their constructors stay `internal`.
- **`StatisticsRequest`'s sizing is one enum**: a stated size, or density
  bounds. The field that reads as stated and the method of the same name
  that answers resolved both go.

The census and the database change what the cache persists, so the slice
that lands them bumps `CACHE_FORMAT_VERSION` (the `bump-cache-format`
skill).

## Readying a DataFusion session

This closes cluster M.
- **One `datafusion-pgdump` function readies a session.** It takes a
  `SessionStateBuilder` and applies three things: the guard that refuses
  `pgdump_unrepresentable` at planning, what `install` does today, and
  `enable_aggregate_dynamic_filter_pushdown` off (`KD56`), unless the caller
  already set that option. The crate doc's two "an embedder should" requests
  become this call. UF1's code marker moves with the workaround.
  The environment override stays `pgdt sql`'s.
- **A finding keeps its subject.** `Located` becomes public, so a structured
  sink that downcasts a finding still knows the table it concerns.
  `DiagnosticSink`'s signature is unchanged.
- **`PgDumpTable` takes its sink when it is built**, not through a later
  `report` call.

## The small fixes

These close the rest of clusters C, G and L.
- **The caller says when the cache is disabled.** `CacheMode::resolve` stops
  reading a path of `"none"` as disabled. That spelling is `pgdt`'s to parse.
- **Reading a cache alone, without its source, is its own type.** `CacheMode::Offline`
  becomes a type no live entry point accepts, and
  `Error::CacheModeMismatch` goes.
- **`pgdt_core::decode_field` and `encode_field` become `unescape_field` and
  `escape_field`.** `TableName::qualified` and `CopyHeader::qualified_name`
  take one name. `DumpIndex::blocks_for(&str)` goes with the selector.
- **A filter term's operator carries its value.** Neither `IsNull` with a
  value nor `Eq` without one can be built.
- **`scan` and `preamble_only` take only the fields they read.** They stop
  taking a `ScanOptions` whose `cancel` they ignore by design (D26), as D77
  already requires of the statistics request.
- **The cancellation is the mapping pass's own argument, and only the
  mapping pass takes one.** `map_file` takes it as a required argument
  beside the statistics request, not inside an options value. Options are
  configuration, reused across runs, and a fired one-shot token cloned
  inside one stops every later run at byte 0. A query takes none. Dropping
  its stream is its cancellation, as it already is for the provider (D90)
  and `pgdt query`. The token is `tokio_util::sync::CancellationToken`,
  which a tokio embedder already holds, in place of pgdt-io's own
  `Cancellation`. `ScanOptions` is then a `ReadOptions` and a `Parallelism`,
  all that `scan`, `preamble_only`, `build_index` and `build_map` read,
  and they take it; `map_file` gathers, and takes the arrangement (The
  arrangement). `ChunkedScanner` and the replay take
  a `ReadOptions`. Reasoning: [history, 2026-10-18](../status/history/2026-10-18.md),
  "The cancellation is the mapping pass's argument".

## Proof: a test written as an embedder

The phase claims that a non-DataFusion embedder needs nothing from `pgdt` or
`internal`. **One integration test target in `pgdt-query/tests/` makes that
claim checkable.** It walks five journeys over a fixture, naming only API
paths:
1. open a source and map it with statistics;
2. load a cache and judge it;
3. list the tables and their Arrow types;
4. stream with a projection and a filter: serially, partitioned, in file
   order, and resumed;
5. summarise.

**`scripts/layering.py` holds that file to the rule it holds `EMBEDDERS`
to**: no `internal` path, and no `timed!` or `row_evaluated!`. Today the
script reads only the three embedder crates. The test is also the reference
that P24's binding starts from.

## What a hot-path slice owes

Three reshapes touch per-chunk or per-field code:
- `ChunkedScanner`, in the scan, the mapping pass, the back-fill and the
  replay;
- rendering through the column record;
- the key the statistics gatherer holds, which 36.4 built as a `ValueKey`
  and its rework holds as `internal`'s `OrderKey` (history, 2026-10-16,
  "36.4's rework recovers the data-level rise; the summary reads by kind").

**Each of those slices takes `instructions:u` (`count-instructions`) on the
`parse` and `typed` shapes, at its parent commit and at its own**, and is
refused if the count rises beyond the instrument's noise: **a rise is beyond
it where it is resolved on any row**, the tree's fewest above the parent's
most, the rule `downstream_verdict` already reads counts by. The two commits are
adjacent, so the difference is the slice and nothing else
(`.claude/skills/evidence/SKILL.md`, rule 7). The count is the reading D105
already ranks by. Each slice row names its instrument
([`roadmap.md`](roadmap.md), "A slice row that commits to a measurement
names its instrument"). The command that takes it is **`measure.py
--count-pair <rev>`, which this phase's first slice builds.**

**A refusal is reworked or accepted, never left standing.** A rise the
slice's own code costs is reworked where a shape as clean recovers it, and
accepted where none does: this phase's goal is the architecture, and a rise
the size of a reshape's indirection does not outweigh it. Reasoning:
[`../status/history/2026-10-16.md`](../status/history/2026-10-16.md),
"36.4's refusal: attribute exactly, then rework or accept".
- **It builds the shipped (fat) leg twice**, once at `<rev>` and once at the
  working tree. Each build runs in the release image, in its own target
  directory under the scratch alloc-builds root, from a git worktree, as
  `--check`'s generator verification already does.
- **It counts `instructions:u` `release-profile`'s number of reps a shape on
  each leg**, because one build's counts move between runs (36.1's notes,
  "The first reading: two builds of one source"), so a change is read against
  a spread the pair takes at its own two trees, never at another tree's.
  The shapes are: `parse` at the metadata level; `parse` at the data level,
  which covers gathering; `query` strings; `query` typed; and `pgdt sql`.
- **It prints each leg's fewest and most, and each row resolved or
  unresolved**, a fact rather than a verdict. On `pgdt sql` the count's two
  values lie inside both legs' ranges, so a rise smaller than their step is
  unresolved there; the `typed` row counts the same decode at a finer
  resolution.
- **Its first run is a pair of identical sources**, `--count-pair HEAD~1`
  after the commit that adds the reps, which touches only `scripts/`. **Any
  row resolving there falsifies the instrument**: it would be pricing the
  build (mimalloc's `__TIME__` moves each leg's layout) and not the change,
  and no hot-path slice is counted until that is answered.
- **It refuses unless `<rev>` is the working tree's parent commit**, so it
  can never price more than one slice.
- **It prints a comparison and publishes no figure.** Nothing reaches
  `measurements.md`. `test_measure.py` asserts its rules.

`release-profile` sat twice per slice was refused. It means three legs built
twice each, and its `parse` row never reaches statistics gathering. **The
first slice says whether a run fits under `CLAUDE.md`'s ten-minute line.**
If it does not, a hot-path slice launches the count detached, and the slice
after it opens by reading the result.

## What this phase does not do

- **Add a user-visible capability.**
- **Reshape the CLI**, beyond following a library change (Scope).
- **Change a behaviour.** `KD112` stays unowned, and D2, D52 and D90 stand as
  written. A structural change made for the API is no exception: the
  architecture may move, but what the code does may not.
- **Upgrade a dependency**, or start P24's binding.
- **Promise stability.** Everything reshaped here is free to move again
  before 1.0.

## Slices, in the order their evidence demands

**The first slice builds the instrument that the hot-path slices are
counted with. The rest go bottom-up through the crate graph**, so each
consumer adapts once, to a lower surface that has already settled. Every
slice also:
- demotes its own crates' plumbing (D106 as written);
- updates [`concepts.md`](concepts.md) for whatever it adds or moves;
- keeps every file under its limits and every module edge one-way.

Where a slice finds a crate boundary in the wrong place for the surface,
it moves the boundary. That goes through a spec amendment and a placement on
the concept map; the slice is not bent around the boundary.

1. **`measure.py --count-pair <rev>`** (What a hot-path slice owes). It also
   says whether a run fits under ten minutes.
2. **pgdt-io: the arrangement.** One resolution; `Parallelism`'s
   constructors cut to three; the statistics allowance; `AllowanceOrigin`
   moved down; D11 rewritten and its seams and tuning constants made
   `internal`; the seek table opaque. `pgdt` and `ScanBudget` call the
   resolution. Closes `KD29`, because the limit is discovered once and the
   resolution takes it already read.
3. **pgdt-core and pgdt-model.** The escape pair renamed; the qualified-name
   pair given one name; the model fields a view interprets; the census as
   one field and the database stored once, with `CACHE_FORMAT_VERSION`
   bumped; `StatisticsRequest`'s sizing as one enum.
4. **pgdt-codec: a key for every value.** Closes `KD116` and `KD117`.
   Counted: `measure.py --count-pair HEAD~1`.
5. **pgdt-dump: `ChunkedScanner`.** All four sites that write out the
   chunked protocol call it. Counted: `measure.py --count-pair HEAD~1`.
6. **pgdt-dump: the open protocol.** `CacheMode` opens an `Origin`. The free
   `load` and `save` go, `Valid` and `Incomplete` are merged, the cache is
   decoded once, `Offline` becomes its own type, and the caller states
   `Disabled`. `scan` and `preamble_only` take only the fields they read.
7. **pgdt-mapping: back-fill as one call.** Closes `KD115`. The mapping pass
   hands back what a `Dump` is built from.
8. **pgdt-query: the `Dump`.** The selector; `Table::plan` with partitions, a
   file-ordered stream and serial resume; `read_table` gone; `max_rows` on
   the stream; filter operators carrying their values; the summary and the
   divergences as table methods. `pgdt query` moves onto it,
   and `pgdt`'s merge goes.
9. **pgdt-query: one record per column.** `ResolvedSchema` reshaped, and
   rendering and `ValueKey` through the record. Closes `KD121`. Counted: `measure.py --count-pair HEAD~1`.
10. **Errors in library terms.** `pgdt` gets a spelling layer, with a test
    pinning each refusal's flag. The concept map's "Not in one place" line on
    the cache's wording is struck. Closes `KD92`: the provider's spelling of
    "map it first" carries the mode it was opened under.
11. **datafusion-pgdump.** `PgDump` built over a `Dump`; one function readies
    a session; `Located` public; the sink taken at construction;
    `PgDumpTableOptions` folded into `PgDumpOptions`.
12. **The embedder test**, with `layering.py` holding it to the embedder
    rule.
