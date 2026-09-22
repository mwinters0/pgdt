# P6 — DataFusion integration

Query a `pg_dump` file's tables from Apache DataFusion: a `TableProvider` over
the library, in a new workspace member, and a build of `datafusion-cli` that
carries it.

Grilled 2026-09-21; its inbox was drained into this file and deleted. **The
first slice exists to produce evidence**: 6.1 checks the library's orderings
against Arrow's before 6.2 builds a comparison mode on the result, so slice
numbers after it are allocation order as much as schedule. The checklist is
`../status/STATUS.md`, "P6 progress".

## Scope

**DataFusion only.** Python bindings are P24's; Spark and Trino are off the
roadmap. The phase's deliverables are the provider crate and a
`datafusion-cli` build with the provider built in, `datafusion-cli-pgdump`.

**The target is the latest DataFusion release, 55.1.0** (tagged upstream and
on crates.io; `datafusion-cli`'s sources are unchanged from 55.0.0), whose
`arrow` pin (59.2.0) is the one this workspace already carries. How the pin moves after
this phase is a standing rule, [`roadmap.md`](roadmap.md), "Arrow follows
DataFusion".

## The provider reads a complete cache, and never maps

**The provider opens a dump only through a cache that covers the whole file
and matches the source** — what `pgdt parse` leaves: `DumpIndex::scanned_through`
equal to the source's size, and the cache's identity checks passing as they do
for any load. A missing, partial or interrupted cache is an error naming
`pgdt parse`. The provider never runs a mapping pass and never writes a cache.
Whether a parse can be *started* from inside the SQL shell is a separate
question, not yet settled.

The alternative, a provider that falls back to a cold query's early-stopping
map, is refused because every one of the following comes back with it:

- **Ambiguity is detected exactly.** A cold query stops mapping once its target
  is settled and can miss a second `COPY` block for the same name past that
  point (`KD6`); a complete map cannot. The provider therefore never exhibits
  `KD6`. The library's cold query still does, so `KD6` is not closed by this
  phase and is `(c) unowned`.
- **A schema is stated before any scan, from evidence that covers every
  block.** Every block in a map carries a complete array-shape census, and a
  complete map holds every block of the table, so the union the provider
  reports is the union a scan would stream. The partial-map question — report
  optimistically, refuse, or expose partiality — does not arise.
- **"This database's DDL was never read" does not arise.** A mapping pass states
  a database's DDL at its first `COPY` block, so a complete map covers every
  database it holds. The two existing answers to that condition (a stream
  refuses with `Error::MetadataNotScanned`, a listing degrades the column to
  `ColumnResolution::MetadataNotScanned`) stay in the library; the provider
  meets neither.
- **The partitioned replay is available.** `table_stream_partitions` requires a
  complete map.

## How a dump appears in SQL

**A dump is registered as a catalog**: its PostgreSQL schemas are DataFusion
schemas and its tables are tables, so `koji.public.build` names a table and
`SHOW TABLES` lists the dump. **The user names the catalog**, because a plain
single-database dump taken without `--create` does not record its database's
name. **A multi-database file registers one catalog per database, named after
that database**; it is refused only where a name is needed and nothing
supplies one. In `datafusion-cli-pgdump` that reads: a bare `--dump <path>`
takes its catalog names from the file; `<name>=` is required only where the
file names no database, and is refused on a multi-database file. Rejected:
prefixing each database with the given name, which invents names nobody wrote;
and one `--dump` per database selected by a fragment.

**A single table can also be registered on its own**, as
`CREATE EXTERNAL TABLE t STORED AS PGDUMP LOCATION '…'` with the table named in
its options — built over the catalog form rather than beside it.

## One schema per table

**A table's schema is its root's declared columns, in DDL order**, typed over
the census unioned across every block the table owns. A table owning several
blocks — the leaf partitions of a partitioned table loaded through its root,
each block's header naming the root (I2) — may list its columns in a different
order per block, so **each block's batches are reordered by name** into the
table's order. A table whose blocks disagree on the *set* of column names is
refused, naming the blocks. A table with no DDL in the file (a data-only dump)
takes the first block's column order under the same set rule. **A block with
no column list copies no columns** (I5: `pg_dump` writes none exactly when every
column is dropped or generated), so each of its rows is an empty line read as a
row of zero fields, and a line that is not empty is refused; a table whose
blocks list nothing has an empty schema and its true row count.

Rejected: the first block's order with every other order refused, which
refuses ordinary partitioned dumps; the union of names with absent columns
filled as `NULL`, which states values PostgreSQL never held; and a list-less
block taking the table's declared names where its width matches, which reads
an empty line as one field and gives a table of generated columns `''` for
every value.

The rule is the library's, so every query presents it, `pgdt query` included,
not the provider alone: it rests on `pg_dump`'s output rather than on
DataFusion, and a per-block shape is the hazard it removes. Settled 2026-09-21
([`../status/history/2026-09-21.md`](../status/history/2026-09-21.md), "One
schema per table is every query's, and a list-less block copies nothing").

## Comparison means what DataFusion means

**Inside DataFusion a comparison has DataFusion's semantics: its own
comparison of the emitted type** — `apply_cmp`, which is Arrow's `cmp` kernels
with float operands' `-0.0` first made `+0.0`, and `make_comparator` for a
nested type. The kernels alone are not the measure. A filter is pushed down
`Exact` exactly when the library evaluates it as DataFusion does over the value
it emits, and `Unsupported` otherwise; nothing is `Inexact`. This keeps a pushed-down answer and a
re-filtered one identical, which is the property that matters — DataFusion
decides per plan whether a filter reaches the provider (an `OR` it cannot
split does not), so a provider answering in any other semantics makes a query's
rows depend on its plan.

Where the library's comparison and DataFusion's differ (`CompareKind`,
`pgdump_query/src/pgtype.rs`) — floats not among them, `-0` equalling `0` and a
dump's one `NaN` sorting last under both:

| Kind | The library compares | DataFusion compares the emitted value |
|---|---|---|
| Text, under any collation | bytewise | bytewise — the same |
| Enum (emitted `Dictionary`) | by declaration order (I33) | by label text |
| Bare `numeric`, `timetz`, `inet`/`cidr`, `jsonb` (emitted `Utf8View`) | by value | bytewise |
| `interval` (emitted `Interval(MonthDayNano)`) | by value | months, then days, then nanoseconds |
| A column with no plan (emitted `Utf8View`) | `=` bytewise, ordering refused | bytewise |
| `character(n)` | trailing blanks trimmed (I38) | padded text |

So **the library gains an Arrow-semantics comparison mode** beside its
PostgreSQL one — bytewise over the emitted text for the `Utf8View` kinds,
label order for an enum — and the provider asks for it. Collated text is
already in that mode and pushable as it stands. **Statistics bounds are either
gathered in the same mode or reported `Absent`** for a kind whose stored bounds
are ordered otherwise — and a text column's bounds and row order are gathered
bytewise whatever its collation, believed only in Arrow semantics, so collated
text prunes and stops early there too. No `CACHE_FORMAT_VERSION` bump: a cache
written before holds none for it and reads `Absent` until re-parsed
([`../status/history/2026-09-21.md`](../status/history/2026-09-21.md),
"Collated text gets bytewise bounds"). **Every scalar column carries bounds in
each semantics whose order over the file's text is exact**, one set where the
two coincide, so `jsonb`, `character(n)` off `C` and a column with no plan are
bounded in Arrow's order, and a kind whose PostgreSQL bounds are not Arrow's
— an enum, `interval`, a bare `numeric`, `timetz`, `inet`/`cidr`,
`character(n)` under `C` — carries a second set keyed by its Arrow order, a
cache shape that bumps `CACHE_FORMAT_VERSION`
([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
"Which columns carry bounds in Arrow's order").

**Divergence from PostgreSQL is reported, never a reason to decline a
pushdown.** It is a property of the column — `ORDER BY`, `MIN`/`MAX` and every
comparison DataFusion evaluates itself reach it — so it is reported per column
when a table is registered, through the diagnostics sink below. The existing
register's classes (agrees / diverges / refused, [`decisions.md`](decisions.md),
"D55") say how an answer relates to the *server's*; they no longer decide
pushdown.

Superseded within this grilling: pushing down `Exact` where the library agrees
with PostgreSQL, which let an enum `<` answer in declaration order when pushed
and in label order when not. `Inexact` stays refused for the reason it was: it
promises a superset, and a comparison in other semantics can omit a row.

**A nested column's comparison is refused in this mode, `=` included**, so its
terms are never pushed down. DataFusion orders a list, struct or range by
`make_comparator` under default `SortOptions` — nulls first, no zero
normalization, a range by its `lower` field first — and its source marks that
choice `TODO: make SortOptions configurable`: an order upstream means to change
is not one the library can promise to evaluate identically. Nothing is lost to
pruning, a nested column carrying no bounds or dictionary. Settled
2026-09-21 ([`../status/history/2026-09-21.md`](../status/history/2026-09-21.md),
"Arrow semantics is DataFusion's comparison").

## Cancellation

**A query is cancelled by dropping its stream, and that is the provider's only
cancellation.** The provider sets no `ScanOptions::cancel`: with no mapping
pass there is no partial map for a drop to leave behind, a dropped replay
stops, and a remote read in flight is dropped with the future awaiting it. So
`Error::ScanCancelled` cannot arise on a read over a complete cache and needs
no DataFusion translation. `ScanOptions::cancel` stays the CLI's mechanism for
`pgdt parse`.

## Workers and memory

**The worker count is DataFusion's `target_partitions`**, read inside `scan()`
as DataFusion's own sources read it, and the provider declares however many
sub-streams the library returns — which may be fewer, the file's blocks
deciding how a table can be cut. **The byte budget is one object per session**,
held on the session's config or runtime (the precedent is DataFusion's
metadata-cache limit, a bounded object set once on the `RuntimeEnv`) and shared
by every pgdump scan the session runs, so concurrent scans draw from it rather
than each taking the whole. Its default follows [`roadmap.md`](roadmap.md), "A
default runs as fast as the allocation permits": the process's discovered
allowance, as `pgdt` derives it, **less the session's memory pool where that
pool states a finite limit** (`MemoryPool::memory_limit`, `Finite`) — the scans'
read buffers and DataFusion's own operators live in one container, and a
`--memory-limit` already granted to the latter is not the scans' to draw —
**and less the statistics each registered dump's resident map holds**
(`Term::Loaded`, as `pgdt` bills it), taken at registration and returned when
the dump is dropped. Both come off the margin's ceiling, as
[`decisions.md`](decisions.md), "D85" bills statistics: the cap is the reserve's,
standing for the scans' own excess, and neither holding is any of it. **Whatever
of them the ceiling's room cannot absorb at the count a scan resolves comes off
that scan's budget**, whatever the source — so where the count cannot fall, a
plain source or one reader, the predicted resident still stays under the margin,
and a budget too small for a unit declines it (D4). Settled 2026-09-22
([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
"The session budget", "Which bound the holdings come off" and "The holdings at
the floors").

Rejected: a budget per registered table, which has no precedent among
DataFusion's sources and multiplies by the tables a join names; and no budget,
sizing every scan to its worker count, which gives up bounded memory on a
compressed source. DataFusion supplies no reading to divide: its memory pool
explicitly does not cover data sources, and nothing in it reads a cgroup
(`datafusion/execution/src/memory_pool/mod.rs`, v55).

## Library surfaces the provider does not use

The provider pulls partitions under DataFusion's scheduler, so it uses neither
`batch::read_table` (push mode), `TableStream::batch_source_offset` (the CLI's
merge into file order), nor `ResumeToken`. **All three stay as they are**:
tightening the library's public surface is one sweep at feature-completeness,
not piecemeal work inside this phase ([`roadmap.md`](roadmap.md), Future item
"A public-surface sweep at feature-completeness").

**The provider promises no order among partition errors**: the first observed
wins, as for any DataFusion source; `pgdt query` keeps its file-order rule
([`decisions.md`](decisions.md), "D52"). **A partitioned read is not
resumable**, and DataFusion has no resume for the provider to serve.

## Statistics handed to DataFusion

DataFusion answers `COUNT(*)` and `MIN`/`MAX` from `Exact` statistics without
reading a row, so an `Exact` that is wrong is a wrong answer with no error.

- **Row count: `Exact`**, the sum of every block's rows, which a complete map
  holds.
- **Column bounds and null count: `Exact` only where every group of every
  block carries them and they are ordered as the column's Arrow type orders**
  (above, "Comparison means what DataFusion means");
  `Inexact` where some groups lack them; `Absent` otherwise.
- **Output ordering from recorded sortedness** is not declared by this phase;
  it is a Future item.

## The binary: `datafusion-cli-pgdump`

**A new workspace member building `datafusion-cli-pgdump`**, depending on the
`datafusion-cli` library at the targeted release: a copy of its private
`main.rs` plus our registrations, which is upstream's endorsed pattern
(`datafusion-cli/examples/cli-session-context.rs`). **A dump is attached by a
repeatable `--dump <name>=<path>` flag**, registering it as catalog `<name>`,
**and by `CREATE EXTERNAL TABLE … STORED AS PGDUMP`**, whose options live
under a registered `pgdump.` table-options extension. **No backslash commands
of our own**, which would mean owning the REPL loop as well.

Rejected: a `pgdt sql` subcommand, which puts DataFusion into the build every
figure in [`measurements.md`](measurements.md) is taken from; and no binary at
all. The cost accepted is re-copying `main.rs` at each DataFusion major.

## Diagnostics: one sink

**The library gains one diagnostics sink**, a caller-supplied trait object into
which all four channels drain: the file-level `DumpIndex::diagnostics` (L1),
the per-column `ResolvedSchema::notes` (L2), the query-conditional
comparison notes (L4), and what a scan's plan settled
(`TableStream::plan_notes`). Each channel's type reaches it through one shared trait
bearing `diagnostic::Severity`, which is the unification point left open when
the channels were kept as separate types — unifying at the drain rather than at
the storage type, which the layering forbids ([`decisions.md`](decisions.md),
"D68"). **The provider takes a sink; `datafusion-cli-pgdump` supplies one
printing to stderr** — at registration for file- and column-level notes, and
at planning for what a scan's plan settled. What a scan finds while reading —
groups statistics pruned, bytes an early stop left unread — is a plan metric
under `EXPLAIN ANALYZE`, not a finding. Amended 2026-09-22
([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "The
binary's sink reports at planning, and a scan's counts are metrics"). `pgdt`
may adopt the sink later; that is not this phase's work.

DataFusion 55 offers no non-error channel a provider could use instead (below),
and `log` is hidden by `datafusion-cli`'s default level.

## Sources, and no parse inside the binary

**A dump is anything `Origin::resolve` accepts** — a local path, an `.xz`, an
`http(s)://` URL — resolved exactly as `pgdt` resolves it, the cache location
for a URL included. The provider crate carries an `http` feature passing
through to the library's, **off by default**; `datafusion-cli-pgdump` turns it
on, as `pgdt` does. A remote dump reads as one partition, the remote source
declining to advise on partitioning ([`decisions.md`](decisions.md), "D6"),
until the phase that tunes the network changes that.

**`datafusion-cli-pgdump` does not parse.** A missing or partial cache is an
error printing the `pgdt parse --source <path>` that would build it. Doing a
parse properly means `pgdt`'s parallelism and memory discovery, its interrupt
guard, its status output and its cache-path rules, all of which live in
`pgdt`'s `main.rs` rather than the library; a half-configured parse would be a
slow way to build what `pgdt parse` builds properly. Filed as the Future item
"Attach-time parse in `datafusion-cli-pgdump`".

## What the phase promises

**The type floor is stated**, in the binary's manual page: the Arrow schema is
at least as good as the ADBC PostgreSQL driver's, naming the driver release the
sweep was taken against and `money` as the one exception (`KD13`). The claim is
already checked (`scripts/floor_mapping.py` over `fixtures/<major>/adbc/`,
[`decisions.md`](decisions.md), "D38"); publishing it obliges re-taking that
sweep whenever the pin moves.

**No performance claim, and no figure.** Every `query` figure times `pgdt`,
including its text rendering and, under `--dtcache none`, a mapping pass —
neither of which an embedder holding a cache pays. The library's own cost is
kept as profile proportions ([`decisions.md`](decisions.md), "D29"), which
support a ratio or a per-row cost and never a throughput; an instrument that
stops at the batch is later work if a figure is ever wanted, and a claim taken
under it must name the allocator, which is the embedder's.

## Crates

**Two new workspace members, named by DataFusion's convention because they
are DataFusion layers**: `datafusion-pgdump`, the provider library, and
`datafusion-cli-pgdump`, the binary. Two rather than one crate with both
targets, so that a library user does not inherit the binary's
`datafusion-cli` dependency.

## The provider's query surface

- **Projection** maps onto the library's by-name projection
  (`QueryOptions::projection`), so an unprojected column is never decoded; an
  empty projection passes through as the `COUNT(*)` shape.
- **Limit** is honoured by ceasing to pull, not by a new library option;
  DataFusion's streaming exec applies it already.
- **Batch size** is the session's `batch_size`, read inside `execute()` as
  DataFusion's own sources read it, and becomes `QueryOptions::max_rows`.
- **Schema mode** is per dump — a `:strings` suffix on `--dump`, or a
  `pgdump.schema_mode` option — typed by default. It is the escape hatch from
  a wrong type mapping, and until the Future item "Caller-supplied type
  mapping" exists, the only way to pin a schema.

## Verification

All of it a test target in `datafusion-pgdump` over the committed fixtures,
needing nothing machine-local; the koji replica stays ad hoc
(`CLAUDE.local.md`).

1. **Arrow-semantics order**: each Arrow-mode comparison, and each bound
   gathered in that mode, checked against DataFusion's own comparison of the
   emitted arrays. The library's check against the bare `cmp` kernels came
   first, as evidence, and is not the authority. **A nested column's
   announcements are included though its terms are never pushed down**: each
   note's claim — a NULL element first, a nested float's `-0` unequal to `0` —
   is checked through `compare_op_for_nested`, so an upstream release that
   changes either fails a test rather than leaving a false note.
2. **The same rows as the library.** Every fixture table's `SELECT *` through
   the provider equals the library's own stream, value and type.
3. **Pushdown never changes an answer.** Per comparison kind and operator, the
   same query with pushdown on and forced off returns identical rows.
4. **Statistics never change an answer.** `COUNT(*)`, `COUNT(<column>)`,
   `MIN` and `MAX` answered from statistics equal the same queries with
   statistics disabled — **wherever reading the column answers at all**. A
   column holding a value its Arrow type cannot represent (`KD8`) refuses
   when it is read and is counted off the map when it is not, which is the
   trade a pruned replay already makes
   ([`decisions.md`](decisions.md), "D54"). Amended 2026-09-22
   ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
   "A statistic answers where reading the column refuses").

## What DataFusion 55 offers an extension

Facts read from the 55.0.0 source (`datafusion-cli` 55.1.0 diffed identical),
2026-09-21, recorded for the questions still open. Paths are relative to the
upstream tree.

- **`datafusion-cli` is a library as well as a binary.** Its `lib.rs` exports
  `exec::{exec_from_commands, exec_from_files, exec_from_repl}`, the
  `CliSessionContext` trait, `PrintOptions`, `CliHelper` and the catalog
  wrappers. `main.rs` (~465 lines of code) is private: argument parsing, the
  memory pool, the session config, `enable_url_table()`, the built-in table
  functions. Upstream's one endorsed pattern for extending the CLI is
  `datafusion-cli/examples/cli-session-context.rs`: a downstream `main` that
  calls `exec_from_repl` on its own context. So a `datafusion-cli-pgdump` is a
  copy of `main.rs` plus registrations, re-copied at each DataFusion major.
- **Backslash commands are closed.** `Command` is a closed enum and
  `exec_from_repl` parses it inline, so a new meta-command means owning the REPL
  loop (~85 lines of `exec.rs`).
- **Registration routes:** a `TableProviderFactory` keyed by the upper-cased
  `STORED AS` word; a `CatalogProvider` via `register_catalog`; a table function
  via `register_udtf`, whose `call` is **synchronous**; a `UrlTableFactory`
  behind `DynamicFileCatalog` for `SELECT * FROM 'file'`. In `datafusion-cli`,
  `CREATE EXTERNAL TABLE … OPTIONS` keys are validated against registered
  table-options extensions, so a custom format needs one (keys like
  `'pgdump.table'`), and the location is parsed as a `ListingTableUrl`.
- **`SHOW TABLES` and `information_schema.columns` call `table()` on every
  table** — the CLI's catalog wrappers do not forward `table_type` — so a
  catalog's `table()` must be cheap.
- **There is no non-error diagnostic channel a provider can use.**
  `datafusion_common::Diagnostic` rides only on an error, planner warnings are
  internal to SQL planning, and `datafusion-cli` prints neither. What remains is
  `log` (the CLI initialises `env_logger`, default level error), stderr, plan
  metrics under `EXPLAIN ANALYZE`, or a side channel a binary of our own prints.
- **Ctrl-C drops the statement's future** in the REPL only; `-c`/`-f` install no
  handler. A `spawn_blocking` task is not stopped by a drop and must notice its
  send failing.
- **Output types.** `Utf8View`, `Dictionary`, `List` and `Struct` print through
  arrow's pretty printer. No extension type is registered by default, so
  `arrow.uuid`/`arrow.json` columns print as their storage type.
- **`TableProvider::statistics()` is not used by mainline**; statistics reach
  the optimizer through the `ExecutionPlan`, whose `partition_statistics` is
  deprecated in 55 in favour of `statistics_from_inputs`. `ScanArgs` carries no
  ordering.
- **Cadence.** A major every two to three months, each moving `arrow`; `main`
  already carries 56's breaking change to `scan`'s projection argument.
