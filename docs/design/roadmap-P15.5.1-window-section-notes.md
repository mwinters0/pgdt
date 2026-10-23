# P15.5.1 notes — single-member gzip, the cache's window section

The cache's window section and the provider `pgdt-dump` implements over it; the
first pass that produces the windows is 15.5.2
([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md), "15.5
split: the codec, the window section, the first pass").

- **`pgdt-dump`'s `cache/file/windows.rs` is the section**: written after the
  encoded file by `write_cache_file`, attached as a provider by
  `read_cache_file`, which refuses as `Unusable::Unreadable` a file not ending
  with the last window (D114). Every gzip index a load hands out — the
  `KnownCompression` an open settles, `LoadedCache`'s envelope — carries it, so
  `CacheMode::open` over a gzip origin with a complete cache opens the source
  from the cache alone and its units begin from the section's windows.
- **`GzipIndex` gained `checkpoint_count` and `read_window`**, reading through
  whichever provider rides on it; a provider is asked by checkpoint, an
  incomplete index's last included, which a resume will read.
- **`CACHE_FORMAT_VERSION` is 75**; plain and `.xz` caches encode as at 74,
  with no section.
- **`pgdt_io::internal::gzip_indexed_in_memory`**, under pgdt-io's new
  `test-support`, is how a test above io gets a gzip index: `gz_seek`'s forward
  pass over the file held in memory, its windows held beside it
  (`deflate/held.rs`, which io's own tests read through too).
- **The evidence**: `cache/file/windows/tests.rs` (the bytes, the round trip,
  a section cut short or run on, two saves over the cache the index was loaded
  from writing the same bytes, a source over a loaded index reading every unit
  with the cache file removed); `tests/cache.rs`'s
  `a_gzip_cache_reads_its_units_through_its_own_windows` (a map over an index
  built in memory saving the section, a data-level re-read through
  `CacheMode::open`'s source and its save over the cache, the plaintext read
  whole through the next open).

## What 15.5.2 inherits

- **A first pass's index must carry a provider answering every window it has
  placed until the pass ends**, since each save writes the section whole out
  of the saved index's provider (D114): where those windows live meanwhile —
  memory, a resident term no charge names yet, or a file the run alone reads —
  is the slice's first question, narrowed from 15.5's.
- **A resumed pass reads its last checkpoint's window out of the section**
  (`GzipIndex::read_window`), so the provider a load attaches already serves
  `gz_seek::Forward::resume`.
- Everything else 15.5 handed on, unchanged
  ([`roadmap-P15.5-codec-notes.md`](roadmap-P15.5-codec-notes.md), "What
  15.5.2 inherits").

## Negative results

- **`flate2`'s default level wrote two deflate blocks over the 1.4 MB
  statistics fixture**, so a test wanting units inside a member flushes as it
  writes at the default level: a sync flush there ends a block, and at level
  1 over zlib-rs it ends none
  ([`roadmap-P15.5.2-first-pass-notes.md`](roadmap-P15.5.2-first-pass-notes.md),
  "Negative results").
