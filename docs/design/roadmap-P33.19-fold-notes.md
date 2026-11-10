# P33.19 notes — a parse streams a line

**A data-level and a strict parse stream a row that outgrows its chunk**: a
block's statistics observer is a fold over each field's runs, as its census
is, so a parse holds no line of a `text`, `character`, `bytea`, `json`,
`jsonb` or walked-container field, serially, in its interior workers and in a
back-fill's re-read, and holds whole only a field its statistics or check read
whole ([`decisions.md`](decisions.md), "D138", "D142"). No reading was taken.
What proves it is `crates/pgdt-mapping/tests/streamed_line.rs`:
`a_gathering_parse_streams_the_types_long_fields_hold` — a long `text` with
an escape every few bytes, a `bytea` and a `jsonb`, of 16 and 64 MiB, under a
data-level and a strict parse at one worker and four, each under a quarter of
the shorter row resident — which fails on its parent, its first leg holding
the row; `a_streamed_row_gathers_and_checks_what_a_held_one_does` — every
major's `types` and `statistics` fixture in 16- and 61-byte reads, at one
worker and four, under a default, an ignoring and a strict parse, mapping to
the blocks a parse holding every row maps — which fails with an outgrown
field's head left unread; and `a_streamed_field_is_refused_as_a_held_one_is`.

- **The contract** (`pgdt-model`'s `statistics.rs`, `BlockObserver`):
  `holds`, `begin_row`, `fold`, `end_row`; `observe_row` is the fold fed
  each field once. `holds` is fixed per block, so the carry never hands a
  field over partly held.
- **The carry says where a streamed row starts** (`FieldFold::begin_row`,
  `ChunkCarry::absorb`'s `line_start`), which a gatherer places its group by.
- **`RowFold`** (`pgdt-dump`'s `map.rs`): a block's census and observer as
  one `FieldFold`, a field either holds held for both — the serial pass's
  (`Builder::row_fold`), a worker's (`interior/piece.rs`, `scan_read`, which
  now streams every block) and a back-fill's serial re-read
  (`backfill.rs`, `observe_rows`, a query's settling census among its
  callers). `Builder::reads_rows` is now true only for a recorded catalog
  block.
- **The gatherer** (`gather/gatherer.rs`): a field handed over whole is read
  as before (`field`); one handed over in runs is held to `RUN_HEAD` — the
  larger of `LONG_FIELD_HEAD` and `PRINTOUT_ESCAPED_BYTES` — and read whole if
  it ends there, and past it read by its column's `LongRead` and its check's
  walk as it goes by, its head quoting a refusal (I94). Each field is checked,
  then gathered, so a strict parse splits a held row once.
- **One reading of a long field** (`gather/clip.rs`, `LongRead`): `LongText`
  no longer re-reads its field for a `bytea`'s forms; they are read beside the
  text (`ByteaRuns` in `value/bytea.rs`, `CanonicalHex`), held or streamed.
- **`FieldWalk` owns its plan and is readied per field** (`start`, `finish`
  taking `&mut self`), so the gatherer keeps one per walking column, built at
  its first field outside every statistics scope, as a check is no statistic.

## What 33.4 inherits

- **The fields a parse still holds whole**, in the carry's `field`, a
  `LineMapping` past its chunk: a column summing or counting unrepresentable
  values (`ColumnGatherer::reads_whole`), and under a strict check a column
  whose plan keys a scalar (`FieldCheck::holds`). That is "the scalar text it
  collects", and the draw belongs at the carry's `field` as 33.3.3's notes
  place a census's. A strict check's scalar part and a range's bounds are
  still collected through the allocator (`Check::Scalar`), uncharged.
- **What a streamed field costs a parse is bounded**: `RUN_HEAD` bytes and a
  `LongRead`'s head of `LONG_FIELD_HEAD`, never past a chunk term.

## Negative results

- **No count pair was taken**: neither `field_key` nor a query's batch
  changed, which is what the spec's "How it is shown" prices (D108). The held
  row's path changed — a gatherer's row is a fold, a strict check reuses a
  walk per column and reads a row split once rather than twice.
- **A row the file's end cuts short inside a worker's piece** is folded into
  its piece's observer though the piece counts no row; the block fails
  unterminated either way, and the observer only readies its walk at the next
  row (`abandon_run`).
