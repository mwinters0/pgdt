# P13.2 — `SourceIdentity` becomes opaque, `stored_size()`/`size_is_exact()` join the trait, `CompressionIndex` reserved

## What landed

**`ByteRangeSource` grows two defaulted methods (D4, D7).** `stored_size()`
defaults to `size()` — exactly right for a non-decompressing source, and
`LocalFileSource` never overrides it. `size_is_exact()` defaults `true`.
Neither changes any existing call site's behavior; both are additive trait
methods with a default body.

**`SourceIdentity` (in `cache.rs`, private) is now an opaque enum, not a
struct (D4)** — one variant, `LocalFile { stored_size: u64, mtime: Option<(u64,
u32)> }`. `SourceIdentity::observe` now reads `stored_size()` rather than
`size()`, so the structure cache's staleness check is a `stat` even for a
source whose addressable length costs a walk to compute — nothing in this
tree is such a source yet, but the axis is now the right one for 13.3.
`cache::load` destructures both the cached and live identity through the sole
variant (an irrefutable pattern today; P14's `Remote { etag }` variant is
matched explicitly when it lands, per D4's "match sites read the fields
through the variant" rule) rather than through a shared accessor.

**`CacheFile` gains its own `total_size: u64` field**, populated at `save`
time from `source.size()` (the addressable, decompressed-if-compressed
length) — no longer an alias of `identity`'s stored size. `status_from_file`
reads `file.total_size` directly. This is the split D4 asks for: the
staleness check's axis (`stored_size`, cheap) and the coverage arithmetic's
axis (`total_size`, exact-but-possibly-expensive) are now two fields instead
of one field wearing two meanings.

**`CacheStatus::SourceChanged`'s fields are renamed** `cached_stored_size`/
`live_stored_size` (from `cached_size`/`live_size`), since they now report
the D4 axis and sit beside `Valid`/`Incomplete`'s differently-scoped
`total_size` — keeping the old generic name next to the new specific one
would have read as if they were comparable numbers. `pgdq info`'s printed
message is unaffected (the local variable names it interpolates are
unchanged); only the field labels a caller pattern-matches on moved.

**`CompressionIndex` is added as a sibling of `ContainerKind` (D5)**, with its
eventual real shape already in place — `enum CompressionIndex { Xz(xz_seek::
SeekTable) }` — rather than a stub enum to be reshaped later. This follows the
precedent `CopyBlock::sparse_index`/`column_stats` already set: a reserved
field carries its final type from the start, specifically so that the phase
that populates it needs no further `FORMAT_VERSION` bump for the *shape*.
`CacheFile::compression: Option<CompressionIndex>` is always `None` in this
tree — nothing constructs an `XzSource` yet, so "empty of xz content" is
true of every cache this build can write, even though the type is not empty.

**`xz-seek`'s `serde` feature is now on** in `pgdump_query/Cargo.toml`, since
`xz_seek::SeekTable` (and the types it's built from — `StreamEntry`,
`BlockEntry`, `Check`) need to derive `Serialize`/`Deserialize` to sit inside
`CompressionIndex`. Still nothing in this tree's source constructs an
`XzSource`; only the type is now nameable and (de)serializable.

**One `FORMAT_VERSION` bump (15 → 16) covers all of the above.**

## What this changes for measurement

Touches `cache.rs`, one of the declared paths for `per-block-quadratic`,
`peak-rss` and `preamble-prepass` — but `measure.py --stale` shows all three
already red from 13.1's `io.rs` changes before this slice touched anything.
No newly-stale figure, and no number in `measurements.md` is touched or
contradicted. No sweep is owed, per the standing rule that a stale figure
does not oblige one.

## What 13.3 inherits

- `CompressionIndex::Xz(xz_seek::SeekTable)` is already declared and
  serde-derivable end to end — the decoder slice constructs
  `Some(CompressionIndex::Xz(table))` at save time and reads it back at load,
  with no `FORMAT_VERSION` bump needed for the field's *shape* (bincode's
  encoding already accounts for the variant from this slice on; a bump is
  still free pre-1.0 if the decoder work turns up its own reason for one).
- `SourceIdentity::observe` is generic over `&dyn ByteRangeSource` and always
  builds the sole `LocalFile` variant from `stored_size()`/`modified()` —
  `XzSource` needs no new match arm in `cache.rs`, only its own overrides of
  those two trait methods (`stored_size()` to the compressed file's on-disk
  length, `size()` to the uncompressed length from the seek table). D4's
  opacity is about the *shape of the evidence* (stored-size-plus-mtime vs. an
  ETag), not about "local" vs. "compressed" — every source this phase and
  P14 add still backs `LocalFile`-shaped identity except P14's own remote one.
- `size_is_exact()` stays the trait's `true` default for `XzSource`: xz's own
  length is exact from the stream index (D1), so 13.3 does not need to
  override it. P15's gzip/zstd sources are the first to answer `false`.
