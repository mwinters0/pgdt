# Roadmap

This supersedes `docs/design/historical/initial.md` (frozen) as the live design
set. Phase 1 is fully specified in `docs/design/mvp.md`. Phases 2-5 are
sketched here at a level sufficient to keep Phase 1 from painting us into a
corner; each gets its own full grilling session when it becomes current.

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

## Phase 5 — Extended pg_dump format support

Support for dump variants beyond the MVP's `COPY ... FROM stdin` TEXT-format
assumption: `--inserts`/`--column-inserts` output, and any other gaps
identified by the compatibility matrix in `docs/design/pg-dump-compatibility.md`
as that matrix fills in via the Phase 1 fixture tooling. Custom/directory/tar
archive formats remain permanently out of scope (existing tools already cover
them well) — this phase is about variance *within* plain-format output, not
adding new pg_dump output formats.
