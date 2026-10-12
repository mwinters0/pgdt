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
  pool every arm but the remote one reads into, and what both `.xz` sources
  share above the two of them.

## Memory and parallelism

The memory allowance and its discovery, a caller's parallelism, what a worker
holds resident, the partition advice a source gives and the cut
([`decisions.md`](decisions.md), "I/O, memory and parallelism").

- **pgdt-io** — `memory.rs`, `parallelism.rs`, `partition.rs`: a source says
  what a worker holds, so sizing sits beside it (D68); the partition advice
  and the cut read a worker's cost, and nothing in sizing reads them.
- **datafusion-pgdump** — `budget.rs`: the one allowance every scan a session
  runs draws from.
- **pgdt** — `parallel.rs`: `--jobs` and `--memory` as a front end spells
  them, and where each number it resolves came from, which the library has
  nowhere to carry (D64).

## Stopping

A caller's ask that a scan stop, and a signal that ends the process
([`decisions.md`](decisions.md), "D26").

- **pgdt-io** — `cancel.rs`: the token a scan polls, below everything that
  scans.
- **namespace-init** — `lib.rs`, `unshare.rs`: ending as a PID namespace's
  init on a signal, shared by `pgdt` and the DataFusion CLI it composes.
- **pgdt** — `interrupt.rs`: what a signal does to a `parse`, the binary's
  own: cancel the scan, save, then end by that signal.

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
- **count-instructions** — `main.rs`: the user-space instructions
  `scripts/measure.py`'s `release-profile` figure reads beside its wall clock.

## The `COPY` format

A `COPY` header, and the text format's rows and fields: split, unescaped,
escaped ([`decisions.md`](decisions.md), "The scanner").

- **pgdt-core** — `copy.rs`: the scanner, the decoders, the batcher and the
  gatherer all read it, so it sits below them all (D68).

## SQL lexing

Which bytes of a line outside a `COPY` block are code, and which are inside a
string, a quoted identifier or a comment; a type name canonicalized.

- **pgdt-dump** — `lex.rs`: read by the scanner, the map's statement scan
  and the DDL grammar.
- **pgdt-core** — `type_name.rs`: a type name's canonical form, read by the
  DDL grammar and by the catalog and the type register, so below the model
  and the codec, and kept out of the model, whose edits ask whether the
  cache format bumps.

**Not in one place.** A schema-qualified name is read four ways: pgdt-core's
`COPY` header splits and unquotes it, its type-name canonical form re-quotes
it, pgdt-dump's DDL grammar joins it raw, and pgdt-codec's collation lookup
splits it without skipping the spaces around the dot the others skip.

## The scanner

The line-anchored state machine finding statements and `COPY` blocks in a
plain-format dump ([`decisions.md`](decisions.md), "The scanner").

- **pgdt-dump** — `scan.rs`: no decoding and no Arrow (D74).

## The file map

The spans tiling a dump and the `COPY` blocks among them, the builder that
extends a map, and what is found wrong with the file
([`decisions.md`](decisions.md), "The file map and the preamble").

- **pgdt-model** — `span.rs`: what the cache persists (D68); `diagnostic.rs`:
  what is found wrong with the file, recomputed on load rather than persisted.
- **pgdt-dump** — `map/`: building a map — where a statement ends, a comment
  block read, and what every map is checked for and carries.

## The mapping pass

Extending a map forward from its frontier, a block's interior split into
pieces and folded back, each block closed, the cache saved on a throttle
([`decisions.md`](decisions.md), "D48", "Batches, streams and the leader").

- **pgdt-mapping** — `forward/`, `whole_file.rs`, `backfill.rs`,
  `interior/`: the pass whole — a map extended
  from its frontier with its observers attached, the whole-file map and the
  back-fill re-reading what a mapped block lacks, and a block's interior split
  into pieces and folded. Above the codec, since a block it maps is gathered
  as it is read, and below the query layer, since it builds no array (D68).
- **pgdt-query** — `stream/mapping.rs`: pass 1 of a query, which reads the
  replay's resume token.

## The declared schema

