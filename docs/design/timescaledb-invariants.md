# TimescaleDB invariants we rely on

Every entry is a property of what TimescaleDB writes into a plain `pg_dump`,
or of what its server answers over a restore of one, that a decision treats as
guaranteed. Each records the claim, the source that proves it, the releases it
was verified against, and how to re-verify it.

**This is [`postgres-invariants.md`](postgres-invariants.md)'s sibling**, and
that file does not hold these: its trigger is a PostgreSQL release, and
TimescaleDB's catalog changes on its own schedule, read by pgdt at every
release and not only at the pinned ones. **Walk this file at each new
TimescaleDB release**: move the newest pin in `scripts/generate_fixtures.py`'s
`TIMESCALEDB_PINS`, regenerate (`cd scripts && uv run generate_fixtures.py
--schema timescaledb --skip-oracle --skip-floor --skip-values`), read the diff of
`fixtures/<major>/timescaledb/`, then run each entry's `Re-verify` against the
new tag.

**Every proof is TimescaleDB's Apache-licensed source** (`src/`, `sql/`), its
documentation, or a dump the fixture generator produced — **never `tsl/`**
([`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "TimescaleDB").
What only `tsl/` would prove is stated as observed, from a fixture.

**A site relying on an entry carries a `timescaledb: TS<n>` comment**, and the
entry's "Relied on by" names that site's function, as `pg-refuses: I<n>` and
`scripts/pg_refuses.py` hold `postgres-invariants.md`'s; the script resolving
them lands with the first site. Until then "Relied on by" names the decision
in the phase spec.

Source: the bare clone at `/mnt/wd12t/upstream/timescaledb/`, every release tag
present, read with `git show <tag>:<path>`. **Identifiers are `TS<n>`**,
allocated on discovery and never reused; **`TS1`–`TS10` are allocated**.

---

## TS1 — A chunk is an inheritance child of its hypertable, never a partition

**Claim.** Every chunk, a continuous aggregate's materialization chunks
included, is dumped as `CREATE TABLE <chunk> (<its CHECK constraints>) INHERITS
(<hypertable>);`, inherited columns omitted. A release build never makes a
hypertable a declaratively partitioned table.

**Proof.** `src/chunk.c`, `ts_chunk_create_table`: `stmt.inhRelations =
list_make1(makeRangeVar(<hypertable>))`. `src/partition_chunk.h`,
`is_partitioning_allowed`, reads a GUC `src/guc.c` defines only under
`TS_DEBUG`. Observed at every pin: `INHERITS (public.metrics)` on each of
`metrics`' chunks, `INHERITS (_timescaledb_internal._materialized_hypertable_<N>)`
on each materialization chunk.

**Scope limit.** The catalog, not the name or the schema, says which children
are chunks: a chunk's schema and prefix are `create_hypertable` arguments, and
from 2.21 `attach_chunk` adopts any table.

**Verified against:** 2.13.1, 2.21.4, 2.27.2, 2.28.3, 2.31.0 (the pins).
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"The generic mechanism" — a hypertable read through `INHERITS`.
**Re-verify:** `git show <tag>:src/chunk.c | grep -n inhRelations`; `grep -c
'INHERITS (public.metrics)' fixtures/<major>/timescaledb/default.sql` is 6.

---

## TS2 — The catalog reaches a dump as `COPY` blocks, by name, unless the extension is not dumped

**Claim.** The catalog tables are extension configuration tables, so a dump of
a database holding the extension carries their rows as `COPY
_timescaledb_catalog.<table> (<every column, named>) FROM stdin;`, `regclass`
values schema-qualified. A dump taken with `--schema` or `--table` dumps no
extension, and so neither `CREATE EXTENSION timescaledb` nor any catalog row,
unless `_timescaledb_catalog` is itself selected or (from `pg_dump` 14)
`--extension` names it. Excluding the chunks' schema keeps the catalog.

**Proof.** `sql/pre_install/tables.sql`'s `pg_extension_config_dump` calls.
PostgreSQL `src/bin/pg_dump/pg_dump.c`: `selectDumpableExtension` (no extension
when `include_everything` is false), `processExtensionTables` (a configuration
table of an undumped extension only where its table or schema is selected;
`COPY`'s explicit column list). Observed: `schema-public.sql` at 13 and 18 holds
two `COPY` blocks, the hypertables' roots; `exclude-chunks.sql` holds the
extension and every catalog block.

**Scope limit.** With no extension and no catalog in the file, a `--schema`
dump of a hypertable is two plain tables with rows of their own and no
children, and is not recognisable as TimescaleDB's from its rows; through 2.22
its root still carries the `ts_insert_blocker` trigger (TS8).

**Verified against:** the pins; PostgreSQL 13.23 and 18.6's `pg_dump`.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — detection per database, and the catalog read by column name.
**Re-verify:** `git show <tag>:sql/pre_install/tables.sql | grep -n
pg_extension_config_dump`; `grep -c '^COPY' fixtures/{13,18}/timescaledb/schema-public.sql` is 2.

---

## TS3 — The version is stated by the `metadata` row from 2.14, and by the catalog's shape before it

**Claim.** From 2.14 the dump's `_timescaledb_catalog.metadata` block holds
`timescaledb_version` (its filter `WHERE key <> 'uuid'`, so `install_timestamp`
and `exported_uuid` too); through 2.13 it holds `exported_uuid` alone. The
pins' catalog blocks differ in exactly the columns
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "TimescaleDB in a
dump" lists for them: `chunk.dropped` and `hypertable.replication_factor` and
the multinode tables at 2.13; `compression_settings` with `compress_relid` at
2.21; `dropped` gone and `chunk_constraint` present at 2.27; `chunk_constraint`
gone and `dimension_slice.chunk_id` at 2.28; `chunk.relid` at 2.31. Every pin's
`COMMENT ON EXTENSION timescaledb` ends `(Community Edition)`.

**Proof.** `sql/pre_install/tables.sql` at 2.13.1 and 2.14.0: the `metadata`
filter; at each pin, the tables' columns. `sql/metadata.sql`. Observed in each
pin's `default.sql`.

**Scope limit.** An Apache-edition build is not pinned; `sql/comment_apache.sql`
states its comment. A catalog an update script rebuilt may order its columns
otherwise than a fresh install's (`sql/updates/2.25.2--2.26.0.sql` rebuilds
`chunk`); every pin is a fresh install.

**Verified against:** the pins.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — the version decides the recipe, a fingerprint before 2.14.
**Re-verify:** `git show <tag>:sql/pre_install/tables.sql | grep -n -A2
"config_dump('_timescaledb_catalog.metadata'"`; `awk '/^COPY
_timescaledb_catalog.metadata/,/^\\\./' fixtures/<major>/timescaledb/default.sql`.

---

## TS4 — `chunk.status` bit 1 is compressed and bit 8 partially compressed

**Claim.** A compressed chunk's `status` has bit 1; one holding rows inserted
after its compression also has bit 8.

**Proof.** `src/chunk.h`: `CHUNK_STATUS_COMPRESSED 1` ("a corresponding
compressed chunk exists"), `CHUNK_STATUS_COMPRESSED_PARTIAL 8`. Observed: the
four compressed `metrics` chunks are `1` and `9` at 2.21, 2.27, 2.28 and 2.31;
at 2.13 the chunks with a `compressed_chunk_id` are `1` and `9`.

**Verified against:** the pins.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — compressed and partially compressed chunks refused.
**Re-verify:** `git show <tag>:src/chunk.h | grep -n 'define CHUNK_STATUS'`.

---

## TS5 — A compressed chunk's rows are in another relation, and its own block is short of them

**Claim.** A compressed chunk's `COPY` block holds only rows inserted after its
compression; the rest are batches in its compressed relation's block, whose
non-segmentby columns are `_timescaledb_internal.compressed_data`. That
relation is a chunk inheriting `_timescaledb_internal._compressed_hypertable_<N>`
at 2.13, a standalone `compress_hyper_<N>_<id>_chunk` with a `chunk` row of its
own at 2.21–2.28, and `<chunk>_compressed` linked only through
`compression_settings` at 2.31. A chunk in the 2.18–2.21 `hypercore` access
method dumps the same way, preceded by `SET default_table_access_method =
hypercore;`. The server reads a compressed chunk by its name with every row, and
`ONLY` it with the block's.

**Proof.** Apache source states the links, not the layout:
`sql/pre_install/tables.sql` (`chunk.compressed_chunk_id`,
`compression_settings.compress_relid`), `sql/updates/2.13.1--2.14.0.sql` (`NO
INHERIT` of the compressed relations). The rest observed: each pin's
`default.sql` and `default.answers.tsv` (`_hyper_1_2_chunk`: 14 rows read
through, 0 `ONLY`; 2.21's `columnar` chunk an empty block, 7 rows read through).

**Scope limit.** The batches' format is not claimed and is not read
([`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "TimescaleDB").

**Verified against:** the pins.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — compressed rows refused wherever reached.
**Re-verify:** `grep -n -A3 'CREATE TABLE _timescaledb_internal.*compress'
fixtures/<major>/timescaledb/default.sql`, and the chunk's two answer lines.

---

## TS6 — A dropped chunk's catalog row survives only beside a continuous aggregate of the old format, past 2.14

**Claim.** `drop_chunks` keeps a chunk's `chunk` row, marked `dropped`, with
its table gone: through 2.14 when the hypertable has any continuous aggregate,
and from 2.15 only when one of them is unfinalized (the format before 2.7). The
column is gone from 2.26. A fresh install from 2.15 keeps none, since it cannot
create an unfinalized aggregate: only a database carrying one through an
upgrade holds a `dropped` row at 2.15–2.25.

**Proof.** `src/chunk.c`, `ts_chunk_do_drop_chunks`: `if (has_continuous_aggs)`
preserves the row at 2.13.1 and 2.14.0, `if (has_continuous_aggs &&
!all_caggs_finalized)` from 2.15.0. Observed: at 2.13 chunks 1 and 5 are
`dropped` rows; at 2.21 they have no row; 2.21 refuses `timescaledb.finalized =
false` ("Continuous Aggregates with partials is not supported anymore").

**Verified against:** the pins; the source at 2.13.1–2.21.4.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — a `dropped` row is no chunk.
**Re-verify:** `git show <tag>:src/chunk.c | grep -n 'preserve_catalog_row(chunks'`.

---

## TS7 — A continuous aggregate dumps as a materialization hypertable and views

**Claim.** A continuous aggregate's rows are its materialization hypertable's,
`_timescaledb_internal._materialized_hypertable_<N>`, held in its chunks; its
user view, `CREATE VIEW`, reads that table, and is listed by `continuous_agg`
with `materialized_only`.

**Proof.** `sql/pre_install/tables.sql`'s `continuous_agg`. Observed at every
pin: `metrics_daily` and `metrics_weekly` as `_materialized_hypertable_2` and
`_3`, 81 and 5 rows read through, none `ONLY`.

**Verified against:** the pins.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — an aggregate's rows read as its materialization hypertable's.
**Re-verify:** `grep -n '^COPY _timescaledb_catalog.continuous_agg '
fixtures/<major>/timescaledb/default.sql`, and its answers.

---

## TS8 — The root is dumped, refused rows through 2.22, and omitted by the server's reads after

**Claim.** A hypertable's root has a `COPY` block. Through 2.22 its
`ts_insert_blocker` trigger refuses a row reaching it; from 2.23 nothing does,
and a row there survives a dump and restore. **The server's default reads of
the hypertable omit the root's rows**: read through at 2.27, 2.28 and 2.31 it
answers its chunks' rows alone, and `ONLY` it answers the root's row at 2.27
and none at 2.28 and 2.31. Planning with
`timescaledb.enable_constraint_exclusion = off` reads it, at 2.31 too.

**Proof.** `sql/updates/2.22.1--2.23.0.sql` drops the trigger from every
hypertable. Observed: the root's row in `public.metrics`' block at 16, 17 and
18; their answers; the setting probed by hand on 2.31 over the same restore.

**Scope limit.** How a row reaches the root is not claimed; the fixture's comes
from an insert in restoring mode (`fixture_schema_timescaledb_root_rows.sql`).

**Verified against:** 2.27.2, 2.28.3, 2.31.0; the trigger at 2.13.1 and 2.21.4.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"The generic mechanism" — a table's rows its own and every descendant's.
**Re-verify:** `grep -c ts_insert_blocker fixtures/<major>/timescaledb/default.sql`;
`grep 'public.metrics' fixtures/<major>/timescaledb/default.answers.tsv | grep -v
$'\trow\t'`.

---

## TS9 — A restore is answered between `timescaledb_pre_restore()` and `timescaledb_post_restore()`, into the same release

**Claim.** A plain dump restores with `timescaledb.restoring` on, set by
`timescaledb_pre_restore()`, and `timescaledb_post_restore()` refuses a catalog
whose `timescaledb_version` is not the server's from 2.14. Its `\restrict` line
needs the dumping `pg_dump`'s own `psql`.

**Proof.** `sql/restoring.sql` ("catalog version mismatch"). Observed: every
pin's `default.sql` restores with no error.

**Verified against:** the pins.
**Relied on by:** `scripts/generate_fixtures.py`, `write_answers`.
**Re-verify:** `git show <tag>:sql/restoring.sql | grep -n mismatch`.

---

## TS10 — A catalog listing chunks the file lacks is refused or emptied by the server

**Claim.** A dump excluding the chunks' schema holds the catalog's rows for
chunks it does not hold. Restored, 2.13's server refuses the hypertable read
through ("relation `<chunk>` not found"); 2.31's restore refuses the catalog's
rows for them on its foreign keys, and the hypertable reads empty. Neither holds
the absent chunks' rows.

**Proof.** Observed: `exclude-chunks.answers.tsv` at 13 and 18.

**Verified against:** 2.13.1, 2.31.0.
**Relied on by:** [`roadmap-P38-descendants.md`](roadmap-P38-descendants.md),
"TimescaleDB" — absent chunks read as absent, with a finding.
**Re-verify:** `grep -v $'\trow\t' fixtures/{13,18}/timescaledb/exclude-chunks.answers.tsv`.
