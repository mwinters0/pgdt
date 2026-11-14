# P33.4.1 notes — a parse draws

**A parse's readers and its statistics draw on the run's ledger.** Every
read loop of `map_file` — the forward pass's serial loop, each interior
worker, and the back-fill's serial re-read — draws what its carry would hold
past its chunk before absorbing each chunk (`pgdt-dump`'s `draw_absorbing`,
D158), and the statistics account draws its total at every update
(`StatisticsAccount::charging`, D157). Nothing a run answers moved: under
today's line limit the earliest line is still admitted alone past the room
([`roadmap-P33.4-ledger-notes.md`](roadmap-P33.4-ledger-notes.md)).

## What the next slices inherit

- **One draw for every carry**: `ChunkedScanner::next_chunk_folding` takes
  the reader's `Holder`, and an interior worker calls `draw_absorbing` in its
  async half, before the blocking half absorbs. The charge
  (`ChunkCarry::charge_absorbing`) is the bytes held once past the chunk —
  the line completed or joined whole, or a streamed row's held field grown to
  its next delimiter — with no factor: 33.4.2 multiplies a replay's by
  `ReplayPlan::factor`. A draw of zero returns the last.
- **A replay keeps its line past the next absorb** — a batch holds the frozen
  `Bytes` — where a parse frees it there, so a replay's draw cannot be
  returned at its next chunk as a parse's is. 33.4.2's.
- **A held field is a line of its own to the ledger**, keyed by where it
  starts (`ChunkCarry::holding_from`), so a row's next held field returns the
  last field's draw rather than keeping it.
- **A holder suspended on work a later reader does must hold nothing**: the
  forward loop returns its draw before handing a region to workers, its line
  being earlier than all of theirs. The same holds for any reader 33.4.2
  parks: a stream left unpolled keeps its draw, which is the polling contract.
- **Giving up re-reads by position**: what a carry holds is always the suffix
  of what it has read (`GivenUp::range`), a CR a streamed row holds back
  excepted, so a re-read needs no record of where the line began.
- **A query's mapping pass draws on a ledger without limit**: `Table::settle`
  is handed `ScanOptions`, no arrangement; 33.4.2 hands it one, a query's
  census drawing as its held line does
  ([`roadmap-P33-line-bound.md`](roadmap-P33-line-bound.md), "A pass holds a
  line only where it decodes it").
- **`backfill_block`'s arrangement is carved around the block it reads**: its
  account loads the block's statistics and its first update replaces the
  `held` the ledger opened at, as `map_file`'s replaces the cache's, which
  `pgdt parse` carves around (`cached_statistics`).

## Negative results

- **No count pair was taken**: no row's or byte's path changed. The draw is
  once a chunk, and a carry holding nothing past its chunk answers zero
  without touching the ledger's lock; the account draws once an update, which
  is once a charge step or a group's close, not a row (D108's trigger).
- **The contention test is an equivalence, not a count**: `tests/ledger.rs`
  reads the fixtures at four workers on a ledger with no room against one
  reader under it. Both waits and give-ups occur there, checked with a
  temporary probe this slice does not keep.
- **`scan/chunked.rs` grows further past its soft function limit**
  (`file_limits.py`), a meter.
