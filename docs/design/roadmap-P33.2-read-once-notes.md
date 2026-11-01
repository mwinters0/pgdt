# P33.2 notes — each line read once

**An interior worker reads each byte of its piece once, and a query's segment
searches for its first row only to its own limit.** No figure was taken: the
slice changes reads, not what is held, and `long-line-resident` is re-taken at
33.3.

- **The worker carries its line in the serial loop's carry.** `scan_partition`
  (`pgdt-mapping`'s `interior.rs`) reads from where its last read ended and
  scans each read through `pgdt-dump`'s `ChunkCarry`, now reached through
  `pgdt_dump::internal` (`scan_read`, `interior/piece.rs`): the carried line
  completed by the read's first line-terminated prefix, then the rest in
  place, each a `scan_piece` of its own. A read therefore yields up to two
  `PieceScan`s; `merge` and `BlockEndings` fold any count.
- **No worker read passes the chunk**, or on a `Whole` source the first read's
  unit; the growth branch of both read-size tests is gone, and
  `a_worker_reads_a_line_longer_than_a_read_once` asserts each read begins
  where the last ended.
- **A worker's line limit is measured as the serial loop's**: a carry past
  `max_line_bytes` after a read short of the file's end, refused at the line's
  start. A worker's old refusal was a doubled read reaching the limit with no
  line boundary in it.
- **A resyncing piece that finds no row start before its limit stops there**,
  where it read one chunk further before
  (`a_piece_inside_a_line_reads_its_own_range_alone`).
- **A segment's search no longer refuses a long line.** `first_row_start`
  searches `[start, min(limit, block end))` and holds one chunk at a time, so
  only the segment owning a line measures it (`pgdump.max_line_bytes`'s
  refusal in `datafusion-pgdump`'s `settings.rs` test is the owner's).
  `a_segment_inside_a_line_reads_only_to_its_limit` bounds a cut replay's
  reads to twice the file and a chunk a partition, which the search to the
  block's end fails.

## What the later slices inherit

- **The serial loop and the worker hold a line in one type**, `ChunkCarry`,
  so 33.3.1's streaming of a line no pass decodes, and 33.3's freeing after
  the row, are each one change for both. A worker's carry lives as long as its piece, a
  window at most, and keeps both buffers at the longest line it carried until
  then, as the serial loop's does for the whole pass.
- **A worker on a block at the metadata level is handed no census and no
  observer** (`scan_piece`), the leader's case of the pass 33.3.1 streams.
- **The leader's four-worker readings in `long-line-resident` were taken over
  the doubled re-read** this slice removes — one read buffer of up to twice
  the line, where the worker now holds the carry's two `Vec`s, grown by
  `Vec`'s doubling as the serial loop's are. Nothing says which holds more
  resident; 33.3's re-take reads the four-worker column over both of its
  changes at once, and this slice alone over none.
