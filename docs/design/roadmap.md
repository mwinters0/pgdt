# Roadmap

This supersedes `docs/design/historical/initial.md` (frozen) as the live design
set. Each phase that has been specified gets its own doc, named
`roadmap-phase<N>-<slug>.md`; the sections below are the index. Phases still
sketched here are at a level sufficient to keep earlier phases from painting us
into a corner — **each gets its own full grilling session when it becomes
current**, and the resulting spec becomes its own numbered doc.

Phases 1 and 2 are complete (`docs/design/roadmap-phase1-mvp.md`,
`docs/design/roadmap-phase1-mvp-notes.md`,
`docs/design/roadmap-phase2-typed-columns.md`,
`docs/design/roadmap-phase2-typed-columns-notes.md`). Phase 3 is current.

`docs/design/layering.md` cuts across every phase below: it assigns each module
to one of four layers and fixes the direction dependencies may point. Several
phases here are cross-layer by nature — Phase 5's pushdown and statistics
especially — and that doc holds the decision rules for them.

## Pre-1.0: no compatibility obligations

Everything in this roadmap happens before 1.0, and **nothing here carries a
backwards-compatibility or API-stability guarantee**. Public types, the CLI
surface, the cache format, and the Arrow schema a given table resolves to are
all free to change in any release until 1.0. We are iterating, not publishing
contracts.

This is a standing decision, not a question to reopen each phase: don't design
around hypothetical downstream breakage, don't add compatibility shims, and
don't caveat a proposal with migration concerns. The cache's `format_version`
envelope exists so that a stale cache is *detected* rather than misread — not
as a promise to keep reading old ones.

The one place this genuinely costs something is the Arrow schema drifting as
Phase 2's type coverage grows (a column that resolves to `Utf8View` today may
resolve to `Int32` after the next release, invalidating a downstream query
plan). The Future item "caller-supplied type mapping" below is the answer for
anyone who needs a schema pinned; until 1.0 it is the only one.

## Project goals

Two things distinguish this project from existing `pg_dump` tooling
(`pgdumplib` and friends), and both shape the phase ordering below:

- **Embeddable as a query data source**, not just a dump reader — Arrow-native
  output, and ultimately a DataFusion `TableProvider` (Phase 6).
- **High performance is a core goal, not a later optimization**, specifically
  for the local-file reader. Dumps are routinely hundreds of gigabytes; the
  difference between a saturated-device scan and a merely-correct one is the
  difference between a usable tool and an overnight job. Concretely: the
  local-file path should stay device-bound, not CPU-bound, on hardware from
  HDD through NVMe, at flat memory. See
  `docs/design/roadmap-phase7-scan-performance.md`.

  This targets the `COPY`-block/bulk-row path specifically. Preamble and other
  non-data DDL scanning (Phase 3, below) is bounded by schema size, not file
  size — a few thousand lines even for a multi-hundred-GB dump — and isn't
  held to the same device-bound target.

## Phase 1 — MVP (complete)

Streaming, string-typed row extraction from a single plain-format dump file,
with a best-effort structural cache. Binary `pgdq`, library crate
`pgdump_query`. Full spec: `docs/design/roadmap-phase1-mvp.md`; implementation
notes: `docs/design/roadmap-phase1-mvp-notes.md`.

## Phase 2 — Typed columns (complete)

Specified in `docs/design/roadmap-phase2-typed-columns.md`; the sketch below is
the origin of that spec and is kept for the reasoning it carries. Where the two
disagree, the phase doc wins.

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

**Where the work lands.** The scan and the CLI display are Phase 2 work. The
cache slot it needs already exists: `DumpIndex::metadata` is reserved and always
`None`, alongside the sparse row index
(`docs/design/roadmap-phase7-scan-performance.md`) and the per-row-group
statistics under Phase 5 below — all three were reserved together in Phase 1
precisely so populating one is not a format break. The reserved `DumpMetadata`
type is a placeholder; its real shape is this phase's design work.

## Phase 3 — Full DDL object inventory (current)

Specified in `docs/design/roadmap-phase3-object-inventory.md`; the sketch
below is the origin of that spec and is kept for the reasoning it carries.
Where the two disagree, the phase doc wins.

