# P10.5 — Reporting in `info --detail` and `--json`: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Reporting"; the call is `decisions.md`, "D67". Nothing `parse` or a query does
changed.

## What exists

- **`pgdump_query-cli/src/info_statistics.rs`** rolls the map's blocks up per
  table and column, `table_statistics` feeding both renderings, so the two
  cannot disagree: counts only, as `--json`'s components, and the two `line`
  methods as `--detail`'s text. A table is keyed by database and
  `CopyHeader::qualified_name`, a column by name.
- **`--detail` ends with a `statistics:` section** below the last block and
  above the totals, under the listing's own `database:` headings, or with
  `statistics: none gathered` when no block carries any. Plain `info` prints
  none of it.
- **`--json` carries a top-level `statistics` array, and no `COPY` block's
  `statistics` field**: `strip_block_statistics` removes it from a
  `serde_json::Value` of the whole document, since the field is the persisted
  struct's and the cache needs it.
- **Pinned by** `pgdump_query-cli/tests/statistics.rs` (the counts against the
  fixture's asserted shapes, text against JSON line by line, no block field,
  partition roots and database headings) and the module's unit tests (the
  merge, empty groups, mixed sortedness). The CLI tests that read a block's
  statistics through `--json` now read the rollup.

## For the slices after

- **10.7.** A block lacking statistics shows as `gathered_blocks` short of
  `blocks`, and one gathered at another size as a second entry in
  `group_sizes`; that is what a back-fill's result is visible as. The stderr
  count of re-read blocks the spec asks of a back-filling `parse` is 10.7's,
  not here.
- **10.8.** Nothing here reads `declared_type` or `collation`; a statistic a
  query would not believe under the current comparison is still reported.
- **Any new persisted statistic** reports through `TableStatistics::add`, or
  the export silently omits it, the block field being stripped.

## Negative results

- **`--json`'s keys are now in alphabetical order** throughout the document, a
  consequence of the strip: `serde_json` has no `preserve_order` here, and
  enabling it adds a dependency. No test read the order.
- **A group of NULLs counts as a group without bounds**, so an all-NULL column
  reads `bounds in 0 of N group(s)`. Only a group no row starts in is outside
  the shares; `rows - null_count` cannot tell a value-less group, a short row
  observing nothing for a column.
- **A zero-row block contributes a block and no group**, so `empty_table`
  reads `0 rows and 0 bytes per group over 0 group(s)`.
- **Two hand mutations each failed a new test**: the strip disabled, and
  `groups_with_rows` counting empty groups.
