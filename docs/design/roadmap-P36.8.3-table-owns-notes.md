# P36.8.3 notes — a `Table` owns what it plans over

`pgdt_query::Table` holds the dump's source and watch by `Arc`, the DDL by
`Arc`, its `TableName`, and every block of the table, cloned once by
`Dump::table`. It has no lifetime, so it outlives the `Dump` that resolved
it, and `schema`, `summary`, `divergences` and `plan` read its own blocks.
`PgDumpTable` holds one, resolved once in `build`, and `PgDump::table_of`,
`PgDump::index` and `PgDump::source` are gone; the adapter reaches the
`Dump` through a crate-private `PgDump::dump`. The reasoning is the
[history, 2026-10-19](../status/history/2026-10-19.md), "The `Dump`'s table
owns its blocks; `info` reads the cache".

## The shape

- **The DDL's `Arc` sits in the index**: `DumpIndex.metadata` is
  `Option<Arc<DumpMetadata>>`, so the `Dump`, every `Table` and every
  `ReplayPlan` share one copy. Before, each plan deep-cloned the whole
  `DumpMetadata`, and a sub-stream batching at a size the plan was not
  planned at cloned it again. The cache's bytes are unchanged: serde's `rc`
  feature was already on in pgdt-model, and an `Arc<T>` serializes as its
  `T`, so `CACHE_FORMAT_VERSION` is not bumped. `preamble_only` still hands
  back an owned `DumpMetadata`.
- **The helpers take the table's parts, not a `&Table`.** `table_schema`,
  `table_summary` and `TablePartitions::new` take the database, the blocks
  and the DDL. `dump.rs` depends on `stream` and `summary`, and a `&Table`
  argument would turn that module edge into a cycle (`scripts/module_graph.py`).
- **`Table::blocks` and `Table::source` are API.** The adapter's
  metadata-level pre-check reads the blocks, and its scan reads the source's
  worker-memory default. Nothing else reached past the `Dump` for them.
- **`Table` is neither `Clone` nor `Copy`.** Nothing clones one, and a clone
  would copy the blocks.

## Negative results

- **The `Arc` beside the index in the `Dump`** was refused. Moving the DDL
  out of the index would leave `Dump::index().metadata` reading `None`,
  which contradicts `DumpIndex`'s own doc. Keeping both would make a second
  resident copy.
- **A plan still clones the table's blocks** into `TablePartitions`' own
  `matches`, stripping their statistics where the query does not use them.
  The per-use clone the history entry priced was `table_schema`'s and the
  borrow's re-resolution. Those are gone. The plan's own clone is left.
- **`PgDumpTable::new` still panics on a name that is not one of the dump's
  tables**, as `table_of` did. Making that an error would change behaviour,
  which this phase does not do.

## What the next round inherits

- **36.8.4 builds an owning `Table` from an early-stopped pass.** Only
  `Dump::table` builds one today, and its fields are private, so it needs a
  crate-private constructor from the parts. `MappedTable`'s DDL is already an
  `Arc`.
