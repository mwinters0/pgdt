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

## Phase 3 — Pushdown

- **Predicate pushdown**: evaluate predicates *during* the scan/parse, so
  non-matching rows never get fully unescaped/materialized — as opposed to
  Phase 1's post-parse filtering.
- **Column projection pushdown**: only parse/materialize columns the caller
  actually requested.

Both depend on Phase 2's typed DDL parsing being available (at minimum for
knowing column boundaries/positions cheaply), though projection could
plausibly land against string columns first if it proves valuable earlier.

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