`pg_dump` output is full of statements Phase 2's preamble pass never looks
at — `GRANT`/`REVOKE`, `OWNER TO`, `ALTER DEFAULT PRIVILEGES FOR ROLE ...`,
`SECURITY LABEL`, `CREATE FUNCTION`, `CREATE INDEX`, sequence `setval`, and
more — most of it in the post-data section, which sits *after every* `COPY`
block and so is invisible to Phase 2.2's I1-based short-circuit
(`PreambleBuilder` marks a database's preamble complete at its first `COPY`
block and never looks again). This is a **second use case for the CLI**,
distinct from the embeddable-query-source goal the rest of this roadmap is
organized around: a sysadmin or engineer handed a dump file of unknown
origin, wanting to understand what it needs before loading it anywhere.

Roles are the concrete first instance: a regular `pg_dump` never emits
`CREATE ROLE` (roles are cluster-level, out of scope for a single-database
dump), but `OWNER TO`/`GRANT`/`ALTER DEFAULT PRIVILEGES`/`SECURITY LABEL` all
*reference* roles that must already exist on the restore target. Today the
only way to learn which roles a dump needs is to attempt the restore, watch
it fail on a missing role, create that role, and repeat — `pgdq info` could
just list them up front. Referenced tablespaces (`TABLESPACE <name>`) are
the same shape of problem, and the extensions Phase 2 already captures
partly serve it too.

### Scan shape: two tiers, not a size heuristic

Role/tablespace references (and post-data DDL generally) aren't confined to
the pre-data preamble, so this can't reuse I1's stop-at-the-first-`COPY`-
block trick — it needs a scan that covers the whole file, not just the
region ahead of the first block.

- *Cheap tier, unconditional, every gap.* The existing keyword-dispatch
  grammar (`preamble.rs`) costs almost nothing per line — dropping its I1
  short-circuit and running it over every byte range between `CopyBlock`s
  (including the tail, from the last block to EOF, where post-data DDL
  lives) costs no meaningful CPU against a few thousand DDL lines total, and
  directly closes the post-data blind spot with no threshold needed for
  correctness. **Except where large objects are present** — see below, which
  is the one case where a gap is not schema-sized.
- *Deferred, index-driven, heavier tier.* A separate pass, run after any
  scan (full or incremental) that already has an index, seeks directly to
  each gap using `CopyBlock`'s existing byte offsets — no rescanning needed.
  It classifies whatever the cheap tier left unattributed into
  known-but-unhandled (recognized statement type, not modeled), unknown
  (unrecognized), comment-run, and whitespace-run spans. A size cutoff
  belongs here, not as a correctness gate (the cheap tier already guarantees
  nothing is silently dropped) but to decide whether a leftover span is
  worth the heavier classifier's cost, and as a reporting signal ("40KB of
  unrecognized content between these two tables").

### Full file map

**This is Phase 3's exit criterion, not a byproduct of it.** A full scan
yields an ordered set of spans that *tiles* the file: every byte belongs to
exactly one span, spans sum to the file size, none overlap. Every span is
attributed to something — known object, known-but-unhandled, unknown,
comment-run (n lines), whitespace-run, `COPY` block, or large-object data.
Getting there needs byte-offset fields added to `DatabaseMetadata`'s own DDL
records (`Extension`, `TypeDef`, table entries) — today only `CopyBlock`
carries a position at all.

Coverage then becomes a hard invariant rather than a heuristic judgment
call, which also makes "this dump has zero comments" or "this dump has an
unusual amount of unrecognized content" objectively queryable facts — useful
for the sysadmin story and for recognizing non-`pg_dump`-generated input.

**Standing rule, from Phase 3 onward: a later phase may subdivide a span or
attach detail to it, never reduce coverage.** Specificity increases
monotonically; the tiling property does not degrade. Splitting one
large-object span into one span per object is the intended shape of "more
specific"; introducing a span kind that leaves bytes unaccounted for is not.
This outlives Phase 3 the way `layering.md` outlives the phase that
introduced it.

The rule is only real if something checks it. A test asserts tiling over
every fixture, and the cases that matter most are the degenerate ones:
`--data-only` (no DDL), `--inserts` (no `COPY` blocks at all), and the
concatenated multi-database shape. Absent that test, "spans sum to file
size" decays into an aspiration the first time a span kind is added.

### Large objects: ranges, not contents

