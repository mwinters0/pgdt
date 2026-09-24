# P25.6 — The second format bump, sums and byte sizes: notes

The slice built the spec's last two "Same-pass gathering" items
([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)): a per-group
sum and a per-group byte size. `CACHE_FORMAT_VERSION` is 26. The stored
fields are `ColumnStatistics::sums` and `ColumnStatistics::value_bytes`. The
reasons the code cannot give are `decisions.md`, "D91". One call made without
the maintainer is under STATUS's "Decisions worth another look": text bytes
are gathered for the typed read's text and `bytea` columns only.

## What the next slices inherit

- **A sum is kept at 128 bits and narrowed by whoever reads it.** Each group's
  sum is the wrapping sum of its values as the typed read parses them
  (`gather::Summand`, the same `parse` calls and `decimal_unscaled_digits` as
  `batch.rs`). The provider narrows it to `Int64` for `int2`/`int4`/`int8`,
  `UInt64` for `oid`, and hands a `Decimal128` sum in the column's own type.
  `table_summary` gives the sum only where every group of every block kept
  one, the DDL it was gathered under still stands, the column is emitted in
  the type it was summed as, and it holds a non-NULL value.
- **A value that does not decode drops the column's sums for the block**,
  not the group's. Nothing reads a partial sum, and a column-level `None`
  joins across pieces and merges without a per-group flag.
- **Why the `Decimal128` answer matches a read.** DataFusion 55's `SUM` of a
  `Decimal128(p,s)` returns `Decimal128(min(38, p+10), s)` and adds with
  `add_wrapping` on the `i128`, with no precision check.
  `Sum::value_from_stats` casts our column-typed value to that type, and
  arrow-cast 59 clones a same-scale widening without validating
  (`cast_decimal_to_decimal_same_type`). `public.spans.huge` wraps `i128`
  across three blocks and answers as the blind session reads it. There is no
  invariants entry for this: the blind-session oracle is what re-checks it at
  a DataFusion or arrow upgrade, and a cast that started validating would
  make the sum go unanswered rather than wrong.
- **Byte sizes are the provider's.** `table_statistics` states a fixed-width
  column's `rows × width` `Exact` (`primitive_width`, and `FixedSizeBinary`),
  a `Utf8View` or `Binary` column's text bytes `Inexact`, and nothing for a
  boolean, an enum or a nested column. `total_byte_size` is recomputed after
  the projection and is `Absent` where any projected column is. Under a
  filter the scan restates each column from `TablePartitions::kept_value_bytes`
  and the filtered row bound, summed where `kept_rows` is. That uses a
  `kept_groups` mask now on `BlockPruning`.
- **What the harness now pins.** `statistics_never_change_an_answer` asserts
  `Seen::sum` in the typed pass and fails a `SUM` answered over a column the
  read refuses. The new target `a_sum_answers_where_every_group_kept_one`
  pins which columns state a sum: every integer, `oid` and typmodded
  `numeric` of `public.spans` read as three blocks. It also pins which state
  none: a `timestamptz`, a float, an all-NULL column, and a `numeric(10,2)`
  holding `NaN`. The estimate target counts `sized` and `shrunk` and floors
  both. `a_join_builds_the_side_its_statistics_call_smaller` now builds
  `ordered`, and it asserts that `long_value`'s bytes are past the collecting
  threshold. The library's file oracle checks each group's sum and text bytes
  against the file. `a_plan_bounds_its_rows_by_the_groups_its_pruning_kept`
  checks the kept text bytes. Mutation runs, each failing a target: sums not
  merged; a piece's lost sums not propagated; an `Int64` sum saturated rather
  than wrapped; a text byte size understated by a fiftieth; the kept-group
  mask shifted by one.

## Negative results

- **The collect-left mode did not change on the join target, only its
  side.** With bytes stated, `ordered` (a thousand `int4`s) is under
  DataFusion's collecting threshold, so it is collected whole instead of
  `long_value`. 25.1's notes expected both the build side and the mode to
  move. The side moved, and the mode is still `CollectLeft`, now over the
  small side.
- **About half the filtered scans state a total.** A table projecting a
  boolean, an enum or a nested column states no total. In the typed pass
  those are the only columns without a measure.
- **A `numeric(p,s)` past 38 digits keeps no sum.** It is emitted as
  `Decimal256`, which the spec left out and a 128-bit sum cannot hold. A
  bare `numeric` keeps none either, since it is emitted as text.
- **The statistics grew, and no figure has read them since.** A summed
  column adds 16 bytes a group, a measured one 8. `statistics-gathering`
  and `statistics-pruning` were already red at 25.5's commit, and this slice
  gives the first a reason of its own: what it reads is a smaller statistic
  than a pass now holds.
- **`GOLDEN_ORDER`'s digest did not move.** No comparison changed, so the
  version was re-pinned alone.
