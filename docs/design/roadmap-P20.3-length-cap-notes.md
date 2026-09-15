# P20.3 — The per-block length cap: notes

The cap is `statistics::STATISTICS_GROUP_CAP`, reached through
`StatisticsRequest::group_cap` and `StatisticsBackfill::group_cap`; the merge is
`gather::Gatherer::fit_cap` and what it calls. Why it is shaped that way is
[`decisions.md`](decisions.md), "D82"; the user-facing statement is
[`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics`:
what `parse` records for later queries", and the flag's help.

## The checks

- `gather.rs`'s `a_capped_block_gathers_what_the_size_it_reaches_gathers`:
  random blocks at caps of one to six groups, gathered serially, by pieces
  folded a window at a time with one left alive past the block's end, and
  exactly at the size the capped pass reached — all three equal, the account
  holding only the retained block afterwards. The random blocks are the join
  test's, now shared.
- `tests/statistics.rs`'s
  `every_fixture_block_past_its_cap_gathers_what_its_final_size_gathers`: every
  block of every fixture re-read through `gather_block_statistics` at 8 bytes
  under a cap of two, serially, exactly at the size reached, and split by the
  leader at three and eight workers; most blocks merge.
- **Neither test is vacuous**: each of six deliberate breaks fails the unit
  test — a union ignoring `DICTIONARY_CAP`, no renumbering, a merge while a
  piece is alive, a reopen not moving the open group's start, a bounds merge
  keeping the first group's, and no reopen.
- The `introspect` reconciliation (`pgdump_query-cli/tests/statistics_account.rs`)
  passes over the changed closed-group shape,
  `runs/statistics-account-20260915-heads/log.txt`, but **no leg of it merges**:
  its stated-size legs are exact and its flagless legs hold far fewer groups
  than the cap.

## Negative results

- **Merging stored bytewise bounds is not exact.** A clipped upper bound does
  not order as its value: `a`×300 stores `a`×252 then `b`, which sorts above
  `a`×254 then `b` — a larger value stored whole — so a merge picking the larger
  stored text keeps the smaller value's inexact bound where one pass over both
  groups keeps the larger value exactly. A closed group keeps its extremes'
  heads until `finish` clips them.
- **Merging only at an even group count leaves a block stuck past its cap**: a
  window's fold can leave the count odd, and so can every window after it. The
  last closed group is taken back into the open one instead.
- **A merge with a piece alive breaks the join**: the piece gathered at the
  finer size, and the break above is what the unit test catches.

## What the next slices inherit

- **A merge is exact against gathering at the coarser size**, so 20.4's density
  rule can coarsen further at a block's end by the same `merge_pairs`, after
  `fit_cap(true)`, a last group standing alone.
- **A dictionary renumbers on every merge**, walking every group's indices, and a
  keyed column re-keys each pair's stored bounds. Neither cost is priced;
  `statistics-gathering` is re-taken in 20.9.
- **A block can hold more groups than its cap while it gathers**: two past it
  while a long row's empty groups are listed, and a window's groups — already
  held by the window's pieces — until the window's last piece folds.
- **A merged block keeps the capacity its vectors grew to before merging**, and
  the account charges capacity; nothing shrinks at `finish`.
- **A back-fill under an unstated size re-reads a held block exactly at the size
  it holds** (`group_cap: None`), which is what an unstated size lacking a column
  asked for before; 20.4's recorded bounds decide it per bound.
- **Merges are outside every account reconciliation leg** (above). 20.8's
  attribution legs gather flagless and so reach the cap only on an input past
  4 GiB a block.
