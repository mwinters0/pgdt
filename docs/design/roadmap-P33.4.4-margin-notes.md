# P33.4.4 notes — the ledger under the margin

**The ledger is carved under the margin's ceiling and the statistics draw on
it first; no pass draws yet.** `Arrangement::resolve` carves it wherever an
allowance is found as what the ceiling leaves the readers' budget, less what
others drew (`memory.rs`'s `margin_room`), and a read's longest line
bounds the count off that ceiling alone (`Parallelism::within_shared`'s
`line`). The cap is left to the floor
(33.5), as the 2026-11-14 history entry settled (D157).

## What the next slices inherit

- **Nothing a run answers moved.** With `line_charge: 0`, a mapping pass's,
  every count, budget and statistics allowance is what it was:
  the statistics allowance is the room less the line, the old formula
  exactly. Only `Ledger::capacity` moved, which nothing reads.
- **`Ledger::draw_statistics(total)` is the statistics' whole draw**, in
  place of the last, synchronous, never waiting or declined: what bounds
  statistics stays the account's decline against
  `Arrangement::statistics_allowance`. 33.4.1 calls it with the account's
  total at each update.
- **The draw opens at `held`**: the arrangement was carved around it, and in
  a parse it is the cache's statistics, which the account charges as
  `Term::Loaded`, so the account's first charge replaces it. 33.4.1 checks
  that each pass's arrangement was carved around exactly what its account
  loads, `backfill_block`'s included. In a query's replay it stays the draw
  for the run, nothing replacing it.
- **The earliest line is admitted where no other line holds**, whatever the
  statistics hold, since they give nothing up; and statistics passing the
  room ask every later holder, latest first, never the earliest line.
- **`Ledger::drawn` counts the statistics** beside the lines;
  `Ledger::statistics` is their draw alone.
- **A line at the count's floor is not budgeted for**: `line` lowers the
  count and never one reader's budget, where `held` does. Placing the
  earliest line under the cap at one reader is 33.5's.
- **An uncarved estimate's room can hold less than the line**: its readers
  are fitted to the halved cap less the line, not to the ceiling, so the
  line fits beside them under that cap and the ledger admits it as the
  earliest alone. Only `pgdt query`'s replay meets it, gathering nothing,
  the provider carving its estimate (D157).

## What was weighed

- **Taking `held` off the room** instead of opening the draw at it: every
  parse's statistics allowance would fall by its cache's statistics, which
  the account counts against it already (D85), and a back-fill moving them
  from loaded to retained would bill them twice. A replay's lines get the
  same room either way, its settled statistics being resident (D157).
- **An uncarved estimate's room carved from the halved cap**: every flagless
  pass could hold the whole half plus `MEMORY_UNPOOLED_BOUND`, where the
  margin keeps one whose count `--jobs` bounds at four fifths of it, so two
  runs at once could pass `MemAvailable` (RT8). **Solving its count against
  the ceiling only where a line is charged**: a one-byte line would cost a
  fifth of the estimate and `MEMORY_UNPOOLED_BOUND` in readers.
- **Statistics asking the earliest line too**: it is admitted again alone,
  so the ask buys a re-read and no room.
- **Waking waiters when the statistics rise**: nothing can be admitted that
  was not; only a fall wakes them.
