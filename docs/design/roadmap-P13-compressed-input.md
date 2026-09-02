# P13 — Compressed input

Read an `.xz`-compressed plain dump directly — `pgdq parse --source
koji.dump.xz`, `pgdq query --source koji.dump.xz` — in every shape xz has.
Other codecs are [P15](roadmap.md)'s; an archive container's *internal*
per-entry compression is P8 Track B's and shares nothing with this phase but a
decoder.

**`pg_dump` never writes `.xz`.** Its plain-format `--compress` speaks gzip,
lz4 and zstd, which is why closing a *compatibility* gap is P15's job and not
this one's. Every file this phase reads was compressed by a third party after
the fact — which is exactly how the dumps that are actually shipped around
arrive, koji's included.

## Why xz first, and why it is not the easy case

Scheduled ahead of P7 because it is the maintainer's priority, and because it
changes what that phase is measuring: a compressed source inverts the
arithmetic behind "device-bound". Reading koji compressed is 37.6 GiB off the
device instead of 730 GiB, paid for in CPU at roughly 446 MB/s of plaintext per
core. P7 sets readahead, chunk-size and parallelism defaults, and it should set
them knowing both source shapes exist.

**The trait already fits and the cost model does not.** A decompressing
`ByteRangeSource` satisfies `read_range`/`size`/`modified` exactly; what it
cannot satisfy is the assumption every caller above it makes without stating —
that a read at an arbitrary offset costs what a read at the next offset costs.

## The three shapes, and what each costs

An `.xz` file is a sequence of **streams**, each holding one or more **blocks**
and ending in an index that lists every block's compressed and uncompressed
size. Which shape a file has decides what random access costs, and the ranking
is not the one this phase was sketched with:

| Shape | Produced by | Seek table costs | Random access |
|---|---|---|---|
| One stream, many blocks | `xz` 5.6+ default (`-T0`), any `-T>1`, `--block-size` | **one footer read** | per block |
| Many streams | concatenation — `cat a.xz b.xz`, chunked pipelines | one footer read **per stream** | per stream |
| One stream, one block | `xz -T1`, xz older than 5.6, library writers | one footer read | **none — every backward read decodes from zero** |

The koji corpus on this machine holds one of each of the first two, and
`--block-size` makes the third constructible at fixture scale:

- `koji-2026-07-23.dump.multistream.xz` — the upstream download, 40,397,009,888
  bytes, **31,150 streams of one block each**, ~24 MiB uncompressed and ~1.29 MB
  compressed per stream, 730.2 GiB uncompressed. **This is the file the
  maintainer needs to read**, so the many-streams shape is the phase's primary
  target, not its exotic one.
- `koji-2026-07-23.dump.multiblock.xz` — the same dump recompressed locally
  with `xz -T15 --block-size=128MiB`, giving one stream of ~5,700 blocks.
- Its container parameters, read out of the first 32 bytes: LZMA2 with an
  **8 MiB dictionary** (`xz -6`, the default preset), **CRC64** per block, and a
  block header that declares neither size — both live only in the stream index
  at the footer. The dictionary is the decoder's per-stream memory floor, which
  is what makes concurrent decode fit the 512 MB cgroup the koji runs use.

Walking 31,150 footers takes **85 s** on the HDD at 10% CPU (`time xz --list`),
because each is a separate seek. One footer read is free. That difference is the
whole reason the seek table is a persisted artifact rather than something
re-derived per run.

## Decisions

### D1 — `size()` keeps its promise, and the walk happens once per file

`ByteRangeSource::size()` continues to mean *the exact number of bytes
`read_range` can address*, which for a decompressing source is the exact
**uncompressed** length. It is derived from the stream index — the same footer
walk that builds the seek table — and **persisted in the cache**, so only a
source with no usable cache ever pays for it. `pgdq info --dqcache` never pays
it at all.

*Rejected: relaxing the contract*, letting `size()` return a bound and making a
zero-length read the termination condition. It is mechanically cheaper than it
looks — every read loop is already written as `want = chunk_size.min(size -
read_pos)` … `read_pos += bytes.len()`, so all of them already tolerate a short
read, and only the loop's exit test and the coverage denominator depend on the
number being exact. It is rejected here because the exact number is what
`total_size`, `scanned_through` and `pgdq info`'s coverage arithmetic mean, and
because xz can answer it honestly. **P15 is where that relaxation has to be
faced** — gzip's `ISIZE` is useless above 4 GiB and zstd's frame content size is
optional — and pre-spending it here would buy P13 nothing but a weaker `info`.

