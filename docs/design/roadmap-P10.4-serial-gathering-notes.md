# P10.4 — Serial gathering and persistence: notes

What the later P10 slices inherit from this one, and what it did not do. The
spec is [`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md).
No reserve or resident bump follows it
([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "P10 is
grilled and sliced"). No query reads a statistic yet.

## What exists

- **`statistics.rs` (L1)** holds the persisted types — `BlockStatistics`,
  `RowGroup`, `ColumnStatistics`, `ColumnBounds`, `Bounds`, `Sortedness`,
  `ColumnDictionary` — the request (`StatisticsRequest`, on
  `ScanOptions::statistics`, `None` by default), the three constants, and the
  `BlockObserver` trait. `CopyBlock::statistics` is
  `Option<Arc<BlockStatistics>>`; `sparse_index` and `column_stats` are gone.
- **`gather.rs` (L4)** implements the observer. `map_forward` installs one per
  `CopyStart` through `Builder::observe_block`, only when `map_file` passed a
  request, after the metadata restatement so the block resolves against its own
  database; `Builder::on_row` now takes the row's absolute offset.
- **The leader is never offered a gathered block**: `map_forward` skips the
  offer and reports `leader::gathering_shortfall`, a `scan arrangement` line
  with `bound_by="statistics"`. An untracked block (a selection naming other
  tables) is still split.
- **`FORMAT_VERSION` is 18**, and `predicate.rs`'s
  `golden_order_is_pinned_to_the_format_version` pins `GOLDEN_ORDER` beside it.
- **`pgdq parse --statistics <all|none|list>` and `--statistics-group-size`**;
  both refuse `--preamble-only`, even `none`, and a size beside `none` is
  refused. `measure.py`'s `NO_STATISTICS` is on every `parse` it builds, the
  preamble shapes excepted, and `--check` fails a shape without it.

## For the slices after

- **10.5.** `info --json` already carries every group's statistics, being the
  internal struct (`decisions.md`, "D67"), where the spec says individual group
  values are not reported; 10.5 decides that shape. `columns` is positional to
  the header, `None` untracked. An empty group (`rows == 0`) holds a zero NULL
  count, no bounds and an empty dictionary, which a share of groups carrying
  bounds should exclude.
- **10.6.** `Gatherer` assumes rows in offset order, one block at a time. A
  piece join has to carry each tracked column's open group and its previous
  value — a `Clipped` head for a bytewise kind, a `ValueKey` otherwise — and
  answer the join's step through `Clipped::locate`, whose `None` is `Unsorted`.
  Deleting the `continue` after `observe_block` is what re-offers the leader;
  `tests/statistics.rs`'s `a_gathering_scan_is_the_serial_scan_whatever_the_worker_count`
  fails without it and is the test to turn into serial-equals-parallel.
- **10.7.** `StatisticsRequest::group_size` is `None` when unstated, and each
  block records the size it was gathered at. A resumed `parse` gathers only the
  blocks it maps; a block mapped under `--statistics none`, or outside a
  selection, holds `None`.
- **10.8.** A `GroupStatistics` over `BlockStatistics` is a lookup: column `i`
  is header column `i`, which is a term's unprojected index. `ColumnStatistics`
  carries `declared_type` and `collation` to compare against the current plan.
  The six `expect(dead_code)` items in `predicate.rs` still stand, and
  `ValueKey` is the `pub(crate)` wrapper over `OrderKey`.

## Decided here, inside the spec

- **A truncated `max` is a character successor, not a byte one.** The spec's
  "last byte incremented after dropping trailing `0xFF`s" is kept for `bytea`,
  over its decoded bytes; for text, a successor byte can end a character and
  `Bounds` holds `String`, so the last character is replaced by the next scalar
  value, one that has none is dropped, and `character` never takes a blank
  (`gather.rs`, `text_upper`). 10.3's notes left this to this slice.
- **A bytewise kind is never keyed while gathering.** A key copies the whole
  value, which the spec's quarter-gigabyte rows make a copy per row held three
  times over; `text`, `character` and `bytea` order by their text instead, kept
  to 260 bytes. Two values agreeing on those bytes share every byte a stored
  bound reads, so bounds stay exact where they fit and valid where not, and only
  their row order is unknown — which makes the block `Unsorted`. `bytea` is
  ordered by its `\x` lowercase hex; another spelling is a value not placed.
- **A decoded-kind value past the cap makes its block `Unsorted`** as well as
  leaving its group unbounded, reading "as a value past the cap does" in the
  spec's "What is gathered, and what is derived" literally.
- **Resolution uses an empty census**: it moves only an array column, and a
  nested column gets neither bounds nor a dictionary.
- **A header-less block gathers groups and no column**; a row shorter than its
  header observes nothing for the missing columns, so NULLs and values count
  short of `rows`, which the evaluator reads as a possible value.

## Negative results

- **No figure was taken**: gathering's cost and resident growth are unmeasured
  until `statistics-gathering`. The `census-*` figures' pinned
  `runs/pgdq-nocensus` predates `--statistics` and rejects it; its stamp guard
  refuses it anyway once this change is committed, paths those figures declare
  having moved.
- **Four hand mutations each failed a new test**: the gathered-block decline
  removed, `Clipped::locate` answering `Less` past a partial head, `character`'s
  successor allowed a blank, and a `numeric` comparison reversed under the
  golden-order digest.
