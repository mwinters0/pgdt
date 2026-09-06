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

It is the maintainer's priority, and it **inverts the arithmetic behind
"device-bound"**: reading koji compressed is 37.6 GiB off the device instead of
730 GiB, paid for in CPU at roughly 446 MB/s of plaintext per core. The
local-file read path's own defaults were measured before this phase, against
uncompressed input on three device classes, and the number they were decided on
— that no overlap scheme can put a scan below the time the device takes to
deliver the bytes — is exactly the one a compressed source breaks
([`architecture.md`](architecture.md), "Execution model and API surface"). So
this phase does not inherit those defaults as settled; it inherits them as
measured **for a source shape it does not have**, and owes its own reading of
whether the chunk size and the read shape still suit when the device is no
longer the bound.

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

**The warning is a new `DiagnosticKind` variant, pushed once when the seek
table becomes available** — freshly walked or loaded from the cache — through
the same channel `TilingBroken`/`CacheMtimeChanged`/`TocCoverage`/`CacheOffline`
already use (`diagnostic.rs`), rather than a bespoke print or a `CacheStatus`
addition. Something in the shape of `DiagnosticKind::NonSeekableCompressedSource
{ block_count: usize }`, named from `SeekTable::is_seekable()`/`block_count()`
(`xz-seek`'s `SeekTable::blocks_in(range)` is the sharper form for a bounded
warning, once that method lands — see D6's fourth bound property below), with
text naming the cause and the remedy exactly as the paragraph above states it.

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

### D4 — `stored_size()` joins the trait, `total_size` leaves `identity.size`, and identity becomes opaque

The cache's staleness check must stay a `stat`. Under D1 an xz source's `size()`
is the uncompressed length, which costs the footer walk to observe — so using it
as the identity would make the *check* the most expensive thing in `pgdq info`,
and would check a derived number rather than the file.

So `ByteRangeSource` grows **`stored_size()`** — the bytes as stored on the
device — defaulting to `size()`, which is what every non-decompressing source
returns. The **uncompressed total** that `CacheStatus::Valid`/`Incomplete` hand
out as `total_size` becomes its own recorded field instead of an alias of
`identity.size`. `FORMAT_VERSION` bumps and nothing migrates.

**`SourceIdentity` becomes an opaque enum, not a struct**, with one variant
today:

```rust
enum SourceIdentity {
    LocalFile { stored_size: u64, mtime: Option<(u64, u32)> },
}
```

This phase is already rewriting every `SourceIdentity` call site for
`stored_size`, so the cost of taking this shape now is near zero. The payoff is
downstream: `ByteRangeSource` and `SourceIdentity` are edited by four phases in
sequence — P13, P15, P14, P16, in that order
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "What
`xz-seek` was told, and what it commits us to") — and P14's remote source has
no mtime at all — an ETag is not a
`SystemTime`. With identity opaque, P14 adds a `Remote { etag: String }` variant
and the match sites that already exist gain an arm; with the struct kept as-is,
P14 redesigns the type from scratch and every accessor with it. Match sites
read the fields through the variant rather than through a shared struct
accessor, since the two kinds of identity share no fields worth naming
generically (an ETag is not a size-plus-mtime pair with a different name for
the second half).

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

**That sibling field is an enum tag from day one, xz-only in its content**:

```rust
enum CompressionIndex {
    Xz(xz_seek::SeekTable),
}
```

P15's gzip index is a different *shape*, not a variant of this one — a set of
checkpoints carrying ~32 KiB of dictionary state each, not a list of
independently decodable blocks — so nothing here tries to unify the two
layouts. What the enum buys is that P15 adds `CompressionIndex::Gzip(...)` as a
sibling variant when it lands, rather than restructuring this field or
guessing its shape now from one instance.

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
concurrently today, and parallel stream-aligned decode is P16's — its inbox
already carries the note that this is where the scaling is.

**Four properties of `xz-seek`'s own interface bind this decoder, settled in
that crate's own `P3` grilling rather than here**
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "What
`xz-seek` was told, and what it commits us to"):

- **Delivery is repeated fill** — `Reader::read_at(&mut self, offset, buf) ->
  Result<usize>`, fill-or-EOF, the caller loops. This is what the three read
  loops already do, and an iterator of owned chunks was refused because it
  would reintroduce the per-chunk `calloc` the buffer pool exists to remove
  and would hand the query path 24–128 MiB buffers to retain behind Arrow
  string views addressing a few kilobytes of them.
- **The boundary is blocking**, matching `LocalFileSource`'s existing
  `spawn_blocking` around a synchronous read. `XzSource::read_range` wraps
  `Reader::read_at` the same way.
- **Verification defaults to `Verify::Full`**, which completes a
  partly-decoded block's check before the reader leaves it — the bulk path's
  stronger default, matching D2's forward-only case where nothing is ever
  abandoned mid-block anyway. On failure, `Error::BlockCheckFailed` carries
  the **uncompressed** range, which is the coordinate space this project's
  whole index lives in.
- **`SeekTable::blocks_in(range)`** is D2's warning with a number in it — the
  block count a warned-about non-seekable file has, used as noted at D2 above.

### D7 — `size_is_exact()` joins the trait now, defaulted `true`, for P15's benefit

`size()` keeps meaning the exact addressable length (D1), but P15's codecs
cannot always answer that exactly — gzip's `ISIZE` is useless above 4 GiB and
zstd's frame content size is optional. D3 is already rewriting every
`ByteRangeSource` signature in this phase, so this is the one pass in which
adding a defaulted `fn size_is_exact(&self) -> bool { true }` costs nothing;
deferring it to P15 would mean that phase touching the same signatures a
second time for a signal this phase could carry for free. Every source that
exists today, xz included, answers `true` — xz's size is exact, from the
stream index. `pgdq info`'s coverage arithmetic reads this once P15 lands a
source that answers `false`; nothing consumes it yet.

*Rejected: leaving this for P15.* D1 rejected pre-spending the *relaxation*
(a bound instead of an exact number) on the ground that xz can answer exactly
and pre-spending buys this phase nothing. That reasoning does not extend to
the signal alone: the relaxation is a behavior change with no consumer yet,
where the signal is one trait method this phase's own rewrite makes free.

### D8 — Recognition is a library-level convenience, separate from the agnostic trait

The trait and its implementations stay source-agnostic: `LocalFileSource` and
the new `XzSource` are each constructed directly, and neither the trait nor an
embedder that already knows what it has is obliged to go through recognition
at all. On top of that, the library exposes an explicit opening convenience —
`pgdump_query::io::open_local(path) -> Result<Arc<dyn ByteRangeSource>>` (name
to be settled at implementation) — that reads the first bytes of the file
looking for the `.xz` magic (`\xfd7zXZ\x00`) and returns a plain
`LocalFileSource` or an `XzSource`-wrapped one accordingly. The CLI calls this
once instead of hand-rolling the same dispatch at its three call sites
(`parse`, `info`, `query`), and P15 extends the same function with its own
codecs' magic bytes rather than the CLI growing a second dispatch.

Content sniffing rather than a path-extension check: a `.xz` file is
recognised whatever it is named, and a file named `.xz` that is not one fails
with `xz-seek`'s own clear `Error::NotXz` rather than being silently handed to
`CopyScanner` as binary garbage. The cost is one small read (the magic is 6
bytes; a real implementation reads a small header-sized buffer once) ahead of
the source construction it was already about to do.

*Rejected: extension-based dispatch.* Cheaper (no read before the source
exists) but wrong on a renamed or extensionless file, and it would have to be
reimplemented by every embedder that wants the convenience rather than living
once in the library.

## Fixtures

**Two, both derived from `pgdump_query/tests/data/edge_cases.sql`** (2,352
bytes, already the source-of-record for the scanner/cache-level tests in
`tests/cache.rs`, `tests/census.rs` and others — not a per-major
`fixtures/<version>/...` schema fixture, since nothing about xz container
shape depends on a PostgreSQL major). No variety of xz encoding options: one
fixture per shape this phase's tests actually exercise, not a matrix.

**Generated at test time, not committed.** A test helper shells out to the
`xz` binary (`std::process::Command`, into a `tempfile` directory) rather than
committing binary `.xz` blobs to git: the input is 2,352 bytes, the
compression cost per test run is negligible, and this project's own instinct
is regenerable artifacts over committed ones wherever regenerating is cheap.
`xz` is not `mise`-pinned — it is assumed present the way `docker`/`nerdctl`
already are for fixture-*generation* scripts — so the test asserts its
presence (`which xz` or a failed spawn reported clearly) rather than silently
skipping, per the standing rule for a tool a test reaches for externally.

- **Seekable, multi-block**: `xz --block-size=<small>` on `edge_cases.sql`,
  the block size picked small enough that the 2,352-byte input still splits
  into at least two blocks — confirmed on this machine's `xz` 5.8.3 that a
  small `--block-size` forces the split on an input this size. Exercises the
  normal path: recognition, the seek table, `XzSource::read_range`,
  `stored_size()`/`size()` against the compressed/uncompressed split.
- **Non-seekable, single block**: plain `xz` on the same input with no flags.
  Confirmed on this machine that a bare `xz` invocation with no `-T`/
  `--block-size` produces exactly one stream, one block — this repo's `xz`
  5.8.3 does not default to `-T0`. Exercises D2's diagnostic and the
  decode-from-zero backward-read path.

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
`measure.py` registration.

**This phase's slices commit to no measurement row, deliberately.** D6 is
serial decode only, restarted on seek — the maintainer's call is that a figure
taken against that shape is not worth registering before P16's parallel decode
exists, since the number a caller actually wants (concurrent decode throughput
against the plain path's device-bound figures) is unreachable until then. The
probes above stand as probes, not as a promise this phase owes a sweep for;
`scripts/measure.py --list` gains no P13 row and none is implied by "the
standing rule that a performance claim needs `measurements.md`" — none of this
phase's slices makes one. Re-open when P16 lands parallel decode, which is
also where the honest comparison lives.

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

### D9 — The dependency is a frozen vendored copy, not published, wired in by this phase

**The blocker that opened this phase is discharged.** No published crate
answered a positioned read over an `.xz` file when this phase was specified —
`liblzma`'s safe Rust surface exposes no stream-index parser or block decoder,
and the survey of `lzma-rust2`, `xz4rust`, `gibblox-xz` (GPL), `ixz-core` and
`iluvatar` found no library providing `read_range(offset, len)` over the
uncompressed stream. That gap was carved out into its own crate and repository,
`xz-seek`, on the collation spike's pattern (`CLAUDE.local.md`): requirements
written from here, kept there as that repo's frozen
`docs/design/historical/initial.md`. It now ships `Reader::read_at`, held to
`xz -dc`'s own output over 531,684 `(offset, len)` pairs on its fixture corpus
and confirmed over both koji files across all 730 GiB the 40 GB multistream
file decodes to
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "P13's
blocker is discharged: a positioned read over `.xz` exists"). D1–D8 above
were decisions about `pgdump_query`, never about the decoder, and stand
unchanged.

**`xz-seek` is not published until this repo has integrated against it** —
settled by the maintainer. This project is its first real-world consumer, and
that consumption vets the interface before it is frozen into a published
version. The gate is both this phase and P10 landing, with the shape questions
resolved; publication and a proper versioned dependency follow.

**The mechanism is a frozen copy committed at `vendor/xz-seek/`, not a path
dependency onto the sibling working tree.** A live path dependency would break
`cargo check`/`cargo test --workspace` on any checkout lacking the sibling
repo — verified: even behind an off-by-default feature, the dependency graph
is resolved before features are considered, so a missing sibling checkout
fails with `failed to read …/Cargo.toml` regardless. A committed copy keeps
this repo self-contained and decoupled from a tree that is still iterating.
The copy is **read-only** — a bug found here is fixed upstream and returns at
the next sync, never patched in place — and excluded from this workspace
(`exclude = ["vendor/xz-seek"]`), re-synced on demand by
`scripts/vendor_xz_seek.py`, which reads `git archive HEAD` from the source
checkout, trims the manifest to what a dropped-in copy needs, and stamps
`vendor/xz-seek/VENDORED_FROM` with the source commit. The snapshot today is
`54c7983`, taken while `xz-seek`'s own `P3` (parallel block decode) was still
unstarted, which is what made it a stable moment to vendor.

**This phase is what wires it in.** Being excluded from the workspace does not
stop `pgdump_query`'s own `Cargo.toml` naming it as a path dependency
(`xz-seek = { path = "../vendor/xz-seek" }`) — exclusion only keeps it out of
the workspace's shared lockfile and member list. D3–D8 above are what that
dependency is for: the dyn-compatible trait, `stored_size()` and the opaque
identity, the `CompressionIndex::Xz` envelope field, the streaming decoder,
the size-exactness signal, and the recognition convenience.

**The build takes `liblzma`**, the crate's own default and the vendored
manifest's, over `xz4rust` (pure Rust, unsafe-free, ~2.2× slower) — pulling
vendored C into this otherwise-pure-Rust workspace, on the ground that the
integration being vetted is then the one that ships. Nothing here builds
against either yet; this is the manifest's default, open to revisit once this
phase's slices actually compile against it.

**What this bounds this phase's own promise to**: no published crate exists,
so nothing here names a version to pin, and there is a window — until P10 also
lands — in which the build has a dependency whose provenance is this repo's
own vendoring script rather than crates.io.
