# P38 inbox — facts filed for its grilling

Evidence P38 (a table reads its descendants; TimescaleDB under its own names)
will need. **This is a queue, not a document**: when P38 is grilled, walk every entry,
fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## Today a hypertable reads as an empty root and its chunks as tables

**Fact.** The maintainer's account of the reference dump: pgdt presents every
TimescaleDB chunk table under its own name, and the hypertable under its name
holds none of their rows. The preamble keeps a table's `INHERITS` parents as
text and no query reads through them (`decisions.md`, "D36").

**Why P38 cares.** This is the presentation `--no-timescaledb` restores, so it
is the one the phase must keep reachable unchanged.

**Origin.** The 2026-10-30 session that sketched P38.

---

## A table can already be backed by several `COPY` blocks

**Fact.** A table's identity is `TableName { database, schema, table }`
(`pgdt-model/src/span.rs`), `DumpIndex::blocks_of` returns every block of one
identity in file order, and a plan reads them all, refusing with
`TableColumnsDisagree` unless they name one column set in any order
(`pgdt-query/src/stream/replay/plan.rs`). `--load-via-partition-root` blocks
merge into their root this way, their `partition_root` recorded on the block.
An `INHERITS` child or a `PARTITION OF` table is never merged into its parent:
no query-layer code reads `TableDef::parents` or `partition_of`.

**Why P38 cares.** A hypertable read through its chunks is a table backed by
blocks of other identities. The merge machinery and its column-set rule are
the precedent; an inheritance child may declare columns its parent lacks, which
the column-set rule refuses where PostgreSQL projects them away.

**Origin.** The 2026-10-30 session that sketched P38, from a read of the code.

---

## What the preamble keeps of a table, and what it leaves unparsed

**Fact.** `TableDef` (`pgdt-model/src/catalog.rs`) keeps columns, `parents`
(the `INHERITS` list, qualified), `of_type`, `not_null`, `checks` and
`partition_of` with its bound as text, folded from the
`ALTER TABLE ONLY … ATTACH PARTITION` every `pg_dump` writes (I86); a `CheckConstraint` keeps its name and
`no_inherit`, not its expression. `Extension` keeps a name and schema, no
version (`pg_dump` writes none). Triggers and `CREATE SCHEMA` have no grammar:
they are `SpanBody::Unparsed` spans with their TOC header and up to 64 KiB of
text stored. All of it is in the cache's `DumpIndex`; no row of any table is.

**Why P38 cares.** A chunk's time range is in a `CHECK` expression the model
drops, and TimescaleDB's catalog is `COPY` rows the cache never holds: what the
cache must carry for both presentations is new persisted state either way.

**Origin.** As above.

---

## Tables are listed by DataFusion's catalog and by `pgdt info` per block

**Fact.** `register_dump` (`datafusion-pgdump/src/catalog.rs`) registers a
catalog per database, a schema per PostgreSQL schema, every table's provider
built up front; `pgdt sql` lists through `SHOW TABLES` and
`information_schema`. `pgdt info` lists per `COPY` block, not per table. There
is no `tables` or `describe` command. `pgdt query --table` resolves through
`TableSelector` (`pgdt-query/src/dump.rs`), an unstated schema matching any.

**Why P38 cares.** "Masking" chunk names has three surfaces to decide —
the DataFusion catalog, `TableSelector`, and `info` — and a bare
`--table messages` matches a chunk's schema as readily as `public`.

**Origin.** As above.

---

## A chunk is an inheritance child, but the catalog says which tables are chunks

