# P6.5 — `datafusion-pgdump`: notes

The fifth slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md). A
new workspace member, `datafusion-pgdump`, holds `PgDump` (a dump opened
through its complete cache), `register_dump` and `PgDumpCatalog` (one catalog
per database), `PgDumpTable` (one table, alone or from a catalog) and
`ScanBudget` (the session's budget). The library gained what a provider
holding its own map needs: `TableName`, `DumpIndex::tables`/`blocks_of`,
`table_schema`, `TablePartitions`, `Parallelism::within_shared` and
`Error::MapIncomplete`. The check is `datafusion-pgdump/tests/provider.rs`;
the library half is `a_plan_over_a_held_map_replays_as_the_mapping_pass_would`
in `pgdump_query/tests/partitioned_replay.rs`.

## What 6.6 and later slices inherit

- **A scan is planned in `scan()` and streamed in `execute()`.**
  `TablePartitions::plan` runs the library's replay plan once over the map
  `PgDump` holds, and `TablePartitions::stream(k, max_rows)` starts partition
  `k` afresh each call, so re-execution works. The plan is wrapped in
  DataFusion's `StreamingTableExec`, one `PartitionStream` per sub-stream, and
  that exec applies the limit. 6.6's pushdown goes into the `QueryOptions`
  `scan()` builds (`semantics` is already `ComparisonSemantics::Arrow`), and
  6.7 needs its own `ExecutionPlan` or a wrapper, `StreamingTableExec` carrying
  no statistics.
- **Batch size is read twice.** `scan()` plans with the session's batch size
  and `execute()` restarts at the `TaskContext`'s. `max_rows` is outside the
  plan (D50), so a mismatch clones the plan with the new size.
- **The schema is settled when the provider is built, from the map alone**
  (`table_schema`), and a scan's batches carry exactly
  `TablePartitions::resolved_schema`. The fixture walk asserts the two agree
  with the library's stream on every table, typed and as text.
- **The provider took no diagnostics sink here.** `PgDump::diagnostics` and
  `PgDumpTable::resolved_schema().notes` expose the file and column channels;
  the sink and the registration report are 6.8's
  ([notes](roadmap-P6.8-datafusion-cli-notes.md)).
- **`PgDump::table(database, schema, table)` is the single-table form** that
  6.8's `STORED AS PGDUMP` factory builds on: an omitted schema or database
  matches any, and more than one match is refused, naming them. Parsing an
  option string into those three is 6.8's.
- **A dump's sources are shared across concurrent scans.** Each scan
  announces its own drawn budget to the one source (`hint_parallelism`), so
  a source's pools follow the last announcement. What the scans drew between
  them bounds it, since no announcement exceeds a draw.
- **The `http` feature passes through and is off by default**, as the spec
  says. The tests read only local fixtures.

## Negative results

- **A table whose `COPY` header names no schema is listed under `public`**
  (`UNQUALIFIED_SCHEMA`). `pg_dump` qualifies every table it writes, so only
  a hand-written file reaches it, and such a table beside a real
  `public.<same name>` shows only one of the two.
- **The stretch of a `pg_dumpall` file before its first `\connect` is not a
  database.** It holds roles and no table, and counting it would refuse every
  `pg_dumpall` file for want of a name for it.
- **`table()` on a table whose plan refuses fails the listing.** `SHOW TABLES`
  asks every table, so one `TableColumnsDisagree` table fails it. `pg_dump`
  cannot write that shape (I5).
- **Some fixture tables are refusals on both sides.** A typed column holding
  one of `KD8`'s values fails the library's stream and the provider's scan
  alike, and the fixture walk compares the refusals' words. It reads every
  fixture at four partitions and three-row batches.
- **No `D<k>` entry.** The register is at its line cap. The two judgement
  calls are under STATUS's "Decisions worth another look", and the budget's
  shape is in `ScanBudget`'s rustdoc.
- **No manual page.** The provider is a library, and its user reads rustdoc.
  The manual page is 6.8's, for the binary.
- **No cache format change.** Nothing persisted moved.
