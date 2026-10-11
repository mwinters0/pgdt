# The concept map

Each concept the code holds, the crates and modules holding it, and why each
crate holds its part. **Read it before deciding where new code goes.** The rule
it serves is [`../process.md`](../process.md), "Seams"; `scripts/concepts.py`
holds every production file in the workspace to exactly one concept here.

A line `- **<crate>** — …` claims the backticked paths in it, relative to that
crate's `src/`: a file, or a directory written with a trailing `/`, which
claims everything under it and the `<dir>.rs` beside it. A crate root
(`lib.rs`, `main.rs`) holding only its crate's doc, `mod` lines and
re-exports is claimed by none. **A concept spans crates only along the
layering** ([`decisions.md`](decisions.md), "D68", "D74"), and each crate's
part says why it sits there. **Not in one place** names what today's tree
scatters and the layering does not explain; the `seams` skill's design removes
it.

## The byte sources

Where a dump is and how its bytes are read: the read contract, the plain file,
the `.xz` file pulled or fetched, the remote object
([`decisions.md`](decisions.md), "The compressed source and the cache").

- **pgdt-io** — `source/`: the whole concept, below the parsers that read
  through it — the read contract with its arms nested under it, the buffer
  pool they all read into, and what both `.xz` sources share above the two of
  them.

## Memory and parallelism

The memory allowance and its discovery, a caller's parallelism, what a worker
holds resident, the partition advice a source gives and the cut
([`decisions.md`](decisions.md), "I/O, memory and parallelism").

- **pgdt-io** — `memory.rs`, `parallelism.rs`, `partition.rs`: a source says
  what a worker holds, so sizing sits beside it (D68); the partition advice
  and the cut read a worker's cost, and nothing in sizing reads them.
- **datafusion-pgdump** — `budget.rs`: the one allowance every scan a session
  runs draws from.

**Not in one place.** `pgdt`'s parallelism flags are in its `main.rs`.

## Stopping

A caller's ask that a scan stop, and a signal that ends the process
([`decisions.md`](decisions.md), "D26").

- **pgdt-io** — `cancel.rs`: the token a scan polls, below everything that
  scans.
- **namespace-init** — `lib.rs`, `unshare.rs`: ending as a PID namespace's
  init on a signal, shared by both binaries.

**Not in one place.** `pgdt`'s interrupt guard (`InterruptGuard`, `die_by`)
is in its `main.rs`.

## Failure

The library's vocabulary of failure ([`decisions.md`](decisions.md), "D74").

- **pgdt-core** — `error.rs`: every crate fails in it, and a dependency's
  error is a cause it carries opaquely.

## Self-measurement

