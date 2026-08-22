# Roadmap

This supersedes `docs/design/historical/initial.md` (frozen) as the live design
set. Phase 1 is fully specified in `docs/design/mvp.md`. Phases 2-6 are
sketched here at a level sufficient to keep Phase 1 from painting us into a
corner; each gets its own full grilling session when it becomes current.

## Project goals

Two things distinguish this project from existing `pg_dump` tooling
(`pgdumplib` and friends), and both shape the phase ordering below:

- **Embeddable as a query data source**, not just a dump reader — Arrow-native
  output, and ultimately a DataFusion `TableProvider` (Phase 4).
- **High performance is a core goal, not a later optimization**, specifically
  for the local-file reader. Dumps are routinely hundreds of gigabytes; the
  difference between a saturated-device scan and a merely-correct one is the
  difference between a usable tool and an overnight job. Concretely: the
  local-file path should stay device-bound, not CPU-bound, on hardware from
  HDD through NVMe, at flat memory. See
  `docs/design/scan-performance.md`.

## Phase 1 — MVP

Streaming, string-typed row extraction from a single plain-format dump file,
with a best-effort structural cache. Binary `pgdq`, library crate
`pgdump_query`. Full spec: `docs/design/mvp.md`.

## Phase 2 — Typed columns

Parse the `CREATE TABLE` DDL preceding a table's `COPY` block to recover
column types, and map known PostgreSQL types to Arrow types, so results come
back as properly typed `Utf8View`/numeric/temporal/etc. arrays instead of
all-`Utf8View` columns. The MVP row/batch model should convert without a
breaking rewrite: this is additive (typed batches alongside, or instead of,
string batches), not a replacement of the streaming/cache architecture.

### Companion: dump-level metadata

A column's declared type is not always self-explanatory. `public.geometry` means
nothing to a type→Arrow mapping unless you know PostGIS put it there;
`public.mood` means nothing unless you saw the `CREATE TYPE ... AS ENUM` that
defined it. The source server's major version matters for the same reason, since
the built-in type set and some type behaviours move between majors. So Phase 2's
mapping needs the dump's preamble — versions, extensions, **and user-defined
types** — and that information is worth surfacing in its own right: `pgdq info`
should print it.

**Versions and extensions.**

| Field | Source | Availability |
|---|---|---|
| Source server version | `-- Dumped from database version <v>` | Always |
| `pg_dump` version | `-- Dumped by pg_dump version <v>` | Always |
| Extension name + schema | `CREATE EXTENSION IF NOT EXISTS <name> WITH SCHEMA <schema>;` | Whenever the dump has extensions |
| Extension **version** | 4th argument to `binary_upgrade_create_empty_extension(...)` | **`--binary-upgrade` dumps only** |

The last row is a constraint to design around rather than a gap to close: a
regular `pg_dump` **deliberately omits the extension version**, so that the
restore target installs its own default. Model it as an optional field, expect
it to be absent in the common case, and have the CLI omit it rather than guess.
Both version header lines come from the archiver and are not suppressed by
`--no-comments`, which only affects `COMMENT ON` statements. Evidence and
upstream references: `docs/status/history/2026-08-22.md`.

**User-defined types.** Capture `CREATE TYPE` / `CREATE DOMAIN` in the same pass.
The premise that these can only feed a warning holds for two of the six forms
`pg_dump` emits; the rest carry enough structure to map properly, and they are
the ones real schemas actually use.

| Emitted form | Arrow story |
|---|---|
| `CREATE TYPE x AS ENUM ('a', 'b', …);` | **Directly mappable.** COPY TEXT emits the label as plain text, and the full label set is right there — `Dictionary(Int32, Utf8)`, or `Utf8View` with the domain known. |
| `CREATE DOMAIN x AS <basetype> [COLLATE …] [NOT NULL] [CHECK …];` | **Directly mappable.** A domain is its base type plus constraints; resolve to the base type (transitively — a domain over a domain is legal) and reuse the existing mapping. `NOT NULL` is a free nullability refinement. |
| `CREATE TYPE x AS (f1 t1, f2 t2, …);` | Field names and types known, so an Arrow `Struct` is reachable later; blocked meanwhile on decoding COPY TEXT's record literal (`(a,b,"c,d")`, with its own nested quoting rules). Warn + string for now. |
| `CREATE TYPE x AS RANGE (subtype = …, …);` | Subtype known; value arrives as `[a,b)`. Same position as composite — reachable, not free. Warn + string for now. |
| `CREATE TYPE x (INPUT = …, OUTPUT = …, …);` | **Opaque.** A C-level base type; the dump says how the server parses it, which tells us nothing. This is the warn case. |
| `CREATE TYPE x;` | **Opaque.** Shell type (breaking a circular dependency) or an undefined type. No information at all. |

