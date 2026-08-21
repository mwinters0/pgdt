# pg_dump Plain-Format Parser — Design Handoff

> **Status: Historical.** This was the original handoff doc and is frozen as-is.
> It informed the MVP design/roadmap docs in `docs/design/` — see those for the
> current plan.

## Problem

`pg_dump` in **plain format** (`--format=plain`, the default) produces a single SQL
text file containing schema DDL interleaved with `COPY ... FROM stdin;` data
blocks. There is no existing library that lets a client efficiently query
individual tables out of these files without loading the whole dump into
memory — especially for dumps in the tens-to-hundreds-of-GB range.

Existing tools (`pgdumplib`, etc.) only support the **custom**/**directory**
formats, which have a table of contents and are seekable. Plain format has
none of that: it's just SQL, read top to bottom.

This library exists to fill that gap: a Rust library that treats a plain-format
dump as a queryable source, producing Arrow data, usable standalone or later
embedded in query engines.

Target compatibility: PostgreSQL `pg_dump` plain-format output from **version
13 onward**. The `COPY ... FROM stdin` TEXT-format wire format has been stable
across this range, so this is a reasonable floor rather than a hard technical
ceiling.

## Design Philosophy

This library is meant to be **versatile, not opinionated** — the right
strategy for a given workload (one-shot single-table scan vs. repeated
queries across many tables vs. embedding in a query engine) varies, and we
shouldn't hardcode a single strategy where a workload-dependent or
hardware-dependent option is reasonable to expose. At the same time, we don't
need to *implement* every option in the first pass — it's fine for an option to
exist as a documented future extension point rather than working code on day
one.

Every design decision in this doc should be explicit about which of these three
buckets it falls into:
- **Hardcoded behavior** — not configurable, at least for now
- **Configurable option (MVP)** — implemented and exposed now
- **Configurable option (future)** — the design should not preclude adding
  this later, but it's not built in the first pass

## MVP Use Cases

1. **Single-table, single-pass extraction.** Client wants all rows (or rows
   matching a simple predicate on one column) from one table in a large dump,
   without loading the full file into memory. This is the core use case that
   motivated the project.

2. **Repeated queries against the same dump file.** A client may query table
   X today, and table Y (or table X again, with a different predicate)
   tomorrow, against the same dump file on disk. The second and later queries
   should be able to go faster than the first by reusing whatever the first
   query learned about the file's structure.

3. **Querying a table that appears earlier in the file than one already
   queried.** If a client's first query touches table X, and a later query
   asks for table W which appears *before* X in the dump, that second query
   should still benefit from whatever partial structural knowledge was
   gathered while scanning up to and through X — not fall back to a fully
   naive scan.

4. **Streaming consumption of results.** Clients should not be required to
   materialize an entire table's result set in memory. Results should be
   handed back incrementally as they become available.

5. **Basic typed access is not required for MVP** — returning each row's
   columns as strings (comparable to a CSV row with a header) is an
   acceptable and useful first-class use case on its own, independent of
   whether typed parsing (see below) is ever layered on top.

## Aspirational / Future Use Cases

These inform the shape of the MVP design (so we don't paint ourselves into a
corner) but are explicitly **not** required in the first implementation.

6. **Typed columns via `CREATE TABLE` parsing.** Parse the DDL preceding a
   table's `COPY` block to recover column types, and map known PostgreSQL
   types to appropriate Arrow types, so results come back as properly typed
   Arrow arrays instead of all-string columns.

7. **Predicate pushdown.** Beyond "give me all rows of table X," support
   filtering (e.g. `timestamp_col > Y`) *during* the scan/parse itself, so
   uninteresting rows never get fully parsed or materialized — not just
   filtered after the fact.

8. **Column projection pushdown.** Only parse/materialize the columns the
   client actually asked for, skipping the work of splitting or unescaping
   columns that weren't requested.

9. **Use from other languages/engines.** The library is Rust-native, but the
   intent is that it eventually becomes usable from Python, and embeddable in
   engines like Apache DataFusion, Apache Spark, and Trino — likely via Arrow
   as the common interchange format and/or a table-provider-style interface.
   This pulls the project toward being something like a lightweight, embedded
   query source ("poor man's database") over a pg_dump file, rather than just
   a row extractor. This is a substantial goal in its own right and won't be
   fully resolved in the MVP — but MVP interfaces (especially the streaming
   API and the index cache) should be designed with this destination in mind,
   not actively closed off from it.

10. **Full first-pass indexing as an explicit, opt-in mode.** If a client
    knows upfront they'll be issuing many queries against a dump, they should
    be able to request a full structural pass over the file (locating every
    table's `COPY` block boundaries, and optionally more) before issuing any
    query, trading upfront time for consistently fast subsequent queries —
    as opposed to the default of building this knowledge up incrementally,
    query by query.

## Key Design Decisions

### Streaming API shape

The primary API is **streaming**, not "load a table and hand back one big
result." The library produces Arrow `RecordBatch`es (or equivalent) up to a
configurable size threshold (row count and/or in-memory byte size — both
should be supported as limits, whichever is hit first), and either:

- returns each batch to the caller as it fills (pull-based iteration), and/or
- invokes a callback per batch (push-based),

with both modes supported since different client contexts (embedding in a
sync CLI tool vs. an async engine integration) will prefer one or the other.
**Configurable option (MVP):** batch size limits (rows, bytes) and choice of
pull vs. callback delivery.

Whichever mode is used, the library must track and expose enough position
information (file offset, row offset within the table, etc.) that a client
which stops consuming partway through can resume later without re-scanning
from the start of the table. **Hardcoded (MVP):** the mechanism for encoding
this resume position exists; **configurable (future):** letting a client
persist and hand back a resume token across process restarts, independent of
the index cache described below.

### Index/structure cache

Because clients may issue multiple queries against the same dump file over
time (use cases 2 and 3 above), the library should be able to persist
whatever structural knowledge it has accumulated about a dump file to a
**side file / cache**, so future queries against the same dump can skip
re-discovering that structure.

This is described here conceptually — the specific on-disk encoding is left
open for implementation, not specified in this doc.

Conceptually, this cache holds things like:
- The byte offsets of each discovered `COPY <table> (...) FROM stdin;` header
  and its corresponding terminating `\.` line
- Which portion of the file has been scanned so far (so the library knows
  whether a requested table is "known to not exist before offset N" vs.
  "simply not yet discovered")
- Optionally, in the future, finer-grained row-level offsets within a table's
  data block, if a workload justifies that level of granularity

Behavior around this cache:

- **Configurable option (MVP): whether to build/consult a cache at all**, and
  where it lives on disk (e.g. colocated with the source dump, or a
  client-specified path). Some clients may want pure streaming with no side
  effects; others want acceleration across repeated runs.
- **Configurable option (MVP): eager full-file indexing vs. incremental,
  query-driven indexing.** Eager mode (use case 10) scans the whole file up
  front to populate the cache before answering any query. Incremental mode
  (the default) discovers structure only as far as needed to answer the
  current query, but *still persists whatever it discovered along the way* —
  so a query for table X that happens to scan past tables A, B, and C on the
  way there should leave the cache useful for future queries against A, B,
  or C too (use case 3), even though neither of those tables was the actual
  target.
- **Hardcoded (MVP):** the cache is a best-effort accelerator, never a
  requirement for correctness — if the cache is missing, stale, or doesn't
  cover the requested table, the library falls back to scanning the file
  (from the furthest point the cache does cover, or from the start if
  nothing usable is cached).
- **Configurable option (future):** cache invalidation strategy if the
  underlying dump file changes (e.g. size/mtime check vs. trusting the
  client to manage this).

### Row representation (MVP)

Each row is returned as a set of named columns, all represented as strings
(nullable, to represent SQL `NULL` distinctly from an empty string) — i.e.
comparable to a row from a CSV with headers. This is a deliberately minimal
but complete and useful MVP output shape, independent of whether typed
parsing (use case 6) ever gets layered in. Typed Arrow columns based on
parsed `CREATE TABLE` DDL are explicitly **out of MVP scope** and left as a
future phase; the MVP row/column model should not assume string-only output
is permanent, but it also shouldn't be blocked on typed support to ship.

### Format/version scope

- **Hardcoded:** input is plain-format `pg_dump` output (not custom or
  directory format — those are different problems with different, better
  existing tooling).
- **Hardcoded:** target Postgres versions >= 13, on the basis that the
  `COPY ... FROM stdin` TEXT wire format the library depends on has been
  stable across this range.
- **Configurable option (future):** support for dumps produced with
  `--column-inserts` or `--inserts` (i.e. `INSERT INTO ...` statements
  instead of `COPY` blocks) or CSV-format `COPY` blocks. MVP assumes the
  default `COPY ... FROM stdin` TEXT format, which is what `pg_dump` produces
  without those flags.

## Non-Goals (for now)

- Writing or modifying dump files.
- Supporting custom/directory pg_dump formats (existing tools already cover
  this reasonably well).
- A full SQL/DDL parser — only enough DDL recognition to locate `COPY` blocks
  and (in a later phase) extract column names/types from a `CREATE TABLE`
  statement for the table(s) being queried.

## CLI
For testing and general utility purposes, our project will produce both a
library crate and a binary crate which leverages the library for common
operations.  An MVP surface here would be:
- A `parse` command to perform the full file scan and build the cache file.
- An `info` command to print what we know about the dump file based on our
  cache.  Essentially a pretty-print / debug for inspecting the cache.  With a
  complete cache, this should show things like table names and columns, total
  row counts, etc.  A debug or verbose mode may show details such as file
  offsets for each copy block, etc.
