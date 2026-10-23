# P15.5 notes — single-member gzip, the codec

The `deflate` adapter over `gz-seek`, reading a gzip file over a complete
index handed in, on the local and the fetched arm; the first pass that builds
the index is 15.5.1
([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md), "15.5
split: the codec landed, the first pass is 15.5.1").

- **`source/compressed/deflate.rs` is the codec**, `DeflateCodec` and its
  `UnitRead`, and `deflate/checkpoints.rs` the layout leaf the index names —
  `GzipIndex`, `WindowProvider` and the magic — the two the only modules naming
  `gz_seek`, as `xz.rs` and `xz/table.rs` are `xz_seek`'s. `pgdt-io` now links
  `gz-seek`, which builds no C.
- **The window provider rides on the index**, `GzipIndex::with_windows`, and
  is neither persisted nor compared: two indexes are equal where their
  checkpoints are. A unit's window is read as its decode begins and dropped
  once the inflater holds it, so a reader is charged `gz_seek::DECODER_BYTES`
  and one `WINDOW_SIZE`, plus the input chunk where the source lends nothing.
  The chunk is `xz_seek`'s 1 MiB, capped at the widest unit, so a gzip reader's
  input term is an `.xz` one's.
- **`DECODER_BYTES` is `gz-seek`'s statement of one decode's state and
  window**, held above zlib-rs's allocations by
  `crates/gz-seek/tests/footprint.rs`, a counting global allocator in a test
  binary of its own; the C engines allocate through `calloc`, which it cannot
  see, so their bound is their sources' shape, not a reading.
- **What the codec refuses.** An index ending elsewhere than the file is
  `gz_seek::Error::InvalidIndex`, which `index_refused` turns into
  `Recognized::Mismatch`; parts `gz_seek::Index::from_parts` refuses fail to
  decode, so a damaged cache is `Unusable::Unreadable`. An incomplete index,
  and one whose units begin inside a member with no provider, are a caller's
  error, `Error::Io`, claiming nothing about the file.
- **Recognition knows gzip** by `gz_seek::GZIP_MAGIC`, so a cache saying
  "plain" of a gzip file is now a mismatch, and an uncached gzip file is
  refused by name (`deflate::unindexed`, `SourceNotReadable`) on both openers:
  no command builds an index yet. `crates/pgdt/tests/gzip_source.rs` holds
  `parse` and `query` to that refusal, and to writing no cache.
- **`CACHE_FORMAT_VERSION` is 74**: `CompressionIndex` gained `Gzip`, its parts
  persisted as `{spacing, checkpoints, end}`, each checkpoint
  `{plaintext, bit, window, crc}` with `window` `None` at a member start.
  `.xz` caches encode as they did at 73.
- **`CompressionShape`** maps gzip onto its fields — members as `streams`,
  units as `blocks` — and names its container from the codec.
- **The evidence** is `deflate/tests.rs`: every unit decoded whole pulled and
  lent; reads inside, across and behind checkpoints on both arms at a
  block-decoding and a piecewise budget; a damaged unit refused whole and
  compared at `complete_reads` when a read stopped inside it; partitions at
  checkpoints; recognition and the refusal; the persisted round trip; and
  `pg_dump`'s own `-Z` fixture at every routine major read through its index.
  A checkpoint's size is pinned beside xz's per-entry cost (`KD26`).

## What 15.5.1 inherits

- **Where a first pass keeps the windows it produces until a save writes
  them** is its first question, and the spec answers it for the index only
  ("Only offsets, bit positions and CRCs are resident"). A save renames a new
  cache over the old (`decisions.md`, "D78"), so a window section written
  after the envelope is copied whole by each save of a pass unless it is laid
  out otherwise, and the windows placed since the last save are held
  somewhere until the next — in memory, a resident term no charge names yet,
  or in a file beside the cache, a second file the cache's identity rules do
  not cover.
- **A cached incomplete index is refused today** (`DeflateCodec::over`'s
  `Unsupported` error), and is what 15.5.1's resume opens instead, through
  `gz_seek::Forward::resume` with its last checkpoint's window.
  `CompressionIndex::uncompressed_size` answers the last checkpoint's offset
  for one, which `compressed_block_path_declined` reads as the whole file.
- **Three messages still speak `.xz`** where a gzip source reaches them:
  `NonSeekableCompressedSource`, `CompressedBlockPathDeclined` ("this .xz
  source has … block(s)") and `pgdt info`'s compression line ("block(s) in …
  stream(s)"). None is reachable from the CLI until a first pass is.
- **`deflate::unindexed`, the manual's sentence on refusing gzip and
  `gzip_source.rs`'s test go when the first pass lands**, as does
  `CHECKPOINT_SPACING`'s `dead_code` expectation.
- **The first pass's charge is `DECODER_BYTES` plus `Forward`'s own window
  copy**, which this slice's codec never holds.

## Negative results

- **A unit can be empty** where a member starts at the plaintext a previous
  one ended at, a `from_parts` index admitting equal offsets, so `units_in`
  starts a read at the last checkpoint at or before its offset and never at
  an empty unit before it.
- **A `gz_seek::UnitRead::read` can produce nothing and not be finished** —
  a header, a trailer or a block's end read — so the adapter loops to the
  first call producing plaintext, `LiveBlock::fill` reading zero as the unit's
  end.