So the warning should distinguish *what kind* of unknown a column is — "opaque
base type, emitted as text" is a different message from "composite type, not yet
decoded, emitted as text," and both are different from "type not declared
anywhere in this dump."

**The two sources are complementary, and neither alone is sufficient.** An
extension's types are *not* emitted as `CREATE TYPE`:
`checkExtensionMembership()` clears `DUMP_COMPONENT_DEFINITION` on every
extension member in a regular dump, so PostGIS `geometry` appears only as a
column type, explained solely by the `CREATE EXTENSION` line. Conversely a
user-defined type has a `CREATE TYPE` and no extension. And koji has a third
combination — `public.pgstattuple_type` is emitted as a bare `CREATE TYPE` with
no `CREATE EXTENSION` anywhere in the file, because the type is not owned by an
extension there. Classifying an unknown column type correctly, and wording its
warning usefully, needs both lists plus the knowledge that either can be empty.

**This is one preamble pass, not three.** `CREATE TABLE` (Phase 2's own input),
`CREATE TYPE`, `CREATE EXTENSION` and the version headers are all pre-data, all
ahead of the first `COPY` block, and all wanted at the same time.

**New API surface: a diagnostics channel.** Phase 1 has only `Error` — a
warning has nowhere to go today, and `eprintln!` is wrong for a library destined
to sit inside DataFusion. The warnings belong as structured per-column
resolution outcomes hanging off the resolved schema, which the CLI renders and an
embedder can ignore or surface as it likes. Scope this with Phase 2 rather than
bolting it on.

**Terminating the search early is an invariant, not a heuristic.** `pg_dump`
classifies `DO_EXTENSION`, `DO_TYPE` and `DO_SHELL_TYPE` as pre-data objects in
`addBoundaryDependencies()`, which makes the pre-data boundary depend on every
one of them and every `DO_TABLE_DATA` depend on that boundary. The topological
sort therefore *cannot* place a `CREATE EXTENSION` or `CREATE TYPE` after a
`COPY` block; the `PRIO_EXTENSION`/`PRIO_TYPE` < `PRIO_PRE_DATA_BOUNDARY`
ordering in `pg_dump_sort.c` says the same thing more weakly. Identical in v13
through v18. So once the scanner reaches the first `COPY` header it can stop
looking for metadata — no fallback scan needed.

Three qualifications on that, all cheap to honour:

- **Re-arm the search at each `\connect`.** The invariant is per-database. A
  `pg_dumpall` stream concatenates one `pg_dump --create` output per database, so
  database N's preamble legitimately follows database N-1's `COPY` blocks.
  `pg_dumpall` is currently undecided scope
  (`docs/design/pg-dump-compatibility.md`), but the koji sample is a `--create`
  dump and has the same `\connect` shape, so the re-arm costs one state
  transition and removes the sharpest failure mode.
- **An empty list is not the same as an empty database.** `--data-only` and
  `--section=data` omit the preamble entirely, and `--extension`/
  `--exclude-extension` can truncate the extension list by request. The metadata
  must therefore distinguish "nothing declared" from "preamble never seen" — the
  first justifies a confident warning, the second does not.
- **`--binary-upgrade` moves the enum labels.** In that mode the labels are not
  in the `CREATE TYPE ... AS ENUM ()` body; it emits an empty body and adds each
  label afterwards through oid-preserving calls. A label-set extractor that only
  reads the body would silently yield an empty enum there.

**Where the work lands.** The scan and the CLI display are Phase 2 work, but one
piece is due in Phase 1: `DumpIndex` must carry the metadata field *before* the
cache is first serialized, or adding it later is a format break. That is the same
reasoning as the sparse row index in `docs/design/scan-performance.md` and the
per-row-group statistics under Phase 3 below; the cache work has not started yet,
so reserve all three slots together.

## Phase 3 — Pushdown

- **Predicate pushdown**: evaluate predicates *during* the scan/parse, so
  non-matching rows never get fully unescaped/materialized — as opposed to
  Phase 1's post-parse filtering.
- **Column projection pushdown**: only parse/materialize columns the caller
  actually requested.

Both depend on Phase 2's typed DDL parsing being available (at minimum for
knowing column boundaries/positions cheaply), though projection could
plausibly land against string columns first if it proves valuable earlier.

