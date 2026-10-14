# P36.3 notes — the core and the model: names, plumbing and getters

The pieces with no change of behaviour or format. The rest of the spec's row
is 36.3.1 (the 2026-10-14 history entry). `instrument` is waiting on a
decision under STATUS's "Decisions worth another look".

## What landed

- **The escape pair is `pgdt_core::unescape_field` and `escape_field`.** The
  private escaped path they wrapped was already called `unescape_field`, so it
  is now `unescape_owned`. `pgdt_query::decode_field` keeps its name, since
  it is the text-to-Arrow reader that 36.9 reshapes.
- **One qualified name**: `TableName::qualified_name`, as `CopyHeader`'s was
  already spelled.
- **`RowSplit`, `RawRow` and `validated_prefix` are `pgdt_core::internal`'s.**
  Only `pgdt-query`'s batcher, filter, pruning and replay use them, and no
  `pgdt-query` API takes one.
- **`Diagnostic` and `Unrepresentable` have private fields with getters.**
  `Diagnostic::severity()` returns what `Finding::severity` does, so a caller
  holding the concrete type need not import the trait. Tests that state a
  count build it with `UnrepresentableExt::counted`. `datafusion-pgdump`'s
  test is an embedder, so it compares `(format(), engine())` pairs instead.
- **`StatisticsRequest { selection, sizing: StatisticsSizing }`**, where
  `StatisticsSizing` is `Stated(NonZeroU64)` or
  `Density { min_rows, max_rows }`. A bound beside a stated size can no longer
  be written. The resolved readers (`gathered_size`, `density_min_rows`,
  `density_max_rows`, `recorded_sizing`) are `pub(crate)`; `group_cap` stays
  API. `pgdt parse` builds the enum from its three flags, and `clap` already
  refuses a bound beside a size.

## What the next slice inherits

- **36.3.1** makes `ColumnStatistics`' `null_counts`, `bounds`,
  `datafusion_bounds` and `unrepresentable`, and every `ColumnBounds` field,
  private. Those are the fields cluster J names: a view gives them their
  meaning. `declared_type`, `collation`, `dictionary`, `sums` and
  `value_bytes` take no view and stay plain data. Three kinds of reader will
  need another route:
  - the gatherer's construction, which needs an `internal` constructor;
  - `summary.rs`, which reads `null_counts` raw;
  - the assertions in `pgdt-query/tests/statistics.rs` and
    `pruning.rs`, a few of which tamper with a held block's statistics.
- **`StatisticsBackfill`'s fields are still `pub`.** 36.7 makes the type
  `pub(crate)`.

## Negative results

- **Renaming every `qualified`/`qualified_name` to `qualified`** was not done.
  `CopyHeader::qualified_name` has about three times the call sites, and the
  longer name says what it returns.
