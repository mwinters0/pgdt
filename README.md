# Postgres Dump Tool

Inspect Postgres dumps and query them like they're parquet (with streaming Arrow batches).

Available as:
- A Rust library: `pgdump_query`
- A CLI: `pgdt`
    - Parse & cache your dump's metadata
    - Inspect it: tables, roles, functions, row counts, etc.
    - Execute simple queries
- A DataFusion shell: `datafusion-cli-pgdump`
    - Full SQL support (See: the [Datafusion SQL reference](https://datafusion.apache.org/user-guide/sql/index.html))
    - Export to Parquet, etc

_Human author, LLM autocomplete._

## Quickstart

### `pgdt`

Use case: you have a dump and you want to know what's in it.

```bash
# Parse a dump (builds foo.xz.dtcache)
pgdt parse --source=foo.xz

# Inspect what you parsed: tables, roles, etc
pgdt info --detail --source=foo.xz
# Or as JSON
pgdt info --json --source=foo.xz | jq '.roles[]'

# Run a simple query.  Look ma, no daemons!
pgdt query --source=foo.xz --table=mytable --where='mycolumn = bar'
```

See: [dump inspection docs](docs/manual/dump-inspection.md) for more.  Notably, if you're never
going to query the data then you can speed up the parse and reduce the cache file size with:

```bash
# Parse a dump with only metadata-level statistics
pgdt parse --source=foo.xz --statistics-level=metadata
```

### `datafusion-cli-pgdump`

Use case: you want to inspect your data and/or extract some portion of it.

```bash
# Parse a dump (builds foo.xz.dtcache)
pgdt parse --source=foo.xz

# Then query it as catalog `foo`
datafusion-cli-pgdump --dump foo=foo.xz
```

```sql
SHOW TABLES;

-- Export some hive-partitioned Parquet
COPY (
  SELECT
    *,
    date_part('year', event_ts)  AS year,
    date_part('month', event_ts) AS month
  FROM foo.public.events
)
TO '/tmp/events_parquet'
STORED AS PARQUET
PARTITIONED BY (year, month)
OPTIONS (
  'format.writer_version' '2.0',
  'compression' 'zstd(3)'
);

INSERT INTO ...; -- Unsupported!
```

For more, see:
- [Our `datafusion-cli-pgdump` docs](docs/manual/datafusion-cli-pgdump.md)
- Upstream's [`datafusion-cli` docs](https://datafusion.apache.org/user-guide/cli/index.html)
- The [Datafusion SQL reference](https://datafusion.apache.org/user-guide/sql/index.html)


## Status
⚠️ **Functional, with extensive tests, but still early development.**  No stability guarantees for
our API, CLI, or data until we reach v1.0.

- Input
    - `pg_dump` formats
        - [x] plain, including `pg_dumpall` / multi-database dumps
        - [ ] plain with `--inserts` / `--column-inserts` (rows as `INSERT` statements)
        - [ ] directory
        - [ ] tar
        - [ ] custom
    - `psql` COPY formats
        - [ ] CSV
        - [ ] text
        - [ ] binary
    - Compression
        - [x] xz (seekable)
        - [ ] gzip (non-seekable) / bgzip (seekable)
        - [ ] zstd (non-seekable and seekable)
        - [ ] lz4
    - File locations
        - [x] local
        - [x] http / https ranged requests (unauthenticated)
        - [ ] object store
- Output
    - [x] Streaming Arrow batches, with typed columns aiming for "at least as good as ADBC".
    - [x] CLI text / TSV
    - [x] Parquet, CSV, etc via `datafusion-cli-pgdump`
- Consumers
    - [x] Rust library
    - [x] DataFusion provider + shell
    - [ ] Python
    - [ ] DuckDB
    - [ ] Trino (?)
    - [ ] Spark (?)
- Metadata collection:
    - [x] A full byte-exact file map and DDL object inventory.
    - [x] Row groups with per-column statistics.
    - [ ] Statistics gathered by a cold `query`.
- Postgres Correctness
    - [x] Tests cover all major Postgres releases, v13-v18.
    - Data types
        - [x] Almost all common base types parsed into Arrow types (see: [type
        handling](docs/manual/type-handling.md))
            - [ ] User-configurable handling for Postgres types which cannot be represented in Arrow:
                - [ ] `infinity`, `-infinity` and `NaN` in a `numeric`, `date` or `timestamp`
                column
                - [ ] An `interval` past Arrow's range
                - [ ] A `timestamp` in PostgreSQL's last three decades, past `294247-01-10`
                - [ ] `time` `24:00:00`
        - [x] Any type that we don't parse is returned as `Utf8View` (aka string) so you can parse
        it yourself.
        - [ ] Common extension types, e.g. PostGIS
    - Collation
        - [x] `C` / `POSIX`
        - [ ] Everything else.  Text compares bytewise, with a warning.  (See: KD7)
    - [ ] Encodings other than UTF-8
    - [ ] Large object (BLOB) contents
- Query handling
    - [x] Pushdown:
        - [x] Column projection
        - [x] Boolean filter expression — `AND`, `OR`, `NOT`
        - [x] Parens
        - [x] Typed single-column comparisons (`=`, `!=`, `<`, `<=`, `>`, `>=`, `IS [NOT] DISTINCT FROM`,
          `IS [NOT] NULL`).
            - Comparisons follow Postgres's semantics per type. Wherever DataFusion's comparison differs
              from the server's, e.g. collation, `interval`, `jsonb`, the column gets a warning.
    - [x] Row-group pruning
    - DataFusion integration
        - [x] Filters pushed down as `Exact` where we answer them the way DataFusion would: the above,
          plus `BETWEEN`, `IN (…)`, `IS [NOT] TRUE`/`FALSE`/`UNKNOWN` and bare boolean columns.
          Anything else (`LIKE`, functions, cross-column comparisons) runs in DataFusion after the scan.
        - [x] `LIMIT` pushed into the scan.
        - [x] Exact statistics for the optimizer: row counts, NULL counts, min/max, distinct counts,
          sums and byte sizes. `COUNT(*)`, `MIN`, `MAX` and `SUM` can answer without reading a row,
          and joins are ordered by size.
        - [x] Sort order: `ORDER BY` skips when a column is detected as sorted
        - [x] Dynamic filters from joins, `ORDER BY … LIMIT` and ungrouped `MIN`/`MAX`: the filter
          updates as the scan proceeds. It skips row groups and stops reading a sorted block past
          the bound.
            - Dropping rejected rows before decoding them is opt-in (see: [scan
            settings](docs/manual/datafusion-cli-pgdump.md#scan-settings)).
            - [ ] Membership pruning for a join with more distinct keys than DataFusion lists (150 by
            default), several key columns, or a key column with no dictionary. These prune by the
            join's key bounds alone.
- Parallelism
    - [x] Parallel I/O, parallel scan, parallel query (where the input is suitable)
    - [ ] Perform full `parse` and `query` in one file pass
    - Auto-configuration:
        - [x] Bare minimum attempt to autoconfig per your machine's (or container's) CPU / RAM.
            - Defaults to 50% of a bare machine, or the full container when in a container.
        - [ ] Optimal per-machine config
    - Adaptation to your data:
        - [x] Minimal adaptation to keep memory flat with large files
        - [ ] Optimal per-dump and per-table config

Two things set the direction:
- It is meant to be embeddable as a query data source
- High performance on local files is a core goal rather than a later optimization. Dumps are
routinely hundreds of gigabytes, so the local-file reader aims to stay device-bound rather than
CPU-bound, with a flat RSS profile.

### Next
Prior to v1.0 we must provide:
- Support for all pg_dump formats and compression methods
- Postgres-equivalent collation, at minimum for the common default of `en_US.utf8`.
- A Python library interface

- [`docs/design/roadmap.md`](docs/design/roadmap.md) - Sketches of future phases (aka epics)
- [`docs/status/deficiencies.md`](docs/status/deficiencies.md) - Known deficiencies (some TODO, some
simply properties of our design).


## Documentation
For humans:
- [`docs/manual/`](docs/manual/) — user manual:
    - [Dump inspection](docs/manual/dump-inspection.md)
    - [Type handling](docs/manual/type-handling.md)
    - [SQL over a dump](docs/manual/datafusion-cli-pgdump.md)
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — setting a machine up to work on this: building, testing, and the debug-symbol setup a readable profile depends on.

Mostly for LLMs:
- [`docs/status/STATUS.md`](docs/status/STATUS.md) — current implementation status (what's built vs. not); [`docs/status/deficiencies.md`](docs/status/deficiencies.md) beside it indexes the known deficiencies. [`docs/status/history/`](docs/status/history/) holds dated notes for future-session pickup and plan-changing discoveries.
- [`docs/design/decisions.md`](docs/design/decisions.md) — the decisions the code cannot explain, one numbered entry each, capped at 700 lines; how the system works is the code and its rustdoc.
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — project goals, the standing rules that cut across all work, and the phases still ahead. Each specified phase gets its own `roadmap-P<N>-<slug>.md` doc.
- [`docs/design/out-of-band.md`](docs/design/out-of-band.md) — the ledger of one-session work belonging to no phase: the admission rule, the watermark of spent `M<k>` numbers, and the rows still outstanding.
- [`docs/design/postgres-invariants.md`](docs/design/postgres-invariants.md) — `pg_dump` behaviours the design relies on, with source evidence and re-verification steps.
- [`docs/design/runtime-invariants.md`](docs/design/runtime-invariants.md) — the same, for the environment the process is given: the cgroup memory and CPU interfaces, and `std`'s reading of them.
- [`docs/design/measurements.md`](docs/design/measurements.md) — every performance figure the design relies on, each with the command that reproduces it.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`docs/process.md`](docs/process.md) — the phased development process this project follows (project-agnostic; written to be copied into other projects). Agents reach it through the `process` skill, which requires reading it in full before implementing a roadmap slice.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents: the command reference and one pointer per document with the moment to read it.
