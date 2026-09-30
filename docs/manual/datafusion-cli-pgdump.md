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
  mapping you do not trust, and the way to keep a value a typed column cannot
  hold (below).
- **`:unrepresentable=refuse` refuses a query needing a column that holds a
  value its type cannot hold**, before it reads a row, where the default,
  `:unrepresentable=null`, reads such a value as NULL, and
  `:unrepresentable=text` reads such a column as its text (below).
- **`:strict-identity=TERMS` is this dump's `--strict-identity`**
  ([below](#--strict-identity-when-a-moved-file-should-stop-the-session)),
  in place of the session's. The suffixes may come in any order.
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
| `pgdump.unrepresentable` | `null` (the default), reading a value a typed column cannot hold as NULL, `text`, reading a column holding one as its text, or `refuse`. |
| `pgdump.strict_identity` | This dump's `--strict-identity` terms, in place of the session's (below). |

A name that matches more than one table is refused, naming them. The table's
columns are the dump's, so the statement declares none.

## `--strict-identity`: when a moved file should stop the session

`pgdt`'s flag, with its terms and its default, for every `--dump` and every
`STORED AS PGDUMP` statement of the session that does not state its own —
with `:strict-identity=TERMS` or `pgdump.strict_identity`, above: `time`
refuses a cache written against another modification time or entity tag,
`location` one written for a dump fetched from elsewhere, the bare flag both,
`advisory` is the default and `none` binds nothing, the check below included. A dump's own
terms replace the session's rather than adding to them, so `advisory` is how
one dump under `--strict-identity=time` stops binding the time and keeps the
check below. **A dump changing under a scan fails that scan unless it says
`none`**, so a server stating neither a strong entity tag nor a
`Last-Modified`, which no check can see change, is refused unless it says
`none` too — which one such dump can say for itself, leaving every other dump
its check. See
[dump inspection](dump-inspection.md), "`--strict-identity`: when a moved file
should stop the run".

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

**Planning a query prints what its scan could not do as asked**, once for
each scan of a table, before any row is: fewer readers than DataFusion's
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
parse` recorded statistics for a `WHERE` ruled out unread;
`row_groups_pruned_dynamic_filter`, how many more were ruled out while the
query ran, by what a join's other side, an `ORDER BY … LIMIT`'s rows so far
or an ungrouped `MIN`/`MAX` had narrowed the scan to;
`rows_pruned_dynamic_filter`, how many rows that narrowing dropped before
decoding them, whether or not `pgdt parse` recorded statistics — none unless
[`pgdump.dynamic_filter_rows`](#scan-settings) is on; and `bytes_unread_early_stop`, the bytes of rows
left unread in a block sorted past either one's bound. The node's `predicate=` opens with the part of the
`WHERE` the scan answers itself, followed by each `DynamicFilter [ … ]` a
join, sort or aggregate above hands it, `empty` until it first narrows; any
other part of the `WHERE` is a `FilterExec` above it.

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
`timestamp`, a typed `numeric`), `time` `24:00:00`, an `interval` too long for
its Arrow type, nor a timestamp past `294247-01-10`, and DataFusion cannot
print a `date` or timestamp past `262142-12-31`. **Each reads as NULL, for
every purpose**: a `WHERE` compares it as NULL, `IS NULL` matches it,
`COUNT(column)` leaves it out and `MIN`, `MAX` and `SUM` skip it, whether the
statistics answer or the rows are read, so a query answers the same however
its partitions run. Each scan says on stderr how many a column it reads
holds. `:strings` reads it as its text instead.

**`pgdump_unrepresentable(column)` finds them**: `true` for a value the
column's declared type accepts and DataFusion cannot hold, `false` for every
other, a NULL the dump holds among them, so
`WHERE v_date IS NULL AND NOT pgdump_unrepresentable(v_date)` keeps the dump's
NULLs alone. It works in every `:unrepresentable=` mode, and only in a `WHERE`
over a pgdump table's column — alone, under `NOT`, `AND` and `OR`, or beside
terms the scan answers — since the scan answers it on the dump's text: a query
that would have DataFusion evaluate it, in a `SELECT` list, a `GROUP BY` or a
`WHERE` beside a term the scan leaves to DataFusion, is refused when it is
planned. So is one under `:strings`, which reads no declared type.

**`:unrepresentable=text` reads a column holding such a value as its text**,
a `Utf8View` whatever its declared type, and every other column as its type;
registration says which columns it read that way. The dump's text is kept, so
`infinity` prints as `infinity`, and the column compares as text does:
`'10000-01-01' < '9999-12-31'`, and a comparison with a number casts each
value to that number's type, failing on the first that is not one. Which
columns are text is decided by the whole table, so every query of it sees
the same schema.

**The shell starts with
`datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown` off**, unless
`DATAFUSION_OPTIMIZER_ENABLE_AGGREGATE_DYNAMIC_FILTER_PUSHDOWN` states it:
DataFusion 55.1's filter for an ungrouped `MIN` and `MAX` can lose a column's
bounds once a batch holds no value of it — a NULL, or a value read as one —
and a scan skipping rows under what is left answers a `MIN` too high, or
another column's `MIN` and `MAX` as NULL. Turning it on, directly or by
setting or resetting `datafusion.optimizer.enable_dynamic_filter_pushdown`,
which sets it too, brings that back.

**`:unrepresentable=refuse` refuses when the query is planned**, before a
row is read, wherever it needs the values of a column the dump holds such a
value in — anywhere in the table, whatever its `WHERE` keeps — naming the
column and how many. A column only a `WHERE` the scan answers itself reads is
not needed, and its values compare in PostgreSQL's order, `-infinity` below
every finite value and `infinity` above; a scan does not answer a filter it
leaves to DataFusion, so it plans as though the column were selected.
The same query refuses on every run, or answers on every run.

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
| `pgdump.dynamic_filter_rows` | None, `pgdt query` having no join or sort above it: `true` or `false`, whether a scan drops each row a join's other side, an `ORDER BY … LIMIT`'s rows so far or an ungrouped `MIN`/`MAX` has ruled out before decoding it. Off by default. |

The first three are whole numbers of bytes. **A setting binds the queries
planned after it**: a query already running keeps the memory it was given. `SET
pgdump.memory = 0` returns to the limit found, as `SET
datafusion.execution.target_partitions = 0` returns to the machine's cores;
`RESET` does not, reaching only DataFusion's own settings. The other two may
not be `0`, and neither may `pgdt --memory`, which returns to the limit found
by being left off.

**`pgdump.dynamic_filter_rows` trades one kind of query against another.**
Off, a scan still skips the row groups such a narrowing rules out and stops
reading a sorted block past its bound, which is what makes a selective join on
a clustered key or an `ORDER BY … LIMIT` fast, and leaves each remaining row to
the join, sort or aggregate above it. On, it also checks each row itself, which
pays where the narrowing rejects most rows that no row group could skip — a
selective join on a key scattered through the table — and costs where it
rejects few, such as a join most rows match. The answer is the same either
way.