Large-object data is mapped as a span and **never parsed**. There is nothing
in it a query engine wants — it is opaque bytes belonging to no table — but
leaving it out would put a hole in the map, and a hole is exactly what the
map exists to forbid.

`pg_dump` makes this cheap (I12):

- All LO data is **one contiguous region**, after every `COPY` block and
  before post-data DDL, introduced by a single `BLOBS` archive entry and
  wrapped in `BEGIN;`/`COMMIT;`. One TOC comment opens it (I3), so the cheap
  tier recognizes it at its header and closes it at `COMMIT;`.
- Its contents are `SELECT pg_catalog.lowrite(0, '\x…');` lines at roughly
  2× hex expansion, so the region can be hundreds of gigabytes. **This is
  the one gap that is not schema-sized**, and running the keyword-dispatch
  grammar across it would turn the cheap tier into a full-file parse. The
  fast-path is not an optimization; it is what keeps the tier's cost claim
  true.
- LO *definitions* (ownership, ACL, comments) sort ahead of the pre-data
  boundary, so they are already inside the region Phase 2's preamble pass
  walks. Only the bytes are far away.

**Per-object detail is deferred, deliberately.** One span for the whole
region satisfies the map; subdividing it into one span per large object is
the reference case for the standing rule above, available whenever something
needs it. The only identity the data region carries is the OID in each
`lo_create('%u')` opener — recorded here because it is the thing a later
phase would have to go looking for, and finding it costs a walk of the whole
region, which is precisely what one span avoids paying today.

### How the map relates to `DumpIndex`

Left to Phase 3's own spec. If every byte is a span and `COPY` blocks are
spans, then `DumpIndex::blocks` is a filtered view of the map — but making
that true touches the cache format, `blocks_for`, `table_stream`'s segment
planner, and Phase 7's sparse index, and the pass that has to build it is
better placed to weigh that than this sketch is. The lean is one ordered
`Vec<Span>` as the primary structure with `CopyData { block: usize }`
pointing into `blocks`; it is a lean, not a decision.

What earlier phases owe the map is only that span-level facts be recorded in
a form it can absorb. Phase 2.3.3's per-`CopyBlock` database attribution is
the first of them, and its `Option<String>` name already qualifies.

### Object model stays read-side

The inventory this produces should be a kind/classification plus the raw
statement text and its byte span, not a write-compatible representation —
ordering/dependency fidelity, exact comment/whitespace preservation, and
OID stability are writer concerns the sysadmin use case doesn't need.
Storing the raw text is cheap and doesn't foreclose a future writer, but
round-trip fidelity shouldn't shape this design now.

Reasoning and evidence: `docs/status/history/2026-08-22.md`.

## Phase 4 — Composite value decoding

Arrays, composite types, and ranges — the three type families Phase 2
deliberately leaves as `Utf8View`. They are grouped into one phase because they
are one piece of work: each is a value rendered by COPY TEXT with **its own
nested quoting rules, inside the field escaping `copy.rs` already decodes**
(`{a,b,"c,d"}`, `(a,b,"c,d")`, `[a,b)`). Writing that nested decoder once
unlocks all three; writing it three times is how it goes wrong.

Arrays are the reason this is worth a phase rather than a footnote — koji has
6 `text[]` columns and they are ordinary in real schemas. The synthetic
performance dataset (`scripts/generate_perf_data.py`, introduced in Phase 2)
grows an array-heavy stress section here, so array decoding is measured against
something that surfaces a regression rather than averaging it away. Composites and ranges
follow for free once the decoder exists, and Phase 2's metadata pass already
recovers the field types and subtype needed to give them Arrow `Struct` and
range representations rather than strings.

Placed here, ahead of pushdown, because Phase 5 is designed *against the type
set*: "min/max for collation-independent orderable types" is a different table
when arrays and ranges are still strings, so designing pushdown and statistics
against a partial type set means designing them twice.

## Phase 5 — Pushdown

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
needs a parsed value), pays off in Phase 5 (the pruning consumer), and — like the
metadata block — already has its cache slot reserved (`CopyBlock::column_stats`,
always `None`).

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
rows (`docs/design/roadmap-phase7-scan-performance.md`). Statistics attach to
those checkpoints; block-level statistics are then just the roll-up, free to
compute and still worth storing for the coarse first pass.

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

