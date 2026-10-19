# P36.8 notes — the `Dump` and its `Table`

`pgdt_query::Dump` holds the source, the map and the watch of one open, and
is complete by construction: `Dump::opened(Opened)` takes a cache reaching the
file's end, `Dump::mapped(MapRun)` a mapping pass run to it, and either
refuses anything short with `Error::MapIncomplete`. It answers `tables()`,
`databases()` and `table(&TableSelector)`, whose `Table` gives the schema, the
summary, the divergences and the plan (`TablePartitions`) under a
`QueryOptions`. The free `table_schema`, `table_summary`, `column_divergences`
and `TablePartitions::plan` are gone from the API. `PgDump` holds a `Dump`.
The slice split: the cold path and the streams are 36.8.1, the filter
operators 36.8.2 ([history, 2026-10-18](../status/history/2026-10-18.md),
"36.8 splits").

## The shape

- **`TableSelector` states a table's name and as much of its schema and
  database as the caller does**, an unstated part selecting any. Built from a
  `TableName`, it states every part, `None` included, so it selects that
  table alone. `Dump::table` answers `None` for no table, the one table, or
  `Error::AmbiguousTable` naming each candidate. `PgDump::table`'s three
  `Option<&str>` build a selector, and keep the adapter's own wording for none
  and for several.
- **`Table<'a>` borrows the `Dump`**, and `Table::plan` hands back the
  `TablePartitions` owning the source and the watch, so its streams are
  `'static` as before. `PgDumpTable` keeps its `TableName` and resolves its
  `Table` again at each use (`PgDump::table_of`). The resolution clones
  nothing; each method clones the table's blocks, and a plan the DDL.
- **A `Table` method takes the options its answer is read under.**
  `summary` and `divergences` resolve the schema under the same options, so
  the semantics and the statistics view can no longer come from a different
  query than the schema. `PgDumpTable` settles its divergences with its
  schema at build, and takes its summary on the first scan as before.
- **`Table::plan` is synchronous** and refuses a source the watch cannot
  check (`require_checkable`), where the replay's reads are planned, as the
  history entry for 36.6's review placed it. The completeness check it made
  against `source.size()` moved into the constructors.
- **The replay plan holds a `ReadOptions`**, not a `ScanOptions`: the replay
  read only its `read`. The cold entry points pass `scan_options.read`, and
  the adapter's settings answer `read_options()`.
- **Tests reach a plan through a `Dump`.** `tests/common` gains `mapped`, a
  fixture mapped whole with its cache disabled, and `dump_over`, a
  hand-reshaped map saved as a cache and opened through it, so a partial
  map's refusal is the open's. The adapter's tests build theirs the same way:
  the registration test opens a `Dump` apart from the provider for its
  expected divergences, and the nested-column test writes a one-table dump
  for each declared type in place of resolving a hand-built `DumpMetadata`.

## Negative results

- **`pgdt info` was not moved onto the `Dump`.** It reports an incomplete
  cache, its completion stated at the top (D67), and a cache with no source
  at all (D61), and a `Dump` holds neither. It opens through
  `CacheMode::open` already, the first of the two layers
  (`docs/design/decisions.md`, "D61").
- **Keeping the free functions until 36.11** was refused. It leaves two ways
  to plan a table, and the adapter was their only production caller, so
  moving it onto a `Dump` here is a field swap in `PgDump` and three call
  sites in `PgDumpTable`.
- **A `Dump` constructor handing the `Opened` back on refusal**, so the
  adapter could read why from it, was refused. The adapter reads the `Held`
  before it builds the `Dump`, keeping its wording, and the library's
  refusal stays an `Error`.
- **`Table<'a>` borrowing its `Dump` is what landed, and is reversed**: a
  `Table` owns what it plans over, and an early-stopped pass yields one too
  ([history, 2026-10-19](../status/history/2026-10-19.md), "The `Dump`'s
  table owns its blocks; `info` reads the cache").

## What the next round inherits

- **36.8.1 moves the cold path onto the `Dump`.** `table_stream`,
  `table_stream_partitions` and `read_table` remain, over `map_for_query`'s
  own load and watch; the merge in `pgdt/src/query.rs` remains; the
  `&str` pattern, `QueryOptions.database` and `DumpIndex::blocks_for` remain.
  An early-stopped pass settles one table from a map short of the file's
  end, which no `Dump` holds, and yields the owning `Table`.
- **36.11 inherits `PgDump` over a `Dump`.** What is left there is the
  session-readying function, `Located` public, the sink at construction and
  `PgDumpTableOptions` folded; the metadata-level pre-check in
  `PgDumpTable::build` and its `parse` wording stay the adapter's until
  36.10's errors.