**Fact.** `ts_chunk_create_table` (`src/chunk.c`) creates a chunk with the
hypertable as its inheritance parent, so `pg_dump` writes it as `CREATE TABLE
… (CONSTRAINT constraint_<slice> CHECK (…)) INHERITS (<hypertable>)`. Its
schema and name prefix are configurable, a chunk can be renamed or moved, and
since 2.21 `attach_chunk` adopts an arbitrary table, so the name pattern
(`_timescaledb_internal._hyper_<ht>_<id>_chunk`) is not authoritative. A
release build never makes a hypertable a declaratively partitioned table
(the GUC exists under `TS_DEBUG` only). The root is normally empty but still
dumped: through 2.22 an insert-blocker trigger refuses rows reaching it, and
from 2.23 (#8804) nothing does. Chunks are not extension members, so their DDL
and data are dumped as any table's; a dump filtered with `-n`/`-t` can hold
catalog rows whose chunk tables are absent.

**Why P38 cares.** Generic inheritance reads an uncompressed hypertable whole,
the root's own rows included, without the catalog. The catalog is what tells a
chunk from any other child, and what notices a chunk missing from the file.

**Origin.** The 2026-10-30 session that sketched P38: a survey of the
TimescaleDB source at every release tag through 2.31.0 (full report,
machine-local: `runs/p38-timescaledb-survey/report.md`).

---

## The catalog reaches the dump as `COPY` rows whose shape changed by era

**Fact.** TimescaleDB marks its catalog with `pg_extension_config_dump`, so
`hypertable` (`id >= 1`), `dimension`, `dimension_slice`, `chunk`,
`compression_settings`, `continuous_agg*`, `bgw_job` (`id >= 1000`) and
`metadata` (`key <> 'uuid'`) are dumped as ordinary `COPY` blocks, even under
`--schema-only`, each with an explicit column list, `regclass` values fully
qualified (`pg_dump` dumps under `search_path = ''`). Joining a chunk to its
hypertable, range and compressed relation has three recipes:

| TimescaleDB | chunk's table | its range | its compressed relation | dropped chunks |
|---|---|---|---|---|
| 1.0–2.27 | `chunk.schema_name`, `table_name` | `chunk_constraint` → `dimension_slice`, slices shared | 1.5–2.28: `chunk.compressed_chunk_id`, another `chunk` row | 1.6–2.25: `chunk.dropped` |
| 2.28 | as above | `dimension_slice.chunk_id` | as above | — |
| 2.29+ | `chunk.relid` | `dimension_slice.chunk_id` | `compression_settings.relid` → `compress_relid` | — |

Ranges are half-open `bigint`s: Unix-epoch microseconds for time types, the
integer itself for integer time, `INT64_MIN`/`MAX` for an open side; a hash
dimension slices `[0, INT32_MAX)`. From 2.14 `metadata` carries
`timescaledb_version`; before it the version is fingerprinted from the catalog
blocks' column lists and DDL markers (the `ts_insert_blocker` trigger,
`_timescaledb_internal` against `_timescaledb_functions`, the edition suffix of
`COMMENT ON EXTENSION`, which names an Apache build that cannot compress).
`timescaledb_post_restore()` refuses a restore into another version from 2.14.

**Why P38 cares.** Reading the catalog means three recipes, chosen by a
version a dump may not state, and the fixture generator must produce each.

**Origin.** As above.

---

## A compressed chunk's rows are absent from the chunk and held in a codec

