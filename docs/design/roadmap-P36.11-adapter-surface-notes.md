# P36.11 notes — The adapter's surface

`datafusion-pgdump`'s API now has one session-readying function, a public
`Located`, a table that takes its sink when it is built, and table selection
through the library's `TableSelector`. `PgDump` was already over a `Dump`
(36.8). The spelling a front end hands the adapter, and the adapter's two
errors restated in library terms, are 36.11.1
([2026-10-19](../status/history/2026-10-19.md), "36.11 splits").

## The shape

- **`ready_session(SessionStateBuilder) -> SessionStateBuilder`**
  (`session.rs`) applies the guard (`with_guard`, now crate-private), the
  `PgDumpSettings` and `ScanBudget` extensions unless the builder's config
  carries them, and the aggregate's dynamic filter off. `install(&ctx)` stays
  crate-private: `register_dump` and `register_table_factory` still give an
  unreadied session the settings, the budget and the function. The aggregate
  filter is turned off whatever the config says, and a caller wanting it on
  sets it after; see "Decisions worth another look" in
  [`STATUS.md`](../status/STATUS.md). `pgdt sql` restores the option where
  `DATAFUSION_OPTIMIZER_ENABLE_AGGREGATE_DYNAMIC_FILTER_PUSHDOWN` states it,
  so its environment override is unchanged. UF1's marker moved with it.
- **`Located` owns its subject and its finding** (`String`,
  `Box<dyn Finding>`), and its `as_any` is itself. `Any` needs `'static`, so
  the borrowing `Located<'a>` could be made public but never downcast to. A
  sink now reaches the library's record as
  `located.finding.as_any().downcast_ref::<ResolvedColumn>()`. Each reported
  finding is cloned once into its `Located`; every finding type was already
  `Clone`. `UncollatedColumns` is not wrapped, since it names its own subject.
- **The sink at construction.** `PgDump::table(&TableSelector, subject,
  sink)` resolves through `Dump::table` and reports the table's findings
  before it returns. An ambiguous selector is the library's
  `AmbiguousTable`, carried as `Error::Library`. `Error::Table` now means only
  "holds no table". `PgDumpTable::new` and `report` are gone. The catalog
  builds every table with its sink and reports each refused table after its
  schema's listed ones, which is the order the old `report` pass used. The
  schema no longer keeps refused tables, since nothing reads them after
  registration.
- **`PgDumpTableOptions` folded.** The factory's table-options extension is
  the crate-private `TableOptions`: the three name parts, a `PgDumpOptions`
  holding the modes, and `strict_identity` kept as an `Option` so that an
  unstated one falls back to the factory's.
- **Made crate-private** (D106 as written): `PgDumpCatalog`, which nothing
  outside named and only `register_dump` builds; `PgDumpTableFactory`, which
  `register_table_factory` wraps; `PgDumpUnrepresentable`.

## Negative results

- **Not counted.** No per-row or per-field code moved. The only added work
  is one clone per finding, made at registration and when a scan is planned.
- **"Unless the caller already set that option" cannot be detected.**
  DataFusion 55.1's `ConfigOptions` keeps values, not whether they were
  stated, so an explicit `true` and the default `true` look the same.

## What the next round inherits

- **36.11.1's spelling** has to reach `PgDump::open`, which raises
  `CacheNotComplete` before any table exists, as well as the factory, the
  catalog's refused tables (whose `RefusedTable.error` message a sink prints)
  and every `external` conversion a scan makes. The factory and the catalog
  already hold the sink beside the place a spelling would go.
- **36.12's embedder test** reads findings through `Located`.
