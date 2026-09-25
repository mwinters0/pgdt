# SQL over a dump: `datafusion-cli-pgdump`

`datafusion-cli-pgdump` is [DataFusion](https://datafusion.apache.org/)'s own
SQL shell, `datafusion-cli` 55.1.0, with a `pg_dump` file's tables available
to it. Everything `datafusion-cli` does, it does — the REPL, `-c`, `-f`,
`--format`, `--memory-limit` — and it adds two ways to attach a dump.

```sh
pgdt parse --source koji.dump                 # once: builds koji.dump.dtcache
datafusion-cli-pgdump --dump koji=koji.dump   # then query it
```

```sql
> SELECT state, count(*) FROM koji.public.build GROUP BY state;
```

**It reads a dump only through the cache `pgdt parse` leaves**, and only a
cache covering the whole file. It never parses: a dump with no cache, or with
one an interrupted parse left, is refused before any SQL runs, with the `pgdt
parse` command that would build it. So `pgdt parse` first, once per dump; see
[dump inspection](dump-inspection.md). The cache is looked for where `pgdt`
writes it — beside a local dump, and in the working directory for a URL.

**Run as a container's init process** — `docker run image
datafusion-cli-pgdump …` makes it one — it exits 128 plus the number of any
signal that would end it elsewhere, which an init otherwise ignores, so Ctrl-C
and `docker stop` end it. A crash's signal — `SIGSEGV`, `SIGBUS` and the like —
still ends it as a crash; sent by hand, it is ignored. In the REPL, Ctrl-C is left to `datafusion-cli`,
which cancels the statement running rather than the session.

## `--dump`: a dump as catalogs

```sh
datafusion-cli-pgdump --dump koji.dump
datafusion-cli-pgdump --dump koji=koji.dump
datafusion-cli-pgdump --dump koji=https://example.com/dumps/koji.dump.xz:strings
```

Each database in the file becomes a **catalog**, each of its PostgreSQL
schemas a schema, and each table a table, so `koji.public.build` names a
table and `SHOW TABLES` lists them all — bar a table no query can read,
which is left out and named on stderr with why
([below](#what-it-says-on-stderr)). `--dump` is repeatable.

- **A database the file names is a catalog of that name.** A dump taken with
  `--create`, and every database in a `pg_dumpall` file, is named by the file.
- **`NAME=` names a dump that names no database** — a plain `pg_dump` taken
  without `--create`, which is the common case — and is required there. Given
  for a file naming one database, it replaces that name; given for a file of
  several, it is refused, since it could name none of them.
- **`:strings` reads every column as its text**, the escape from a type
  mapping you do not trust, and the way to read a column holding a value its
  type cannot (below).
- **The source is anything `pgdt --source` takes**: a path, an `.xz` file, or
  an `http(s)://` URL. A path containing `=` is written `./a=b.sql`; a URL's
  `=` is never read as a name.

## `CREATE EXTERNAL TABLE … STORED AS PGDUMP`: one table

```sql
CREATE EXTERNAL TABLE build STORED AS PGDUMP LOCATION 'koji.dump'
    OPTIONS ('pgdump.table' 'build', 'pgdump.schema' 'public');
```

| Option | |
|---|---|
| `pgdump.table` | The table. Required. |
| `pgdump.schema` | Its PostgreSQL schema, where the name alone matches tables in more than one. |
| `pgdump.database` | Its database, where the file holds several. |
| `pgdump.schema_mode` | `typed` (the default), or `strings` for every column as its text. |

A name that matches more than one table is refused, naming them. The table's
columns are the dump's, so the statement declares none.

## What it says on stderr

Attaching a dump prints a line for every column you should know about, each
naming its table as SQL reaches it, and a line for anything amiss with the
cache itself:

```
$ datafusion-cli-pgdump --dump shop=types.sql
warning: shop.public.t_base_type: column `v_mybase` (public.mybase): opaque base type — information-free in the dump; its value is the file's text, and compares as that text
warning: shop.public.t_enum_domain: `v_mood` (public.mood) is compared by its labels' text, as DataFusion compares the emitted dictionary, where PostgreSQL orders an enum's labels as its type declares them
warning: types.sql: 14 column(s) in 6 table(s) are each compared bytewise: the column declares no COLLATE clause, so its collation is the database's, which a plain dump does not record — this matches the server only if that collation is C or POSIX
```

Two kinds of column line. A column that came back as text says why. A column
whose **comparison in SQL is not PostgreSQL's** says how it differs: that is every
`=`, `<`, `ORDER BY`, `MIN` and `MAX` over it, since DataFusion compares the
value as its Arrow type compares — text byte for byte whatever its collation,
an enum by its labels' text rather than their declared order, an `interval` by
months, then days, then time, a bare `numeric`, `jsonb` or `inet` as the text
it is emitted as. `pgdt query` compares as PostgreSQL does where it can
([type handling](type-handling.md)); here SQL is DataFusion's.

**A table no query can read is an `error:` line, and is not listed** — one
whose `COPY` blocks name different columns, say, which no one schema holds.
Every other table attaches.

**Text columns with no `COLLATE` clause are counted, not listed**: on a real
dump that is nearly every text column, so the dump gets one line saying how
many there are, after its other lines — and a `CREATE EXTERNAL TABLE`
statement one line naming its table.

**Planning a query prints what its scan could not do as asked**, once per
table it reads, before any row is: fewer readers than DataFusion's
`target_partitions` because the memory the scans share could not seat them,
one reader only because it could not seat even that, or an `.xz` dump read
through its streaming decoder because one of its blocks would not fit
([Memory](#memory)).

```
warning: shop.logs.events: a memory budget of 0 byte(s) is less than the 8388608 byte(s) one reader of this source holds, so this runs at its one-slot floor whatever concurrency is asked for — what lifts it off the floor is a larger memory budget, which is not the same as a larger allowance on a source that recommends no per-reader cost of its own — that budget was carved from an allowance of 4294967296 resident byte(s), the limit /sys/fs/cgroup/memory.max states, against which the session's memory pool is granted 8589934592 byte(s), the attached dumps' statistics hold 7598 byte(s) and the scans still running had drawn 0 byte(s); the settings that move it: pgdump.memory (the allowance), datafusion.runtime.memory_limit (the pool's grant)
```

**A warning that quotes a memory budget says where that budget came from**:
the allowance and whether `pgdump.memory` stated it, a memory limit's file
did, or half of what was free; what `--memory-limit` granted DataFusion's own
operators; what the attached dumps' caches hold; and what queries still
running had taken. Then it names the settings that would change what the
warning reports for this dump and this budget, each as you would `SET` it, and
no other: `pgdump.memory` where a larger allowance raises the budget, with
`datafusion.runtime.memory_limit` beside it where a pool limit was taken;
`pgdump.chunk_size` where a smaller read makes a reader cheaper; and
`datafusion.execution.target_partitions` where fewer readers would each get
more. A plain dump's budget stops rising at 64 MiB whatever the allowance, so
past that its warnings name the chunk and not `pgdump.memory`.

`-q` keeps these warnings off and leaves errors on.

**What a scan skipped is under `EXPLAIN ANALYZE`**, as the scan node's
metrics: `row_groups_pruned_statistics`, how many of the row groups `pgdt
parse` recorded statistics for a `WHERE` ruled out unread, and
`bytes_unread_early_stop`, the bytes of rows left unread in a block sorted
past the filter's bound.

```sql
> EXPLAIN ANALYZE SELECT * FROM koji.public.build WHERE id < 1000;
```

## Types

**The Arrow schema is at least as good as the ADBC PostgreSQL driver's**
(`adbc-driver-postgresql` 1.12.0): wherever the driver gives a column a real
Arrow type, this one's is never wider. The exception is **`money`**, which
comes back as its text, because the dump does not record the locale that
formatted it. `oid` is `UInt32` where the driver says `Int32`, and `regproc`
is the function's name where the driver gives its OID.
[Type handling](type-handling.md) covers what each type becomes.

**A typed column cannot hold `infinity`, `-infinity` or `NaN`** (`date`,
`timestamp`, a typed `numeric`) nor an `interval` too long for its Arrow type:
reading such a value is an error, and `:strings` reads it as its text. A
`COUNT(column)` with no `WHERE` can still answer, from the statistics `pgdt
parse` recorded, without reading the column; a `SUM` cannot, a sum having no
way to leave the value out, so it reads the column and errors.

## Memory

Every dump's scans share one budget, carved from the memory limit the process
runs under — a container's or a cgroup's — or, where nothing limits it, from
half of what is free. A `--memory-limit` given to DataFusion's own operators
comes off it, as does what each attached dump's cache holds in memory, so the
scans never count on memory already spoken for. `SET pgdump.memory` states
the allowance instead ([below](#scan-settings)).

## Scan settings

What `pgdt query` takes as flags, this shell takes as settings, set with `SET`
like DataFusion's own — in the REPL, by `-c`, or in an `--rc` file — and
listed by `SHOW ALL`:

```sql
SET pgdump.memory = 4294967296;
```

| Setting | As `pgdt`'s |
|---|---|
| `pgdump.memory` | [`--memory`](dump-inspection.md#--jobs-and---memory-the-workers-and-the-allowance): bytes every dump's scans may hold resident between them. Unset, or `0`, the limit found, as above. |
| `pgdump.chunk_size` | [`--chunk-size`](dump-inspection.md#--chunk-size-you-almost-certainly-do-not-need-it): bytes a scan asks of the dump per read. |
| `pgdump.max_line_bytes` | [`--max-line-bytes`](dump-inspection.md#--max-line-bytes-a-dump-holding-very-large-values): the longest row a scan holds before refusing the dump. |

Each is a whole number of bytes. **A setting binds the queries planned after
it**: a query already running keeps the memory it was given. `SET
pgdump.memory = 0` returns to the limit found, as `SET
datafusion.execution.target_partitions = 0` returns to the machine's cores;
`RESET` does not, reaching only DataFusion's own settings. The other two may
not be `0`, and neither may `pgdt --memory`, which returns to the limit found
by being left off.
