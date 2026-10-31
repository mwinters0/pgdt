# P38 notes — a table reads its descendants; TimescaleDB under its own names

What the phase leaves the next one, and what it tried and refused. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md); the mechanisms'
decisions are D130–D136 in [`decisions.md`](decisions.md), what they rely on of
PostgreSQL I93 in [`postgres-invariants.md`](postgres-invariants.md) and of
TimescaleDB TS1–TS10 in [`timescaledb-invariants.md`](timescaledb-invariants.md);
each crate's part is [`concepts.md`](concepts.md). The per-slice notes went at
the wrap, and git holds them at `a7c88adf`. Each slice's decision reached the
register in the slice that made it or sits beside its mechanism in code. The
refusals the phase leaves a valid dump with are `KD158`–`KD161`, each `(c)
unowned` and naming what promotes it; no `KD<k>` was owned by P38. `M266` and
`M267`, admitted during it, block nothing.

## What the next phase inherits

### The fixtures and their oracle

- **`fixtures/<major>/inheritance/`** at all six routine majors, `default` and
  `load-via-partition-root` (`scripts/fixture_schema_inheritance.sql`), and
  **`fixtures/<major>/timescaledb/`** at the five pins, `default` at each and
  `schema-public`, `exclude-chunks` and `tables` at 13 and 18. Regenerate the
  second with `cd scripts && uv run generate_fixtures.py --schema timescaledb
  --skip-oracle --skip-floor --skip-values`, in minutes.
- **`<flag-set>.answers.tsv` beside every dump of `inheritance`, `partitions`
  and `timescaledb`** (D130), taken again over committed dumps by
  `--answers-only`, which reproduces every existing line byte for byte. A
  hypertable's `held` line is its root read `ONLY` under
  `timescaledb.restoring = on` (TS8), equal to its root's own block at every
  pin and flag set; `exclude-chunks` holds `held` lines for the user
  hypertables alone, `schema-public` none.
- **A dump and its answers come from one run.** A full regeneration re-dumps,
  and chunk `creation_time`s and OIDs drift run to run, so a new flag set at a
  pin is generated alone, `--schema <schema> --version <major> --flag-set
  <name>`, its siblings' dumps and answers untouched. `partitions`' regeneration moved `feel`'s, `shuffle`'s and
  `spread`'s rows between their partitions' blocks (D69).
- **Not fixtured**: multinode foreign chunks (2.0–2.13) and tiered storage's
  foreign chunk, both read from source alone; an Apache-edition build; an
  upgraded install's catalog. Only 2.13's fixture holds a `dropped` row (TS6),
  and no pin before 2.3's `status` column, where a chunk is taken as
  compressed by naming a compressed relation.

### Behaviour a later phase meets

- **A generated column is no column of a read** (I5), where the server's
  answers compute it: the oracle test exempts `public.derived` by name.
- **A block no TOC entry names** (`KD1`'s `--disable-triggers` dump) refuses
  every leaf below its root, and its root's finding names no partition.
- **`pgdt query` prints no file-level diagnostic**; a read hears the
  hypertables' findings as its plan's notes (D135), and what refuses a read
  as its error (D136).
- **`pgdt info`'s parent line counts what the file holds and says no
  refusal**: what refuses is a file-level finding. A rewording guarding
  against reading the count as an answer waits on someone misreading it, the
  CLI-feedback pass's to hear (`STATUS.md`, "Not started"). `--json` states
  the catalog's conclusions and the DDL's `parents` and `partition_of`, and no
  parent's rows summed over its descendants (D67).
- **The version is fingerprinted from the catalog's columns and tables
  alone**; the DDL markers the spec's facts list are not read (the reasoning
  is at `version`, `timescaledb/version.rs`).
- **A refusal's precedence under one mode** is written through a root
  (`KD158`), then compressed, then an unreadable catalog; no fixture meets two.
- **The findings are derived on load**, `DumpIndex::diagnostics` not being
  persisted, so a new `DiagnosticKind` needs no `CACHE_FORMAT_VERSION` bump.
  A cold query's partial map carries none.
- **Nothing refuses one path attached twice**: two `--source`s naming one file
  under two catalog names read the one cache, each under its own mode.

### The hand run on the reference dump

