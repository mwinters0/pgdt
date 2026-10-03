# P31.13.2 — A recorded refusal is per column: notes

What the slices after this one inherit. The decision is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
recorded refusal is per column"; where it now sits is D103. The record it
reshapes is 31.13.1's
([`roadmap-P31.13.1-ignored-refusals-notes.md`](roadmap-P31.13.1-ignored-refusals-notes.md)).

## What exists

- **`IgnoredRefusals::columns`**, one `ColumnRefusals` per column holding a
  refused field an ignoring pass keyed — its first in row order and its count
  — in header order. The block's field is still `Option<Box<…>>`, so a block
  holding none pays one pointer. `CACHE_FORMAT_VERSION` is 51; neither pinned
  digest moved, no fixture holding a record.
- **`refuse_recorded`** takes the first block in file order with a record in
  a column `tracked_columns` marks, and quotes `IgnoredRefusals::first_among`
  those columns: least by line, then column — the field a fresh read keying
  them meets first.
- **A back-fill keys a column it gathers only because the block held it as an
  ignoring pass does** (`StatisticsBackfill::requested`, read by
  `gather::observer_tracking`): under `Default`, a resized block's re-read
  gathers every held column, and refusing in one the request does not track
  would fail a parse a fresh one passes — the verdict depending on the
  gathering run again. The record a re-read makes is merged per column
  (`IgnoredRefusals::merge`, public for `gather_block_statistics`' callers):
  a column's first is the dump's whichever read met it, the count the larger.
  31.13.1 replaced the record on a gathering re-read, which dropped a column
  an earlier pass recorded in a block that had declined and this re-read did
  not gather.
- **`pgdt info`** prints one `refused by PostgreSQL: column <c>, …` line per
  such column, in column order; `--json` carries `ignored_refusals.columns`.
- **Evidence**: `tests/statistics.rs`'s
  `a_recorded_refusal_fails_exactly_the_parses_tracking_its_column` (every
  tracked subset of four columns, row order beating column order, a tie
  broken by column, a back-fill over held refused columns passing and
  renewing the record — which fails with the back-fill's per-column mode
  removed); `statistics.rs`'s `a_record_keeps_each_columns_first_and_count`
  (add, fold, merge, `first_among`); 31.13.1's tests, asserting per column.

## What the slices after this inherit

- **31.14**: a `strict` re-read decodes every field of every column, so its
  `requested` is every column and nothing is gone past; its checked-in-full
  mark sits beside this record, and `refuse_recorded` still returns early for
  every mode but `Default`.
