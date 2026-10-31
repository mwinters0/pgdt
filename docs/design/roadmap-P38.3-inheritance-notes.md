# P38.3 — `--inheritance`: notes

What 38.3 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md); the mode's shape is
D132, the refusals it adds are under `KD158`, and its oracle check is
`crates/pgdt-query/tests/descendants.rs`, now holding every table to the
server's `only` answers as well as its read-through ones, with
`crates/pgdt/tests/descendants.rs` holding `pgdt query --inheritance` and
`pgdt sql`'s `:inheritance=` and `'pgdump.inheritance'` to the same at 18.

## What the next slices inherit

- **A refusal is settled per mode, beside the blocks**: `Refusals` in
  `dump/descendants.rs` holds one for a read with descendants and one for a
  read alone, and `Table::read` picks by `QueryOptions::inheritance`. 38.5's
  compressed-chunk refusal refuses under both, so it goes in both fields; its
  unreadable-catalog refusal refuses the hypertable read through, and its
  chunks are read under their own names under either mode.
- **A partitioned table read alone** reads no row, under the schema its read
  with its descendants settles (38.3.1 for the load-via-root case). A
  hypertable root holds a block of its own, however empty, so `only` reads
  it as that block (38.5's stray root rows included).
- **The mode is a dump's in the provider**: `PgDumpOptions::inheritance`,
  `:inheritance=` and the `STORED AS PGDUMP` option `'pgdump.inheritance'`,
  read at registration, so a catalog lists a table by its schema under that
  mode. There is no `SET pgdump.inheritance` (D132).
- **A refusal raised under `only` names the descendants mode as its
  remedy**: `PartitionThroughRoot { inheritance_only: true }`, spelled
  `--inheritance descendants`, `:inheritance=descendants` and
  `'pgdump.inheritance' 'descendants'`.
- **The manual is untouched**: `--inheritance`, `:inheritance=` and
  `'pgdump.inheritance'` are documented in the binaries' help alone until
  38.7 writes the manual.

## Negative results

- **The cold pass is unchanged**: a parent's runs to the file's end and, at
  the metadata level under a typed query, re-reads its descendants for their
  census, under `only` too, the table it hands back being read under either
  mode (D132).
