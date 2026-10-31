# P38.6 — `pgdt info`'s logical model: notes

What 38.6 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "What is shown,
claimed and checked"; the code is `crates/pgdt/src/info/logical.rs`. Checked
by `pgdt`'s `tests/descendants.rs`: each parent's rows held to its server's
answer read with its descendants in both inheritance flag sets, and the
TimescaleDB section and the folding at 2.31 and 2.13, over `exclude-chunks`,
`tables` and a catalog broken by a renamed column.

## What 38.7 reads

- **The listing opens with two sections, ahead of the blocks**:
  `tables with descendants:`, one line per table the DDL names a child of,
  its descendant count and the rows it and they hold; then per TimescaleDB
  database `TimescaleDB: <release>`, a line where the extension is not
  created, a line per hypertable in `id` order, internal ones marked, and a
  line per continuous aggregate. An unreadable catalog prints one line
  naming the hypertables its rows name. `pgdt parse` closes on the same
  listing.
- **A hypertable's chunks are counted whole, then split into uncompressed,
  compressed and absent from the file**, compressed counted among the chunks
  the file holds: one the file lacks refuses nothing (D134). Partially
  compressed chunks are counted within compressed. Without its database's
  DDL, absent is not concluded and the split omits it.
- **Rows are attributed by `DumpIndex::blocks_held`**, moved into the model
  from `pgdt-query`'s `dump/descendants.rs`, so `info` and a read agree on
  which partition a block written through its root holds.
- **Folding**: a chunk's block, and a compressed relation's that is no chunk
  of its own (2.29 on), fold into one line beneath their hypertable's first
  block, counting blocks and rows, unless the listing names each block's
  unchecked lines (`--detail`, a strict `parse`). Through 2.28 the compressed
  relations are an internal hypertable's chunks and fold under it. A block
  stays listed where its hypertable has no block, or where it holds fields a
  parse went past. The totals still count every block.
- **`--json` is unchanged**: it states the catalog's conclusions (38.4) and
  the DDL's `parents` and `partition_of`; a parent's rows summed over its
  descendants would be a rollup D67 refuses.

## Negative results

- **A parent's line says nothing about refusals**, for a compressed chunk
  (`KD159`) or a table written through its root (`KD158`): `info` reports
  the cache and builds no `Dump` (D61), and the refusals are the query
  layer's. Under STATUS's "Decisions worth another look".
- **A through-root block counted by its header name leaves a partitioned
  table below the root holding no row**, its partitions' rows all counted to
  the root; hence `blocks_held`.
