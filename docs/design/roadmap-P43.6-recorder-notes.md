# P43.6 — The recorder: notes

Slice 43.6 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). No
format change. The shape is D169.

## What the next slices inherit

- **`Recorder` is in `pgdt-model`'s `recorder.rs`**, reached as
  `pgdt_model::internal::Recorder`. `Recorder<O: ?Sized = dyn BlockObserver>`
  holds an optional `CensusFold`, optional `RecordedRows`, an optional
  `Box<O>` and the block's `data_offset`, against which it places each row
  for its observer. It is built by `Recorder::new(data_offset)` and three
  `with_*` calls: `with_census(width, counter)`, `with_observer` and
  `with_rows`. 43.7's open is where those calls move.
  - Every recorder has `row` (a held row), `begin_row`/`fold`/`end_row` (a
    streamed one), `folds`, `holds_rows` and `holds`.
  - Only `Recorder<dyn BlockObserver>` has `piece`, `absorb` and `finish`.
- **`finish(header, row_count, terminator_offset, longest_line)`** answers a
  fresh `BlockRecord`. It returns the observer's refusal as the error that
  `Builder::on_copy_end` used to raise. pgdt-dump's `finish_block(recorder,
  header, &CopyEnd)` is the one caller that passes a scanner's `CopyEnd`.
  It is reached through `pgdt_dump::internal`, and it places the longest
  line in the block.
- **pgdt-dump's `FieldFold` is implemented for every `Recorder<O>`** in
  `map.rs`, where `RowFold` was, beside `finish_block`. Each host passes
  `Some(recorder)` only where `recorder.folds()`, as `RowFold::new` answered
  `None` before.
- **`Builder` records nothing.** `on_copy_start(event)` takes no census flag,
  and `on_copy_end(end, record)` returns nothing and pushes the record it is
  handed. Both mapping passes hold the open block as
  `Option<(CopyHeader, Recorder)>`, from its `CopyStart` to its `CopyEnd`.
  `forward.rs`'s `close_open` finishes the recorder and closes the block on
  the builder. `close_copy_block` now takes the watermark, so it stays under
  clippy's argument count.
- **The interior records through pieces.** A `Region` carries `&mut
  Recorder`. Each partition is handed `recorder.piece()` and gives it back,
  and `run_region` absorbs the pieces in file order through the terminating
  one, as it absorbed observers before. `RegionScan::Closed` holds the
  `CopyEnd`, and `Interior`, `CensusPlan`, `PieceObserver`, `empty_census`
  and `PieceScan::census` are gone.
- **`reread_rows` takes `Option<Box<dyn BlockObserver>>` and answers the
  finished `BlockRecord`.** `settle.rs` takes its census from it. The
  back-fill's `Rereads::record` reads the declined, gathered or checked
  outcome off that record. This is the record 43.7's merge through
  `BlockRecord` starts from: the field-by-field writes are still in
  `record`.
- **The observer contract is two traits.** `RowObserver` has `holds`,
  `begin_row`, `fold`, `end_row` and `into_any`. `BlockObserver:
  RowObserver` adds `close`, `piece` (answering `Box<dyn RowObserver>`) and
  `absorb`.
  - `PieceGatherer` implements `RowObserver` alone.
  - `Checker` and `BlockGatherer` implement both. A checker's pieces are
    checkers.
  - The `unreachable!` arms are gone.
- **Neither trait has `observe_row` any more**, and neither does
  `BlockCensusExt`. Only the recorder splits a held row. The gatherer tests
  feed one through a `HeldRow` helper in `gather/tests.rs`.
- **`CensusFold` is crate-private.** It gained `fresh`, `absorb`, `reach`
  and `test`. `reach` is the whole-row reject, answering a `Reach`: a held
  row holding a `{` or a `[` is folded field by field, any other only
  `test`ed through its last tested column, whatever else reads the row.
  Where the census alone reads it, the recorder splits it no further. A
  field handed over in one run is tested where it lies, not copied into
  the head.
- **A recorded row's fields come from the same split**, through
  `timescaledb::recorded_field`.

## Negative results

- **The recorder holds no row count.** The spec lists one. The scanner
  counts every row of a block, and that includes the rows a block recording
  nothing hands to no recorder, such as a metadata-level parse's. A
  recorder's count would either miss those rows or have every row handed to
  it. So `finish` takes the scanner's count, as it takes the longest line.
- **A streamed row cut short by the file's end is no longer dropped from a
  piece's census.** Before, `streamed_piece` filtered it out per scan. The
  census now lives in the partition's recorder, and such a row ends a
  partition that has no terminator in it. That partition is either past the
  terminator, and so never absorbed, or the block is unterminated and the
  region fails. Either way the census is never read.
- **A held row's census cannot fold every field where an observer reads
  the row.** Done that way, the data-level parse rose: callgrind placed the
  rise in `CensusFold::fold` and its `memchr2`, called on every field, a
  row holding no array shape-folded a field at a time
  (`runs/count-pair-43.6-20261121/callgrind/reading.txt`). With the
  whole-row gate kept whatever else reads the row, the parse falls below
  the parent: one split where it made two, and a field past its head bound
  not tested (`runs/count-pair-43.6-rework-20261121/callgrind/reading.txt`).
- **The metadata-level parse rises by `Recorder::row`'s prologue**, ahead of
  its early return on a recorder recording nothing. Accepted under D108:
  moving the prologue or the test into the host rests on codegen. The query
  rows' rises are layout (same file).
