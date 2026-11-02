# P33.3.2 notes — a replay reading no field streams a line

**A replay whose batch, filter, sorted stop and dynamic filter read no field
of a row streams it once it outgrows its chunk**: `pgdt query --no-columns`,
and a `COUNT(*)` DataFusion does not answer from the map's row count, hold no
line at any length ([`decisions.md`](decisions.md), "D138"). The carry counts the row's
fields as its bytes go by and the batcher holds them to the block's width, as
it holds a split row's (D28). No figure was taken: no leg of
`long-line-resident` reads no field.

- **The carry counts fields** (`pgdt-dump`'s `scan/chunked.rs`):
  `StreamedLine::read` adds each run's delimiters
  (`pgdt_core::internal::delimiters`, counting as `split_fields` splits), and `StreamedRow::fields` carries the
  count out.
- **The replay asks at each chunk** (`pgdt-query`'s `stream/replay.rs`,
  `reads_no_field`), on the block's `CopyStart` and before each later chunk's
  events, under the dynamic state `at_chunk` just read:
  `ChunkEvents::stream_rows` now takes the answer, so a reader whose answer
  moves says so before the line it bears on is carried. A state that moves
  to one reading a field while a row streams leaves that row kept, which a
  dynamic filter's contract allows.
- **A streamed row** is asked of the filter as a tree reading no row
  (`ResolvedExpr::matches_unread`) and taken by `RowBatcher::push_counted_row`,
  whose width check is `push_row`'s (`hold_to_width`).
- **What proves it**: `crates/pgdt-query/tests/streamed_replay.rs` reads a
  sixteen-MiB row with no column projected holding nothing near it resident,
  where projecting its column or filtering on another holds it, and refuses a
  row a field wider or narrower alike streamed or held;
  `dynamic_filter.rs`'s `a_row_the_state_reads_is_held_though_no_column_is`
  drops the rows a field-reading state rejects with nothing projected. The
  carry's tests hold a streamed row's field count to `split_fields`' at every
  chunk size. Each fails with its half of the change removed.

## What 33.3.3 inherits

- **`StreamedLine::read` is where a row's bytes go by**, a run at a time; the
  field a run starts in is the count so far less one. The census's fold over
  byte ranges hangs there, and a censusing pass decides as the forward pass
  does, once per block on its header.
- **A cold query still holds every long line of every block its mapping pass
  censuses** (`forward.rs`, `Pass::Query`), the table it reads included: only
  its replay streams.

## Negative results

- **No count pair was taken**: the row path changed (`push_row`'s width
  check and span update moved into `hold_to_width` and `count_row`, the
  flush check out of the `Row` arm to after the event), and P33's spec adopts
  no pricing of a hot-path change ([`decisions.md`](decisions.md), "D108").
- **Deciding once per block** would answer under the state read at its
  header for the rest of it — before a state is first read, streaming every
  long row and dropping none — where the state moves at each chunk (D93).