*Rejected: deriving the size from a forward decode instead.* The seek table
does fall out of a forward scan for free, so `pgdq parse` would learn both
without the walk — but every caller takes `size()` **before** the first read,
so a forward-only derivation means having no answer at the moment the answer is
required. That is D1 restated as an implementation detail rather than a
different decision.

### D2 — All three shapes are read; the third is warned about, never refused

No shape is rejected at open. The one-stream-one-block file is read exactly as
the others are, and the cost — a backward read decoding from byte 0 — is
**announced as a diagnostic when the container is discovered**, not discovered
by the user as a stall. `pgdq parse` is unaffected in fact as well as in
principle: it never reads backwards.

*Rejected: refusing `query` on a non-seekable file.* It denies the user a
command that would work, merely slowly, and the user is the only one who can
judge whether one decode-from-zero is worth waiting for. A warning that names
the cause and the fix (`xz -T0`, or `--block-size`) leaves the choice where it
belongs.

The Future item *"let a live scan emit rows again, by carrying the map in the
resume token"* removes the replay pass's backward seek by construction, and a
non-seekable source is what promotes it from an optimization to an enabler. It
stays a Future item; this phase only makes the case for it concrete.

### D3 — `ByteRangeSource` becomes dyn-compatible

The trait today returns `impl Future` (RPITIT), so it is not dyn-compatible and
every consumer is generic: `scan`, `build_index`, `scan_preamble`,
`preamble_only`, `map_forward`, `map_file`, `table_stream`, `attach_text`,
`cache::load`/`save`. With one implementation the CLI names it concretely at
three sites; with a *wrapping* second one the branch becomes a product, and P14
squares it again (local, xz-over-local, remote, xz-over-remote).