### Companion: per-row-group column statistics

Parquet-style statistics, gathered during a scan and persisted in the cache, so
a later query can skip data instead of reading it. Depends on Phase 2 (a min/max
needs a parsed value), pays off in Phase 3 (the pruning consumer), and — like the
metadata block — needs its cache slot reserved in Phase 1.

Starting set, cheapest and most useful first:

- **`null_count`** — nearly free, and directly answers `IS NULL` / `IS NOT NULL`.
- **Sortedness** — a tri-state (`ascending` / `descending` / `unordered`) plus
  NULL placement. One comparison per value, two bits stored.
- **`min_value` / `max_value`** — for collation-independent orderable types only;
  see the trap below.
- **Distinct count** — deliberately *not* in the starting set. It needs a hash set
  or an HLL sketch, which is a different cost class from everything above.

**Attach these to row groups, not to whole `COPY` blocks.** Per-block is the
granularity that suggests itself, but it is close to useless on exactly the
tables big enough to matter: koji's blocks run to billions of rows, and the
min/max of a monotonic `id` column over a whole block spans the entire domain, so
it prunes nothing. Parquet's win comes from row-group granularity, and there is
already a natural unit to reuse — the sparse row index checkpoints every 8192
rows (`docs/design/scan-performance.md`). Statistics attach to those checkpoints;
block-level statistics are then just the roll-up, free to compute and still worth
storing for the coarse first pass.

**Sortedness is worth more here than min/max, and costs less.** `pg_dump` emits
rows in physical heap order, and for an append-only table that is very often
ascending by surrogate key — much of koji (build, task and RPM id columns) should
qualify. A column confirmed ascending, combined with the sparse index, turns a
range predicate into a **binary search for a byte range** rather than a
scan-and-prune over row groups. That is a qualitatively better outcome than page
pruning, and it is the reason to put sortedness ahead of min/max rather than
treating it as a nice extra.

Two conditions on it. Sortedness is only a guarantee over a **fully scanned**
block — an incremental scan that stopped partway can honestly say "ascending so
far," which is not something a query may rely on, so the flag has to be tied to
the block's scan watermark rather than set optimistically. And NULL placement
must be recorded, not assumed.

**The correctness asymmetry is the thing to get right.** Phase 1's rule is that
the cache is a best-effort accelerator, never required for correctness: a stale
structural index costs a rescan and nothing else. **Statistics break that
symmetry.** A stale or wrong statistic causes a wrong *answer* — pruning a row
group that does in fact contain matching rows silently drops data, with no error
to notice. So statistics cannot inherit the structural index's relaxed
validation:

- The dump-file identity check (size/mtime) that `docs/design/mvp.md` files under
  "Configurable (future)" for the structural cache is **mandatory** before any
  statistic is trusted.
- Statistics must be **discardable independently** of the structural index, so a
  cache written by a version with a stats bug can be downgraded to "structure
  only" rather than thrown away.
- Each entry records the **type it was computed as**. Phase 2's type mapping will
  keep changing; a min/max computed under an older mapping must not be silently
  reused under a newer one.

**Trap: string min/max is a correctness bug, not an optimization.** PostgreSQL
orders `text` by collation — koji's own header records `LOCALE = 'en_US.UTF-8'` —
while Arrow and DataFusion compare byte-wise. A byte-wise min/max used to prune a
collation-ordered predicate can exclude rows that actually match. So restrict
min/max to types whose ordering is collation-independent: integers, `numeric`,
dates/timestamps, `boolean`, `uuid`. This is what makes the "basic orderable types
such as int" instinct the right starting point rather than merely the easy one.
Floats need an explicit NaN and `-0.0` policy before they join the list — this is
the same footgun that forced Parquet to rework its own float column ordering.
Text min/max stays open only if the collation is recorded and matched.

**Cost: this converts the index pass into a full parse.** `build_index()` today
finds block boundaries and counts rows without ever splitting a field. Statistics
require splitting every field of every row and parsing the tracked ones. koji
measured 243 MB/s at ~33% of one core, so an HDD scan has headroom to absorb it,
but the same scan is CPU-bound on NVMe and there the tax is real. Statistics
gathering should therefore be **opt-in and column-selectable**, not something
`pgdq parse` does by default.

