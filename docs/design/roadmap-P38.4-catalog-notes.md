# P38.4 — The catalog recorded: notes

What 38.4 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md); where the rows live
and why is D133; what is relied on of TimescaleDB is
[`timescaledb-invariants.md`](timescaledb-invariants.md), its sites now
carrying `timescaledb: TS<n>` markers that `scripts/timescaledb_invariants.py`
resolves both ways. The pins are checked by `pgdt-mapping`'s
`tests/timescaledb.rs`; `pgdt info --json` by `pgdt`'s `tests/descendants.rs`.

## What the next slices read

- **`TimescaleDb::of(index, database)`** is the one read: `None` where the
  database holds neither the extension nor a catalog block, else the version
  (`Stated`, `Inferred` era, or `Unknown`) and a `Resolution` — `Resolved`
  hypertables, chunks and continuous aggregates, or `Unreadable` naming the
  columns its recipe lacks and the fields that do not parse. 38.5.1 refuses on
  `Unreadable`; it carries no hypertable list, so which tables that refuses is
  38.5.1's to settle, the `hypertable` rows' names being readable wherever their
  columns are.
- **Internal compressed hypertables are listed**, `internal: true`. Through
  2.28 their chunk rows are the compressed relations; at 2.13 those tables
  inherit the internal hypertable (TS5), so a walk over `INHERITS` meets them.
  A user hypertable's chunks are its non-dropped, non-internal rows.
- **A dropped row is listed** with `dropped: true` and names no table (2.13).
  An absent chunk is listed like any other; only the DDL's
  `DatabaseMetadata::tables` tells it is absent (`exclude-chunks`).
- **A chunk's `compressed_relation` is set exactly where status bit 1 is**,
  at every pin; a chunk's slices are one per dimension of its hypertable.
- **The rows are on the blocks a pass mapped**, so a map stopped before the
  catalog concludes from what it holds; a cold query of a parent already runs
  to the end (D131).
- **`tables`** (`--table` over `public`, the chunks' schema and the catalog's)
  at 13 and 18 is the dump holding the catalog without the extension. Its
  restore answers with no error, the generator creating the extension first,
  and its catalog concludes what `default`'s does.

## Negative results

- **The fixture was generated alone**: the generator regenerates every flag
  set of a pin, and the other three's regenerated dumps were reverted, chunk
  `creation_time`s and OIDs drifting run to run while each committed dump
  stays beside the answers its own run took.
- **A worker-cut block records no row**: with the leader's decline removed,
  the parallel test loses the catalog. Hence D133's "on the leader's loop".
- **The fingerprint reads catalog columns and tables alone**, as the spec's
  settled text states; the DDL markers its facts list (`ts_insert_blocker`,
  the function schema, the extension comment) are not read.
- **Before 2.3 there is no `status`**: a chunk is taken as compressed where it
  names a compressed relation. No pin holds that shape.
- **A code span in an entry's "Relied on by" that is a bare identifier names
  a site**, so prose naming a column or a keyword spells it otherwise
  (`` `chunk.dropped` ``, `` `INHERITS (…)` ``).
