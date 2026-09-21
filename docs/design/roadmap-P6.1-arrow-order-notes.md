# P6.1 — Arrow-order evidence: notes

The evidence slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md).
It landed test code only: `each_kind_s_order_and_bounds_against_arrow_s` in
`pgdump_query/src/predicate.rs`'s `oracle` test module. That test puts every
value the committed oracle holds, on six majors, through the column a batch
builds from it, and checks two things against Arrow's `lt`/`eq`/`gt` kernels on
that array: each pair's `ValueKey` order, and the bounds gathering stores for
each group. Its `ARROW_AGREEMENT` table is the record of which kinds agree, and
the doc comment on the table gives the first disagreeing pair for each kind
that does not. Two test-only helpers came with it: `batch::column_of` and
`gather::one_group_bounds`.

## What 6.2 inherits

- **Eleven of the twenty kinds already order and bound as Arrow does:** `Bool`,
  `Int`, `UnsignedInt`, `Decimal`, `Date`, `Time`, `Timestamp`, `Uuid`,
  `Bytea`, `Text`, `MacAddr`. Every kind but `MacAddr` agrees because its key
  *is* the decoded Arrow value. `MacAddr` agrees because `macaddr_out` writes
  fixed-width lowercase hex. `Text` was walked with no collation. Its key is
  bytewise under any collation, so collated text is in Arrow's semantics as it
  stands, as the spec expected.
- **Nine disagree.** The spec's table names seven of them, one with the wrong
  emitted type. The two missing kinds and the misdescribed one:
  - **`Float32`/`Float64`.** Arrow's kernels order floats by IEEE `totalOrder`,
    so `-0` is below `0` and `eq(-0, 0)` is false, where PostgreSQL equates
    them. `NaN` places the same way under both. DataFusion does not compare
    with the bare kernels: its `apply_cmp` makes `-0` into `0` first, so in
    DataFusion's semantics a float agrees with PostgreSQL's (slice 6.2.1).
  - **`Interval` is emitted as `Interval(MonthDayNano)`, not `Utf8View`** as
    the spec's table has it (`pgtype.rs`, `"interval"` in `builtin_scalar`).
    Arrow compares that type field by field (months, then days, then
    nanoseconds), so `30 days` is below `1 mon`. Arrow mode for `Interval` is
    that lexicographic order, not a bytewise one.
- **`PaddedText` disagrees on both counts.** Its order disagrees because Arrow
  compares the padded text, so a byte below the blank (a tab) orders
  differently. Its bounds disagree because a stored bound is unpadded and so is
  never a value of the emitted column.

## What 6.7 inherits

- **A stored lower bound does not say whether it is a whole value.** A
  bytewise kind whose least value is longer than `DICTIONARY_ENTRY_MAX_BYTES`
  stores a prefix of it (`gather.rs`, `clipped_bounds`). `statistics::Bounds`
  carries only `max_exact`: the whole-minimum flag is kept while gathering and
  dropped when the bound is stored. So nothing stored can tell an `Exact`
  minimum from a truncated one. The oracle's values are short, so the walk
  never reaches this case.

## Negative results

- **The oracle's `inet`/`cidr` population orders alike both ways by
  coincidence.** Every IPv4 case in it has a first octet of two digits or more.
  Without the two values added in `SUPPLEMENT` (`9.0.0.1` and `9.0.0.0/8`), the
  walk records `Network` as agreeing. A kind the walk marks as agreeing
  therefore agrees over this population only. The eleven that agree by
  construction or by their output format are what make the result hold in
  general, not the walk.
- **Nested plans were not walked.** Arrays, composites, ranges and
  multiranges are `ComparisonPlan::Nested`, and `cmp` does not support struct
  or list arrays. Whether DataFusion's element-wise list comparison matches
  the library's nested comparison is still open for 6.2 and 6.6.
- **A special value the emitted type cannot hold is left out.** Examples are
  `infinity` in a `date` and `NaN` in a `numeric(p,s)` (`KD8`). No array
  carries such a value, so Arrow has nothing to order.
