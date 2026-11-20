# P43.1 — `pgdt-model`'s `persisted/` split: notes

Slice 43.1 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). A move:
no serde shape changed, no bump, the crate's public API unchanged.

## What the next slices inherit

- **`crates/pgdt-model/src/persisted/` holds the serde shapes and nothing
  else**: `span.rs` (the spans, `CopyBlock`, `LongestLine`, `DumpIndex`),
  `catalog.rs`, `statistics.rs` (with the `deserialize_with` function the
  statistics field names), `block.rs` (the census and the count) and
  `timescaledb.rs` (`RecordedRows`). The module is private: the root
  re-exports every shape exactly as before, `RecordedRows` still at
  `timescaledb::RecordedRows`, and `BoundsView` and `ColumnBounds` still in
  `internal` alone.
- **Every impl stayed beside its concept**, inherent ones included
  (`DumpIndex`'s walks in `span.rs`, `LongestLine::fold`, the catalog walks,
  `ColumnStatistics`' view reads, `RecordedRows::observe_row`), as did the
  `introspect` build's `Drop` for `BlockStatistics`. 43.4's `BlockAddress` and
  `BlockRecord` go into `persisted/span.rs`, and their methods into `span.rs`.
- **The fields that were module-private are now crate-private**:
  `ColumnStatistics`' `null_counts`, `bounds`, `datafusion_bounds` and
  `unrepresentable`, and `Unrepresentable`'s `format` and `engine`, so their
  methods can read them from beside the tree. Outside the crate nothing
  changed; inside it, a raw read is no longer refused by the compiler, only by
  the rule the type's doc states.
- **The constants that bound what a field holds stayed with their rule**:
  `SPAN_STORED_TEXT_MAX_BYTES` in `span.rs`, `DICTIONARY_ENTRY_MAX_BYTES` and
  `DICTIONARY_MAX_ENTRIES` in `statistics.rs`. Changing one still asks the bump
  question. `persisted.rs`'s doc says the tree is not the only place that asks
  it, and points at `CACHE_FORMAT_VERSION`'s doc, which stays at the crate
  root.
- **`scripts/measure.py`'s mechanism declarations name the shapes too**:
  `MAP`, `UNREPRESENTABLE`, `PREAMBLE` and `STATISTICS` each list the
  `persisted/` file beside the module they already named. Without that, a
  shape change, such as 43.4's, would leave stale figures reading green.

## Negative results

- **`BlockId` is never persisted** (the spec, "Identity"), so it is no serde
  shape. The spec's "Where it lives" table places it under `persisted/` with
  `BlockAddress` and `BlockRecord`, but the rule this slice installs puts it in
  `span.rs` beside `BlockAddress::id()`. 43.3 places it.
- **A persisted shape's doc linking a reader is a doc link against the
  edges** (`module_graph.py --docs`). The moved docs keep their
  `crate::StatisticsView`- and `crate::DICTIONARY_*`-style links rather than
  dropping to code spans. The shapes are read beside the tree by design, so
  each link names the reader its field is read through. The split adds
  four such links and no cycle.
