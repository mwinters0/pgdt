# P38.2 — A table reads its descendants: notes

What 38.2 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md); the mechanism's
shape is D131, its one refusal `KD158`, and its oracle check
`crates/pgdt-query/tests/descendants.rs` over 38.1's `inheritance` and
`partitions` answers at every major, with `crates/pgdt/tests/descendants.rs`
holding `pgdt query` and `pgdt sql` to the same answers at 18.

## What the next slices inherit

- **Where a read's blocks are chosen**: `pgdt-query`'s `dump/descendants.rs`,
  `table_read`, called by `Dump::table` and by a cold query's pass alike. A
  refusal decided there is carried on the `Table` and raised by `schema`,
  `summary`, `divergences` and `plan`, so the table stays listed and
  addressable (the DataFusion catalog then leaves it unlisted with its
  `RefusedTable` finding, as for any table whose provider refuses). 38.5's
  compressed-chunk and unreadable-catalog refusals belong in `Refused`,
  today the through-root case alone.
- **Own blocks and descendants' are told apart by the header's name** against
  the table's (`TableColumns::settle`, `read_schema`), never stored. 38.3's
  `only` is the table's own blocks alone, a root's carrying `partition_root`
  holding its partitions' rows rather than its own (38.3.1).
- **A cold query's pass takes no `QueryOptions`** (D48): it runs to the end
  for any table the DDL gives a descendant (`reads_descendants` in
  `dump/settle.rs`), whatever the mode. Under 38.3's `only` that end is not
  needed; whether the pass learns the mode is 38.3's call.
- **The schema a read reports is the table's**, its first own block's or, with
  none, its declared columns resolved as its own: a descendant's stricter
  `NOT NULL` (the `emitters` schema's `child.label`) is that block's alone,
  still refusing a NULL in the child's rows.
- **A table is listed where a block holds its rows or a descendant's**, so a
  partitioned table appears at its first descendant's block; a declared table
  nothing below holds a block for (a partitioned table with no partition, a
  schema-only dump's) is still unlisted. 38.6's `info` model reads the same
  walk (`DatabaseMetadata::descendants`, `ancestors`).

## Negative results

- **A generated column is no column of a read here**, the dump carrying no
  value of one (I5), where the server's answers compute it: `public.derived`
  answers two columns and reads none. The oracle test exempts it by name; it
  predates P38 and no slice of it owns it.
- **The other schemas' answers moved only at a parent**: `emitters.parent` now
  reads its child's rows, `objects.events` and `statistics`' `spans` are listed
  and read through their partitions, and the partitions a dump writes through
  their root are refused, which each sweep over every fixture (`pruning`,
  `partitioned_replay`, the provider's, the statistics' and the dynamic
  filter's) now skips.
- **A `--load-via-partition-root` block's partition is its TOC entry's name**;
  a block no TOC entry names (`KD1`'s `--disable-triggers` dump) refuses every
  table below its root instead, none being told apart.
