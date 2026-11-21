# P43.7 — One open, one close: notes

Slice 43.7 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). No
format change. The shape is D170.

## What the next slices inherit

- **`crates/pgdt-mapping/src/open.rs` holds the open.** `open(header,
  data_offset, metadata, database, policy)` answers an `OpenBlock`: the
  block's `Recorder`, whether it `streams`, and whether it `offers_interior`.
  Both are derived from whether the recorder records rows as text.
- **`Policy` has two arms.**
  - `Maps(Pass)` is a forward pass. `Pass::Parse` censuses by the request's
    level and attaches `observer_for`. `Pass::Query` censuses and gathers
    nothing. Both record TimescaleDB's catalog's rows.
  - `Rereads(Reread)` is a block read again. It censuses, builds the
    observer the `Reread` names and records no rows
    ([43.7.1's notes](roadmap-P43.7.1-reread-policy-notes.md)).
  - 43.8's eager pass needs a third arm: census, no observer, rows recorded,
    and no interior offered. It reaches the open through
    `pgdt_mapping::internal`, as the spec places it.
- **`Pass` moved from `forward.rs` to `open.rs`.** `forward` uses `open`, so
  `open` cannot use `forward` without a module cycle. `internal` still
  exports it as `pgdt_mapping::internal::Pass`.
- **`map_forward`'s `CopyStart` arm calls the open.** It still restates the
  database's metadata, tests the target and makes the offer itself. Those are
  the forward pass's business.
- **`reread_rows` opens through `Policy::Rereads`.** Its serial fallback,
  `record_rows`, streams as the open says, which is always for a re-read.
- **The back-fill merges through `BlockRecordExt::merge`**, in
  `pgdt-model`'s `span.rs` and reached through `pgdt_model::internal`. It
  answers the heap bytes of the statistics a gathered read replaced, and
  `Rereads::record` credits the account with them. `merge_ignored` went with
  it. `KD107` stays at `IgnoredRefusals::merge`, which the merge calls.
- **The close needed nothing new.** The forward pass already hands
  `finish_block`'s record to `on_copy_end`, and the back-fill already checks
  extent, row count and longest line before `finish_block`.
- **`span/tests.rs` tests the merge**: a gathering read, a declining one,
  and a check alone.

## Negative results

- **The open takes no `PassContext`** (D170). It reads none of the
  context's fields, and `reread_rows`, which opens through it, takes the
  `ReadContext` alone, `backfill_block` holding nothing more.
- **`reread_rows` does not branch on `offers_interior`.** It is always true
  for a re-read, so the function asserts it instead.
- **The data-level parse over `long_bytea_32` rises by layout, accepted.**
  The `--count-pair` resolves it and the data-level parse's fall. Callgrind
  places the rise in `memcpy` under `ColumnGatherer::close_group`, unchanged
  code called as often in both legs. The open's own code, moved out of
  `map_forward`'s arm, nets a fall
  (`runs/count-pair-43.7-20261121/callgrind/reading.txt`).
- **The account's credit moved outside the statistics scope.** Before, it
  ran inside it. `StatisticsAccount::apply` allocates nothing, so the
  instrument's attribution does not move.
