# P33.5 notes — the floor's refusal

**A line one reader could not hold is refused for memory, and nothing else
is.** `Arrangement::resolve` gives every limited ledger a `Floor`
(`one_reader_leaves`, D161); a draw past it answers `Admission::Refused`,
which `draw_absorbing` and the replay's re-draw turn into
`Error::LineOutgrowsMemory`, and `TablePartitions::new` holds the longest
line its read holds (`held_line`, `Table::line_charge`'s line) against it
before reading a byte. `pgdt` spells the remedy `--memory`, the shell
`SET pgdump.memory`. The spec's row was four reviews — this refusal, the
pools shrinking, and parking in two read paths — so it landed alone; 33.5.1
to 33.5.3 carry the rest (STATUS, and the 2026-11-14 history entry).

## What the next slices inherit

- **Until readers park, a line between the room and the floor is read past
  the room**, beside readers that keep their terms, as the earliest line
  alone (D157): the overshoot is bounded by the floor now, where it was
  unbounded. 33.5.2 and 33.5.3 make it honest by parking readers down to
  one, which is the count the floor was carved at.
- **The floor is a number the arrangement fixes**: `Ledger::floor`. Parking
  changes how a line reaches it, never where it stands, so a refusal never
  depends on `--jobs` or on what the other readers hold when the line comes.
- **The floor is at least the room**, so no line the ledger lends room for
  is refused: below the crossover the margin's ceiling lends past the cap,
  and the floor takes the higher of the two.
- **A refusal returns the line's draw** (`Holder::draw` releases), waking a
  line behind it; the reader's error then ends its pass.
- **A test states a floor by `pgdt_fixtures::floored(holds)`**: one reader
  under a gibibyte others drew down to it, with no room, every admitted line
  the earliest alone. pgdt-mapping's tests carry their own copy, the
  fixture's `mapping` feature being out of their reach.

## Negative results

- **A streamed row can be refused** (`KD170`): its first draw is an upper
  bound of two read chunks (33.4.6's), so under an allowance near
  `MEMORY_RESERVE` a pass holding none of the row fails. The tests that
  read a no-field plan under a floor state one of two chunks.
- **Below about the reserve plus the margin's bound the floor is zero**, so
  every line past its chunk is refused there, as the spec's "The floor is
  one reader" reads. A test wanting a ledger of no room carves its room
  away under a gibibyte, never a small allowance, whose floor is zero.
- **A provider's pool grant does not come off the floor**: it can pass its
  allowance (`pgdt-sql.md`'s example), which would refuse every long line.
  The statistics and other scans' line charges do, since 33.30.
- **The refusal names where what is held begins**: a held line's start, or
  a streamed row's field held whole, which a data-level parse collects past
  its chunk — `tests/ledger.rs` in pgdt-mapping refuses a zero-padded
  `bigint` at the field's start.
- **No count pair**: the draw gained one comparison a chunk, and a plan one
  pass over its blocks' longest lines (D108's trigger is per row or byte).
