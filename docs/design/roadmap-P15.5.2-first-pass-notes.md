# P15.5.2 notes — single-member gzip, the first pass

A gzip file no cache has indexed is read by its first pass, which builds the
index as the scan reads; what the pass says in gzip's words is 15.5.3
([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md), "15.5.2
split: the first pass, and its words").

- **`source/compressed/deflate/first_pass.rs` is the pass**, `FirstPass` —
  its decodes and the windows it places in `first_pass/pass.rs` — the
  source `open` builds for an uncached gzip file on both openers and for a
  cached index that stops short of the end, which it resumes from its last
  checkpoint (`FirstPass::resumed`); a resumed index whose last checkpoint
  lies at or past the file's end, or that `gz_seek` will not resume, is
  `Recognized::Mismatch`. `CompressedFileSource::open` still refuses gzip,
  opening one only over a complete index.
- **One forward decode at the frontier, read one read at a time**, so
  checkpoints are placed in file order whoever reads; a read behind it
  decodes from the last checkpoint at or before its first byte through a
  second `gz_seek::Forward` resumed over the index cut there, kept across
  reads. A local file is pulled inside the read's one blocking task, a
  fetched one fetched between decode steps — the arms' split (D15), one
  body.
- **At the end it hands over**: the complete index, its windows beside it,
  opens `CompressedFileSource::with_index` or
  `FetchedCompressedSource::with_index`, every call is that source's from
  then on, and the hints callers gave are replayed on it — so a cold `query`'s
  replay cuts at units. Until then the extent is open, one partition is
  advised, one worker is recommended (D2) and two forward decodes are
  charged.
- **Windows are held in memory for the rest of the run** (`KD130`), the
  index the hand-over builds keeping them as its provider, each save
  writing the section through the pass's provider (D114); a resumed pass reads
  the earlier ones through the loaded cache's own.
- **The mapping pass asks the extent again after each chunk**
  (`pgdt_mapping::forward`), and both preamble prepasses after their scan: a
  source whose end was open at the start can meet it in a read, and a map
  saved after that must carry the `Unscanned` tail the saved extent states. A
  cold `query` stopping at its table over a small gzip file is where it
  showed: the save recorded an exact extent beside a map tiling only to its
  watermark, which a load reported as a tiling bug.
- **`non_seekable_compression_diagnostic` waits for a complete index**: a
  first pass's partial index says nothing yet of how many units the file
  holds (`CompressionIndex::is_complete`).
- **A pass's reads are checked at the member's end**, the file's only check;
  `complete_reads` on an unfinished pass compares nothing, and a resumed pass
  compares the trailer against the units' recorded CRC-32s combined, so a
  corrupted prefix is refused at the end of the run that reaches it.
- **The evidence**: `deflate/first_pass/tests.rs` (both feeds: the index
  equal to `gz_seek`'s own pass's, windows included; reads behind the
  frontier inside a closed and the open unit; resumes from a block and a
  member checkpoint, from before and past it; the hand-over's partitions;
  a resumed index past the file's end a mismatch; a file cut short or run on
  refused where the pass meets it); `crates/pgdt/tests/gzip_source.rs` (every
  major's `-Z` fixture read as its plaintext by a cold `query`, a `parse` and a
  warm `query`, the cold query's map tiling); `determinism.rs`'s
  `a_gzip_dump_writes_one_cache_however_its_first_pass_ran` (a member of
  three units at the shipped spacing, byte-identical caches at three legs
  and from two interruptions resumed by `pgdt parse`); `remote.rs` (the
  fetched pass, and `info` from its cache costing the probe alone).

## What 15.5.3 and later slices inherit

- **Three messages still speak `.xz`** where a gzip source reaches them, and
  now every one is reachable from the CLI: `NonSeekableCompressedSource` on
  every complete one-unit index — every gzip file under the spacing, whatever
  its writer — `CompressedBlockPathDeclined` and `info`'s compression line.
  Whether a file under the spacing should be warned at all is 15.5.3's to put
  to the maintainer: one unit there is the file's size, not its writer's.
- **No status line announces the pass**, as `seek table build started` does a
  cold `.xz` walk; the `PlanNote` 15.5.3 adds is the place.
- **15.8 replaces the frontier**, nothing else: `Pass::serve`'s frontier arm
  is where units found speculatively would land.
- **15.5.4 closes `KD130`**, spilling the windows to a file the run alone
  holds ([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md),
  "A first pass's windows, reviewed").

## Negative results

- **`flate2` at level 1 over zlib-rs ends no block on a sync flush**: a
  flush every 256 KiB over a payload of some megabytes wrote no `00 00 ff ff`
  marker, where the default level wrote one a flush, so a member it writes
  that way indexes as one unit whatever the spacing (a probe, not kept). `determinism.rs` compresses at the default level for
  it.
- **A unit can be empty at the end**: that level-1 member indexed as a
  checkpoint at its start and one at its plaintext's end, the final empty
  block's.