- The dump-file identity check (size/mtime) that
  `docs/design/roadmap-phase1-mvp.md` files under "Configurable (future)" for
  the structural cache is **mandatory** before any statistic is trusted.
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

## Phase 6 — Embeddable engine story

**Inbox:** [`roadmap-phase6-inbox.md`](roadmap-phase6-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

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

## Phase 7 — Scan performance

**Inbox:** [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

Concentrated optimization of the local-file read path: SIMD-accelerated
structure discovery, zero-copy row extraction into Arrow buffers, bulk UTF-8
validation, and device-aware parallelism (sequential on rotational media,
parallel on NVMe). Full sketch, including the measurements that should gate
each piece and the Phase 1, 2, 4, and 5 decisions it constrains:
`docs/design/roadmap-phase7-scan-performance.md`.

Scheduled here, after the engine story, for two reasons. Phase 5's pushdown
changes which bytes get touched at all, so optimizing the pre-pushdown parser
would partly optimize code that pushdown deletes; and Phase 6's
`object_store` backend settles the I/O layer that any readahead/parallelism
scheme has to live behind. Deliberately *before* Phase 8 — the format work
multiplies the surface area that any later optimization has to be correct
against, so the fast path should exist first and archive containers should be
built to fit it.

## Phase 8 — Format coverage beyond plain COPY TEXT

**Inbox:** [`roadmap-phase8-inbox.md`](roadmap-phase8-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

Everything that widens the set of `pg_dump` outputs we can read. Two
independent tracks; A is listed first because it is cheap, not because it
matters more.

**Track A — variants within plain format.** `--inserts` /
`--column-inserts` output, and any other gaps the compatibility matrix in
`docs/design/pg-dump-compatibility.md` turns up as it fills in via the Phase 1
fixture tooling. Small: a second row-source implementation feeding the same
batch layer.

**Not CSV-format `COPY` blocks.** `docs/design/historical/initial.md` listed
those alongside `--inserts` as a future option, and the compatibility matrix
carried the pairing forward — but `pg_dump` has no CSV mode at all (I13), so
there is no such output to support. The `--format` flag names the archive
container, which is Track B. Reading CSV would mean accepting input `pg_dump`
never emits: a new input source, to be argued on its own merits rather than
inherited from a superseded sketch.

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
listed under "Decisions that keep later phases open" in
`docs/design/roadmap-phase1-mvp.md`.

## Future — wanted, unscheduled

Work we intend to do without committing it to a phase. An item moves out of
this section when it acquires a phase number, not when it acquires a design.

- **Caller-supplied type mapping.** Let a caller override the
  PostgreSQL-type→Arrow-type resolution: per column, per declared type, or
  wholesale. Two uses, and the second is the important one. It lets a caller who
  distrusts our mapping substitute their own; and it lets a caller **pin a
  schema** against the drift that Phase 2's progressive type coverage otherwise
  causes, by mapping everything to a string type and getting the unparsed
  values, CSV-style. That makes it the mitigation named under "Pre-1.0" above.
  `SchemaMode::Strings` is the crude version of this that Phase 2 ships.

- **Exhaustive built-in type coverage, with tests to match.** Phase 2 maps the
  types that carry real data in real schemas and leaves the rest as strings.
  The eventual goal is every PostgreSQL built-in type, plus the types the
  standard extensions (`hstore`, PostGIS, `citext`, …) introduce, each with a
  round-trip test against real `pg_dump` output rather than a hand-written
  literal — the pattern `public.escapes` already establishes. This is careful,
  case-by-case work; the value is in the test coverage, not in the mapping table.

- **Let a live scan emit rows again, by carrying the map in the resume token.**
  Phase 3.2.1.2.1 separated map-building from row emission
  ([`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md),
  "Mapping and streaming are separate passes"), which costs a second read of
  the queried block: once to find its extent, once to emit its rows. The
  interleaved form can be recovered without reintroducing the unmapped hole
  that motivated the split, because `ResumeToken` is opaque and valid only
  within the producing process — so it can carry the live segment's in-flight
  spans and its open `CopyStart`, and a resumed stream can splice them in
  rather than re-deriving them. Deliberately deferred: it is an optimization
  over a query path still being iterated on, and it should be revisited once
  the feature set is settled rather than designed around now.
