# P38 — A table reads its descendants; TimescaleDB under its own names

The phase's spec, replacing the roadmap's sketch; its inbox is drained into it.

## What and why

**A table reads its own rows and every descendant's, for every dump**, as
`SELECT … FROM parent` does on the server, **and a TimescaleDB hypertable is
read under its own name** on top of that, its catalog read to say which
children are chunks and which rows the file does not hold.

- **Today pgdt reads every table as SQL's `ONLY`.** A dump's `COPY` block holds
  a table's own rows, and no query reads through `INHERITS` or an attached
  partition (`TableDef::parents` and `partition_of` are read by type lookups
  alone), so a TimescaleDB hypertable reads empty beside its chunks and so does
  a declaratively partitioned root. The input contract is valid PostgreSQL
  (`roadmap.md`, "The input contract is valid PostgreSQL, not `pg_dump`'s
  output"), in which a parent includes its children; the defect is one, and
  TimescaleDB is one producer of it.
- **The reference workload is Datanommer**, Fedora's message archive: a
  2.43 TiB TimescaleDB dump whose one hypertable, `public.messages`, has 728
  plain chunks ("Facts the grilling found"). The goal is querying it in its
  native form — `SELECT … FROM messages` — and stating plainly in the README
  that pgdt supports TimescaleDB, or a supported/unsupported list where it
  falls short. Datanommer surfaced the need as koji did for scale: the
  requirements, fixtures and harnesses stand on their own, and no test assumes
  the dump exists. Its lines past `max_line_bytes` are P33's, which runs after
  this phase; until it lands the dump is read with the limit raised.

## Settled in the grilling

### The generic mechanism

- **A table's rows are its own and every descendant's**: an `INHERITS` child
  and an attached partition alike, at any depth, each descendant's rows
  projected to the parent's columns. A column the parent lacks — an
  inheritance child may declare its own — is projected away before it can
  refuse anything; the block merge's column-set rule (`TableColumnsDisagree`),
  which `--load-via-partition-root` blocks meet, does not apply between a
  parent and its descendants. A child of two parents is read under both.
  Rejected: a TimescaleDB-only rule, leaving partitioned roots empty; a
  separate switch per kind of descendant.
- **The switch back to today's presentation is one mode, `--inheritance
  <descendants|only>`**: `pgdt query --inheritance only`, `pgdt sql`'s
  `--source <path>:inheritance=only`, the `pgdump.inheritance` session setting,
  and `QueryOptions::inheritance` in the library — the CLI's mode convention
  (`--schema-mode`, `:unrepresentable=`). `only` reads every table as its own
  rows, inheritance and TimescaleDB alike. **Session-wide, not per table**,
  because DataFusion's SQL parses no `FROM ONLY` (sqlparser 0.62 reads `ONLY`
  for `ALTER TABLE` and `TRUNCATE`, not a table factor). Named for
  `pg_inherits`, which records partitions and inheritance children both.
  Rejected: `--no-timescaledb`, the escape being from PostgreSQL's semantics
  and a partitioned dump needing it as much; a boolean `--only`, against the
  mode convention.
- **`descendants` is the default everywhere, the library included**: the same
  name means the same rows in `pgdt sql`, `pgdt query` and an embedder's
  session. The library's conservative defaults are for resources
  (`roadmap.md`, "A default runs as fast as the allocation permits"), not
  meaning.
- **Every table stays listed and addressable under its own name**, a chunk
  included, as on the server, where `_timescaledb_internal` holds them: nothing
  is hidden from `information_schema`, `SHOW TABLES`, `pgdt query --table` or
  `pgdt info`. Rejected: chunks left out of listings, which would make pgdt's
  catalog disagree with the server's and is the one rule that would change what
  exists; the clutter is a listing's, answerable only by a search-path-like
  default schema DataFusion does not offer today.
- **A descendant is another block to prune, by its statistics alone.** A
  default parse gathers them, at least as tight as any declared bound (I86;
  inline `CHECK`s precede `COPY`), so neither `CHECK` expressions, partition
  bounds nor TimescaleDB's `dimension_slice` are read for pruning. A cold
  parent query scans every descendant, as a cold query scans for every other
  predicate. Rejected: bound-based pruning, a second proof path for the cold and
  declined cases only (D75, D85), and not filed as Future: only a figure
  showing a cold parent query matters would reopen it.
- **A descendant that cannot be read refuses its parent**, with a finding
  naming the descendant and why. A parent answering short of the server is a
  wrong answer; `--inheritance only` still reads each readable descendant
  under its own name. Rejected: reading the rest and reporting the missing one;
  a setting choosing between them.
- **"Short" is measured against the server this dump restores to**, the only
  one its rows can be checked against. A row a restore would not hold is no
  gap; a row a restore holds that pgdt cannot place is refused, never read
  short. Rejected: the original server as reference, which would refuse every
  dump of a database with foreign chunks.
- **A `--load-via-partition-root` dump's partitions refuse under their own
  names, and its root refuses under `--inheritance only`**, each with a finding
  naming the blocks written through the root — whether `pg_dump` was given the
  flag or forced it (a hash partition key on an enum,
  `fixture_schema_partitions.sql`). A restore routes those rows into partitions
  by the partition key under its operator classes and collations (I86), which
  pgdt does not evaluate. The root read through, the common query, stays whole.
  Rejected: routing here, a feature of its own (the Future item "Rows routed to
  their partitions"); today's presentation, a silent short answer for a
  partition read by name.

### TimescaleDB

- **TimescaleDB is detected per database**, by `CREATE EXTENSION timescaledb`
  or its catalog's `COPY` blocks, and **its catalog is read in every
  TimescaleDB dump; no switch reaches that**. Detection is correctness, not
  presentation: it is the only way to notice rows the hypertable's chunks do
  not hold.
- **The catalog is read by column name, whatever version it states.** `pg_dump`
  writes every catalog block with an explicit column list, and update scripts
  can reorder a catalog's columns, so a position means nothing. The version
  decides which recipe joins a chunk to its hypertable and its compressed
  relation ("Facts the grilling found"): the `metadata` row from 2.14, a
  fingerprint of the catalog's columns before it. A release whose catalog
  still carries what pgdt reads is read without a pgdt release. **Where columns
  pgdt needs are missing, every hypertable of that database refuses**, with a
  finding naming the catalog's shape, and its chunks read under their own
  names; a chunk read so cannot be checked for compression, and the finding
  says so. Rejected: refusing a version newer than the newest pinned; falling
  back to plain inheritance with a warning; a second, catalog-free detection
  path (a non-empty relation with `compressed_data` columns), kept to the
  finding's wording instead.
- **Rows a restore would not hold are read as absent, with a finding**: a
  multinode access node's foreign chunks (2.0–2.13), tiered storage's foreign
  chunk (OSM), and chunk tables a `-n`/`-t` dump left out though the catalog
  lists them. The finding names what the catalog lists and the file lacks. A
  catalog row marking a chunk dropped (`chunk.dropped`, 1.6–2.25) is no chunk.
- **A compressed chunk's rows refuse wherever they are reached**: the
  hypertable, and the chunk read by its own name, under `--inheritance only`
  too. A restore holds them — compressing a chunk empties it and moves its rows
  into a compressed relation of codec batches — and TimescaleDB returns them
  for a query on either. Today's empty read of a compressed chunk is a silent
  short answer, not a presentation kept. A partially compressed chunk (status
  bit 8) refuses as a compressed one. The compressed relation stays readable as
  the table it is, its `compressed_data` columns as text, as on the server.
- **P38 does not decode compressed chunks.** The reference workload compresses
  nothing; the codecs are Timescale License source (`tsl/src/compression/`),
  so a decoder in this Apache-2.0 tree is a clean-room reimplementation whose
  conditions — and Timescale's US 10,936,562 — are outside a grilling
  ("Facts the grilling found"). It is the Future item "Compressed TimescaleDB
  chunks, decoded from a clean room". So **no `--no-timescaledb` is built**:
  with nothing decoded there is no presentation to turn off, `--inheritance
  only` being the one way back. Rejected: decoding in P38; a permanent
  non-goal, giving up a format stable since 1.5 for a question nobody has put
  to counsel.
- **pgdt's TimescaleDB code is written from TimescaleDB's Apache-licensed
  source (`src/`, `sql/`), its documentation and the dumps the fixture
  generator produces — never from `tsl/`.** P38 needs nothing there: the
  catalog DDL is `sql/pre_install/tables.sql`, the chunk status bits
  `src/chunk.h`. A clean room for a decoder is possible later only if this has
  held from the start, which cannot be established after the fact.
- **A continuous aggregate is identified and inspected in `pgdt info`, and
  read no other way**: its view, its materialization hypertable and whether it
  is real-time are shown for an administrator; its rows are reachable as the
  materialization hypertable's, under that table's name, through the generic
  mechanism; the aggregate under the view's name is named unsupported. pgdt
  models no views. Rejected: materialized-only aggregates as a special case,
  which is view support by the back door (the Future item "Views read as
  DataFusion views").
- **TimescaleDB 1.x is outside what pgdt reads**: it runs on PostgreSQL 12 and
  older, and pgdt reads `pg_dump` 13 onward. The README's list says so.

### What is shown, claimed and checked

- **`pgdt info` shows the logical model**: each parent with its descendant
  count and rows; per TimescaleDB database a section with the version, stated
  or the era inferred, each hypertable's dimensions and its chunks counted
  whole, compressed and absent from the file, and each continuous aggregate as
  above. The per-block listing stays, chunks folded under their hypertable
  unless `--map` or `--detail` asks for them. What decides whether a query
  refuses is what an administrator reads first. Rejected: a TimescaleDB section
  over today's flat listing, thousands of chunk lines at Datanommer's scale.
- **The README claims "supports TimescaleDB" when every way a dump can store
  a hypertable's rows, across the pinned versions, is read under the
  hypertable**, and states a supported/unsupported list otherwise. The test is
  the dump's rows: policies, jobs and functions are server behaviour with no
  rows of their own and owe a dump reader nothing. With compressed chunks
  refused, P38 ends on the list: hypertables over uncompressed chunks
  supported, compressed and partially compressed chunks refused, continuous
  aggregates readable only as their materialization hypertable, 1.x outside.
- **The fixtures pin one TimescaleDB version per dump-visible shape**, each
  the last release of its shape on a PostgreSQL major pgdt reads, and the
  newest release, which moves as the routine `pg_dump` set does:

  | TimescaleDB | PostgreSQL | The shape it pins |
  |---|---|---|
  | 2.13 | 13 | `chunk.dropped`; compressed chunks inheriting an internal compressed hypertable; no version row; multinode tables; the reference dump's pair |
  | 2.21 | 15 | `compression_settings.compress_relid`, standalone compressed chunks, the version row, hypercore's dump |
  | 2.27 | 16 | `chunk.dropped` gone, `chunk_constraint` its last, no insert blocker |
  | 2.28 | 17 | `dimension_slice.chunk_id`, `chunk_constraint` gone |
  | newest (2.31 today) | 18 | `chunk.relid`, compressed relations linked only through `compression_settings` |

  2.27 is the one pin where the first recipe holds with `dropped` gone, so the
  catalog read by column name meets a missing column in a real dump. Rejected:
  pins at 2.14 (the first version row) or 2.25 (`bgw_job`'s move), which change
  nothing pgdt reads.
  Each fixture holds every way its version stores a hypertable's rows:
  uncompressed, compressed and partially compressed chunks, a dropped chunk
  where the version keeps a row for one, a hash dimension, an integer-time
  hypertable, a continuous aggregate, and from 2.23 stray rows in the root. A
  second dump of 2.13's and the newest's database with `-n public` holds
  catalog rows for absent chunks. **The server is `timescale/timescaledb-ha`
  at each pin**, the image TimescaleDB's own Docker install page pulls, on
  Ubuntu because it needs glibc 2.33 or later; TimescaleDB's packages are
  glibc builds too, so glibc is upstream's standard configuration and D69's
  rule needs no exception. The dump is written by the routine
  `postgres:<major>.<minor>-trixie` `pg_dump` against it, so the fixture is the
  same producer's output as every other, and one image per pin is the whole
  matrix. A pin with no `-ha` tag takes TimescaleDB's Debian packages on the
  routine image instead; one with neither is put to the maintainer, not
  swapped silently.
- **Reading a hypertable is oracle-checked**, D71's method: each fixture
  restored into its version's server between `timescaledb_pre_restore()` and
  `timescaledb_post_restore()`, and the server's answers — each hypertable,
  each chunk read by name, each compressed chunk's row count — committed
  beside the fixture and compared with pgdt's reads and refusals. Generic
  inheritance is checked the same way against a plain server.
- **What P38 relies on of TimescaleDB is registered**, as `pg_dump`'s is in
  `postgres-invariants.md`: a new assumptions register,
  `timescaledb-invariants.md` (`TS<n>`), each entry proved from the
  Apache-licensed source and re-verified at each new TimescaleDB release,
  with its read-trigger in `CLAUDE.md` beside the other registers'. Slice 1
  opens it with what the generator confirmed, and decides whether its sites
  carry markers a `scripts/` check resolves, as `pg_refuses.py` does `I<n>`'s.
  Rejected: `postgres-invariants.md`, whose trigger is a PostgreSQL release;
  the fixtures alone, which watch only the versions they pin while the catalog
  is read at every version.

## Where it lives

The concept map (`concepts.md`) gains one concept and widens four; the
layering (D68, D74) places each part.

- **The declared schema, widened: a table's descendants.** **pgdt-model**,
  `catalog.rs`: the walk over `parents` and `partition_of` across a database,
  beside the walks `declared_column` and `table_checks` already make over the
  same fields — a lookup over what the preamble declared, persisted already.
- **A new concept, TimescaleDB's catalog**: the catalog's rows as the dump
  states them, the recipe joining them by era, and what they conclude.
  **pgdt-model**, a module of its own: the rows by column name as text,
  persisted with the database they belong to (anything persisted is model
  vocabulary, D74; a `CACHE_FORMAT_VERSION` bump), and the era recipes over
  them, which read no Arrow and do no I/O: a row and what it means kept
  together, and `pgdt info` and the query layer reading one set of
  conclusions. Rejected: the recipes in pgdt-query beside resolution, which
  `info` would reach into; a crate of their own, a seam with nothing on its
  other side. **pgdt-mapping**: the rows recorded
  as the pass maps the catalog's blocks, beside the other observers a pass
  attaches; a cold query of a hypertable maps its database's catalog blocks
  as well as its chunks.
- **The cache and the replay, widened: a table read across identities.**
  **pgdt-query**, `dump.rs` (a table's blocks: its own and its descendants',
  the catalog's verdict on each, the refusals and findings) and
  `stream/replay/plan.rs` (each block projected to the parent's columns);
  `options.rs` holds the mode.
- **Failure, widened.** **pgdt-core**, `error.rs`: the refusals — a descendant
  that cannot be read, compressed rows, a catalog whose shape pgdt cannot read,
  a partition written through its root — each naming its remedy in the
  library's terms and spelled by each front end (D107).
- **The front ends, widened.** **datafusion-pgdump**: `pgdump.inheritance` and
  the mode on a dump's options. **datafusion-cli-pgdump**: the
  `:inheritance=` suffix. **pgdt**: `--inheritance` on `query`, and `info`'s
  logical model (`info/text.rs`, `info/json.rs`).
- **Fixtures**: `scripts/generate_fixtures.py`, a TimescaleDB flag set over a
  `scripts/fixture_schema_timescaledb.sql`, and an inheritance schema beside
  `fixture_schema_partitions.sql`; the tree enumerated by **pgdt-fixtures**.

## Slice order

Evidence first, each slice making the next one's mistakes visible: the dumps
the catalog is read from exist before the reader does, and the generic
mechanism lands and is checked on plain PostgreSQL before TimescaleDB leans on
it. Slice 2 stands alone because it reworks a tested core path, every parent's
answer changing before slice 3 offers the way back, which costs no user anything
between releases; slice 4's `info --json` is what slice 5 is checked against,
so the administrator's view waits until the semantics are settled. Each refusal
P38 leaves a valid dump with is a `KD<k>`, stance (c), its Future item named as
what promotes it, registered by the slice that adds the refusal.

1. **Evidence: the fixtures and their oracle.** The TimescaleDB generator at
   each pin, the `-n public` dumps, the inheritance schema (multi-level,
   multiple inheritance, a child declaring its own column, a parent with rows
   of its own), and each server's answers committed (D71). It confirms or
   corrects every shape in "Facts the grilling found" — read from source, not
   from a dump — and opens `timescaledb-invariants.md` with what it confirmed.
   No pgdt code.
2. **A table reads its descendants.** The walk; a table's blocks across
   identities, projected; a descendant's refusal refusing its parent; the
   load-via-root refusals, with a `KD<k>` (c) at the refusal naming the Future
   item; in the library and every front end; checked against slice 1's plain
   oracle. A rework of a tested core path, so alone.
3. **`--inheritance`.** The mode through `QueryOptions`, `pgdump.inheritance`,
   `--source …:inheritance=`, `pgdt query --inheritance`, with each front end's
   remedy spellings naming it.
4. **The catalog recorded.** The model's rows, the pass recording them, the era
   recipes and the version fingerprint, under a `CACHE_FORMAT_VERSION` bump;
   `pgdt info --json` showing them. No query reads them yet, so each pin's
   recipe is checked against its fixture's catalog before anything is refused
   on it.
5. **Hypertables resolved.** Compressed and partially compressed chunks
   refused at the hypertable and by name under both modes, with a `KD<k>` (c)
   naming the decoding Future item; dropped rows skipped; absent chunks read as
   absent with their finding; an unreadable catalog refusing its hypertables;
   every pin checked against its oracle.
6. **`pgdt info`'s logical model.** Parents and their descendants, the
   TimescaleDB section, continuous aggregates, chunks folded.
7. **The record.** The README's supported/unsupported list, the manual,
   [`pg-dump-compatibility.md`](pg-dump-compatibility.md)'s rows for
   inheritance, partitions and TimescaleDB, and a hand run of
   `SELECT count(*) FROM datanommer2.public.messages` on the reference dump
   against its catalog's chunks, its line limit raised past the longest line,
   noted in the slice's notes, not a test.

## Facts the grilling found

The facts the grilling worked from, drained from the phase's inbox. TimescaleDB
behaviour below was read from its source at every release tag through 2.31.0
(2026-10-20) and is **unconfirmed by a dump** except where the reference dump
shows it; slice 1 confirms or corrects it. The survey's full report is
machine-local, `runs/p38-timescaledb-survey/report.md`, and it read `tsl/`: it
is no spec for anything P38 builds or a decoder after it.

### pgdt today

- **A table can already be backed by several `COPY` blocks**: identity is
  `TableName { database, schema, table }`, `DumpIndex::blocks_of` returns a
  name's blocks in file order, and a plan reads them all, refusing with
  `TableColumnsDisagree` unless they name one column set in any order
  (`pgdt-query/src/stream/replay/plan.rs`). `--load-via-partition-root` blocks
  merge into their root so, their `partition_root` recorded on the block.
- **What the preamble keeps of a table**: columns, `parents` (qualified),
  `of_type`, `not_null`, `checks` (a name and `no_inherit`, not the expression)
  and `partition_of`, its bound as text, folded from the `ALTER TABLE ONLY …
  ATTACH PARTITION` every `pg_dump` writes (I86). An extension keeps a name and
  schema, no version. Triggers and `CREATE SCHEMA` are unparsed spans with
  their TOC header. All of it is cached; no row of any table is.
- **Tables are listed in three places**: DataFusion's catalog (`register_dump`
  builds a catalog per database, a schema per PostgreSQL schema, every
  provider up front; `SHOW TABLES` and `information_schema` list them),
  `pgdt query --table` through `TableSelector` (an unstated schema matches
  any), and `pgdt info`, per `COPY` block.
- **A zero-column `COPY` header** (`COPY t  FROM stdin;`) is read already
  (`pgdt-query/tests/stream.rs`); TimescaleDB 2.13 writes three.

### TimescaleDB in a dump

- **A chunk is an inheritance child of its hypertable** (`src/chunk.c`,
  `ts_chunk_create_table`), so `pg_dump` writes `CREATE TABLE … (CONSTRAINT
  constraint_<slice> CHECK (…)) INHERITS (<hypertable>)`; inherited columns
  are not repeated. A chunk's schema and prefix are configurable, a chunk can
  be renamed or moved, and from 2.21 `attach_chunk` adopts any table: **the
  catalog, not the name, says which children are chunks**. A release build
  never makes a hypertable a declaratively partitioned table. The root is
  dumped, normally empty: through 2.22 its `ts_insert_blocker` trigger refuses
  rows reaching it, and from 2.23 nothing does. Chunks are not extension
  members, so their DDL and data dump as any table's.
- **The catalog reaches the dump as `COPY` rows** through
  `pg_extension_config_dump`, even under `--schema-only`, with explicit column
  lists and `regclass` values fully qualified (`pg_dump` dumps under
  `search_path = ''`): `hypertable` (`id >= 1`), `dimension`,
  `dimension_slice`, `chunk`, `compression_settings`, `continuous_agg*`,
  `bgw_job` (`id >= 1000`), `metadata` (`key <> 'uuid'`). Joining a chunk to
  its hypertable, range and compressed relation has three recipes:

  | TimescaleDB | chunk's table | its range | its compressed relation | dropped chunks |
  |---|---|---|---|---|
  | 1.0–2.27 | `chunk.schema_name`, `table_name` | `chunk_constraint` → `dimension_slice`, slices shared | 1.5–2.28: `chunk.compressed_chunk_id`, another `chunk` row | 1.6–2.25: `chunk.dropped` |
  | 2.28 | as above | `dimension_slice.chunk_id` | as above | — |
  | 2.29+ | `chunk.relid` | `dimension_slice.chunk_id` | `compression_settings.relid` → `compress_relid` | — |

  `chunk.status` bits (`src/chunk.h`): 1 compressed, 2 compressed unordered,
  4 frozen, 8 partially compressed. Ranges are half-open `bigint`s: Unix-epoch
  microseconds for time types, the integer itself for integer time,
  `INT64_MIN`/`MAX` for an open side; a hash dimension slices `[0,
  INT32_MAX)`. The internal compressed hypertables (`compression_state = 2`,
  1.x `compressed = true`) are no user table.
- **The version** is the `metadata` row `timescaledb_version` from 2.14;
  before it, fingerprinted from the catalog blocks' columns (`dropped`
  1.6–2.25, `status` 2.3+, `osm_chunk` 2.8+, `creation_time` 2.13+,
  `replication_factor` 2.0–2.13, `hypertable_compression` 1.5–2.13 against
  `compression_settings` 2.14+, `chunk_constraint` to 2.27, `relid` 2.29+) and
  DDL markers (`ts_insert_blocker` to 2.22, `_timescaledb_internal` against
  `_timescaledb_functions` at 2.12, `SET default_table_access_method =
  hypercore` 2.18–2.21, the edition suffix of `COMMENT ON EXTENSION`, which
  names an Apache build that cannot compress). `timescaledb_post_restore()`
  refuses a restore into another version from 2.14. The `-- Dumped from`
  header bounds the PostgreSQL major and so the TimescaleDB range.
- **A compressed chunk's rows are in another relation**: compressing truncates
  the chunk; the relation is a chunk of an internal `_compressed_hypertable_N`
  it inherits (1.5–2.13), a standalone `compress_hyper_<N>_<id>_chunk`
  (2.14–2.28), or `<chunk>_compressed` linked only through
  `compression_settings` (2.29+). Rows inserted after compression stay in the
  chunk (status bit 8). Its rows are batches whose non-segmentby columns are
  `_timescaledb_internal.compressed_data`, written in `COPY` as base64. The
  2.18–2.21 hypercore access method dumps the same way by default.
- **A continuous aggregate** dumps as a materialization hypertable
  (`_timescaledb_internal._materialized_hypertable_<N>`, with chunks of its
  own, possibly compressed), two internal views, and the user's `CREATE VIEW`:
  over the materialization table when materialized-only (the default from
  2.13), or that `UNION ALL` the aggregate's query over the raw hypertable past
  a watermark when real-time. The pre-2.7 format kept partial states as
  `bytea` and is gone from 2.25.
- **Some hypertables have no rows in any dump**: a multinode access node's
  chunks (2.0–2.13) and tiered storage's (OSM, 2.8+, cloud only) are foreign
  tables, DDL only. Policies are `bgw_job` rows. `timescaledb_toolkit` is a
  separate extension whose sketch types can be column types (not surveyed).
- **Plain-format restore is tested upstream** (`test/sql/utils/
  pg_dump_aux_plain_dump.sh`, `tsl/test/sql/compression_null_dump_restore.sql`).
  A dump taken with `-n`/`-t` lacks what recreates a hypertable, per
  TimescaleDB's documentation.

### Licensing

- TimescaleDB's `LICENSE` puts `tsl/` under the Timescale License
  (`tsl/LICENSE-TIMESCALE`, posted 2020-09-24) and the rest under Apache-2.0.
  Every codec, simple8b RLE and `compressed_data`'s I/O are under
  `tsl/src/compression/`; the catalog DDL (`sql/`), the status bits
  (`src/chunk.h`) and the bit array (`src/adts/bit_array.h`) are Apache.
- The TSL defines a Derivative Work as a modification or enhancement of TSL
  software (§3.3), permits one to be distributed in binary form only and only
  inside a Value Added Product, one not primarily database storage or
  operations (§2.1(b), (d); §3.10), permits source to be redistributed only
  unmodified (§2.1(c)), and binds whoever installs or uses the software. It
  has no clause on reverse engineering or reimplementing a format. Timescale,
  Inc. is assignee of US 10,936,562, "Type-specific compression in database
  systems"; its claims were not read. A web search found no decoder of
  TimescaleDB's compressed format outside TimescaleDB.

### The reference dump

A full streaming pass over `datanommer2-2026-10-01.dump.xz` (machine-local
report: `runs/datanommer-survey/report.md`):

- `pg_dump` and server 13.23, database `datanommer2`, TimescaleDB **2.13.x**
  fingerprinted (`metadata` empty; `chunk.creation_time` present;
  `hypertable_compression` and the multinode tables present;
  `_timescaledb_functions` in use). No `timescaledb_pre_restore()` call.
- 2.43 TiB decompressed in 761 `COPY` blocks, 99.35% of the bytes in chunks.
  One hypertable, `public.messages` (14 columns), one time dimension on
  `"timestamp"` (`timestamp without time zone`), 7-day interval. 728 chunks,
  every one `status` 0: none compressed, dropped or tiered; no continuous
  aggregate, no job.
- Each chunk is `CREATE TABLE _timescaledb_internal._hyper_1_<n>_chunk
  (CONSTRAINT constraint_<n> CHECK (…)) INHERITS (public.messages);`, with
  per-chunk defaults, `<n>_<k>_<name>` primary-key and unique constraints and
  indexes in post-data. The root's block is empty and carries
  `ts_insert_blocker`. Chunks are dumped in name order, not time order.
- The `.xz` is 106,071 single-block streams of 24 MiB + 4 KiB whose block
  headers record no sizes — over three times koji's multistream download's
  stream count, which bears on what reading it through the `.xz` costs before
  any row.
