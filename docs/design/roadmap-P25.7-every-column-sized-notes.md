# P25.7 — The third format bump, every column sized: notes

The slice built the spec's seventh slice
([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md), "Same-pass
gathering"): text bytes kept for every tracked column, and a byte size for the
typed read's booleans and enums. It bumps `CACHE_FORMAT_VERSION`. The reasons
the code cannot give are `decisions.md`, "D91", amended here.

## What the next slices inherit

- **`ColumnStatistics::value_bytes` is a `Vec<u64>`, not an `Option`**, kept
  as `null_counts` is. Every gatherer counted every column's text already;
  only `close_group` was gated. `value_bytes_where` still answers `None` for
  a column whose vector is not one per group.
- **`byte_size` is the only place a type's measure is chosen.** A boolean is
  `rows.div_ceil(8)`, as exact as the rows. An enum's `Dictionary` is its key
  width a row plus its text bytes, `Inexact` even over exact rows. A nested
  type is `Absent`. Under `:strings` every column is `Utf8View`, so every
  column is its text bytes, `Inexact`. `table_statistics` now sets the size
  before the enum's early return (`KD45`), which skips only its bounds and
  sum.
- **`emitted_bytes` measures a column over its every batch**, not batch by
  batch. A boolean's measure is a bit a row over the whole scan, and a
  per-batch round-up would put an `Exact` size below the sum. An enum's
  measure is its keys plus the labels each batch's dictionary holds.
- **The enum's bound rests on arrow's `StringDictionaryBuilder`** holding only
  the labels appended since its last `finish`. There is no invariants entry
  for it: the estimate target measures each batch's dictionary, so a builder
  that kept labels across batches fails it at an arrow upgrade.
- **What the harness now pins.** The estimate target reads every table typed
  and as text. `which_are_sized` asserts on each unfiltered scan: read as
  text, every column `Inexact` and a total stated; typed, a boolean `Exact`,
  an enum `Inexact` and a nested column `Absent`. It counts each and floors
  every count. The library's file oracle checks every column's text bytes
  against the file, and the pruning target checks `id`'s kept text bytes
  beside `low_card`'s. Mutation runs, each failing the estimate target: a
  boolean sized by floor division; an enum sized by its keys alone.

## Negative results

- **A typed scan projecting a nested column still states no total.** Those
  are the typed pass's only unmeasured columns. Their text is no bound on
  their leaves' bytes, and nothing else here measures them.
- **An enum's bound is loose by its repeats.** The text counts a label once a
  row, and a batch's dictionary holds it once.
- **The statistics grew again.** Every tracked column now holds a `u64` a
  group for its text bytes. `statistics-gathering` was already red with
  25.6's reason, and this slice adds to the same one: a pass holds more than
  the figure read.
- **`GOLDEN_ORDER`'s digest did not move.** No comparison changed, so the
  version was re-pinned alone.
