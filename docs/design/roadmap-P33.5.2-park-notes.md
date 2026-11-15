# P33.5.2 notes — a parse's readers parked

**A line alone past the room parks the readers after it, the latest first,
and is admitted once they have** (D163). Each reader of a parse — the forward
pass's, an interior piece's, the back-fill's — stands where it reads before
each read (`Holder::stand`, through `Drawing::parking` in `draw_absorbing`);
one asked drops the chunk it read, parks holding no read (`Holder::park`,
which holds a `ParkedReader` on its source), and reads the chunk again as it
resumes. The ledger asks as few readers as `Terms` says cover the line's
excess, and lets them go, the earliest first, as the excess falls — at the
latest when the line's draw is returned.

## What the next slices inherit

- **Parking is opt-in per drawing**: `Drawing::held` and `Drawing::charged`
  never stand, so the replay (`Drawing::charged`) parks nothing and is
  never asked. 33.5.3 opts it in, within `TablePartitions`'s polling
  contract: a sub-stream left unpolled mid-park keeps the line waiting on it,
  where a parse's pieces are all polled by the window's `FuturesOrdered`.
- **The line waits only on readers still reading.** A reader whose own line
  waits, one before the line, and one that never stood are never asked; one
  that leaves (its holder dropped, a piece done) stops being waited on. So a
  line waits at most one read of each asked reader, and a parse cannot
  deadlock on a park.
- **What a park gives back is the carve's fall**, `Terms`: the arrangement's
  budget at its count less at that many fewer, each clipped to the budget. A
  source stating no worker memory — a plain file — gives back nothing, so its
  line is admitted past the room at once, as before this slice; a stated
  count past what the budget affords gives back nothing until the readers
  left reading are fewer than it affords (`KD21`'s marker).
- **Once every asked reader has parked and the excess is still uncovered**,
  the line is admitted up to the floor (D161): below the crossover the
  floor is the cap, which no count of parks reaches.
- **Positions are where a reader last stood**, its read's start: a parked
  reader's is frozen, and a window's pieces read disjoint ranges, so the
  latest stay the latest.

## Negative results

- **A reader's carry is not given back**: a parked reader keeps any line
  under its chunk term it carries, its own buffer, which the per-worker term
  already holds.
- **No count pair**: a reader stands once a chunk, a lock and a sort over
  the run's readers, and nothing a row or a byte runs changed (D108's
  trigger).