What the preamble declares, per database: tables, types, collations and
extensions, kept as the words written ([`decisions.md`](decisions.md), "The
file map and the preamble").

- **pgdt-model** — `catalog.rs`: persisted, so never a codec conclusion
  (D74).
- **pgdt-dump** — `ddl/`, `preamble.rs`: the statement grammar, applied
  across the whole file, and the prepass reading the region before the first
  data, which the scanner and the grammar drive and pgdt-query's pass 1
  reads too.

## The cache

Persisting a map and reloading it, the dump it was saved from identified
([`decisions.md`](decisions.md), "The compressed source and the cache"). Its
format is pgdt-model's, `CACHE_FORMAT_VERSION` at that crate's root.

- **pgdt-dump** — `cache/`: the loader scans, so the cache sits beside the
  parsers (D68).
- **datafusion-pgdump** — `dump.rs`: a dump opened through its complete
  cache (D90).

**Not in one place.** Why a cache cannot be used is worded twice, by
pgdt-core's `Error` and by the `pgdt` binary's `info`, and the two disagree
(a changed source among them).

## What a pass records of a block's rows

The array-shape census and the count of values a column's Arrow type cannot
hold, and the contract over a block's rows that counts them, which a crate
above implements ([`decisions.md`](decisions.md), "D35", "D74").

- **pgdt-model** — `block.rs`: persisted, and a contract over the data is
  model wherever its implementor sits (D74); how a row is folded in is
  model's too, read by every pass that records, each above it.
- **pgdt-codec** — `unrepresentable.rs`: told from a field's text against an
  Arrow type, so codec's.

## Type mapping

A declared type to an Arrow type, and a `COPY` header's columns joined
against the declared schema ([`decisions.md`](decisions.md), "Type
resolution and decoders").

- **pgdt-codec** — `declared.rs`, `pgtype.rs`, `resolve.rs`: Arrow's schema
  types and no more (D74). A declared type read against the catalog is
  beneath every module reading one — the mapping, the comparison register, a
  strict parse's grammar, the datetime grammar.

## Values

Each type's field read into what Arrow holds and rendered back, the date and
time grammar, nested literals, and whether PostgreSQL refuses a field
([`decisions.md`](decisions.md), "Type resolution and decoders").

- **pgdt-codec** — `value/`, and the `decode.rs` and `nested.rs` namespaces
  declaring from it: synchronous and free of Arrow past its schema types, so
  a decoder takes a field and returns a value (D74). A type family is one
  module — its reader, renderer, refusal classifier and key type — and a
  scalar one reads nothing of the order key's dispatch, which the nested
  family's keys are built through.
- **pgdt-query** — `render.rs`: a value read back from an Arrow array, and one
  field's text read into the array a batch of its column would hold; reading
  an array is query's (D74), and it is a one-row column, not the batcher's.

## Comparison semantics

How PostgreSQL orders a value: the comparison register — plans, collation,
where DataFusion's comparison diverges — and the order keys read from a
value's text ([`decisions.md`](decisions.md), "Predicates").

- **pgdt-codec** — `compare/`, `key.rs`: value semantics, shared by a filter
  and the gatherer, so below both (D68). The key is the dispatch building an
  `OrderKey` over the families' key types, which are each family's own
  (Values), the nested keys included. The built-in table answering a type's
  comparison beside its Arrow type is the type mapping's (D40).

## Arrow batches

A block's rows into typed `RecordBatch`es, one builder per column, views over
the retained read buffers ([`decisions.md`](decisions.md), "Batches, streams
and the leader").

- **pgdt-query** — `options.rs`, `batch/`: what a query asks for, and the
  batcher, its column builders and the read chunks its views point into;
  building arrays is query's (D74).

## Filtering

The filter expression — terms, membership, three-valued evaluation, its
resolution against a block, a filter refined while a query runs — and each
front end's spelling of it ([`decisions.md`](decisions.md), "Predicates").

- **pgdt-query** — `predicate/`, `stream/replay/dynamic.rs`: it evaluates the rows
  the batcher splits, and answers a row group off its statistics for pruning
  to read.
- **pgdt** — `query/filter/`: `--filter`'s and `--where`'s grammars, a front
  end's own syntax: the term grammar at the bottom, the expression over its
  leaves above it, and `--filter`'s entry above both (D60).
- **datafusion-pgdump** — `filter/`, `unrepresentable.rs`: DataFusion's
  expressions as the library's filter tree, the leaves in the parent and the
  static and dynamic translations its children, neither reading the other.

## Strict parse

Every field of a block checked against its type's `*_in` and its column's
typmod ([`decisions.md`](decisions.md), "D103").

- **pgdt-mapping** — `statistics/gather/strict.rs`: an arm of the block
  observer, built where a block's gatherer is.
- **pgdt-codec** — `grammar/`, the namespace declaring the types a strict
  parse's listing and the register's plans carry: what a column's fields are
  read by, whether PostgreSQL refuses one, dispatching to the readers a key is
  built by, and the form a filter's literal has to take.

## Statistics

What a pass records of each column per row group, and what reads it: pruning,
a table's summary, DataFusion's statistics, `pgdt info --detail`
([`decisions.md`](decisions.md), "Statistics").

- **pgdt-model** — `statistics.rs`: the stored shape, persisted, and the
  contract over a block's rows that answers it, model wherever its
  implementor sits (D74).
- **pgdt-mapping** — `statistics.rs`, `statistics/request.rs`,
  `statistics/account.rs`, `statistics/stored.rs`, `statistics/gather.rs`,
  `statistics/gather/gatherer.rs`, `statistics/gather/column.rs`,
  `statistics/gather/bounds.rs`, `statistics/gather/order.rs`,
  `statistics/gather/clip.rs`, `statistics/gather/charge.rs`: what a pass
  gathers and what holding it costs, the mapping pass's because gathering
  decodes and builds no array (D68); what a column's statistics are stored
  against, which pruning and summaries read.
- **pgdt-query** — `prune.rs`, `summary.rs`: what reads the statistics —
  pruning through the filter expression, a summary folding every group.
- **datafusion-pgdump** — `statistics.rs`: the map's statistics as
  DataFusion's.
- **pgdt** — `info/statistics.rs`: what `pgdt info --detail` prints of them.

## The replay

Pass 2: each matched block resolved for the query and pruned, the replay cut
into sub-streams and read in file order into batches, and the stream an
embedder pulls, with its resume token ([`decisions.md`](decisions.md),
"Batches, streams and the leader").

- **pgdt-query** — `stream.rs`, `stream/table.rs`, `stream/partitions.rs`,
  `stream/resume.rs`, `stream/replay.rs`, `stream/replay/plan.rs`,
  `stream/replay/segment.rs`, `stream/replay/partition.rs`: the top of the
  stream, pass 1 then pass 2, which no child reads; the handle an embedder
  pulls; the partitioned replay; the resume token; and beneath the replay
  loop, what it plans, the pieces it reads and how they are cut.

## What a query tells its caller

What a plan settles and what registering a dump finds, said to whoever reads
it ([`decisions.md`](decisions.md), "Batches, streams and the leader").

- **pgdt-query** — `stream/note.rs`: each `PlanNote`, and the blocks an early
  stop left unread.
- **datafusion-pgdump** — `report.rs`: what registration and planning report
  to the caller's sink.
- **pgdt** — its `query` command's `announce_*`: a front end printing each
  note where its output already speaks.

## The DataFusion provider

A dump's databases as DataFusion catalogs, a table as a `TableProvider`
scanned by the partitioned replay, and the `pgdump.` session settings
([`decisions.md`](decisions.md), "D90", "D94").

- **datafusion-pgdump** — `lib.rs`, `error.rs`, `catalog.rs`, `factory.rs`,
  `table.rs`, `exec.rs`, `settings.rs`: an embedder, above every library
  crate, its root declaring and defining nothing.

## The DataFusion CLI

`datafusion-cli` with `--dump` and `STORED AS PGDUMP`: upstream's `main.rs`
with each change marked, and everything of ours beside it.

- **datafusion-cli-pgdump** — `lib.rs`, `pgdump.rs`.

## The `pgdt` CLI

The three subcommands, their flags and what each prints
([`decisions.md`](decisions.md), "The CLI").

- **pgdt** — `main.rs`, `version.rs`, `args.rs`, `source.rs`, `parse.rs`,
  `info.rs`, `info/text.rs`, `info/json.rs`, `info/labels.rs`, `query.rs`: a
  module a subcommand, each owning its flags; the root dispatches and holds
  nothing a subcommand reads, and what more than one reads — the shared flag
  groups, the source opened against its cache — sits beside them.

## The binary's build

Which allocator and Cargo profile built `pgdt`, as `pgdt --version` names
them ([`decisions.md`](decisions.md), "D105").

- **pgdt** — `alloc.rs`, `profile.rs`: the binary's choice, never the
  library's.

## The fixture vocabulary

What every crate's tests address the fixtures by: the generated tree and its
enumerator, the sandboxes a test writes a cache beside, the multi-database
concatenation, the fields PostgreSQL refuses and how a sweep maps past them,
and the hand-written dump ([`decisions.md`](decisions.md), "D68").

- **pgdt-fixtures** — `lib.rs`, `refused.rs`: a member only a dev-dependency
  reaches, so every crate's tests share one copy with the graph one-way; the
  sweep's request builders, which name the statistics request, behind its
  `mapping` feature.
