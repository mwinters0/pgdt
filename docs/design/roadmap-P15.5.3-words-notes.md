# P15.5.3 notes — single-member gzip, the first pass in gzip's words

What a gzip source says, and when: the warning D19 gains a clause for, the
first pass's plan note, and the decline and `info` line in gzip's words.

- **The warning's rule is the codec's `is_seekable`**: `.xz` more than one
  block, as before; gzip no unit wider than twice the index's own spacing
  (`GzipIndex::is_seekable`), so nothing new is persisted and no cache bump
  follows. `non_seekable_compression_diagnostic` still waits for a complete
  index.
- **The kind names its container**: `NonSeekableCompressedSource` carries
  `container` (`pgdt_model::CompressedContainer`, `Xz` or `Gzip { spacing }`
  — the model's word, `pgdt-io` sitting above it) and `largest_unit` beside
  `block_count`, on both codecs. The `.xz` sentence is unchanged, so the
  manual's quotation of it stays true.
- **The first pass is `PlanNoteKind::CompressedFirstPass`**, `Info`, no lever
  and no budget, raised by `plan_partitions` wherever the source's index is
  incomplete when the plan is made. So it reaches a cold `pgdt query` whose
  mapping pass stopped before the end, and nothing else: a cold query whose
  mapping pass met the end plans over the handed-over source, a `parse` plans
  nothing, and the DataFusion provider registers complete caches alone.
- **The decline never fires beside either**: `compressed_block_path_declined`
  now also requires a complete index, seek structure and at least two units.
  The last keeps a one-unit gzip file under twice the spacing silent under a
  budget too small for its unit, as a one-block `.xz` was before.
- **Words.** `CompressedBlockPathDeclined` gains `codec`; its sentence says
  "unit" for gzip, and the `.xz` one is unchanged. `info --detail` prints
  "unit(s) in … member(s), largest unit" for gzip. `--json`'s
  `CompressionShape` keeps its field names (`blocks`, `streams`).
- **The evidence**: `pgdt-dump`'s `map/tests.rs` (a level-1 member written in
  8 KiB calls, indexed at 256 KiB, warned in gzip's words; one under the
  spacing and one at the default level not); `partitioned_replay.rs` (a cold
  plan stopped short of the end notes the first pass and is one sub-stream,
  silent once a read reaches the end; a decline over an indexed gzip file in
  gzip's words); `stream/tests.rs` (the sentences and severities);
  `gzip_source.rs` (every major's `-Z` fixture unwarned and described in
  gzip's words; a 52 MiB level-1 member warned by `parse` and in `info
  --json` at the shipped spacing).

## What later slices inherit

- **The level-1 tests read zlib-rs's `deflate_quick` ending**, through
  `flate2`'s zlib-rs backend, and assert the wide unit before the warning, so
  a zlib-rs that ends blocks at level 1 fails them by name rather than
  silently. 15.10 is what stops the warning firing on that output.
- **`bgzip` is not named as a remedy** until 15.6 reads BGZF.
- **The manual** says nothing of the gzip warning or note yet; 15.7.2 writes it.

## Negative results

- **A partial index's size is its last checkpoint's offset**, zero for a
  fresh pass under the spacing, so the first-pass note names no number: how
  far the pass has decoded is the source's open extent, not the index's.
- **The level-1 tests' writer is `flate2`, not `libz-rs-sys`**: one zlib-rs
  0.6.8 in the lock, which both wrap, and no member enabling a `flate2`
  C-zlib feature, the only thing that outranks it. `libz-rs-sys`'s C ABI
  would put `unsafe` and a second test writer in two suites for the same
  engine; a committed level-1 fixture would hide which writer made it, and
  the 52 MiB one is too large to commit. No check pins the backend: the tests
  that rely on the block layout assert it (`../status/history/2026-10-24.md`,
  "15.5.3's test writer, reviewed").
