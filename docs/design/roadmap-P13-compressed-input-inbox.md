# P13 inbox — facts filed for its grilling

Evidence that P13 (compressed input) will need. **This is a queue, not a
document**: when P13 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## The koji sample's own `.xz` is 31,150 concatenated streams, not one

**Fact.** `xz --list` on
`/mnt/wd12t/fedora/fedora_dumps/downloads/koji-2026-07-23.dump.xz`
(40,397,009,888 bytes) reports **31,150 streams / 31,150 blocks**, 730.2 GiB
uncompressed, ratio 0.052. One block per stream, ~24 MiB uncompressed and
~1.29 MB compressed each. Every stream is self-contained — header, block,
index, footer — so the file is randomly accessible at 24 MiB granularity
without any index of ours.

**Why P13 cares.** The phase was sketched as "single-block xz is the MVP, and
the general multi-block case comes later". The one file the maintainer actually
wants to read is the general case, and the degenerate single-block shape is
what has no seek table. That inverts which shape is the risk: seekable is
common and cheap, non-seekable is the one needing a decision about what `query`
does when a backward seek costs a decode from zero.

**Origin.** 2026-09-02, sketching this phase.

**Contingent on** that file staying as it is; it is not ours and does not move
(`CLAUDE.local.md`).

---

## Building the seek table costs 85 s on this file, because the footers are scattered

**Fact.** `xz --list` on that file takes **85 s** wall on the HDD at 10% CPU.
A single-stream file's index is one read of the footer; 31,150 concatenated
streams means walking backwards through 31,150 footers, each a separate seek.

**Why P13 cares.** It is the number that decides whether the seek table is
persisted or re-derived per run. 85 s in front of a scan is tolerable and 85 s
in front of a `query` against an already-complete cache is not — and over P14's
ranged GETs the same walk is 31,150 round trips. It is also the argument for
where it is persisted: `ContainerKind` and the cache envelope already exist for
saying what produced an index's offsets.

**Origin.** 2026-09-02, sketching this phase. Re-take with
`time xz --list <file>`.

---

## xz decodes at ~446 MB/s of plaintext per core, and scales linearly per stream

**Fact.** Three probes on this machine, on a 300 MiB slice cut at a stream
boundary ~20 GB into the koji `.xz` (17.8× ratio there, against 19.4× for the
whole file, so it is representative — the file's *head* compresses 54× and
decodes at 357 MB/s, which is not):

| Probe | Result |
|---|---|
| `xz -dc -T1`, one process | 5.59 GB out in 12.5 s — **~446 MB/s** of plaintext, 108% CPU |
| `xz -dc -T8`, one process | 12.65 s, 108% CPU — **no speedup at all** |
| Four `xz -dc -T1` on four stream-aligned pieces | 3.77 s, 397% CPU — **~1.48 GB/s**, near-linear |

The middle row is not a measurement error: xz's threaded decoder parallelises
*blocks within a stream*, and every stream in this file holds one block. Its
own CLI therefore cannot use more than one core on it, while decoding whole
streams concurrently scales.

**Why P13 cares.** Two decisions rest on it. It sets the ceiling a serial
decoding source can reach — 446 MB/s against the published koji scan's ~240
MB/s means the compressed path is not slower than the plain one even before
parallelism, since it reads 19× fewer bytes (37.6 GiB at HDD speed ≈ 165 s of
I/O against ~3200 s for the plain file). And it says the parallel form is worth
having and is ours to build rather than the codec's to provide.

**These are probes, not figures.** No harness, no `drop_caches` discipline, no
`measure.py` registration — they are evidence that the phase is worth its
shape, not numbers any document may quote. This phase owes real figures through
`scripts/measure.py` like every other performance claim
(`docs/design/measurements.md`).

**Origin.** 2026-09-02, sketching this phase.

---

## `size()` is taken before every scan, and clamps every read

**Fact.** `scan::scan`, `index::build_index`, `index::scan_preamble`,
`stream::map_forward` and `stream::table_stream` each call
`ByteRangeSource::size()` first and compute every subsequent read as
`chunk_size.min(size - read_pos)`. `LocalFileSource::read_range` uses
`read_exact_at`, so a read past the end is an error rather than a short read.

**Why P13 cares.** A decompressing source has to answer the uncompressed size
before it has decompressed anything. xz can: its stream index carries each
block's uncompressed size, so the total is exact and comes from the same footer
walk the seek table needs. That is the concrete reason this phase is xz-only
and P15 is the codecs whose footers cannot answer it.

**Origin.** 2026-09-02, sketching this phase.

---

## Exactly two callers read backwards, and both are needed by `query`

**Fact.** Every other read in the library walks forward from a start offset.
The two that do not:

- `stream.rs`'s replay loop reads `[block.header_offset, block.end_offset)`
  after the mapping pass has already walked past that block — the deliberate
  double read under `architecture.md`, "Query: mapping and streaming are
  separate passes".
- `map.rs`'s `attach_text` re-reads the gaps between `Data` spans once the scan
  has finished, coalescing contiguous text-storing spans into one read per gap.
  This is what `pgdq info --verbose` and `--map` report from.

**Why P13 cares.** These are the only two places where a non-seekable source
turns an O(1) read into an O(offset) decode, so they bound what the phase must
decide. Neither is on the `pgdq parse` path, which is purely forward — so an
eager scan of a single-block file is honest, and it is `query` and
`info --verbose` that need an answer. One available answer is already written
down as a Future item: *"let a live scan emit rows again, by carrying the map in
the resume token"* removes replay's backward seek by construction, and a
non-seekable source is what promotes it from an optimization to an enabler.

**Origin.** 2026-09-02, sketching this phase. Re-check by grepping
`read_range` — there are four call sites.

---

## A complete cache already reduces a query to the target block's bytes

**Fact.** `stream::table_stream` skips the preamble prepass when the cached
first-database preamble is complete (`stream.rs`, the
`first_db_preamble_known` guard), and the mapping pass walks only from
`scanned_through` to the target — nothing, when the cache covers the whole
file. What is left is the cache read, `size()`/`modified()` for the identity
check, and the replay read of the target block.

**Why P13 cares.** It is what makes the seek table pay: with the structural
cache and a seek table in hand, reading one table out of a 40 GB `.xz` is a
decode of the blocks covering that table's uncompressed byte range, not of the
file. It is also the exact fact P14 depends on, so whichever phase lands first
should not restate it privately.

**Origin.** 2026-09-02, sketching this phase.

---

## DataFusion's compression support is forward-only and cannot be adopted

**Fact.** `datafusion/datasource/src/file_compression_type.rs` (origin-main)
is a `Stream`/`Read` adapter — `async-compression`'s `AsyncXzDecoder`,
`MultiGzDecoder`, `XzDecoder::new_multi_decoder`. It exposes no seek table and
no positioned read, and `object_store` beneath it is a byte store that does not
decompress at all.

**Why P13 cares.** The natural first question at this phase's grilling is
whether the ecosystem already solves it. It does not: adopting that layer buys
`pgdq parse` and gives up `pgdq query`, because a `TableProvider` over a
compressed file in DataFusion reads it start to finish and gives up file
splitting. The decoder crate is still worth taking from the ecosystem; the
addressing is ours.

**Origin.** 2026-09-02, sketching this phase.

**Contingent on** DataFusion not growing a seekable-compression path; re-check
that file at the version P6 eventually targets.
