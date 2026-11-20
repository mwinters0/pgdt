# P43.3 — `BlockId` at every identity site: notes

Slice 43.3 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). No
format change and no behaviour moved: every key the slice retypes held the
header offset, which the id wraps, so each map orders and matches as it did.
Off the row path bar one `block.id()` per segment, so no `--count-pair`.

## What the next slices inherit

- **`pgdt_model::BlockId` is in `span.rs`**, the derived side, as 43.1's notes
  placed it: no serde, `Copy`, `Ord`, `Hash`, its integer private. Two things
  build one. `CopyBlock::id()` builds it from the map's block, and pgdt-dump's
  `CopyStart::id()` from the header the scanner reads. Both go through
  `BlockIdExt::at_header`, in `pgdt_model::internal`. 43.4 moves the first
  onto `BlockAddress::id()`. The second stays a header's, since a `CopyStart`
  knows two of the four offsets and no whole address.
- **The sites keyed on it**: `ReplayPlan`'s `blocks`, `kept` and `stops`; the
  statistics map and `Cut::verdicts` a dynamic filter reads; `DynamicRead`'s
  entered block; `EarlyStop::block`, which replaces the public
  `header_offset` field; `InCopyResume::block`; the back-fill's `loaded` set,
  in `backfill.rs` and in `whole_file.rs`; `summary.rs`'s edges and the
  partition runs they are read over; and `descendants.rs`'s three through
  sorts.
- **A site that needs the block's address as well as its identity holds
  both.** That is the replay's `Active` and `InCopyResume`, whose scanner
  resumes reporting the address. `activate` takes the id beside the header and
  the address. `DynamicBlock` keeps the address alone, for the refusals it
  resolves; its identity is `DynamicRead`'s. Under 43.4 the address is a
  `BlockAddress`, so each such pair is one held address and its derived id.

## Negative results

- **`Error::CachedBlockChanged` keeps `header_offset`, though the spec lists
  it among the identity sites.** The error is `pgdt-core`'s, which names no
  model type (D68). Nothing matches on its offset: `pgdt`'s `refusal.rs` and
  `datafusion-cli-pgdump`'s `spelling.rs` match on its path alone. So it is a
  display site and prints the address, as every other error naming a block
  does. That includes `PartitionThroughRoot`'s and `PartitionsThroughRoot`'s
  lists, which the through sorts now order by id.
