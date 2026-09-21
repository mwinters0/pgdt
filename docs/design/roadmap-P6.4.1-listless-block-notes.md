# P6.4.1 — A block with no column list copies no columns: notes

The follow-up to [`roadmap-P6.4-one-schema-notes.md`](roadmap-P6.4-one-schema-notes.md),
building the spec's list-less-block rule as amended after 6.4
([`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "One schema per
table"). The external fact is
[`postgres-invariants.md`](postgres-invariants.md), "I5", now verified
against fixtures.

## What 6.5 and later slices inherit

- **Every block is planned before a row is read.** A list-less block's width
  is zero, known from its header, so `plan_blocks` resolves it with the rest
  and a query has no refusal left that waits for a block to be reached.
  `activate` no longer takes a field count, and a resume token carries none.
- **A table's order is never absent.** `TableColumns::order` is the first
  block's list sorted by DDL rank, and empty for a table whose blocks list
  nothing, whatever its DDL declares. An unprojected query projects it, so a
  zero-column table yields batches of no columns carrying their row count, in
  every `SchemaMode` and serially or partitioned.
- **A list-less block beside a listing one is `TableColumnsDisagree`.** Its
  set is the empty one. `pg_dump` cannot write that mix: a partition has its
  root's columns.
- **The refusal of a non-empty line is `ColumnCountMismatch`**, expected 0,
  found 1, from `RowBatcher::push_row`, still the system's only field-count
  check. `Error::UnnamedBlockWidth` and the `column<N>` placeholders are gone.
- **The fixtures hold all three shapes** in
  `scripts/fixture_schema_partitions.sql`, on every major and both flag sets:
  `shuffle` (a leaf attached in another order), `hollow` (no columns) and
  `derived` (only generated ones). 6.5's "same rows as the library over every
  fixture" reaches them with no extra work.

## Negative results

- **The hand-written `pgdump_query/tests/data/edge_cases.sql` keeps its
  list-less block with non-empty rows**, a shape `pg_dump` never writes, because
  it is the scanner's fixture for a `\\.` row. A query of
  `public.no_column_list` is now refused. The tests that used it as a table
  past `widgets` query `"My Schema"."Odd Table"` instead.
- **The mapping pass still grows a block's census for a row wider than its
  header.** Nothing reads the extra entries now. `union_census` is keyed by name
  only, and a query refuses the row. Stopping the growth would change the
  mapping path for malformed input alone.
- **The fixtures went into the `partitions` schema, not `edge_cases`.**
  About thirty test files read `edge_cases`, against a handful for
  `partitions`. The new tables cost one re-count of the marked blocks in
  `tests/stream.rs`, and a skip in `tests/pruning.rs`, which has no filter to
  draw for a table with no columns.
- **No `D<k>` entry.** The register is at its line cap. The rejected
  alternative, taking the declared names where the width matches, is in the
  spec.
- **No cache format change.** Nothing persisted moved.