**`SELECT count(*) FROM datanommer2.public.messages` answers 530,657,112**,
the catalog's chunks' rows by the survey's per-block counts
(`runs/datanommer-survey/tables.tsv`: 728 blocks naming `public.messages` their
hypertable, the root's own block none). `pgdt info` agrees on every line the
spec's logical model names: `public.messages: 728 descendant(s), 530657112
row(s) in all`; TimescaleDB 2.13 inferred from its catalog's columns, one
hypertable of 728 chunks, all uncompressed, none absent; the chunks' blocks
folded under the root; no finding but TOC coverage. Its total over all 761
blocks, 1,025,133,886 rows, is the survey's own count of data lines, a separate
program's. Nothing on the dump refuses, so it could not show a parent's line
beside a refusal.

The run was `pgdt parse --max-line-bytes 536870912` at the data level, then
`pgdt info`, then the count in `pgdt sql` under `SET pgdump.max_line_bytes`,
the query stating the limit again because the cache carries none
([`roadmap-P33-line-bound-inbox.md`](roadmap-P33-line-bound-inbox.md), "The
refusal names an offset and a limit, and the parse saves nothing after it");
each in `scripts/contained.py --memory 12g`, built at `334ed472`.
`runs/p38.7-datanommer-count-20261031-1224/` holds `run.sh`, the binary,
`parse.log`, `info.txt` and `count.csv`. Its wall clock is untimed and no
figure; by `parse.log`'s timestamps most of the parse was the scan, after the
`.xz`'s seek table was built over its 106,071 streams, at the `jobs=24` the
source recommended. The data-level cache it wrote is kept beside the link it
read through, `/mnt/ssd/fedora/scratch/pgdump_query/datanommer/`, for any later
hand run on the dump.

## Negative results

### Generator and fixtures

- **The `-ha` image restarts its server after initializing**, so a connection
  made on its first `pg_isready` is terminated; the generator waits for
  `postgres` as process 1 (`wait_final_server`). Its template database carries
  `timescaledb_toolkit`, which the schema drops.
- **The restore needs the routine `pg_dump`'s own `psql`**: the `-ha` image's
  older minor does not read `\restrict`.
- **2.13 cannot sort a compressed hypertable's rows by their text** ("could not
  find pathkey item to sort"); the generator sorts the answers.
- **`persisted_index_is_pinned_to_the_format_version` moves with new
  fixtures**: its digest covers every fixture's path and index, so it was
  re-pinned with no `CACHE_FORMAT_VERSION` bump.
- **Zero-column `COPY` headers** (`_timescaledb_cache.cache_inval_*`): three at
  2.13, two at 2.21, one at 2.27 and 2.28, none at 2.31.

### Reads and refusals

- **A worker-cut block records no catalog row**: with the leader's decline
  removed, the parallel test loses the catalog (D133's "on the leader's loop").
- **No hypertable is partitioned** (TS1), so the load-via-root path never
  meets one.
- **The internal compressed hypertables need nothing of their own**: at 2.13
  their compressed relations inherit them and read as their batches; from
  2.21 nothing inherits them and they read as their own empty block, where
  the server refuses them ("no dimension slices").
- **2.21's hypercore chunk (`public.columnar`'s) is marked compressed**, so it
  refuses like any compressed chunk, though its server answers its rows
  `ONLY`.
- **Every answer matched in both schema modes**, a compressed relation's
  `compressed_data` columns read as text included.
- **"Holds a chunk" is the DDL's walk** (D134), not the catalog's list, which
  under an unreadable catalog does not read.

### Findings and notes

- **A root holding no row raises nothing**: every root before 2.23 and most
  after hold none, where pgdt's answer and the server's agree.
- **A named hypertable with no chunk held, read under an unreadable catalog,
  raises no `CompressionUnchecked`**: it reaches no table that may be a chunk;
  what is unknown is what it lacks, `HypertableChunksUnknown`'s alone.
- **A through-root finding names the partitions its blocks hold**, from their
  TOC entries, not every table below the root: the partitioned tables between
  hold no block.
- **A compressed chunk the file lacks is in no finding**: it refuses nothing
  (D134), and the absent-chunks finding names it.
- **A through-root block counted by its header's name** leaves a partitioned
  table below the root holding no row, its partitions' rows all counted to the
  root; hence `DumpIndex::blocks_held`, which `info` and a read share.
- **A code span in an invariant's "Relied on by" that is a bare identifier
  names a site** for `timescaledb_invariants.py`, so prose naming a column or a
  keyword spells it otherwise (`` `chunk.dropped` ``, `` `INHERITS (…)` ``).
