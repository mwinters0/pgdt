# P33.4.2 notes — a query draws

**A query's two passes draw on their own ledgers.** The replay draws a line
held past its chunk term through the carry's draw (`Drawing::charged`, D158),
charged its bytes and its query's one factor of them (`ReplayPlan::factor`);
the line's row ends its batch, and every draw is returned before a batch is
yielded (D159). `Table::settle` takes the mapping pass's arrangement, as
`map_file` does, so a cold query's census draws on that pass's ledger.
Nothing a run answers moved but where batches are cut: under today's line
limit the earliest line is still admitted alone past the room
([`roadmap-P33.4-ledger-notes.md`](roadmap-P33.4-ledger-notes.md)).

## What the next slices inherit

- **A plan's ledger is handed to it**: `TablePartitions::drawing_on(&Ledger)`,
  a plan handed none drawing on `Ledger::unlimited()`. `pgdt query` hands the
  replay's arrangement's (`Resolved::arrangement`); the provider hands none,
  which 33.4.3 carves per scan inside its `ScanBudget` draw. Every sub-stream
  of a plan, and every run of it, shares that one ledger, a holder each.
- **`Table::settle(opened, selector, read, &Arrangement, …)`**: the pass's
  `ScanOptions` are built from the arrangement's parallelism, as `map_file`
  builds them. Test helpers keep a `ScanOptions` and resolve it under no
  allowance at its count (`tests/common`'s `Cold::table`), so its budget is
  the library's default, as `pgdt_fixtures::arranged` has it.
- **The charge is `held + factor.of(held)`**, the plan's one factor folded
  once per sub-stream, so a query reading a `text` column beside a `bigint[]`
  is charged `bigint[]`'s for every held line. `line_charge` is still zero
  everywhere: 33.4.3's.
- **A sub-stream suspended at a yield holds no draw**, so the polling
  contract `TablePartitions` states is the ordinary `Stream` one: a stream
  returning `Pending` is polled again on wake. Only a stream mid-line holds,
  and only a caller that stopped polling one before its next item — which
  neither `in_file_order` nor `CoalescePartitionsExec` does — stalls an
  earlier line. 33.5's parking inherits this: a replay waiting on the ledger
  is mid-poll, its batch's retained chunks still pinned.

## Negative results

- **A drawn line ends its batch on every ledger**, an unlimited one
  included, so `batch.rs`'s `a_long_value_is_a_view_into_its_carried_line`
  now reads two batches where it read one. The cut is armed at the chunk
  (`RowBatcher::holds_drawn_line`), not tested per row: the flush trigger it
  sets is `holds_long`, which `should_flush` already reads, now only on a
  batch holding a row, since the drawn row may be one a filter drops. So no
  row's path changed and **no count pair was taken**, as in 33.4.1; the draw
  is once a chunk.
- **A drawn row a filter drops keeps its bytes until its chunk's events
  end**, in the carry and the retained chunks, uncharged across any yield
  in between, its draw returned there. At most one line a sub-stream.
- **A cut before a long nested row draws its line again once the earlier
  rows are out**, waiting with the line in its carry: the carry pass is
  mid-scan, so it cannot give it up and re-read. Nothing asks it to, since
  it holds no draw while it waits.
- **`tests/ledger.rs`'s contention test needs four-byte reads to give a line
  up**: at sixteen bytes the fixtures' lines waited and none was given up,
  each ending at its next absorb before an earlier line asked. Both occur at
  four, checked with a temporary probe this slice does not keep.
- **A `jsonb` term's number is not the factor's**: it is keyed whole, held
  as a short value's decode is on any line, so the ledger drawing the factor
  left nothing of it charged short (`JSONB_KEY_FACTOR`); `KD168` is struck.
