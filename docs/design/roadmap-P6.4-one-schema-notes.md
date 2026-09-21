# P6.4 — One schema per table: notes

The fourth slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md),
"One schema per table". The rule lives in the library, not in the provider:
`stream.rs`'s `TableColumns` settles a table's one column order and its census
once per plan, and every `table_stream` and `table_stream_partitions` caller —
`pgdt query` included — gets batches in that order. The external fact it rests
on, a leaf's `COPY` list in the leaf's own order, is
[`postgres-invariants.md`](postgres-invariants.md), "I5".

## What 6.5 and later slices inherit

- **Every batch of a table has one schema.** An unprojected query projects
  the table's order, so a block listing `(b, a)` feeds `a` and `b` into the
  table's positions through `field_targets`. A caller's projection is still
  the caller's order. `TableStream::resolved_schema` is the same on every
  sub-stream once it has resolved, so 6.5 can read the provider's schema off
  any of them. The filter still resolves per block against the block's own
  field order, which is what a term's index numbers.
- **The order is the DDL's, or the first listing block's.** Names the DDL
  does not declare sort after the declared ones, in the first block's order.
  Neither the order nor the refusals read `SchemaMode`, so D66 holds.
- **Two new refusals.** `Error::TableColumnsDisagree` names the first block
  listing columns and the first one whose set differs, and is a plan fact:
  it is raised before any row, serially, and before any sub-stream exists.
  `Error::UnnamedBlockWidth` is raised where a block naming no columns is
  reached, its width being its first row's.
- **The census is unioned by name.** `union_census` takes the names to key
  on. Before this slice it was positional, which merged two leaves' different
  columns whenever their orders differed.
- **A cold query sees only the blocks its mapping pass reached.** An unmarked
  table settles at its first block (D49), so the set rule compares only the
  blocks mapped (`KD6`). The provider reads a complete map and is not
  exposed to this.

## Negative results

- **Where no block names its columns and the DDL declares none, a block keeps
  placeholder names at its own width.** So two such blocks of different width
  still come out in two shapes. `pg_dump` writes a header with no list only
  for a table with no columns (I5), so no real dump reaches this.
- **A table with no columns and rows loses its rows to a refusal.** Its DDL
  declares no names, and a row's empty line counts as one field, so the block
  is refused by `UnnamedBlockWidth` where it used to come back as one empty
  `column1`.
- **No fixture has a reordered partition.** The shape is tested on
  hand-written dumps in `tests/stream.rs`, serially and partitioned. The
  invariant rests on the `pg_dump` source. Adding the shape to
  `scripts/fixture_schema_partitions.sql` means regenerating six majors and
  re-counting blocks in the tests that read that fixture.
- **No `D<k>` entry.** The register is at its line cap. The rejected
  alternatives, the first block's order with others refused and a NULL-filled
  union, are in the spec's "One schema per table" until the phase's rationale
  is harvested into the register.
- **No cache format change.** Nothing persisted moved.
