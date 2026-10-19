# P36.8.4 notes — the cold path onto the `Table`

A cold query is now the warm one with a mapping pass in front: a source is
opened through its cache (`CacheMode::open`), `Table::settle` extends the map
the open decoded until the selected table is settled and hands that table
back, and `Table::plan` plans it as it plans one of a `Dump`'s. `table_stream`,
`table_stream_partitions`, `MappedTable` and `map_for_query` are gone, and so
are `claim`, `CacheClaim`, `CacheMode::load`, `CacheLoad`, `DumpIndex::blocks_for`
and `pgdt`'s `open_for_scan`. `pgdt query` reads the plan's stream in file
order (`TablePartitions::in_file_order`), and its own merge is gone.

## The shape

- **`Table::settle(opened, &selector, scan, extent, schema_mode, &cancel)`**,
  in `crates/pgdt-query/src/dump/settle.rs`, is the old `map_for_query`
  whole: the preamble prepass, `map_forward` with the selector as its
  target, the selection, and a metadata-level table's census under
  `SchemaMode::Typed`. It answers `Ok(None)` for no table, and it takes the
  open's source and watch into the `Table` it returns. It is `impl Table`
  in a child of `dump.rs`, so `dump.rs` never reads it, and the module edge
  runs one way. The token is required, as `map_file`'s is (spec, "The
  opened dump"). One that fires answers `Error::ScanCancelled` once the map
  is saved. It is not handed to the source (`hint_cancellation`), so a
  remote read already in flight finishes before the next chunk's poll.
- **`QueryOptions` loses `max_rows`, `database` and `scan_extent`.**
  `max_rows` is an argument of each stream, threaded through `replay` and
  `activate` into `RowBatcher::new`. So `TablePartitions::plan_at`, which
  cloned the whole `ReplayPlan` for a stream batching at another size, is
  gone. The database is the selector's. The extent is the pass's argument.
  `QueryOptions::validate` is the request's own check (a projection naming
  a column twice). `TablePartitions::new` calls it, and so does `pgdt query`
  before its pass reads past the open, as the old entry points did.
- **`map_forward`'s target is a `Target`**: a caller's test of a block's
  database and header, `pgdt_mapping::internal`'s, so the mapping crate names
  no selector. The `CopyStart` gate that decides whether a block might
  settle the target is now that test with the block's database, where it
  used to be the header's name alone. It is still a superset of
  `target_settled`'s test. The only blocks it now leaves out are ones whose
  database the target excludes, and those can never settle it.
- **`TableSelector::selects_block` is crate-private**, and `Dump::table` and
  the pass share one `select`. So an ambiguous cold query names every
  candidate the map reached. The old pass named the first two.
- **`pgdt query`** opens through `CacheMode::open`, bills the cache's
  statistics from `Held::Loaded`, settles the table, plans it, and reads
  `in_file_order(BATCH_ROWS)`. Its printer reads the nested plans once, off
  the plan's schema. `announce_plan_notes` reads the plan's notes, and
  `announce_early_stops` reads the merged stream's. `KD57`'s marker moved to
  `stream/merge.rs`, the merge that now ships, and the concept map's
  **Not in one place** line for the second merge is struck.
- **pgdt-dump's free `load` is `internal`'s, behind `test-support`**, for
  tests reading back what a run saved. A judged read is
  `CacheMode::open_source`'s `Held::Loaded`.
- **Tests reach the cold path through `tests/common`'s `Cold`**: the pass's
  `ScanOptions`, extent, cache and database beside the stream's `max_rows`,
  with `table`, `plan` and `stream` methods, and `selector`, `pgdt query
  --table`'s reading of a bare or qualified name. A test filters
  `blocks()` by `CopyHeader::matches` where it used `blocks_for`.

## Negative results

- **A cold query's replay now holds the queried table's statistics**
  (`KD123`). `TablePartitions` keeps every matched block's statistics for a
  dynamic filter (`under`), and the old cold path planned without one. The
  replay is still carved around none. Under "Decisions worth another look".
- **`TableStream<'a>` and `BlockingTableIter<'a>` keep their lifetime**,
  though every stream is now `'static`. Dropping it touches the adapter's
  signatures and is no part of this row.
- **No `QueryOptions` field for the extent.** The plan would ignore it, which
  is cluster C's complaint about the field. Under "Decisions worth another
  look", with the schema mode the pass and the plan both take.
- **Not counted.** No reshape in "What a hot-path slice owes" is this one.
  The batcher's flush test reads its own `max_rows` field in place of the
  options' field.

## What the next round inherits

- **36.8.2**, the filter operators carrying their values, is untouched by
  this slice.
- **36.11** inherits `PgDump` over a `Dump`, as 36.8's notes left it. The
  provider never reaches the cold path.
