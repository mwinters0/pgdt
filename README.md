# pgdump_query / pgdt

Inspect Postgres dumps, query them like they're parquet, and export results.

Available as:
- A Rust library: `pgdump_query`
- A CLI: `pgdt`, aka "Postgres Dump Tool"
    - Generates metadata and statistics (cached as `*.dtcache`)
    - Supports queries with SQL-like `WHERE` syntax (no joins)
- A DataFusion catalog and `TableProvider`: `datafusion-pgdump`
- A DataFusion SQL shell: `datafusion-cli-pgdump`

Quickstart:

```bash
# Parse a dump (builds a cache)
pgdt parse --source=f00.xz

# Inspect what you parsed, e.g. tables, roles, etc
pgdt info --details --source=f00.xz

# Run a query.  Look ma, no daemons!
pgdt query --where='foo.bar = baz' --source=f00.xz

# Use DuckDB to convert the TSV output to parquet
pgdt query ... \
  | duckdb -c "COPY (SELECT * FROM read_csv('/dev/stdin', delim='\t', header=true, auto_detect=true)) TO 'output.parquet' (FORMAT PARQUET)"
```

_Mostly written by LLMs, reviewed through human wetware._


## Roadmap / Status
**Early development**, pre-1.0, with no compatibility guarantees yet. Two things set the direction:
- It is meant to be **embeddable as a query data source** (ultimately a DataFusion `TableProvider`).
- **High performance on local files is a core goal** rather than a later optimization — dumps are
routinely hundreds of gigabytes, so the local-file reader aims to stay device-bound rather than
CPU-bound, at memory that does not grow with the size of the dump.

### Status
Anything unchecked here is considered "TODO" / Future.
- Input
    - `pg_dump` formats
        - [X] plain
        - [ ] directory
        - [ ] tar
        - [ ] custom
    - Compression
        - [x] xz (seekable)
        - [ ] gzip (non-seekable) / bgzip (seekable)
        - [ ] zstd (non-seekable and seekable)
        - [ ] lz4
    - File locations
        - [x] local
        - [x] http / https — ranged requests, nothing downloaded whole; a
        seekable compressed dump is read the same way, a block at a time
        - [ ] object store
- Postgres Correctness
    - Data types
        - [x] See [the
        docs](https://github.com/mwinters0/pgdt/blob/main/docs/manual/type-handling.md), but
        generally "all common base types".
            - [ ] Notable exceptions: numeric `infinity`, `-infinity`, `NaN`.  (See: KD8)
        - [ ] Common extension types, e.g. PostGIS
        - [x] Any type that we don't parse is returned as `Utf8View` (aka a string) so you can parse
        it yourself.
    - Collation
        - [x] "default" (utf8)
        - [x] `C`
        - [ ] Everything else
- Output
    - [x] Arrow (aiming for "at least as good as ADBC")
    - [x] CLI text
    - [ ] CLI parquet
- Consumers
    - [x] Rust
    - [x] DataFusion
    - [ ] Python
    - [ ] Trino
    - [ ] DuckDB
    - [ ] Spark
- Optimization
    - Parallelization
        - [x] Parallel I/O, parallel scan, parallel query (where the input is suitable)
        - [ ] Perform full `parse` and `query` in one file pass
    - Auto-configuration
        - [x] Bare minimum attempt to autoconfig per your machine's CPU / RAM
        - [ ] Optimal per-machine config
    - Adaptation to your data
        - [x] Minimal adaptation to keep memory flat with large files
        - [ ] Optimal per-dump and per-table config

What works today:
- Streaming row extraction from plain-format dumps into typed Arrow batches, including arrays, composites,
  ranges and multiranges.
- A full byte-exact file map and DDL object inventory.
- A resumable scan that reports what it has.
- A best-effort structural cache.
- Reading a dump straight off an HTTP server, by byte range, with no credential
  handling — a presigned URL works as it is.
- Pushdown: column projection and a boolean filter expression — `AND`, `OR`, `NOT` and parens — over
  typed single-column comparisons.
- Parallel scan (where the input is suitable), with automatic worker count and memory budget
  defaulting to either the full container (when run in a container), or half of the machine (e.g.,
  workstation).

### Next
See:
- [`docs/design/roadmap.md`](docs/design/roadmap.md)
- [`docs/status/STATUS.md`](docs/status/STATUS.md) for exact implementation state, including known
deficiencies.


## Operation
This code essentially has two phases:
1. Parse the file's contents (`pgdt parse`).  Stores a file map, row group statistics, etc in a `*.dtcache` file.
2. Query the contents using the cache (`pgdt query`).

Note that this order is not strictly necessary -- you can "cold query" the file without a cache.  This will build a partial cache as it scans, though beware that:
- A partial cache will never provide the same query efficiency as a full one.  (We can only build certain statistics with a full parse.)
- A partial cache will not see your full data if A) your database is using partitions, or B) your dump file is from `pg_dumpall` and contains multiple databases.


## Documentation

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — setting a machine up to work on this: building, testing, and the debug-symbol setup a readable profile depends on.
- [`docs/status/STATUS.md`](docs/status/STATUS.md) — current implementation status (what's built vs. not, plus known deficiencies). [`docs/status/history/`](docs/status/history/) holds dated notes for future-session pickup and plan-changing discoveries.
- [`docs/manual/`](docs/manual/) — user manual: [type handling](docs/manual/type-handling.md), [dump inspection](docs/manual/dump-inspection.md), [SQL over a dump](docs/manual/datafusion-cli-pgdump.md).
- [`docs/design/decisions.md`](docs/design/decisions.md) — the decisions the code cannot explain, one numbered entry each, capped at 500 lines; how the system works is the code and its rustdoc.
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — project goals, the standing rules that cut across all work, and the phases still ahead. Each specified phase gets its own `roadmap-P<N>-<slug>.md` doc.
- [`docs/design/out-of-band.md`](docs/design/out-of-band.md) — the ledger of one-session work belonging to no phase: the admission rule, the watermark of spent `M<k>` numbers, and the rows still outstanding.
- [`docs/design/postgres-invariants.md`](docs/design/postgres-invariants.md) — `pg_dump` behaviours the design relies on, with source evidence and re-verification steps.
- [`docs/design/runtime-invariants.md`](docs/design/runtime-invariants.md) — the same, for the environment the process is given: the cgroup memory and CPU interfaces, and `std`'s reading of them.
- [`docs/design/measurements.md`](docs/design/measurements.md) — every performance figure the design relies on, each with the command that reproduces it.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`docs/process.md`](docs/process.md) — the phased development process this project follows (project-agnostic; written to be copied into other projects). Agents reach it through the `process` skill, which requires reading it in full before implementing a roadmap slice.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents: the command reference and one pointer per document with the moment to read it.