What the library records of its own work, and what a process built to say so
reports ([`roadmap.md`](roadmap.md), "Attribution is introspective; only
the gate is blind").

- **pgdt-core** — `instrument/`: the hooks every crate calls.
- **pgdt** — `introspect.rs`: what the process says about its memory, in an
  `introspect` build alone.
- **peak-rss** — `main.rs`: the peak resident set `scripts/measure.py` reads.

## The `COPY` format

A `COPY` header, and the text format's rows and fields: split, unescaped,
escaped ([`decisions.md`](decisions.md), "The scanner").

- **pgdt-core** — `copy.rs`: the scanner, the decoders, the batcher and the
  gatherer all read it, so it sits below them all (D68).

## SQL lexing

Which bytes of a line outside a `COPY` block are code, and which are inside a
string, a quoted identifier or a comment; a type name canonicalized.

- **pgdt-dump** — `lex.rs`: read by the scanner and the DDL grammar alone.
- **pgdt-core** — `type_name.rs`: a type name's canonical form, read by the
  DDL grammar and by the catalog and the type register, so below the model
  and the codec, and kept out of the model, whose edits ask whether the
  cache format bumps.

## The scanner

The line-anchored state machine finding statements and `COPY` blocks in a
plain-format dump ([`decisions.md`](decisions.md), "The scanner").

- **pgdt-dump** — `scan.rs`: no decoding and no Arrow (D74).

## The file map

The spans tiling a dump and the `COPY` blocks among them, the builder that
extends a map, and what is found wrong with the file
([`decisions.md`](decisions.md), "The file map and the preamble").

- **pgdt-model** — `span.rs`, `diagnostic.rs`: what the cache persists (D68).
- **pgdt-dump** — `map.rs`, `load.rs`: building a map, and loading one
  before a scan.

**Not in one place.** `load.rs` holds the preamble region's prepass and the
map's tiling diagnostics, and loads nothing (`cache/` does); the map's
statement framing (`StatementScan`) is in `preamble.rs`.

## The mapping pass

Extending a map forward from its frontier, a block's interior split into
pieces and folded back, each block closed, the cache saved on a throttle
([`decisions.md`](decisions.md), "D48", "Batches, streams and the leader").

- **pgdt-dump** — `leader.rs`: what one piece of a block answers alone and
  how the pieces fold; it decodes nothing, so it sits by the scanner.
- **pgdt-query** — `stream/map.rs`, `stream/map_query.rs`: the leader that
  drives the pieces with the gatherer attached, query's because gathering
  decodes (D68); and pass 1 of a query.

**Not in one place.** `leader.rs` is named for its caller, the leader being
`stream/map.rs`'s `map_forward`; `stream/map.rs` and `stream/backfill.rs`
import each other.

## The declared schema

What the preamble declares, per database: tables, types, collations and
extensions, kept as the words written ([`decisions.md`](decisions.md), "The
file map and the preamble").

- **pgdt-model** — `catalog.rs`: persisted, so never a codec conclusion
  (D74).
- **pgdt-dump** — `preamble.rs`: the DDL statement grammar.

**Not in one place.** "Preamble" names two things: the DDL grammar
(`preamble.rs`, applied across the whole file) and the region's prepass
(`load.rs`).

## The cache

Persisting a map and reloading it, the dump it was saved from identified
([`decisions.md`](decisions.md), "The compressed source and the cache"). Its
format is pgdt-model's, `CACHE_FORMAT_VERSION` at that crate's root.

- **pgdt-dump** — `cache/`: the loader scans, so the cache sits beside the
  parsers (D68).
- **datafusion-pgdump** — `dump.rs`: a dump opened through its complete
  cache (D90).

**Not in one place.** A cache's identity: `WeakIdentity`, `OriginMatch`,
`StrictIdentity` and `SourceWatch` are in `cache.rs`, `SourceIdentity` in
`cache/identity.rs`, which imports back from its parent.

## What a pass records of a block's rows

The array-shape census and the count of values a column's Arrow type cannot
hold, and the contracts over a block's rows a crate above implements
([`decisions.md`](decisions.md), "D35", "D74").

- **pgdt-model** — `block.rs`: persisted, and a contract over the data is
  model wherever its implementor sits (D74); how a row is folded in is
  model's too, read by every pass that records, each above it.
- **pgdt-codec** — `unrepresentable.rs`: told from a field's text against an
  Arrow type, so codec's.

## Type mapping

A declared type to an Arrow type, and a `COPY` header's columns joined
against the declared schema ([`decisions.md`](decisions.md), "Type
resolution and decoders").

- **pgdt-codec** — `pgtype.rs`, `resolve.rs`: Arrow's schema types and no
  more (D74).

**Not in one place.** `pgtype.rs` holds two other concepts' parts: the
comparison register (comparison semantics) and the text grammar a strict
parse reads.

## Values

Each type's field read into what Arrow holds and rendered back, the date and
time grammar, nested literals, and whether PostgreSQL refuses a field
([`decisions.md`](decisions.md), "Type resolution and decoders").

- **pgdt-codec** — `decode/`, `datetime_in.rs`, `nested.rs`: synchronous and
  free of Arrow past its schema types, so a decoder takes a field and returns
  a value (D74).

**Not in one place.** Four families — float, numeric, bytea, timestamp — are
split between `decode.rs` and a child named for its visibility
(`decode/refusal.rs`, `decode/stored.rs`, `decode/float.rs`); the refusal
classifiers sit in six places, `key/order.rs` among them; JSON's grammar is in
`decode/json.rs` and jsonb's in `key/order.rs`. A value rendered back from an
Arrow array (`render_field`, `decode_field`) is in pgdt-query's `batch.rs`:
reading an array is query's (D74), but it is filed with the batcher rather than
as its own module.

## Comparison semantics

How PostgreSQL orders a value: the comparison register — plans, collation,
where DataFusion's comparison diverges — and the order keys read from a
value's text ([`decisions.md`](decisions.md), "Predicates").

- **pgdt-codec** — `key/`, `pgtype/discrete.rs`, `pgtype/divergence.rs`:
  value semantics, shared by a filter and the gatherer, so below both (D68).

**Not in one place.** The register itself (`CompareKind` to `comparison_for`)
is in `pgtype.rs`, and `discrete.rs` and `divergence.rs` are one item each,
split off it by visibility; `key/order.rs` holds every key family, a jsonb
parser and refusal text; about 2,300 lines of pgdt-query's
`predicate/tests.rs` test these keys.

## Arrow batches

A block's rows into typed `RecordBatch`es, one builder per column, views over
the retained read buffers ([`decisions.md`](decisions.md), "Batches, streams
and the leader").

- **pgdt-query** — `batch/`: building arrays is query's (D74).

**Not in one place.** `batch.rs` also holds a query's request types
(`QueryOptions`, `ScanExtent`), rendering back to text, and `read_table`,
which drains a stream.

## Filtering

The filter expression — terms, membership, three-valued evaluation, its
resolution against a block, a filter refined while a query runs — and each
front end's spelling of it ([`decisions.md`](decisions.md), "Predicates").

- **pgdt-query** — `predicate.rs`, `stream/dynamic.rs`: it evaluates the rows
  the batcher splits.
- **pgdt** — `where_expr.rs`: `--where`'s grammar, a front end's own syntax.
- **datafusion-pgdump** — `pushdown.rs`, `dynamic_filter.rs`,
  `unrepresentable.rs`: DataFusion's expressions as the library's filter tree.

**Not in one place.** Resolution: `resolve_expr` and `resolve_loosened` are in
`stream/resolve.rs`, `resolve_term` in `predicate.rs`. `--filter`'s term
grammar is in `pgdt`'s `main.rs`, and `where_expr.rs` calls back into it for
its leaves. The truth sets over a group's statistics are in `predicate.rs`,
read by `prune.rs` alone.

## Strict parse

Every field of a block checked against its type's `*_in` and its column's
typmod ([`decisions.md`](decisions.md), "D103").

- **pgdt-query** — `gather/strict.rs`: an observer over a block's rows, as the
  gatherer is.
- **pgdt-codec** — `pgtype/strict.rs`: what a column's fields are read by.

**Not in one place.** The codec's part comes from three modules:
`pgtype/strict.rs`, `TextGrammar` in `pgtype.rs`, and `field_refused` and
`grammar_refuses` in `key/order.rs`.

## Statistics

What a pass records of each column per row group, and what reads it: pruning,
a table's summary, DataFusion's statistics, `pgdt info --detail`
([`decisions.md`](decisions.md), "Statistics").

- **pgdt-model** — `statistics.rs`: the stored shape, persisted (D74).
- **pgdt-query** — `statistics.rs`, `gather.rs`, `gather/bounds.rs`,
  `gather/charge.rs`, `gather/column.rs`, `gather/dictionary.rs`,
  `gather/gatherer.rs`, `gather/order.rs`, `gather/tracking.rs`, `prune.rs`,
  `summary.rs`, `stream/backfill.rs`: gathering decodes (D68), and pruning
  and summaries read what it stores.
- **datafusion-pgdump** — `statistics.rs`: the map's statistics as
  DataFusion's.
- **pgdt** — `info_statistics.rs`: what `pgdt info --detail` prints of them.

**Not in one place.** In pgdt-query the concept is four sibling modules and a
child of `stream/`, not one tree; the contract between the gatherer and its
readers is in `gather/tracking.rs`, which `prune.rs` and `summary.rs` import
from inside the gatherer; the truth sets are in `predicate.rs`.

## The replay

Pass 2: each matched block resolved for the query and pruned, the replay cut
into sub-streams and read in file order into batches, and the stream an
embedder pulls, with its resume token ([`decisions.md`](decisions.md),
"Batches, streams and the leader").

- **pgdt-query** — `stream.rs`, `stream/table.rs`,
  `stream/table_partitions.rs`, `stream/partition.rs`, `stream/plan.rs`,
  `stream/segment.rs`, `stream/replay.rs`, `stream/resolve.rs`.

**Not in one place.** `TableColumns`, `resolve_block` and
`refuse_metadata_level` are in `stream/table.rs` and used only by `plan.rs`
and `dynamic.rs`; `ResumeToken` is split between `table.rs` and `resolve.rs`;
`resolve.rs` and `segment.rs` each hold three things; `plan`, `segment`,
`resolve`, `dynamic` and `table` import each other in cycles.

## What a query tells its caller

What a plan settles and what registering a dump finds, said to whoever reads
it ([`decisions.md`](decisions.md), "Batches, streams and the leader").

- **pgdt-query** — `stream/note.rs`: each `PlanNote`, and the blocks an early
  stop left unread.
- **datafusion-pgdump** — `report.rs`: what registration and planning report
  to the caller's sink.

**Not in one place.** `pgdt`'s `announce_*` functions are in its `main.rs`.

## The DataFusion provider

A dump's databases as DataFusion catalogs, a table as a `TableProvider`
scanned by the partitioned replay, and the `pgdump.` session settings
([`decisions.md`](decisions.md), "D90", "D94").

- **datafusion-pgdump** — `lib.rs`, `catalog.rs`, `factory.rs`, `table.rs`,
  `exec.rs`, `settings.rs`: an embedder, above every library crate.

## The DataFusion CLI

`datafusion-cli` with `--dump` and `STORED AS PGDUMP`: upstream's `main.rs`
with each change marked, and everything of ours beside it.

- **datafusion-cli-pgdump** — `lib.rs`, `pgdump.rs`.

## The `pgdt` CLI

The three subcommands, their flags and what each prints
([`decisions.md`](decisions.md), "The CLI").

- **pgdt** — `main.rs`.

**Not in one place.** `main.rs` holds every subcommand's body, `info`'s
rendering, `--filter`'s grammar, the parallelism flags and the interrupt
guard; `where_expr.rs` and `info_statistics.rs` are half of a concern whose
other half is still in it.

## The binary's build

Which allocator and Cargo profile built `pgdt`, as `pgdt --version` names
them ([`decisions.md`](decisions.md), "D105").

- **pgdt** — `alloc.rs`, `profile.rs`: the binary's choice, never the
  library's.
