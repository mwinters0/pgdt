# Postgres Dump Tool

Inspect Postgres dumps and query them like parquet (with streaming Arrow batches).

Available as:
- Rust library: `pgdt-query`
- A CLI: `pgdt`
    - Parse your dump's metadata
    - Inspect its contents: tables, roles, functions, row counts, etc.
- A DataFusion shell: `pgdt sql`
    - Full SQL support (See: the [Datafusion SQL reference](https://datafusion.apache.org/user-guide/sql/index.html))
    - Export to Parquet, CSV, etc


<details>

<summary>
🧐🤖 <i>Human author, LLM collaborator.</i>
</summary>

I'm experimenting in this repo to ask -- what would it look like to _collaborate_ with AI? This is a
third option to the apparent dichotomy that others are taking of A) trying to make it behave like a
human, using only the tools that humans use (e.g. human-only documentation), or B) "vibe coding"
everything, providing nothing but the goal (resulting in an inhumane and usually error-prone code
base).

I'm keeping a tight grip on the architecture while letting the agent perform all of the
implementation, and I'm letting the agent "have" things it benefits from in the repo, which makes it
noisier than a human repo but which helps the agent achieve the quality that I wanted with virtually
no churn.

My goal is a code base where the code quality is high and the code is navigable / approachable by
humans, and I'm willing to sacrifice some noisy AI commentary and git burden (carrying extensive
context) to get there.

Keep in mind, this repo is still young and I'm intentionally leaving some messes around to come back
to and clean up later, just as I would when coding by hand.  (Though admittedly, AI messes _are_
messier.)

I claim authorship because I've made literally thousands of architectural decisions via `/grilling`
and other steers, and I frequently review the code with a "purely human" eye.  But admittedly, not
all of it (until the dust settles), and only at major checkpoints.

Anyone is welcome to open human- or AI-generated PRs and Issues.

</details>

## Why
I have a 40Gb .xz dump which expands to 1.5Tb by the time it's loaded into Postgres. I want just
last week's data from it (3% of the data) as Parquet.

**Traditional approach:**
1. Load it into Postgres: 36 hours + 1.5Tb of disk
2. Extract it to Parquet: 5 minutes

**With `pgdt`:**
1. Build the cache: 20 minutes + 15Mb of disk
2. Extract it to Parquet: 15 minutes


## Install
Linux-only for now.  (Hire me so I can afford more hardware!)

If you're enlightened, you can add this to your `mise.toml`:

```toml
[tools]
"github:mwinters0/pgdt" = "latest"
```

Otherwise, this bash snippet will download it to `$PWD`:

```bash
(
  set -o pipefail
  PGDT_VERSION=0.4.0
  NAME="pgdt-v${PGDT_VERSION}-$(uname -m)-unknown-linux-gnu"
  curl -fsSL "https://github.com/mwinters0/pgdt/releases/download/v${PGDT_VERSION}/${NAME}.tar.xz" \
    | tar -xJ -O "${NAME}/pgdt" \
    > pgdt \
    && chmod +x pgdt
)
```

## Quickstart

1. **Parse:** You almost always start here.  This produces a `.pgdt` file.
    ```sh
    pgdt parse --source=mydump.xz
    ```
2. **Info:** Inspect what's in the dump: tables, roles, etc.
    ```sh
    pgdt info --json --source=mydump.xz | jq '.roles[]'
    ```
3. **Query:** Run simple queries, like if you want a single row.
    ```sh
    pgdt query --source=mydump.xz --table=mytable --where='mycolumn = bar'
    ```
