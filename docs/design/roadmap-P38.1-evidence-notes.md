# P38.1 — The fixtures and their oracle: notes

What 38.1 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md); what TimescaleDB is
relied on for is [`timescaledb-invariants.md`](timescaledb-invariants.md),
`TS1`–`TS10`, opened here.

## What the next slices read

- **`fixtures/<major>/inheritance/`**, all six routine majors, `default` and
  `load-via-partition-root` (`scripts/fixture_schema_inheritance.sql`):
  inheritance three deep with rows at every level, a child in another schema,
  an empty child, a child of two parents, a merged column, and a range
  partition itself list-partitioned with a leaf attached in another column
  order, which `--load-via-partition-root` writes through the top root.
- **`fixtures/<major>/timescaledb/`** at the five pins, `default` at each and
  `schema-public`, `exclude-chunks` and (38.4's) `tables` at 13 and 18
  (`scripts/fixture_schema_timescaledb.sql`, plus 2.21's hypercore chunk and,
  from 2.27, a row in the root). Regenerate with `cd scripts && uv run
  generate_fixtures.py --schema timescaledb --skip-oracle --skip-floor
  --skip-values`, under two minutes.
- **`<flag-set>.answers.tsv` beside every dump of `inheritance`, `partitions`
  and `timescaledb`** (D130): per table, read through (`descendants`) and
  `ONLY` (`only`), one `rows <n>` line and a `row <COPY text>` line per row in
  the text's order, or one `error` line; a restore's errors open the file as `-
  - restore-error` lines. The tables are every one the restore holds but an
  extension's own, so TimescaleDB's catalog is read from the dump, never from
  the answers. Slice 2 holds its reads of `inheritance` and `partitions` to
  these; 38.5.1 its reads and refusals of `timescaledb`, beside the `held`
  lines 38.5 added.
- **Slice 2's reference cases**: `public.animal` reads 7 rows through and 1
  `ONLY`; `public.pet`, with no rows of its own, reads `cat`'s; `public.reading`
  reads 4 through its sub-partitioned `reading_2020`. `--load-via-partition-root`
  answers the same as `default` (the restore re-routes the rows), so a partition
  read by its name under it is held to a server that placed the rows itself.

## What the dumps corrected

Each is filed at its invariant; what bears on a later slice:

- **A `--schema=public` dump holds no `CREATE EXTENSION` and no catalog**
  (TS2), so the spec's `-n public` fixture does not hold "catalog rows for
  absent chunks". `exclude-chunks` (`--exclude-schema=_timescaledb_internal`)
  is that shape; `schema-public` stays as the dump TimescaleDB cannot be
  detected in by either of the spec's two paths, its roots read as plain tables.
- **The server never answers a hypertable's stray root rows by default**
  (TS8): read through, 128 rows where the root holds a 129th. A table's rows
  being its own and every descendant's gives 129, which the spec keeps, with a
  finding; 38.5's `held` lines are the root's own rows.
- **Absent chunks are not read as absent by the server** (TS10): 2.13 refuses
  the hypertable, and 2.31's restore drops their catalog rows and reads it
  empty. The spec's "read as absent, with a finding" answers what neither does.
- **No pin after 2.14 holds a `dropped` row** (TS6): 2.15–2.25 keep one only
  for an unfinalized aggregate, which 2.21 refuses to create, so only an
  upgraded database holds one; 2.13's fixture is the one that does.
- **Internal compressed hypertables refuse a read** at 2.21–2.28 ("chunk
  `compress_hyper_…` has no dimension slices"), as the spec's "no user table"
  expects; a compressed chunk read `ONLY` answers its own block's rows (TS5).
- **Zero-column `COPY` headers** (`_timescaledb_cache.cache_inval_*`): three at
  2.13, two at 2.21, one at 2.27 and 2.28, none at 2.31.

Confirmed as the spec states: TS1's inheritance shape, TS3's columns per pin
and the version row from 2.14, TS4's status bits, TS5's three compressed
relation layouts and hypercore's dump, TS7's materialization hypertables, the
insert blocker through 2.22, and Community Edition comments at every pin.

## Negative results

- **The `-ha` image restarts its server after initializing**, so a connection
  made on its first `pg_isready` is terminated; the generator waits for
  `postgres` as process 1 (`wait_final_server`). Its template database carries
  `timescaledb_toolkit`, which the schema drops.
- **The restore needs the routine `pg_dump`'s own `psql`**: the `-ha` image's
  older minor does not read `\restrict`.
- **2.13 cannot sort a compressed hypertable's rows by their text** ("could not
  find pathkey item to sort"); the answers are sorted by the generator.
- **Not fixtured**: multinode foreign chunks (2.0–2.13, needing data nodes) and
  tiered storage's foreign chunk (cloud only), both read from source alone; an
  Apache-edition build; an upgraded install's catalog.
- **`partitions`' dumps were regenerated** to take their answers, and OID drift
  moved `feel`'s, `shuffle`'s and `spread`'s rows between their partitions'
  blocks (D69).
- **`persisted_index_is_pinned_to_the_format_version` was re-pinned** with no
  `CACHE_FORMAT_VERSION` bump: its digest covers every fixture's path and index,
  so new fixtures move it while the same dump saves the same bytes.
