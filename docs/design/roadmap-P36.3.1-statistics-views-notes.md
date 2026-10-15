# P36.3.1 notes — a column's statistics read through a view

`ColumnStatistics`' NULL counts, its two sets of bounds and its
unrepresentable counts are private. `null_count`, `group_bounds` and
`sortedness` read them under a `StatisticsView`. The census and the stored
database are 36.3.2 (the 2026-10-15 history entry). The persisted shape is
unchanged, so there is no cache bump: `persisted_index_is_pinned_to_the_format_version`
passes unedited, and so does `info --json`'s export.

## What landed beside the spec's words

- **`ColumnBounds` and `BoundsView` are `pgdt_model::internal`'s, with their
  fields `pub`.** The spec says `ColumnBounds` is read only through the
  view-taking methods. Once `bounds_in` is private, no API item hands one
  out, so D106 puts the type in `internal` rather than at the root with
  private fields and no methods.
- **`internal::ColumnStatisticsParts`** holds every field as stored. The
  gatherer builds a column through `ColumnStatisticsExt::from_parts`. Tests
  that check what gathering stored, or that tamper with a held block, take
  one apart with `into_parts`.
- **Three view-free readers are API**: `listed_groups(set)`, the number of
  groups a stored set lists (`None` where the set was not kept);
  `counted_groups()`, the number its NULLs are counted for; and
  `holds_unrepresentable(tier)`. The summary's consistency checks, the
  back-fill's "a bounded column held without bounds", and the summary's
  "is a sum the column's" read through these.
- **The summary's NULL accounting reads through `null_count`**: under the
  query's view where the DDL is believed, under `Every` where it is not.
  `Every` is the dump's own NULLs, which is what the raw read gave.

## What the next slice inherits

- **`pgdt` is an embedder** (`layering.py`), so its inline tests cannot name
  `internal`. `info/statistics.rs`'s tests build a column from its persisted
  JSON with `serde_json::from_value`. Any embedder test that needs a
  hand-built column has the same route.
- **`sums` keeps no view**, as 36.3's notes ruled, although it leaves out
  what `Every` reads as a value. The summary judges whether a sum is the
  column's through `holds_unrepresentable`.
