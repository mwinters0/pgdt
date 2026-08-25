# Roadmap

Where this project is going. Each phase that has been specified gets its own
doc, `roadmap-phase<N>-<slug>.md`; the sections below are the index. Phases
still sketched here are at a level sufficient to keep current work from
painting us into a corner — **each gets its own full grilling session when it
becomes current**, and the resulting spec becomes its own numbered doc.

**Phases 1–3 are complete and the system they built is described by subject in
[`architecture.md`](architecture.md)**, not by phase. Their specs and notes
were struck at the keystone review (`../process.md`, "The keystone: striking
the centering"); git holds them. Phase numbering continues from 4 — the
sequence starts further in rather than renumbering, so every surviving
reference stays valid.

Two standing-constraint docs cut across everything below.
[`layering.md`](layering.md) assigns each module to one of four layers and
fixes the direction dependencies may point; several phases here are cross-layer
by nature — Phase 5's pushdown and statistics especially — and that doc holds
the decision rules for them. [`postgres-invariants.md`](postgres-invariants.md)
is the evidence layer: every `pg_dump` behaviour a decision treats as
guaranteed, with its proof and its re-verification command.

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
type coverage grows (a column that resolves to `Utf8View` today may
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
  non-data DDL scanning is bounded by schema size, not file
  size — a few thousand lines even for a multi-hundred-GB dump — and isn't
  held to the same device-bound target.

## Standing rules

Decisions that apply to all work below, not to any one phase. They are here
rather than in a phase doc because they outlived the phases that produced them.

### Coverage increases monotonically

**A later change may subdivide a span or attach detail to it, never reduce
coverage.** Specificity increases; the tiling property does not degrade.
Splitting the one large-object span into one span per object is the intended
shape of "more specific"; introducing a span kind that leaves bytes
unaccounted for is not.

The rule is only real because something checks it. `every_fixture_tiles_exactly`
asserts tiling over every fixture, and the cases that matter most are the
degenerate ones: `--data-only` (no DDL), `--schema-only` (no data),
`--inserts` (no `COPY` blocks at all), and the concatenated multi-database
shape. Absent that test, "spans sum to file size" decays into an aspiration the
first time a span kind is added.

### Four decisions that keep later phases additive

Plain-format-only and single-threaded is a deliberate scope, not a limitation
to design around. These four choices are what make Phases 7 and 8 additive
rather than a rewrite, and they are cheap to hold to — so hold to them, even
where the work in front of you would not require them.

- **The COPY TEXT decoder stays independent of where its bytes came from.**
  `copy.rs` operates on a caller-owned slice and never assumes "a file at
  offset N". Every dump format stores table data as this same COPY TEXT
  payload, so this decoder is the one component all of Phase 8 reuses verbatim.
- **Structure discovery is a separate concern from row decoding.** `scan.rs`
  finds `COPY` boundaries in a plain file; an archive reads them from a TOC.
  Keeping "what entries exist and where are their bytes" apart from "decode
  these bytes into rows" is what lets a container layer slot in later.
- **The cache format is versioned and records what produced it.** A serialized
  `DumpIndex` carries a format-version field and a container-kind tag, so
  archive-derived indexes and entry-relative offsets are a later variant rather
  than a breaking change. A cache whose version or kind is not recognised is
  treated as absent, which the "never required for correctness" rule makes
  safe.
- **`ResumeToken` exposes no fields, ever.** Its contents today are a file
  offset plus a row index, but a raw file offset is meaningless inside a
  compressed archive entry. Opaque now means the representation can change
  without an API break.

Phase 7 adds a fifth that already binds: the batch layer builds `Utf8View`
arrays over the scanner's existing chunk buffer instead of copying field bytes
out of it. See
[`roadmap-phase7-scan-performance.md`](roadmap-phase7-scan-performance.md).

### Permanent non-goals

- **Writing or modifying dump files.**
- **A full SQL/DDL parser** — only enough recognition to locate `COPY` blocks,
  classify statements into spans, and extract columns and types from
  `CREATE TABLE`.
- **CSV-format `COPY` blocks.** `pg_dump` has no CSV mode at all (I13), so
  there is no such output to support; `--format` names the archive container,
  which is Phase 8 Track B. Reading CSV would mean accepting input `pg_dump`
  never emits — a new input source, to be argued on its own merits rather than
  inherited from a superseded sketch.

## Phase 4 — Composite value decoding

Arrays, composite types, and ranges — the three type families type resolution
deliberately leaves as `Utf8View`. They are grouped into one phase because they
are one piece of work: each is a value rendered by COPY TEXT with **its own
nested quoting rules, inside the field escaping `copy.rs` already decodes**
(`{a,b,"c,d"}`, `(a,b,"c,d")`, `[a,b)`). Writing that nested decoder once
unlocks all three; writing it three times is how it goes wrong.

Arrays are the reason this is worth a phase rather than a footnote — koji has
6 `text[]` columns and they are ordinary in real schemas. The synthetic
performance dataset (`scripts/generate_perf_data.py`)
grows an array-heavy stress section here, so array decoding is measured against
something that surfaces a regression rather than averaging it away. Composites and ranges
follow for free once the decoder exists, and the metadata pass already
recovers the field types and subtype needed to give them Arrow `Struct` and
range representations rather than strings.

Placed here, ahead of pushdown, because Phase 5 is designed *against the type
set*: "min/max for collation-independent orderable types" is a different table
when arrays and ranges are still strings, so designing pushdown and statistics
against a partial type set means designing them twice.

## Phase 5 — Pushdown

- **Predicate pushdown**: evaluate predicates *during* the scan/parse, so
  non-matching rows never get fully unescaped/materialized — as opposed to
  today's post-parse filtering.
- **Column projection pushdown**: only parse/materialize columns the caller
  actually requested.

Both depend on the typed DDL parsing already built (at minimum for
knowing column boundaries/positions cheaply), though projection could
plausibly land against string columns first if it proves valuable earlier.

### Companion: per-row-group column statistics

Parquet-style statistics, gathered during a scan and persisted in the cache, so
a later query can skip data instead of reading it. Needs typed columns (a
min/max needs a parsed value), pays off in this phase (the pruning consumer),
and already has its cache slot reserved (`CopyBlock::column_stats`, always
`None`).

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

**The correctness asymmetry is the thing to get right.** The standing rule is
that the cache is a best-effort accelerator, never required for correctness: a
stale structural index costs a rescan and nothing else. **Statistics break that
symmetry.** A stale or wrong statistic causes a wrong *answer* — pruning a row
group that does in fact contain matching rows silently drops data, with no error
to notice. So statistics cannot inherit the structural index's relaxed
validation:

- The dump-file identity check (size/mtime) that the structural cache treats as
  advisory — a mismatch is a diagnostic, not a hard failure
  ([`architecture.md`](architecture.md), "The cache") — is **mandatory** before
  any statistic is trusted.
- Statistics must be **discardable independently** of the structural index, so a
  cache written by a version with a stats bug can be downgraded to "structure
  only" rather than thrown away.
- Each entry records the **type it was computed as**. The type mapping will
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
crates), not just architectural taste. Rough shape, informed by the decisions
under "Standing rules" above, made to keep this open:

- Feature-gated `object_store`-backed I/O implementation of the MVP's
  internal byte-range trait, alongside the lightweight local-only default —
  unlocks S3/GCS/Azure and any other `object_store`-supported backend.
- Python bindings (likely `pyo3`), as a new workspace member.
- Apache DataFusion `TableProvider` integration, as a new workspace member —
  the async core and the `Utf8View` column choice were made with this
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
each piece and the decisions it constrains, in this phase and before it:
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
`docs/design/pg-dump-compatibility.md` turns up as it fills in via the fixture
tooling. Small: a second row-source implementation feeding the same
batch layer, and its target is **already located and attributed** —
`InsertRun::table` and `row_count` come out of the map, so this track adds
parsing, not scanning.

**Not CSV-format `COPY` blocks** — see "Permanent non-goals" above. Track A is
`--inserts`, and nothing else that once sat beside it.

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

The decisions that keep all of this additive rather than a rewrite are listed
under "Four decisions that keep later phases additive" above.

## Out-of-band work

Small work that belongs to no phase: a CLI ergonomics change, a defect fix
that changes no decision. It gets a number `M<k>` and **one terse ledger line
below** — date, what changed, and the history entry that says why. Nothing
else: no spec (there was no intent doc to write), and no notes doc, because
the history entry *is* the notes. If out-of-band work turns up a fact an
unspecified phase needs, that fact goes in that phase's inbox, as always.

**Admission rule.** An item is out-of-band only if it changes no decision any
spec records **and** fits one session. Anything that changes a decision goes
back through grilling → spec amendment → a numbered slice; that rule is what
keeps this section from becoming where design work goes to avoid review.

**Ledger lines stay one line each.** This section grows for the life of the
project and is read as an index, never as an account — the detail lives in the
dated history entry it points at.

| # | Date | Change | Why |
|---|---|---|---|
| M3 | 2026-08-25 | Synthetic `INSERT`-run throughput measurement (`scripts/generate_insert_run_bench.py`) | [`../status/history/2026-08-25.md`](../status/history/2026-08-25.md) |

**M3's result is not itself out-of-band work.** The measurement fit one
session and changed no decision, which is what admitted it here; the number it
produced — an `INSERT`-run scan costs ~5× a `COPY` scan per byte, CPU-bound —
argues for a scanner-level `INSERT` path, which *does* change a decision. That
goes through grilling → spec amendment → a numbered slice, and is filed in
`roadmap-phase7-inbox.md` until then.

**M1** (`pgdq info` rejects a cache that doesn't cover the whole file) and
**M2** (`pgdq info` prints `DumpIndex::diagnostics`) were queued here but
folded into Phase 3's slice **3.7** ("Cache-only inspection") instead of
landing as standalone out-of-band items — cache-only mode needs both
mechanisms directly, so per the admission rule above they were no longer
independent one-session changes.

## Future — wanted, unscheduled

Work we intend to do without committing it to a phase. An item moves out of
this section when it acquires a phase number, not when it acquires a design.

- **Caller-supplied type mapping.** Let a caller override the
  PostgreSQL-type→Arrow-type resolution: per column, per declared type, or
  wholesale. Two uses, and the second is the important one. It lets a caller who
  distrusts our mapping substitute their own; and it lets a caller **pin a
  schema** against the drift that progressive type coverage otherwise causes, by mapping everything to a string type and getting the unparsed
  values, CSV-style. That makes it the mitigation named under "Pre-1.0" above.
  `SchemaMode::Strings` is the crude version of this that ships today.

- **Exhaustive built-in type coverage, with tests to match.** The mapping
  covers the types that carry real data in real schemas and leaves the rest as
  strings.
  The eventual goal is every PostgreSQL built-in type, plus the types the
  standard extensions (`hstore`, PostGIS, `citext`, …) introduce, each with a
  round-trip test against real `pg_dump` output rather than a hand-written
  literal — the pattern `public.escapes` already establishes. This is careful,
  case-by-case work; the value is in the test coverage, not in the mapping table.

- **Let a live scan emit rows again, by carrying the map in the resume token.**
  Map-building is separate from row emission
  ([`architecture.md`](architecture.md), "Query: mapping and streaming are
  separate passes"), which costs a second read of the queried block: once to find its extent, once to emit its rows. The
  interleaved form can be recovered without reintroducing the unmapped hole
  that motivated the split, because `ResumeToken` is opaque and valid only
  within the producing process — so it can carry the live segment's in-flight
  spans and its open `CopyStart`, and a resumed stream can splice them in
  rather than re-deriving them. Deliberately deferred: it is an optimization
  over a query path still being iterated on, and it should be revisited once
  the feature set is settled rather than designed around now.