**Fact.** Compressing a chunk truncates it and moves its rows into a
compressed relation: 1.5–2.13 a chunk of an internal `_compressed_hypertable_N`
it inherits from, 2.14–2.28 a standalone `compress_hyper_<N>_<id>_chunk`,
2.29+ `<chunk>_compressed` linked only through `compression_settings`. Rows
inserted later stay in the uncompressed chunk (status bit 8, partial). The
relation holds batches of up to 1000 rows: `_ts_meta_count`, the segmentby
columns plainly, and each other column as `_timescaledb_internal.compressed_data`,
whose `COPY` text is base64 of its send format — an algorithm byte and a
big-endian body, unchanged since 1.5. Eight codecs (array, dictionary,
gorilla, delta-delta, bool, null, uuid, and 2.31's opt-in AIC) over simple8b
RLE; array and dictionary carry each value in its type's binary send form. A
NULL in a compressed column means the column's missing value, which a dump does
not carry. The 2.18–2.21 hypercore access method dumps the same way by
default. Every codec and the type's I/O are under the Timescale License
(`tsl/`); pgdt is Apache-2.0.

**Why P38 cares.** Generic inheritance reads a compressed hypertable as only
its uncompressed rows and says nothing: a silent short answer, unless the
catalog is read to notice. Decoding the rows is a codec reimplementation whose
only source is TSL-licensed.

**Origin.** As above.

---

## A continuous aggregate is a hypertable behind a view

**Fact.** A continuous aggregate dumps as a materialization hypertable
(`_timescaledb_internal._materialized_hypertable_<N>`, with chunks of its own,
possibly compressed), two internal views, and the user's `CREATE VIEW`: over
the materialization table alone when materialized-only (the default from
2.13), or that `UNION ALL` the aggregate's query over the raw hypertable past a
watermark when real-time. The pre-2.7 format stored partial aggregate states
as `bytea` finalized by `finalize_agg` in the view; it is gone from 2.25.
pgdt models no views.

**Why P38 cares.** Reading an aggregate under its name is reading a view; its
materialized rows are reachable as a hypertable either way.

**Origin.** As above.

---

## Some hypertables have no rows in a dump at all

**Fact.** On a multinode access node (2.0–2.13) chunks are foreign tables and
a dump holds no data; tiered storage (OSM, 2.8+, cloud only) is one foreign
chunk per hypertable, `osm_chunk = true`, DDL only. Policies are `bgw_job`
rows. `timescaledb_toolkit` is a separate extension whose sketch types can be
column types, mostly in aggregate materialization tables (not surveyed: no
clone here).

**Why P38 cares.** They bound the claim: rows a dump never held are not a gap
in reading it, but a hypertable presented without them is short and the
catalog can say so.

**Origin.** As above.

---

## The catalog gives each chunk's range without reading `CHECK` expressions

**Fact.** `dimension_slice` holds each chunk's range per dimension, and from
2.16 `chunk_column_stats` a per-chunk minimum and maximum for chosen columns.

**Why P38 cares.** A declared-bound prune of a hypertable's chunks needs no
expression grammar, only the catalog rows the phase reads anyway.

**Origin.** As above.

---

## The reference dump compresses nothing

**Fact.** The maintainer's account: Datanommer's hypertables hold no
compressed chunk.

**Why P38 cares.** The phase serves its reference workload without decoding a
codec; refusing compressed rows costs that workload nothing.

**Origin.** The 2026-10-30 session that sketched P38.

---

## The codecs are Timescale License source; what P38 reads is Apache

**Fact.** TimescaleDB's `LICENSE` puts everything under `tsl/` under the
Timescale License (`tsl/LICENSE-TIMESCALE`, posted 2020-09-24) and the rest
under Apache-2.0. Every codec, simple8b RLE and `compressed_data`'s I/O are
under `tsl/src/compression/`; the catalog's DDL (`sql/`), the chunk status
bits (`src/chunk.h`) and the bit array (`src/adts/bit_array.h`) are Apache.
The TSL defines a "Derivative Work" as a modification or enhancement of the
TSL software (§3.3), permits one to be distributed in binary form only and
only inside a "Value Added Product", a product not primarily database storage
or operations (§2.1(b), (d); §3.10), permits source to be redistributed only
unmodified (§2.1(c)), and binds whoever installs or uses the software. It has
no clause on reverse engineering or on reimplementing a format. Timescale,
Inc. is assignee of US 10,936,562, "Type-specific compression in database
systems"; its claims were not read. A web search found no decoder of
TimescaleDB's compressed format outside TimescaleDB, only codecs following
the Gorilla paper's own layout. The codec detail in the entry "A compressed
chunk's rows are absent from the chunk and held in a codec" was read from
`tsl/` source.

**Why P38 cares.** A decoder translated from `tsl/` cannot ship in an
Apache-2.0 source tree, so decoding is either a clean-room reimplementation
or not done; and the provenance of what P38's own code is written from
matters for both.

**Origin.** The 2026-10-30 session that sketched P38, from the license text,
the file headers, and a web search.

---

## The reference dump: TimescaleDB 2.13 on PostgreSQL 13, one hypertable of plain chunks

**Fact.** A full streaming pass over `datanommer2-2026-10-01.dump.xz`
(machine-local report: `runs/datanommer-survey/report.md`):

- `pg_dump` and server 13.23, database `datanommer2`; `CREATE EXTENSION IF NOT
  EXISTS timescaledb WITH SCHEMA public;` and no `timescaledb_pre_restore()`
  call. 2.43 TiB decompressed, 761 `COPY` blocks, 99.35% of the bytes in
  chunks.
- TimescaleDB **2.13.x**, fingerprinted: `metadata` is empty (no version row
  before 2.14), `chunk.creation_time` (from 2.13) and `hypertable_compression`
  and the multinode tables (gone in 2.14) present, `_timescaledb_functions` in
  use.
- One hypertable, `public.messages` (14 columns, `msg text` the long one), one
  time dimension on `"timestamp"` (`timestamp without time zone`), 7-day
  interval, no hash dimension. 728 chunks, every one `status` 0, uncompressed,
  undropped, not tiered; no continuous aggregate, no job.
- Each chunk is `CREATE TABLE _timescaledb_internal._hyper_1_<n>_chunk
  (CONSTRAINT constraint_<n> CHECK ((("timestamp" >= '…'::timestamp without
  time zone) AND ("timestamp" < '…'::timestamp without time zone))))
  INHERITS (public.messages);`, with per-chunk defaults, `<n>_<k>_<name>`
  primary-key and unique constraints and indexes in post-data. The root's
  `COPY` block is empty and it carries `ts_insert_blocker`.
- Chunks are dumped in name order (`_hyper_1_1`, `_10`, `_100`, …), not time
  order; every header names all 14 columns. The three
  `_timescaledb_cache.cache_inval_*` blocks are zero-column (`COPY … FROM
  stdin;` with no column list), a shape `pgdt-query/tests/stream.rs` already
  reads.
- The `.xz` is 106,071 single-block streams of 24 MiB + 4 KiB whose block
  headers record no sizes, so `xz -T` decodes it on one thread.

**Why P38 cares.** It confirms the inheritance premise and the 2.13 pin on
the oldest catalog era, and exercises the version fingerprint for real, a
2.13 dump stating no version. Its stream count is over three times koji's
multistream download's, which bears on what reading it through the `.xz`
costs before any row.

**Origin.** The 2026-10-30 session that sketched P38.
