# Tables with descendants: inheritance, partitions and TimescaleDB

A query of a table reads what `SELECT … FROM <table>` reads on the server:
the table's own rows **and every descendant's** — each `INHERITS` child and
each attached partition, at any depth. A TimescaleDB hypertable is one such
table, its chunks its children, so it reads under its own name too.

```sh
pgdt query --source shop.sql --table public.reading     # its partitions' rows
pgdt sql --source shop=shop.sql -c 'SELECT count(*) FROM shop.public.reading'
```

- **A descendant's rows are read under the table's columns.** A column a child
  declares of its own is left out; the rest come back in the table's column
  order, whatever order the child's `COPY` block lists them in. A child of two
  parents is read under both.
- **Every table is still read under its own name**, a partition and a
  TimescaleDB chunk included, as on the server, unless it refuses (below):
  `SHOW TABLES` and `information_schema` list each that reads.
- **A descendant is another block to prune.** The statistics `pgdt parse`
  records rule its row groups in or out as they do any table's; a partition's
  bounds and a chunk's time range are not read for it. A query run before any
  `parse` reads every descendant, and reads the file to its end to find them.
- **A descendant that cannot be read refuses the table**, naming it and why,
  rather than answering short of the server. Each readable descendant still
  reads under its own name.

`pgdt info` names each table a read takes other tables' rows into, with how
many and the rows they hold ([dump inspection](dump-inspection.md), "`info`:
reporting what is known").

## `--inheritance only`: a table's own rows

`only` reads every table as SQL's `FROM ONLY` does, its own rows alone;
`descendants` is the default everywhere, the library included.

| Where | Spelling |
|---|---|
| `pgdt query` | `--inheritance only` |
| `pgdt sql --source` | `…:inheritance=only` ([SQL over a dump](pgdt-sql.md#--source-a-dump-as-catalogs)) |
| `CREATE EXTERNAL TABLE … STORED AS PGDUMP` | `OPTIONS ('pgdump.inheritance' 'only')` |
| The library | `QueryOptions::inheritance`, `Inheritance::Only`; for DataFusion, `PgDumpOptions::inheritance` |

**The mode is a dump's, not a table's.** DataFusion's SQL has no `FROM ONLY`,
and a dump's tables are registered under one mode, so it is not a `SET`
either.

### Both in one statement: the dump registered twice

Attach the same dump twice, once in each mode, under two catalog names:

```sh
pgdt sql --source whole=shop.sql --source own=shop.sql:inheritance=only
```

```sql
SELECT a.n AS with_descendants, o.n AS its_own
FROM (SELECT count(*) AS n FROM whole.public.animal) a
CROSS JOIN (SELECT count(*) AS n FROM own.public.animal) o;
```

Both read the one cache `pgdt parse` left beside the dump.

## Partitions a dump wrote through their root

`pg_dump --load-via-partition-root` writes each partition's rows under the
root's name, for a restore to route by the partition key — and `pg_dump` 16,
and 13.11, 14.8 and 15.3 onward, does so unasked for a table hash-partitioned
on an enum column. pgdt does not
route them, so:

- **the root read with its descendants is whole**: every partition's rows;
- **each partition read by its own name refuses**, saying which blocks were
  written through which root, and so does each partitioned table below the
  root read with its descendants;
- **a partitioned table read alone answers no row**, as the server does, since
  none holds rows of its own.

`pgdt info` says so under `diagnostics`, one line per root, naming the first
partition its blocks hold and how many more, `--json` each. In `pgdt sql` a refused table is not listed, and
attaching the dump names it on stderr.

## TimescaleDB

A database is read as TimescaleDB's where the dump creates the extension or
holds a block of its catalog, and its catalog then says which children of a
hypertable are chunks, which are compressed and which the file lacks. Nothing
turns that reading off; `--inheritance only` reads every table, a hypertable
included, as its own rows.

| A hypertable's rows held as | Read |
|---|---|
| Uncompressed chunks | Supported: the hypertable reads them, each chunk under its own name too. |
| Compressed or partially compressed chunks | **Refused**, at the hypertable read with its descendants and at the chunk by its own name in either mode, naming the first chunk and how many more. The relation holding a chunk's compressed rows reads as the table it is, its batches as text. |
| Chunks the catalog lists and the file lacks — a dump excluding the chunks' schema, a multinode access node's or tiered storage's foreign chunks | Read as absent, as a restore holds none, with a warning naming the first and how many more. |
| Rows in the hypertable's root itself | Read, with a warning: a restore holds them, and TimescaleDB's own reads of the hypertable leave them out, so a count can exceed the server's. |
| A continuous aggregate | Readable as its materialization hypertable, under that table's name; the aggregate's view reads no way. |

- **The catalog is read by column name**, so a release pgdt has not met reads
  where its catalog holds what pgdt reads. Where it does not, each hypertable
  the catalog names that the file holds a chunk of refuses read with its
  descendants, naming what the catalog lacks — or, where the catalog does not
  even name its hypertables, every table of that database with a descendant
  does; its chunks read under their own names, a warning saying they are
  unchecked for compression. A named hypertable none of whose chunks the file
  holds reads its root's rows alone, a warning saying which chunks the file
  lacks cannot be known.
- **`pgdt info` says what refuses** under `diagnostics`, one line per
  hypertable whose chunks the file holds compressed, beside one per hypertable
  whose chunks it lacks and one per root holding rows of its own.
- **TimescaleDB 1.x is untested**: a 1.x server runs PostgreSQL 12 or older,
  which a routine `pg_dump` still dumps, but no fixture holds a 1.x catalog,
  and the release pgdt infers from a catalog's columns is read from 2.0 on.
- `pgdt info` lists each TimescaleDB database ahead of the blocks: its
  release, each hypertable's chunks counted uncompressed, compressed and,
  where the dump holds the database's DDL, absent, and each continuous
  aggregate; a chunk's block is folded under its hypertable's, which
  `--detail` turns off ([dump inspection](dump-inspection.md), "`info`:
  reporting what is known").