**Cost: the sizing is not negligible.** At 8192-row groups, koji's 19.58B rows
give ~2.4M row groups; at roughly 24 bytes per column per group (min, max,
null_count, flags) and ~10 tracked columns, that is on the order of **half a
gigabyte** of statistics. That is ~0.07% of the 784 GB file — a defensible ratio,
comparable to Parquet's own footer overhead — but it is ~30x the sparse index it
rides on. So the statistics row-group interval should be tunable *independently*
of the sparse index interval; coarsening it to every 64k rows cuts the volume 8x
while still pruning far better than per-block would.

Finally, in incremental mode statistics accumulate as a side effect of scans the
caller asked for anyway, so coverage is naturally partial. The cache must record
which row groups actually have statistics — absent is a normal state, not a
defect.

## Phase 4 — Embeddable engine story

The least-specified phase — the user has explicitly flagged unfamiliarity
with this space, so treat its eventual grilling session as needing real
research (prior art from `object_store`/DataFusion/similar embedded-source
crates), not just architectural taste. Rough shape, informed by Phase 1
decisions already made to keep this open:

- Feature-gated `object_store`-backed I/O implementation of the MVP's
  internal byte-range trait, alongside the lightweight local-only default —
  unlocks S3/GCS/Azure and any other `object_store`-supported backend.
- Python bindings (likely `pyo3`), as a new workspace member.
- Apache DataFusion `TableProvider` integration, as a new workspace member —
  the async core and `Utf8View` column choice in Phase 1 were made with this
  destination specifically in mind.
- Apache Spark / Trino integration — order and approach TBD; likely follows
  whatever pattern the DataFusion integration establishes, if applicable.

## Phase 5 — Scan performance

Concentrated optimization of the local-file read path: SIMD-accelerated
structure discovery, zero-copy row extraction into Arrow buffers, bulk UTF-8
validation, and device-aware parallelism (sequential on rotational media,
parallel on NVMe). Full sketch, including the measurements that should gate
each piece and the Phase 1-3 decisions it constrains:
`docs/design/scan-performance.md`.

Scheduled here, after the engine story, for two reasons. Phase 3's pushdown
changes which bytes get touched at all, so optimizing the pre-pushdown parser
would partly optimize code that pushdown deletes; and Phase 4's
`object_store` backend settles the I/O layer that any readahead/parallelism
scheme has to live behind. Deliberately *before* Phase 6 — the format work
multiplies the surface area that any later optimization has to be correct
against, so the fast path should exist first and archive containers should be
built to fit it.

## Phase 6 — Format coverage beyond plain COPY TEXT

Everything that widens the set of `pg_dump` outputs we can read. Two
independent tracks; A is listed first because it is cheap, not because it
matters more.

**Track A — variants within plain format.** `--inserts` /
`--column-inserts` output, and any other gaps the compatibility matrix in
`docs/design/pg-dump-compatibility.md` turns up as it fills in via the Phase 1
fixture tooling. Small: a second row-source implementation feeding the same
batch layer.

**Track B — archive container formats.** `--format=custom`,
`--format=directory`, and `--format=tar`. Formerly a permanent non-goal;
now planned, because the project's ambition (an embeddable, Arrow-native,
engine-integrated source) puts it in a different class from tools that only
need to read a dump once. Reading a table out of a custom-format archive into
a DataFusion query is squarely in scope for what this library is for.

The work is a **container layer**, not a new parser. All four formats store
table data as the same COPY TEXT payload the MVP already decodes; what
differs is how you find an entry and how you get its bytes:

| Format | How structure is found | Entry bytes |
|---|---|---|
| plain | scan for `COPY ... FROM stdin;` headers | raw, inline |
| custom | parse the `PGDMP` header + TOC | per-entry compressed block |
| directory | read `toc.dat`, one file per entry | per-file compressed |
| tar | read the tar member index + `toc.dat` | uncompressed tar members |

So the layer sits between `ByteRangeSource` (below) and the batch/stream API
(above), and reuses `copy.rs` verbatim. Two things
in it are genuinely new and should be scoped as such when this phase becomes
current:

- **Per-entry streaming decompression** (gzip, and lz4/zstd for PG 16+
  archives). This is why the MVP's "input is already decompressed" rule is
  scoped to plain format rather than stated globally — archives compress
  *internally*, so that rule cannot hold here.
- **Non-seekable positions within an entry.** A raw file offset is not a
  resumable position inside a compressed stream, so `ResumeToken` stays
  opaque and the cache gains an entry-relative addressing mode.

The Phase 1 decisions that keep all of this additive rather than a rewrite are
listed under "Decisions that keep later phases open" in `docs/design/mvp.md`.
