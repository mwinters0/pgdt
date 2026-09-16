# P20.7 — The decline: notes

The allowance is `io::statistics_allowance`, reaching a pass as
`ScanOptions::statistics_allowance` and bounding `StatisticsAccount`; the
decline is `gather::Gatherer::decline`, its answer `statistics::BlockGathered`,
and its record `index::CopyBlock::statistics_declined`, which
`StatisticsRequest::backfill` now takes the pass's allowance to read against.
`stream::report_statistics_declines` is what every run under an allowance says,
and `MapRun::declined_statistics` what it counts. Why it is shaped this way is
[`decisions.md`](decisions.md), "D85"; the user-facing statement is
[`../manual/dump-inspection.md`](../manual/dump-inspection.md),
"`--statistics`: what `parse` records for later queries" and "`--jobs` and
`--memory`: the workers and the allowance". `FORMAT_VERSION` is 22.

**`KD28` is struck**: the margin is now left against the statistics account as
well as the pools' charge, so nothing grows with the dump outside it.

## What the next slices inherit

- **The allowance is `margin_allowance(allowance) − budget`, and
  `MEMORY_RESERVE` is *not* subtracted again.** The reserve carves the *cap* a
  worker count is solved against; the margin ceiling already stands
  `MEMORY_UNPOOLED_BOUND` below its fraction of the allowance. The two are
  different rules over different quantities — the reserve bounds what the pools
  may spend, the ceiling what the charge may reach — so the reserve has no
  business in this one (D85's `Rejected` line). 20.8 moves `MEMORY_RESERVE` and
  so moves the buffer budget, and the statistics allowance moves with it
  through the budget alone.
- **Nothing is gathered at all below `1.25 × MEMORY_UNPOOLED_BOUND`** — 320
  MiB — where `margin_allowance` saturates to zero on its own, whatever the
  budget. That is the line the CLI decline test states an allowance at to get a
  decline out of a small fixture.
- **Above it the two bounds swap which one binds, and the starving band is the
  wide one, not the narrow one.** `Parallelism::fit` solves the count against
  `cap.min(ceiling)`. Below `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` — 640
  MiB — the cap binds, so what is left for statistics is at least `ceiling −
  cap`, strictly positive: 20.9's 512m gates have a guaranteed floor of 25.6
  MiB and cannot be starved. At and above 640 MiB the ceiling binds, the count
  is solved right up against the number `statistics_allowance` subtracts from,
  and statistics get only the slack one worker's step leaves — so a wide host
  at a high `--jobs` is where every block declines. Both lines are a
  consequence of the two constants rather than a choice this slice made, and
  **20.8 moves both** by moving the reserve. What to do about the wide band is
  open: it revises the spec's "workers are resolved first; statistics take what
  is left", so it is a grilling's to settle, not a slice's
  ([2026-09-16](../status/history/2026-09-16.md)).
- **The check runs where the charge updates, not per row.** `Gatherer::over` is
  set by `charge_held` — at every group close and once a row's growth passes
  `CHARGE_STEP` — and read at a row's end, at a fold's and at the observer's
  construction. So the account can stand over the allowance by one step plus
  one row between the update that passed it and the decline, which is the same
  window D81's tolerance already names. `Charge::ahead`'s announcement does not
  itself decline; the next `charge_held` sees the growth it announced.
- **A pass whose account is already full declines each later block before its
  first row**, `observer_tracking` checking at construction. So a long dump past
  its allowance costs one charge and one release a block, not a gather.
- **A decline is not exclusive with statistics.** A re-read that declines keeps
  whatever the block already held and records the allowance beside it, because
  the re-read's own observer freed its own and has nothing to put there. A
  re-read that *succeeds* clears the record, so a later, tighter request is not
  skipped by a decline an earlier allowance caused.
- **The account is bounded; the process is not.** The allowance bounds
  statistics alone. Whether the margin it leaves is the margin the process
  actually leaves is 20.8's reading, and this slice takes none.
- **`info`'s text views do not name a decline; `info --json` does.** A declined
  block reads as a block with no statistics in `--detail`'s rollup, and
  `statistics_declined` beside `statistics` is what tells it from a block
  nobody asked about. The remedy a person has today is to run `parse` again,
  which prints the line whether or not it re-reads anything, so this is a
  property of the shape rather than a deficiency; a text view for it is
  out-of-band work if the CLI-feedback pass wants one.

## Negative results

- **A block's peak is not its retained heap, and an allowance read off the
  retained heap does not decline it.** The first unit test set the allowance to
  a quarter of `BlockStatistics::heap_bytes` and the block gathered fine: the
  interning maps are a term of the account that the retained heap does not
  carry, and on a small block most of the growth the account sees arrives in
  `finish` — the last groups closing and the dictionaries being charged — after
  the last row. The test now reads its allowance off an unbounded control run's
  own `StatisticsHeld::peak`, and the integration test states a group size well
  under the block so that the growth is row-driven and the decline is the
  mid-scan one rather than the one `finish` would have made anyway.
- **Declining inside a merge was refused.** `charge_held` runs from inside
  `merge_pairs` and `close_through`, where the groups and each column's vectors
  are half rebuilt; freeing there would have to know which half. `over` is
  recorded there and acted on at the next whole boundary instead, which is why
  the flag and the decline are separate steps.
- **A declined piece has to decline its block, not be skipped.** A piece that
  freed its own rows leaves the block unable to state anything over the rows it
  lost, and the block's groups are dense (every index from the first), so a gap
  is not expressible. `absorb` therefore drops the piece and declines the
  block; the other direction — a block that declined while its window ran —
  drops every piece it is handed, and `piece()` hands out pieces already
  declined so the ones still to come hold nothing.
- **The decline is not deterministic between a serial and a parallel pass, and
  it is not meant to be.** The account sees a window's pieces as well as its
  blocks, so the same file at a higher `--jobs` can decline where a serial pass
  did not. The spec allows it — a cache is a function of the file and the
  request "wherever it holds statistics", the decline being an absence and a
  number — and `pgdump_query-cli/tests/determinism.rs`, which compares serial
  and parallel caches byte for byte, states no allowance and so declines
  nothing.

## What is unpriced

- **What the check costs.** One `bool` off an update already taken under the
  account's lock, and a branch a row. No figure takes it; `statistics-gathering`
  is re-taken in 20.9 and covers it along with everything else 20.1 added.
- **What a decline saves.** A pass that declines every block does less work
  than one that gathers, and no figure says how much. 20.9's generated gates
  and 20.10's koji run are where a real decline is exercised at all.
