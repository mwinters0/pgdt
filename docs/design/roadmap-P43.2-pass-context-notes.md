# P43.2 — The mapping pass context: notes

Slice 43.2 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). Off
the row path: no behaviour moved, so no `--count-pair`.

## What the next slices inherit

- **`crates/pgdt-mapping/src/context.rs` holds three types.** `ReadContext` is
  what every reader of a pass reads under: source, `ScanOptions`, ledger,
  token, extent. `PassContext` is a `ReadContext` plus the mode and watch the
  pass saves through. `Gathering` is a parse's request, account and invalid
  mode, and the allowance is read off the account. All three are `Copy`.
  `internal` exports `PassContext` and `ReadContext` for `pgdt-query`'s
  `dump/settle.rs`, which builds one for `map_forward` and `reread_rows`.
- **The read half is apart because a block can be read without a pass.**
  `backfill_block` reads with no cache and no watch. So `scan_region`,
  `run_region`, `scan_partition`, `reread_rows` and `observe_rows` take
  `ReadContext`, and `map_forward`, `close_copy_block`, `backfill_statistics`
  and `map_file_watched` take `PassContext`. 43.7's open takes `PassContext`.
- **`Pass::Parse` carries a `Gathering`**. `Rereads` holds a `ReadContext`,
  the cache path and a `Gathering`, in place of the twelve fields it had.
- **`interior::Region` is a block as the leader is offered it**: table,
  header offset, data offset, census plan and observer. `scan_region` takes
  the region and its `ReadContext`, so `run_region` ends at five parameters.
- **`forward::Segment` is what each `map_forward` splice closes against**:
  the prefix, the frontier, and the file's end, which is updated as the source
  learns it. `close_copy_block`'s `target` and `targets` are now one
  `settles: Option<Target>`.
- **Each exit is written once.** In `map_forward`, `interrupted` saves and
  answers `MapStop::Interrupted` at all four check points. In `map_file`,
  `interrupted` checks the watch and builds the short `Mapped` at all four
  exits. `PassContext::save` is the one spelling of
  `cache.save(watch, source, index)`, and `SaveThrottle::save` takes the
  context.

## Negative results

- **Three functions keep `clippy::too_many_arguments`, and none takes a pass
  field the context would replace.**
  - `backfill_block` opens no pass. It is called by tests alone, so it leaves
    the API for `pgdt-fixtures` at 43.8, where it takes a `ReadContext`
    (`roadmap-P43-block-model.md`, "The eager pass is the fixtures' oracle").
  - `interior/piece.rs`'s `scan_read` is one read's parse on the blocking
    pool, over the piece's carry, fold and observer.
  - `statistics/gather/column.rs`'s `ColumnGatherer::new` is a gatherer's
    constructor. 43.5 reshapes it, and drops the allow only if that reshape
    removes the need.
- **The exits that bank after an unwinding read now save, then check.** That
  is `map_file`'s `map_forward` error arm. It used to check, then save. An
  enabled save checks the watch before it writes (`CacheMode::save`), so the
  run makes the same checks either way and refuses at the same one. Only the
  order changed.
- **`map_file` asks for the source's extent before `map_file_watched`**, so
  that it can build the context. It attributes a failure through the watch,
  just as the attribution of the whole run did.
