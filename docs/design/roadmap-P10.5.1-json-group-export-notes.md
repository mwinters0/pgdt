# P10.5.1 — `info --json` exports every block's groups: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Reporting"; the call is `decisions.md`, "D67". Nothing `parse`, a query or
`--detail` prints changed.

## What exists

- **`print_index_json` serializes `IndexJson` straight into a `BufWriter` over
  locked stdout**, compact, with no `serde_json::Value` of the document and no
  rollup field: each `COPY` block's `statistics` is the persisted
  `BlockStatistics`, dictionary indices and all. What is held beside the index
  while writing is the per-block `resolution` records, sized by columns rather
  than groups. Keys are in struct order again, the strip that sorted them being
  gone.
- **A write failure is an error, not a panic**: `report` returns `Result`, so
  `pgdq info --json | head` exits non-zero with `writing the JSON export` and
  the OS error, where `println!` panicked.
- **`info_statistics.rs` is `--detail`'s alone** and derives no `Serialize`.
- **Pinned by** `pgdump_query-cli/tests/statistics.rs`: the flag tests read
  each block's `statistics` from the export;
  `info_json_exports_every_groups_statistics_compact_and_unrolled` checks one
  line, no top-level `statistics`, per-group arrays as long as `groups`, and
  bounds that ascend and descend group over group; and
  `the_detail_listing_rolls_up_the_exports_groups` re-derives every `--detail`
  statistics line from the exported groups, an empty group included.
  `xz_source.rs`'s plain-against-`.xz` parity now compares statistics too, with
  no change to the test.

## For the slices after

- **10.6.** Landed: [its notes](roadmap-P10.6-parallel-gathering-notes.md),
  comparing the cache's bytes rather than two exports.
- **10.7.** Landed: [its notes](roadmap-P10.7-backfill-notes.md). A block lacking statistics is `"statistics": null` in the export.
- **P20.** The export grows with group count times tracked columns, like the
  cache, and most at the tiny group size the correctness check gathers at.
  Nothing about its size is measured.
- **`M108`** exports the rest of the cache file — `format_version`,
  `container_kind`, the seek table and the source identity — through
  `CacheEnvelope`, so the export is the whole file.

## Negative results

- **Two hand mutations each failed a new test**: `to_writer_pretty` for
  `to_writer`, and `groups_with_bounds` counting one group too many.
