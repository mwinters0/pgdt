# P33.3.1 notes — a pass reading no row streams it

**A block whose rows nothing reads past their count streams a row that
outgrows its chunk**: a metadata-level parse holds no line at any length,
serially and in its interior workers ([`decisions.md`](decisions.md), "D138").
A piece's resync and a segment's row-start search already held nothing (the
33.2 notes). A query's census holds what it held: it reads its rows' fields,
and streaming it is 33.3.2, below. No figure was taken: no leg of
`long-line-resident` is a metadata-level parse.

- **`ChunkCarry` streams, the serial loop and the worker alike**
  (`pgdt-dump`'s `scan/chunked.rs`): told that the line it carries is a row
  its reader does not read (`Passes::streams`), it folds that row into a
  `StreamedLine` once a chunk holds no LF of it, and the chunk ending it hands
  the row to the carry pass in place of a held line. The serial loop is told
  by `ChunkEvents::stream_rows` on the block's `CopyStart`, which the forward
  pass calls where `Builder::reads_rows` is false; the worker by its block
  having no census and no observer (`interior/piece.rs`, `scan_read`).
- **A streamed row is an event of its own**, `Event::StreamedRow`, its
  offset, index and length, handed out by `CopyScanner::streamed_row`, which
  holds its ending to the block's first row's as a row's is (I91). A pass
  that reads its rows never meets one.
- **What proves it**: `crates/pgdt-mapping/tests/streamed_line.rs` maps a
  sixteen-MiB row at the metadata level, at one worker and at four, with no
  allocation near the row, where a data-level parse makes one past it; each
  half fails with the forward pass's or the worker's call removed. The
  carry's tests hold a streamed row's events, refusals and endings to a held
  one's at every chunk size, and `a_row_ending_found_in_pieces_is_the_whole_lines`
  holds the ending found a run at a time to `RowEnding::of`.

## What 33.3.2 inherits

- **A census reads every field of a row**: `BlockCensusExt::observe_row`
  splits a row holding a `{` or `[`, or one whose block counts a column, and
  `FieldCount::tier` decodes a counted field whole. Streaming it reads each
  field's head as the row goes by, and needs the counter's answer for a field
  longer than any head it keeps; `StreamedLine` is where a row's fields would
  be folded as its bytes go by. STATUS's "Decisions worth another look" holds
  whether that is worth building.
- **A query's mapping pass censuses every block it maps**
  (`forward.rs`, `Pass::Query`), so a cold query holds each long line of a
  table it does not read, which its replay never would.

## Negative results

- **A replay decoding no field still holds its line**: `pgdt-query`'s replay
  skips a row's validation where its batch, filter and dynamic filter read no
  field, and still takes the row whole; it was not streamed, the spec naming
  no replay among the passes that stream.
- **No count pair was taken**: the row path changed (`Endings::hold` lifted
  out of `CopyScanner::next_event`, a `PassEnd` from `ChunkEvents::end_pass`),
  and P33's spec adopts no pricing of a hot-path change
  ([`decisions.md`](decisions.md), "D108").