So the trait's methods return `Pin<Box<dyn Future<Output = …> + Send + '_>>`
and callers take `&dyn ByteRangeSource` / `Arc<dyn ByteRangeSource>`. Pre-1.0
there is no compatibility cost, `object_store`'s own trait is dyn-safe, so this
*increases* the mirroring the trait was shaped for, and one boxed future per
`read_range` is one allocation per **1 MiB chunk** — noise beside a 446 MB/s
decoder.

*Rejected: staying generic* with a per-command `match` over the source kind.
Zero runtime cost and no API change, but it pushes the same combinatorial
branch onto every embedder and every test, and it grows with P14 rather than
being paid once. *Rejected: an `enum AnySource`* — it keeps static dispatch and
one code path, at the price of L1 enumerating every source the project will ever
have, including the recursive wrapping case.

**The honest cost of D3**: it touches the hot read path behind every published
throughput figure, so `measure.py --stale` will redden them with no mechanical
oracle to clear it — `--verify-additive` settles generator changes only. That
is a library change, so the figures stay stale until a sweep.

## Layering

Settled ahead of this phase by [`layering.md`](layering.md): the decompressing
source is an **L1 addition**, a further `ByteRangeSource` implementation that
wraps whatever supplies its compressed bytes, and a seek table is L1 data like
the rest of the cache. Neither byte source knows about the other; they compose
in one direction only.

### D4 — `stored_size()` joins the trait, and `total_size` leaves `identity.size`

The cache's staleness check must stay a `stat`. Under D1 an xz source's `size()`
is the uncompressed length, which costs the footer walk to observe — so using it
as the identity would make the *check* the most expensive thing in `pgdq info`,
and would check a derived number rather than the file.

So `ByteRangeSource` grows **`stored_size()`** — the bytes as stored on the
device — defaulting to `size()`, which is what every non-decompressing source
returns. `SourceIdentity` records `stored_size` plus `modified()`, both of them
`stat`-cheap for a wrapped local file. The **uncompressed total** that
`CacheStatus::Valid`/`Incomplete` hand out as `total_size` becomes its own
recorded field instead of an alias of `identity.size`. `FORMAT_VERSION` bumps
and nothing migrates.

*Rejected: handing `cache::load` the inner source beside the outer one.* Every
caller would carry two sources so that one could be interrogated about the
other, which puts the composition back in front of the layer D3 just cleared it
out of.

### D5 — The seek table is a sibling of `ContainerKind`, not a value of it

`ContainerKind` means *what produced the indexed blocks' byte offsets*, and an
xz-derived index's span offsets are **uncompressed** offsets — byte for byte the
offsets a plain scan of the decompressed file produces. They *are* plain-format
offsets, so the tag stays `Plain` and the seek table goes in a sibling envelope
field describing the *source* instead. P8's entry-relative offsets remain the
only thing that ever changes that tag, which is what it was reserved for.

A property falls out and is worth stating: a cache built from the `.xz`
describes the decompressed file equally well, differing only in the identity
that guards it.

**An entry is `(uncompressed_start, compressed_start)` per block, plus the
enclosing stream's check type** — a block decoder needs the check flag from the
*stream* header, which the block header does not carry. Size is not a concern in
any shape: 31,150 entries for the multistream file, ~5,700 for the multiblock
one, against a koji cache that is already 833 spans.

### D6 — One streaming decoder, restarted on seek; no retained output

`XzSource` holds a single live decoder and its current uncompressed position,
behind a mutex — the trait takes `&self` and is `Send + Sync`. A read at the
current position keeps pulling, which is every read on the forward path. A read
anywhere else restarts the decoder at the block covering the offset and discards
to it.

*Rejected: retaining the last decoded block*, so repeated reads inside one block
are free. It costs 24 MiB resident on the multistream file and **128 MiB** on
the multiblock one, to accelerate a pattern the library barely has: every read
but the two backward ones is sequential, and those are served with zero waste by
the streaming form. *Rejected: decoding from the covering block on every call*,
which re-decodes 24–128 MiB per 1 MiB read.

Concurrent `read_range` calls serialize on the mutex. Nothing calls
concurrently today, and parallel stream-aligned decode is P7's — its inbox
already carries the note that this is where the scaling is.

## Evidence this phase rests on

**Decode throughput, as probes rather than figures.** Three runs on a 300 MiB
slice cut at a stream boundary ~20 GB into the multistream file (17.8x ratio
there against 19.4x for the whole file, so it is representative — the file's
*head* compresses 54x and decodes at 357 MB/s, which is not):

| Probe | Result |
|---|---|
| `xz -dc -T1`, one process | 5.59 GB out in 12.5 s — **~446 MB/s** of plaintext, 108% CPU |
| `xz -dc -T8`, one process | 12.65 s, 108% CPU — **no speedup at all** |
| Four `xz -dc -T1` on four stream-aligned pieces | 3.77 s, 397% CPU — **~1.48 GB/s**, near-linear |

The middle row is not an error: xz's threaded decoder parallelises *blocks
within a stream*, and every stream in the multistream file holds one block, so
its own CLI cannot use more than one core on it. Decoding whole streams
concurrently scales.

446 MB/s against the published koji scan's ~240 MB/s says the compressed path is
not slower than the plain one even serially, since it reads 19x fewer bytes —
37.6 GiB at HDD speed is ~165 s of I/O against ~3200 s for the plain file. **No
document may quote these numbers**: no harness, no `drop_caches` discipline, no
`measure.py` registration. This phase owes real figures through
`scripts/measure.py` like every other performance claim
([`measurements.md`](measurements.md)).

**Exactly two callers read backwards**, which is what bounds D2 and D6. Every
other read in the library walks forward from a start offset.

- `stream.rs`'s replay loop reads `[block.header_offset, block.end_offset)`
  after the mapping pass has already walked past that block — the deliberate
  double read under [`architecture.md`](architecture.md), "Query: mapping and
  streaming are separate passes".
- `map.rs`'s `attach_text` re-reads the gaps between `Data` spans once the scan
  has finished, coalescing contiguous text-storing spans into one read per gap.
  This is what `pgdq info --verbose` and `--map` report from.

Neither is on the `pgdq parse` path, which is purely forward. Re-check by
grepping `read_range`; there are four call sites.

**A complete cache already reduces a query to the target block's bytes.**
`stream::table_stream` skips the preamble prepass when the cached first-database
preamble is complete (the `first_db_preamble_known` guard), and the mapping pass
walks only from `scanned_through` to the target — nothing, when the cache covers
the whole file. What is left is the cache read, `stored_size()`/`modified()` for
the identity check, and the replay read of the target block. That is what makes
the seek table pay: with both in hand, reading one table out of a 40 GB `.xz` is
a decode of the blocks covering that table's uncompressed byte range, not of the
file. P14 depends on the same fact.

*Rejected: adopting DataFusion's compression layer.*
`datafusion/datasource/src/file_compression_type.rs` is a `Stream`/`Read`
adapter over `async-compression`'s `AsyncXzDecoder` and
`XzDecoder::new_multi_decoder`. It exposes no seek table and no positioned read,
and `object_store` beneath it does not decompress at all. Adopting it buys
`pgdq parse` and gives up `pgdq query`, because a `TableProvider` over a
compressed file there reads it start to finish and gives up file splitting.
Re-check that file at the version P6 eventually targets.

## Blocked: the seekable-xz layer is not a library that exists

Grilling D6 turned up the fact that reshapes this phase. **`liblzma`'s safe Rust
API exposes no stream-index parser and no block decoder** — `new_stream_decoder`
and `new_raw_decoder` are the whole surface, while
`lzma_index_buffer_decode`, `lzma_block_header_decode` and `lzma_block_decoder`
exist only as raw FFI in `liblzma-sys`. Block-granular seeking is not optional
here: the multiblock koji file is one stream of ~5,700 blocks, so a
stream-granular implementation would decode it from byte 0 on every backward
read — behaving as shape 3 for the very file built to avoid that.

So somebody has to parse the xz container. The survey of who already does:

| Crate | License | What it has | Why it is not this |
|---|---|---|---|
| `liblzma` 0.4.8 / `liblzma-sys` (vendors xz 5.8.3) | MIT/Apache | the fastest decoder; index and block APIs as raw FFI only | no safe index or block surface; using it means `unsafe` FFI in L1 |
| `lzma-rust2` 0.20.1 | Apache-2.0 | `StreamHeader`, `StreamFooter`, `Index`, `BlockHeader::parse`, multi-stream block scanning, parallel block decode | all of it **private**; the only public hook is `XzReaderMt::block_count()`, and **no `Seek` impl exists anywhere in the crate** |
| `xz4rust` 0.2.3 | MIT | pure-Rust, no-std, memory-safe decoder; public `XzBlockHeader`, `XzCheckType`, decoder reset, caller-driven in/out buffers | a decoder, not an addressing layer — no index parsing, no seek table |
| `gibblox-xz` 0.0.2-rc.4 | **GPL-3.0-or-later** | closest in shape: async block reader over xz with a decoded-block cache, footer scanning, multi-stream | GPL, pre-alpha, hard-caps a block at **64 MiB** (koji's multiblock file uses 128 MiB), and drags in `gibblox-core`'s own `ByteReader`/error framework |
| `ixz-core` 1.0.0 | BSD-2 | pixz-compatible indexed extraction; seeks to a block's compressed offset | extraction-oriented (`WantedFile` → `Write`), not a positioned read; provenance is poor — `repository = "https://github.com/example/ixz-core"`, authors "ixz contributors" |
| `iluvatar` 0.3.0 | MIT/Apache | random access to compressed **tar/cpio** via decompression checkpoints | archive-scoped, checkpoint model rather than block addressing |

**Nothing provides a positioned read — `read_range(offset, len)` over the
uncompressed stream — as a library primitive.** The substrates are all there and
two of them are permissively licensed; the addressing layer is what is missing.

That layer is its own piece of software, with its own test corpus and its own
fuzzing story, and it is not `pgdump_query`'s subject matter. **It lives in a
separate crate and a separate repository** — provisionally `xz-seek`, running on
a copy of this project's process and skills — on the pattern already set by the
collation spike (`CLAUDE.local.md`): the requirements are written from here and
kept there, as that repo's frozen `docs/design/historical/initial.md`, and
nothing here builds against it or is scheduled to.

**The requirements are stated there, not here**, and the two that bind this
phase hardest are worth naming: the seek table must be an extractable,
re-injectable value, since D5 persists it and nobody may re-walk 31,150 footers;
and the crate takes its compressed bytes through a caller-supplied trait rather
than opening files, which is what makes D3's composition — and P14's remote
case — work at all.

**P13 is therefore blocked on that crate.** D1–D6 above stand — they are
decisions about `pgdump_query`, not about the decoder — and the phase resumes
when the crate can answer a positioned read.
