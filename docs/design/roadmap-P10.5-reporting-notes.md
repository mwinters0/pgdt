# P10.5 — Reporting in `info --detail` and `--json`: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Reporting"; the call is `decisions.md`, "D67". Nothing `parse` or a query does
changed.

## What exists

- **`pgdump_query-cli/src/info_statistics.rs`** rolls the map's blocks up per
  table and column for `--detail`: counts in `table_statistics`, text in the
  two `line` methods. A table is keyed by database and
  `CopyHeader::qualified_name`, a column by name.
- **`--detail` ends with a `statistics:` section** below the last block and
  above the totals, under the listing's own `database:` headings, or with
  `statistics: none gathered` when no block carries any. Plain `info` prints
  none of it.
- **`--json` carries no rollup**: it exports every block's groups as the cache
  holds them, which is
  [10.5.1's](roadmap-P10.5.1-json-group-export-notes.md).
- **Pinned by** `pgdump_query-cli/tests/statistics.rs` (partition roots and
  database headings; the text against the export is 10.5.1's) and the module's
  unit tests (the merge, empty groups, mixed sortedness).

## For the slices after

- **10.7.** Landed: [its notes](roadmap-P10.7-backfill-notes.md). A block lacking statistics shows in `--detail` as `statistics over
  k of n block(s)` with `k` short, and one gathered at another size as a second
  size in the table line; that is what a back-fill's result is visible as. The stderr
  count of re-read blocks the spec asks of a back-filling `parse` is 10.7's,
  not here.
- **10.8.** Landed: [its notes](roadmap-P10.8-pruning-consumer-notes.md). Nothing here reads `declared_type` or `collation`; a statistic a
  query would not believe under the current comparison is still reported.
- **Any new persisted statistic** reaches `--json` with no change, and
  `--detail` only through `TableStatistics::add`.

## Negative results

- **A group of NULLs counts as a group without bounds**, so an all-NULL column
  reads `bounds in 0 of N group(s)`. Only a group no row starts in is outside
  the shares; `rows - null_count` cannot tell a value-less group, a short row
  observing nothing for a column.
- **A zero-row block contributes a block and no group**, so `empty_table`
  reads `0 rows and 0 bytes per group over 0 group(s)`.
- **A hand mutation**, `groups_with_rows` counting empty groups, failed a new
  test.
