# P33.5.3 notes — a query's readers parked

**A replay's sub-stream parks as a parse's reader does** (D163): it stands
where it reads before each read, and one asked parks at that read holding no
read, resuming as the line that asked is admitted or returned. Two things a
parse's reader never meets shape it, both from its batch and its yields.

- **A batch holding rows is emitted before the park.** Its views pin the
  chunks they were read from (D46), so a sub-stream parked with rows in its
  batch would keep reads alive that `Terms` credits as given back (D162).
  With rows held, the replay stands itself before the read
  (`ChunkedScanner::read_pos`) and draws without parking; asked, it flushes,
  yields, and parks at its next read, whose drawing parks
  (`Drawing::parking`) since its batch is empty. Filed under "Decisions worth
  another look": the batch's cut then depends on when the park was asked.
- **A sub-stream stands aside before every yield** (`Holder::stand_aside`),
  so it is asked nothing while suspended there and a line asking it stops
  waiting on it. Without it the merge in file order stalls: past its first
  round it polls only the sub-stream it just emitted from (`KD57`), the line
  in that one asks the later ones, held back at a yield, and none ever
  parks — checked by removing it, `public.t_bytea` on 13 stalling.

## What the next slices inherit

- **A sub-stream held back at a yield is not parked.** It keeps what it holds
  there — the read chunk it yielded inside, its carry, its share of the
  pools — and a line alone past the room asks only the sub-streams still
  reading, so under `pgdt query`'s merge past its first round, or a
  DataFusion merge holding an input's batch, that line is admitted past the
  room beside those terms, up to the floor (D161), which is one reader's.
  `TablePartitions`' rustdoc states it as the polling contract's limit.
  Nothing parks on a plain file (`Terms` is empty there), nor in a query
  under a carved allowance, whose longest line `Arrangement::resolve` leaves
  room for (D160), so 33.7's query run over the `.xz` reference dump, stating
  a `--memory`, meets none of this; a query parks only under an uncarved
  estimate.
- **The ledger waits only on readers between a stand and a yield.** A
  sub-stream searching a piece's first row start reads without standing,
  and the re-draw after a cut-before yield draws without standing, so
  either parks only at the replay's next read, where it stands.
- **The test wrappers**: `tests/ledger.rs` in pgdt-query carries its own
  `Parks` (pgdt-mapping's twin, a plain file stating a worker cost) and
  `Gated` (a dump in memory whose reads past an offset wait on a flag), the
  fixtures crate holding no source wrapper.

## Negative results

- **No count pair**: a sub-stream stands at most once a chunk, a lock and a
  sort over the run's readers on a limited ledger and nothing on an unlimited
  one, and nothing a row or a byte runs changed (D108's trigger).
- **Each new test fails on its parent**: no sub-stream parks there, and a
  batch is never cut short by a park.
