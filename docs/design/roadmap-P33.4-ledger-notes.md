# P33.4 notes — the ledger

**The ledger stands alone: carved, drawn, ordered and tested, and no pass
draws from it yet.** The spec's row was three reviews — a new module, the
mapping passes' read loops and the replay's — so it landed as the first, and
33.4.1–33.4.3 carry the rest (STATUS, and the 2026-11-13 history entry).
`Ledger`, `Holder` and `Admission` are `pgdt-io`'s (`ledger.rs`); the carve is
`Arrangement::resolve`'s (D157).

## What the next slices inherit

- **A holder draws a line's whole charge, never its growth**:
  `Holder::draw(start, charge)` with `start` the line's file offset, which is
  the order it waits in, and the charge everything the line holds past its
  chunk term so far. A draw no larger than what it holds is met at once, so a
  reader may draw at every chunk without asking whether it grew.
- **`Admission::GiveUp` has released the draw already**: the reader drops its
  line and re-reads from where its draw began, drawing anew from zero. A holder
  asked to give up hears it only at its next draw, whatever that asks for;
  `release` after a row is emitted returns the draw without one. A reader that
  never draws again past being asked keeps its draw until it releases, which
  is the polling contract 33.4.2 states on `TablePartitions`.
- **A draw for another line releases the last**, and a dropped holder
  releases: one holder per reader, kept across its lines.
- **The wait is a future, `Send`, on the ledger's one lock**: it can sit in
  an interior piece's async half (`scan_partition`'s read loop, never the
  `spawn_blocking` closure, which cannot await) and in a replay's
  `poll_next`. Dropped unsettled it stops waiting and keeps what it held.
- **`ArrangementRequest::line_charge` is the read's longest line's charge**,
  pgdt-io seeing no model: 33.4.3 hands it the charge pgdt-query computes from
  the map's `LongestLine` and the query's factor. It comes off the readers'
  cap and ceiling as `drawn` does, and off the statistics allowance; every
  caller passes zero today, so no arrangement moved.
- **The earliest line is admitted past the capacity where nothing else is
  drawn.** Until the floor (33.5) parks readers and refuses, a line the ledger
  cannot hold is read as it is today, never stopped: so 33.4.1 and 33.4.2
  change nothing a run answers, only when its readers wait.
- **`Arrangement` is no longer `PartialEq`**: it holds a run's shared state.
  Nothing compared two.

## What was weighed

- **Wake every waiter on a change**, each re-checking its own admission,
  rather than waking the next in order: a pass has tens of readers, and the
  strict order is in the admission test (no earlier line waits), not in who is
  woken. A woken later line finding an earlier one still waiting stays put.
- **Only lines holding something are asked to give up**, and one asked
  already counts toward what the earliest needs, so a second waiter's ask
  does not empty the file behind it.
- **The capacity is the cap's, not the margin's**, which 33.4.4 moves under
  the margin's ceiling, the cap left to the floor
  ([`2026-11-14.md`](../status/history/2026-11-14.md), "The ledger lends under
  the margin's ceiling; the cap is the floor's").
