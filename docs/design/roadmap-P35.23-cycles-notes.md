# P35.23 notes — the cycles no other cut removes

The cut [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)
drew for pgdt-model, pgdt-dump's `cache/` and the `.xz` advice. Every body
moved whole. No persisted shape changed (serde names fields, not modules), so
the cache format was not bumped. `module_graph.py` finds none of the three
cycles, and their `OWED` entries are struck.

## Where the cut fell

- **pgdt-model**: `BlockObserver` now sits in `statistics.rs`, `union_census` in
  `span.rs` and `TilingIssue` in `diagnostic.rs`, and the crate root
  re-exports each under its old name. `block.rs` now holds the one contract
  that counts a block's rows, `FieldCount`.
- **pgdt-dump's `cache.rs` only declares**: the module doc, `mod` lines and
  `pub use` lines. `load`, `load_offline`, `claim`, `CacheClaim`, `save` and
  `colocated_path` are in `cache/store.rs`. `CacheStatus`, `CacheEnvelope`
  and `CompressionShape` are in `file.rs`, and `StrictIdentity` and its
  impls are in `identity.rs`. Each API path is unchanged. Two visibilities
  widened inside the crate so that `store` can build an envelope `file` now
  defines: `CacheEnvelope`'s fields and `CompressionShape::of` are now
  `pub(super)`. Outside `cache/`, those fields stay private.
- **`xz_partition_advice` is in `xz/blocks.rs`**, and the `KD35` marker in
  its body moved with it, along with its index line's `Detail:`. Its two
  tests, which call the free function directly, moved to
  `xz/blocks/tests.rs`. The tests that drive a source's `partitions` stay in
  `xz/tests.rs`.

## What later cuts inherit

- **The design doc's three 35.23 `moves` blocks are deleted**, because the
  moves they describe have landed and no longer project. With the remaining
  blocks applied, `--design` finds no cycle except the accepted `origin` ↔
  `remote`.
- **`measure.py`'s `CACHE` now declares `cache/store.rs`**. `READ` already
  covered both `.xz` files. `MAP` declares `span.rs`, which now also holds
  `union_census`.
- The comments that name a moved private function now carry its new path:
  `status_from_file` is `cache::store::status_from_file` in `preamble.rs`,
  `map/tiling.rs` and pgdt-model's `diagnostic.rs`.