4. **SQL:** When you want to run more complex queries or convert e.g. to Parquet.
    ```sh
    pgdt sql --source mydump=mydump.xz
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

For more SQL, see:
- [Our `pgdt sql` docs](docs/manual/pgdt-sql.md)
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
        - [x] gzip (seekable once a first pass has indexed it) / bgzip (seekable)
        - [ ] zstd (non-seekable and seekable)
        - [ ] lz4
    - File locations
        - [x] local
        - [x] http / https ranged requests (unauthenticated)
        - [ ] object store
- Output
    - [x] Streaming Arrow batches, with typed columns aiming for "at least as good as ADBC".
    - [x] CLI text / TSV
    - [x] Parquet, CSV, etc via `pgdt sql`
- Consumers
    - [x] Rust library
    - [x] DataFusion provider + shell, in the one `pgdt` binary
    - [ ] Python
    - [ ] DuckDB
    - [ ] Postgres via `pgrx`
    - [ ] Trino (?)
    - [ ] Spark (?)
- Distribution
    - [x] Linux binaries for x86-64 and arm64 (see: [installing](docs/manual/installing.md))
    - [ ] macOS (Help wanted!)
    - [ ] Windows (Help wanted!)
- Metadata collection:
    - [x] A full file map and DDL object inventory.
    - [x] Row groups with per-column statistics.
    - [ ] Statistics gathered by a cold `query`.
- Postgres Correctness
    - [x] Tests cover all major Postgres releases, v13-v18.
    - [x] Extensive test fixtures against every literal that `pg_dump` can write, and every option it takes.
    - Data types
        - [x] Almost all common base types parsed into Arrow types (see: [type
        handling](docs/manual/type-handling.md))
            - [x] User-configurable handling for Postgres values which cannot be represented in
            Arrow: read as NULL (the default), refuse the query, or read the column as text; `IS
            UNREPRESENTABLE` tells them from real NULLs (see: [type
            handling](docs/manual/type-handling.md#a-value-its-column-cannot-hold-reads-as-null)):
                - [x] `infinity`, `-infinity` and `NaN` in a `numeric`, `date` or `timestamp`
                column
                - [x] An `interval` past Arrow's range
                - [x] A `timestamp` in PostgreSQL's last three decades, past `294247-01-10`
                - [x] A `date` or `timestamp` past `262142-12-31`, which DataFusion cannot display
                - [x] `time` `24:00:00`
        - [x] Any type that we don't parse is returned as `Utf8View` (aka string) so you can parse
        it yourself.
        - [x] We match the behavior of a PostgreSQL restore in refusing to parse values that
        Postgres would refuse; `--postgres-invalid-values strict` checks every field and names what
        it cannot parse (see: [type
        handling](docs/manual/type-handling.md#when-a-value-does-not-match-its-type))
        - [ ] Common extension types, e.g. PostGIS
    - Tables with descendants (see: [tables with descendants](docs/manual/descendants.md))
        - [x] A table reads its own rows and every `INHERITS` child's and partition's, at any depth, as
        on the server; `--inheritance only` reads its own alone
        - [ ] A `--load-via-partition-root` dump's partitions read by their own names. The root reads
        them all; each partition refuses.
    - TimescaleDB 2.x
        - [x] Hypertables over uncompressed chunks, under the hypertable's name
        - [ ] Compressed and partially compressed chunks.
        - [ ] Continuous aggregates under their view's name.
            - [X] Readable as their materialization hypertable.
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
            settings](docs/manual/pgdt-sql.md#scan-settings)).
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
        - [x] Rows of any length PostgreSQL writes, up to its 1 GiB line bound: a parse streams a long
        line, a query charges it against its `--memory`, and one no reader could hold is refused for
        memory
        - [ ] Optimal per-dump and per-table config

Two things set the direction:
- It is meant to be embeddable as a query data source
- High performance on local files is a core goal rather than a later optimization. Dumps are
routinely hundreds of gigabytes, so the local-file reader aims to stay device-bound rather than
CPU-bound, with a flat RSS profile.

### Next
See:
- [`docs/design/roadmap.md`](docs/design/roadmap.md) - Sketches of future phases (aka epics)
- [`docs/status/deficiencies.md`](docs/status/deficiencies.md) - Known deficiencies (some TODO, some
simply properties of our design).


## Documentation
For humans:
- [`docs/manual/`](docs/manual/) — user manual:
    - [Installing and verifying](docs/manual/installing.md)
    - [Dump inspection](docs/manual/dump-inspection.md)
    - [Type handling](docs/manual/type-handling.md)
    - [SQL over a dump](docs/manual/pgdt-sql.md)
    - [Tables with descendants: inheritance, partitions and TimescaleDB](docs/manual/descendants.md)
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — setting a machine up to work on this: building, testing, and the debug-symbol setup a readable profile depends on.

Mostly for LLMs:
- [`docs/status/STATUS.md`](docs/status/STATUS.md) — current implementation status (what's built vs. not); [`docs/status/deficiencies.md`](docs/status/deficiencies.md) beside it indexes the known deficiencies. [`docs/status/history/`](docs/status/history/) holds dated notes for future-session pickup and plan-changing discoveries.
- [`docs/design/decisions.md`](docs/design/decisions.md) — the decisions the code cannot explain, one numbered entry each, capped at 800 lines; how the system works is the code and its rustdoc.
- [`docs/design/roadmap.md`](docs/design/roadmap.md) — project goals, the standing rules that cut across all work, and the phases still ahead. Each specified phase gets its own `roadmap-P<N>-<slug>.md` doc.
- [`docs/design/out-of-band.md`](docs/design/out-of-band.md) — the ledger of one-session work belonging to no phase: the admission rule, the watermark of spent `M<k>` numbers, and the rows still outstanding.
- [`docs/design/postgres-invariants.md`](docs/design/postgres-invariants.md) — `pg_dump` behaviours the design relies on, with source evidence and re-verification steps.
- [`docs/design/runtime-invariants.md`](docs/design/runtime-invariants.md) — the same, for the environment the process is given: the cgroup memory and CPU interfaces, and `std`'s reading of them.
- [`docs/design/measurements.md`](docs/design/measurements.md) — every performance figure the design relies on, each with the command that reproduces it.
- [`docs/design/pg-dump-compatibility.md`](docs/design/pg-dump-compatibility.md) — tracked `pg_dump` option support matrix.
- [`docs/design/historical/initial.md`](docs/design/historical/initial.md) — frozen original design handoff.
- [`docs/process.md`](docs/process.md) — the phased development process this project follows (project-agnostic; written to be copied into other projects). Agents reach it through the `process` skill, which requires reading it in full before implementing a roadmap slice.
- [`CLAUDE.md`](CLAUDE.md) — repo guidance for AI coding agents: the command reference and one pointer per document with the moment to read it.
