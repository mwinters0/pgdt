# P15 — gzip input

The phase's spec, replacing the roadmap's sketch; its inbox is drained into it.

## What and why

`.gz` input in both shapes it arrives in — the single-member stream `pg_dump
-Fp -Z` writes, and the multi-member one of which BGZF is the disciplined case
— **and the seams of the decompression layer redrawn so that `.xz` and `.gz`
are two implementors of one codec-generic layer** rather than gzip arriving as
a second copy of the xz source.

- **This closes a compatibility gap, not a convenience.** For plain text a
  nonzero `--compress` level compresses the whole output file, and gzip long
  predates the method selector, so `pg_dump -Fp -Z9` has produced a file this
  build cannot read for as long as there has been a build — and the file it
  has to read is the single member `pg_dump` writes streaming (D14).
- **One phase, not two**, because BGZF is a strict subset of gzip rather than a
  sibling format: the magic and the decoder are the same, and a gzip source
  that could not read a BGZF file would be wrong. What differs between the
  shapes is how the index is found, never the recognition path, the decoder or
  the cache's index (below).
- **Ahead of P8**, whose Track B needs per-entry deflate decode inside the
  archive containers: landing the inflater here means that track reuses it
  rather than acquiring it alongside a TOC parser.

## Settled in the grilling

- **The seam is redrawn inside this phase, as its own slice, before gzip
  lands.** The cut is xz-only and behaviour-preserving: the codec-generic layer
  is drawn with xz as its one implementor, and gzip arrives in later slices on
  top of it. Not a `/seams` round ahead of the phase, which would generalize
  from one example; not drawn as gzip goes in, which pairs a new module with a
  rework of a tested core path (`../process.md`, "The loop"). A cheap evidence
  slice goes ahead of the cut — gzip fixtures in both shapes, and `pg_dump -Z`
  output from each tested major — so the design is drawn with gzip's shapes in
  hand.
- **The layer is checked, not built, against what comes after.** xz and gzip
  are built; zstd single-frame, zstd seekable (P18) and P8's per-entry decode
  inside the custom and directory archives are checked at sketch depth: the
  design must admit each without reshaping the trait, `KnownCompression` or
  the cache's `CompressionIndex`, and nothing for them is built here. This
  phase settles the `size()` contract for P18 as well (`roadmap.md`, "P18 —
  zstd and lz4 input").
- **The addressing layer is a crate of its own, `gz-seek`, on `xz-seek`'s
  pattern, not a module of `pgdt-io`**: it answers a file format's positioned
  read with no `pg_dump` in it, and keeping it apart keeps its size contract
  its own, which is what the xz crate declined to generalize. **It is built in
  this workspace first** (below), and split off into its own repo once
  functional. Where an existing crate covers most of it, the standing rule
  against reinventing wheels governs instead. **None does**
  (the crates, below): the inflater is bought, never written, and what no
  crate supplies is the layer above it — checkpoints built during a forward
  decode, a resume from one, the BGZF walk, the index handed out. Rejected,
  on an evaluation run (the 2026-10-21 history entry): depending on
  `rapidgzip`, the one crate covering most of it. It holds every window
  resident, a copy a reader; carries no check a unit and accepts a truncated
  file; places checkpoints by a chunk size that moves with its parallelism and
  can neither resume nor extend an index; holds a chunk cache our budget cannot
  bound, allocated outside it; runs its parallel pass on a thread pool of its
  own (D1, D12); and does not link under a release profile as shipped. What
  stays admitted is its technique, for `gz-seek` to implement natively if the
  evidence slice earns a parallel first pass; `rusty_rapidgzip`, a native port
  not evaluated here, is the first thing slice 4 reads.
  **Why a crate of its own rather than a module** holds for the phases after this one:
  P18's seekable zstd needs an addressing layer of its own, so a codec's
  positioned-read crate beside the generic layer is the pattern `xz-seek` and
  `gz-seek` set, while P8's per-entry decode is forward-only over the inflater
  and never needs the crate.
- **One vocabulary for the compression index, owned by `pgdt-io`.** One enum,
  `CompressionIndex`, a variant per codec layout (`Xz(SeekTable)` and gzip's);
  the trait answers `compression_index() -> Option<CompressionIndex>` in place
  of `seek_table()`; `KnownCompression` becomes `{Unknown, Plain,
  Compressed(CompressionIndex)}`; the cache envelope persists `pgdt-io`'s enum
  under a neutral key in place of `seek_table` (a `CACHE_FORMAT_VERSION` bump,
  and the same key in `pgdt info --json`), `pgdt-dump`'s parallel enum
  deleted. `Error::Xz` becomes a codec error naming its codec. Why: the three
  copies today are one idea stated three times, and the single-variant
  matches that assume xz then fail to compile at every site that would
  otherwise go on assuming it.
- **The arms are generic over a codec; the sources are not per codec.** A codec
  trait supplies each unit's plaintext and compressed ranges, the largest unit,
  the decoder's footprint, "decode unit *i* whole from this positioned source",
  and a forward-only begin/advance/read/complete handle. The local arm (one
  `spawn_blocking` a read, over the file) and the fetched arm (back to the
  runtime between units, over any `Arc<dyn ByteRangeSource>`) are each written
  once over it, with the budget, the block cache, `piece_of` and the live
  handle's state machine; gzip is an implementation of the trait, not a third
  and fourth source. D15's "never one body branching on the provider", carried
  to the codec. Rejected: a source per (codec, transport) pair; one
  fetched-style arm for local files too, which re-decides the local arm's
  scheduling without a measurement.
- **Placement: `pgdt-io`'s `source/compressed/`.** The shared layer — the codec
  trait, the two arms, the block cache, the budget, the live handle,
  `piece_of`, recognition by magic — is that directory, and each codec's
  adapter, holding only what touches its crate, sits under it
  (`compressed/xz.rs`, and `compressed/deflate` — the codec is deflate, below).
  The crate is not in question
  (D68 puts the byte sources in io); a sibling `source/gzip/` beside
  `source/xz/` was rejected because what they share would have no place of its
  own and would spread across `source/` beside the plain and remote arms,
  which share none of it.
- **One gzip index layout: a checkpoint list.** `CompressionIndex::Gzip` holds
  checkpoints, each a plaintext offset, a compressed bit offset and a window —
  empty at a member start, which is a checkpoint at a fresh history. BGZF,
  concatenated members and the single member are one layout, and the codec
  trait presents each checkpoint-to-checkpoint span as a unit as it presents
  an xz block. What differs between the shapes is how the index is *found*,
  not what it is. Rejected: the sketch's two variants (members, checkpoints),
  which leave a multi-member file whose members are wider than the spacing
  with no variant.
- **A single member's index is a by-product of the first full pass.** The
  first `parse` or cold `query` decodes serially and takes checkpoints as it
  goes; they are saved with the span index, every later read is parallel at
  checkpoint units, and an interrupted parse saves the checkpoints up to its
  watermark, so a resume starts from the last one rather than from zero.
  **The seam admits a parallel first pass** (rapidgzip's speculative block
  finding): it would replace only how the first pass finds units, never the
  index, the codec trait or the arms. Whether this phase builds it is decided
  on the evidence slice's reading of serial inflate against the plain parse
  on the measured devices, not here; the reading earned it ("Slice
  order"). Rejected: a separate indexing pass,
  decoding the whole file twice cold for the same index; the `rapidgzip`
  binding's parallel path, which runs only on a thread pool of its own (D1,
  D12).
- **The codec is deflate; gzip is the framing this phase builds.** The
  inflater, the checkpoint resume and the window are deflate's; the gzip
  framing is recognition, member headers and trailers, and their CRC-32 and
  `ISIZE`. P8 adds the zlib framing its custom-format entries are written in
  over the same inflater, for a forward decode that needs no index. Rejected:
  a `gzip` adapter P8 would reach into for a zlib stream.
- **`size()` becomes `extent() -> Extent`: `Exact(n)` or `Open { at_least }`**,
  replacing `size()` and `size_is_exact()`. A read loop reads with a call that
  may answer short only at the source's end, `read_range` staying strict for a
  driver told what to fetch (`xz_seek::Walk`); the cache's `total_size` is
  known once a pass has reached the end, `pgdt info` states a partial first
  pass as bytes scanned with the total not yet known, and anything that cuts
  partitions requires `Exact` — which a single member before its index reaches
  the end never offers, and it is one partition anyway. A source answers
  `Exact` once its end is reached or a complete index is loaded. This settles
  P18's form of the question too: a streamed zstd frame with no content size is
  `Open`. Rejected: a counting pre-pass (Q8's double decode); keeping `size()`
  as a bound with callers reading `size_is_exact()`, which leaves each of the
  sites relying on an exact size to remember a flag where the type would force
  it. The inbox's finding stands as the cost: every read loop already tolerates
  a short read, and what depends on exactness is the loop's exit test and the
  coverage denominator.
- **Each unit carries its own CRC-32, recorded by the first pass.** A gzip
  member's CRC-32 covers the whole member, so a checkpoint unit decoded alone
  has nothing of the file's to compare against: the first pass, which checks
  each member's CRC-32 and `ISIZE`, records each unit's CRC-32 in its
  checkpoint, and a later unit decode compares against it — **wherever no
  check in the file covers the unit**: a unit of whole BGZF members is covered
  by the members' own trailers, and the walk that finds it (below) decodes
  nothing to record one from. `crc32_combine` relates the stored values to the
  member's own check. Why: D15's "a read compares its check" stays true on
  the parallel path. Rejected: trusting unit decodes after a checked first
  pass, where a bit flip arrives as wrong rows rather than an error; checking
  only reads spanning a whole member.
- **Only offsets, bit positions and CRCs are resident; windows are read when a
  unit's decode begins.** The cache is decoded whole at open, so a window in
  the envelope is resident before any budget exists — about 0.77 GB at 32 MiB
  spacing over a koji-scale single member (arithmetic). Windows go in a
  section of the cache file read positionally, 32 KiB a unit decode, so the
  resident index is the size of xz's table and D4's ordering argument for not
  billing it (`KD26`) stays sound. **Layering**: `pgdt-io` names nothing in
  the cache, so the index carries a window provider — a trait `pgdt-io`
  defines and `pgdt-dump` implements over the cache file — and the cache format
  becomes an envelope plus a window section. Rejected: the index resident and
  billed as the count-independent third `WorkerMemory` term `KD24` priced,
  charging every run for windows only the decoded units need; resident and
  unbilled, past the size `KD26`'s argument is sound at. **Spacing** is a
  shipped constant in the range of xz's block sizes, read off the evidence
  slice, never derived from the file — a first pass does not know the file's
  size when it must choose — because the unit is also the decode unit, the
  block-cache unit and the partition grain. Sparse windows (rapidgzip's) are an
  admitted later shrink, not built. **A first pass holds none either**: it
  writes each window as it places it to an unnamed file the run alone holds,
  in the enabled cache's directory or else the OS temp directory, named by
  `open`'s caller so `pgdt-io` names no cache; it is read as the section is,
  and a save copies the section out of it. A file that cannot be made is
  refused, never answered from memory. Why, and what was rejected:
  [`../status/history/2026-10-23.md`](../status/history/2026-10-23.md), "A
  first pass's windows, reviewed". **Whatever crate builds the index hands it
  out as a shareable handle from the start** — `xz_seek` took its table by
  value and held a second copy until its API was widened (inbox, D4).
- **A checkpoint's place is a function of the file alone**: the first deflate
  block boundary at or after each multiple of the spacing in plaintext, and
  every member start — independent of `--jobs`, the chunk size and where an
  interrupt fell, so a resumed parse writes the cache an uninterrupted one
  does. **`crates/pgdt/tests/determinism.rs` grows compressed inputs**: `.xz`
  in the cut slice, gzip single-member and BGZF in the gzip slices, with a
  resume-equals-uninterrupted assertion for gzip. Why (drained from the inbox):
  the sweep's whole input tree was plain `.sql`, so "a compressed parse at two
  job counts writes one cache" held only in the 2026-09-08 koji run, once —
  and a compressed source is where `--jobs` changes which decode work happens,
  so it is where a determinism defect would live. Rejected: checkpoints
  wherever the first pass's reads land, determinism asserted at one count.
- **`gz-seek`'s bar is ours; its inflater is its own.** What it must meet,
  which becomes its frozen `docs/design/historical/initial.md` at the split as
  `xz-seek`'s requirements are its: sans-IO — a walk and a unit decode driven by bytes the caller
  supplies, as `xz_seek::Walk` and `BlockTask` are, which is what lets the
  fetched arm exist; a forward decode emitting checkpoints by the rule above;
  a single member's parallel first pass, its units found speculatively and
  decoded on the caller's threads (D1, D12), placing checkpoints by the same
  rule;
  a resume from a checkpoint given its window; a CRC-32 a unit; the index as a
  shareable handle; its own size contract, open until the end; a safe API;
  and a throughput floor against zlib-ng, which the evidence slice measures.
  The backend is the crate's decision on its own measurement, as liblzma is
  `xz-seek`'s, taken through `gz-seek` itself: its checkpointing pass and a
  resumed unit decode over every engine giving what an index needs — a C
  library or a crate's C ABI as much as a safe crate, `unsafe` being
  acceptable where an engine needs it — the fastest winning, the lighter build
  within noise, those known to be slow last. A request upstream for a safe API
  is made and never waited on
  ([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md),
  "`gz-seek`'s backend and `unsafe`, reviewed"). Rejected: pinning
  `miniz_oxide`'s `block-boundary` or `libz-rs-sys`'s C ABI from here; ISA-L,
  whose block stops exist only in rapidgzip's fork and whose resume primes
  bits by writing undocumented `inflate_state` fields, so the resume's
  correctness would rest on a fork's internals — reopened only by a reading
  showing the zlib family leaves a unit decode what bounds a gzip read.
- **A first pass is a `PlanNote`, not a diagnostic.** A single member with no
  complete index raises no `NonSeekableCompressedSource`: the first pass makes
  it seekable and nothing the user does would help, so by D19's line it is a
  property of the run — a note that this run is serial, building the index,
  and later reads are parallel. `NonSeekableCompressedSource` stays the
  one-block `.xz`'s, its text taken from the codec rather than saying `.xz`,
  and is a single member's whose complete index is one unit, its writer having
  ended no deflate block before its last write: a finding about the file,
  which recompressing answers, raised from the first pass's end on
  ([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md),
  "A writer that ends no block, reviewed").
- **The unpooled bound is checked on gzip, not argued.** The phase's own
  resident readings on gzip cover at least two reader counts, and `measure.py`'s
  `charge_model_problem`, whose inner fault line is `MEMORY_UNPOOLED_BOUND`,
  reports against it; the constant is re-derived only if that reports an
  overrun. Why (drained from the inbox): both `MEMORY_RESERVE` and
  `MEMORY_UNPOOLED_BOUND` were read off `.xz` inputs alone, and a new source
  states its `default_worker_memory` with nothing obliging it to look at the
  bound its resident is then predicted against — a decoder retaining more
  than liblzma does gets a correct charge and an optimistic prediction, and
  the failure is a cgroup kill at a large limit rather than an error
  (D3, `KD111`, the `reserve` figure). Rejected: arguing that the term is
  allocator retention and so carries over.
- **A `.gz` over HTTP is in scope**, falling out of the fetched arm's
  genericity, tested as `.xz` over HTTP is — composed over a local file and
  against `crates/pgdt/tests/remote.rs`'s server. A single member's first pass
  is a serial ranged read of the object; with no footer walk, neither `KD36`
  nor `KD51` has a gzip twin. Rejected: deferring it, which writes a refusal
  for a composition the design already provides.
- **BGZF is walked at open, as `.xz`'s footers are.** Each member states its
  compressed length (`BSIZE`), so the member chain is walked without decoding:
  the extent is `Exact` (the `ISIZE`s, each under 64 KiB, summed) and the first
  pass is parallel, the walk persisted as D18 persists xz's table. Members are
  coalesced into units of at least the spacing, a 64 KiB unit making the block
  cache and the partition grain absurd. Recognition reads the first member's
  `FEXTRA` `BC` subfield at bytes 12–17, so `ORIGIN_LEADING_BYTES` grows from
  xz's 6 to 18. A non-BGZF multi-member file gets the first pass above, member
  starts as window-less checkpoints. Rejected: a serial first pass for BGZF,
  forgoing what its headers state; reading a `.gzi` sidecar, which the
  persisted walk makes redundant and whose second file would need its own
  identity check.
- **`gz-seek`, provisionally, is built in this workspace as `crates/gz-seek/`,
  and split off once functional.** A leaf: it depends on no workspace crate
  and names nothing of `pg_dump`, so the split is a move rather than a
  rewrite, and its one caller is `pgdt-io`'s `source/compressed/deflate`
  adapter. Why here first: its interface is drawn at the same time as the
  codec trait it serves, and developing it beside that caller keeps both in
  one review until the interface settles; the leaf crate keeps the separation
  the crate-of-its-own decision was for. **At the split** it becomes its own
  repo on a copy of this one's `docs/process.md` and skills, vendored
  read-only under `vendor/gz-seek/` by `scripts/vendor_xz_seek.py`
  generalized to take the crate's name rather than copied; its own agent, its
  `docs/design/historical/initial.md` written from the bar above, and
  `CLAUDE.local.md`'s confirm-before-conversation rule apply from then, not
  before. **Placement**: a member below `pgdt-io`, claimed by "The byte
  sources" in [`concepts.md`](concepts.md) as a `gz-seek` line beside
  `pgdt-io`'s, the crate holding a container's positioned read and `pgdt-io`
  the source over it; like `namespace-init`, a member that is not one of
  D68's seven libraries. Named for the container it addresses (members,
  `BSIZE`, trailers), as `xz-seek` is; deflate is the codec beneath it, and
  P8's zlib framing is a forward decode that never touches it. Rejected:
  `deflate-seek`; a module of `pgdt-io` (Q3's rejected shape, the leaf crate
  keeping the separation); its own repo from the start.
- **Fixtures**: `pg_dump -Z` output from each tested major, generated by
  `scripts/generate_fixtures.py` from the oracle's `postgres:*` images and
  committed as the existing fixtures are — the compatibility claim's own
  evidence; and synthetic single-member and BGZF payloads written in
  `test_support` by dev-dependencies, `flate2` for gzip and `noodles-bgzf`'s
  writer for BGZF, so our reader is checked against an encoder not ours and
  `Cargo.lock` pins the writer as `mise` pins a tool (`roadmap.md`, "A test may
  assume the tools `mise` pins"). Rejected: the `gzip` and `bgzip` binaries,
  `bgzip` being neither installed here nor pinned; committed binaries for all
  of it.
- **Inherited from the `.xz` source unchanged** (drained from the inbox):
  `stored_size()` on the trait with `SourceIdentity` recording it, so
  staleness stays a `stat` (D21); `ContainerKind` stays `Plain` beside the
  compression index, the span offsets being plain-format offsets; and a unit
  the budget cannot hold whole is read through one live forward-only handle
  kept across reads (D15). The xz crate declined generalizing on purpose
  (`xz-seek`'s `docs/design/historical/initial.md`, its R3), which is why the
  gzip layer is a crate of its own rather than an extension of it.

## Slice order

Each slice makes the next one's mistakes visible: the generic layer is cut and
its contract changed against xz, already tested, before any gzip code lands,
and evidence lands before the slice that first reads it: the serial inflate
figure goes first, deciding `gz-seek`'s throughput floor and whether a parallel
first pass is built, and the spacing figure follows `gz-seek` and precedes the
adapter, the spacing being the caller's parameter to `gz-seek` and `pgdt-io`'s
shipped constant ([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md),
"The spacing figure, reordered"). The checklist is written into `STATUS.md` when the phase is sliced.

1. **Evidence.** The fixtures above; `measure.py` figures for serial inflate
   (zlib-ng, zlib-rs, `miniz_oxide`) against the plain parse on each measured
   device. They decide `gz-seek`'s throughput floor and whether a parallel
   first pass is built.
2. **The cut.** `source/compressed/` with the codec trait and xz its only
   implementor; one `CompressionIndex`, `KnownCompression::Compressed`, the
   codec error, the cache key renamed under a format bump; `determinism.rs`
   gains `.xz`. Behaviour-preserving.
3. **`extent()`.** The trait change and every site relying on an exact size,
   plain and xz answering `Exact` throughout. Behaviour-preserving, and apart
   from the cut because it reworks the scan loops, a different core path.
4. **`gz-seek` built in the workspace.** The sans-IO forward decode emitting
   checkpoints by the placement rule; the resume from a checkpoint given its
   window; the unit CRC; the BGZF member walk; the index as a shareable
   handle; and the leaf admitted to the graph — a `scripts/layering.py`
   `GRAPH` row with no members, `gz-seek` added to `pgdt-io`'s row, `gz_seek`
   added beside `xz_seek` to the names `CORE_NAMES_NO_DEPENDENCY` and `NO_IO`
   refuse, D68's entry naming it beside `namespace-init`, and its files
   claimed in `concepts.md`. The spacing is a parameter the caller passes.
   Then **the spacing figure**, on `gz-seek`: gzip's own costs at candidate
   spacings in the range of xz's block sizes — the checkpointing first pass
   against a serial inflate, a resumed unit decode's fixed cost, the window
   section's size — the costs a unit's size shares with xz being read off
   the figures xz already drives. It decides the spacing constant.
5. **Single-member gzip.** The `deflate` adapter over `gz-seek`; checkpoints by the
   placement rule; unit CRCs; the window provider and the cache's window
   section; the first pass's `PlanNote`; the determinism and resume
   assertions; over HTTP.
6. **BGZF.** `pgdt-io`'s side, over the crate's walk: the walk run at open,
   recognition at 18 bytes, coalescing members into units.
7. **Readings and the record.** Resident readings at two or more reader counts
   against the unpooled bound, the gzip throughput figures, the manual and
   [`pg-dump-compatibility.md`](pg-dump-compatibility.md).

**The evidence slice's reading earned a parallel first pass, slice 8, between
6 and 7**, so 7's gzip figures read it
([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md), "15.8,
reviewed"). **The test**: a single member's serial first pass is held by its
inflate wherever a serial inflate is slower than the pass's own floor — the
`.gz` file's device floor or its scan over plaintext from memory, whichever is
slower — which holds on every measured device
([`measurements.md`](measurements.md), "What serial inflate costs against the
plain parse"); the ratio against the plain `parse` says only whether a `.gz`
file is slower than the plain one. `gz-seek` finds a single member's units
speculatively — rapidgzip's technique, implemented natively and driven sans-IO
on the caller's threads — placing checkpoints by the placement rule, so the
index is the serial pass's at every worker count; slice 4 builds none of it.
**It lifts the inflate, not the scan**: a first pass's scan stays one partition
(`extent()`, above), so a `parse`'s first pass falls toward its scan's floor,
while a cold `query`'s stays bound by its one-partition typed scan, whose rate
is of the order of a serial inflate's (`parallel-scan-throughput`). Whether the
first pass's scan is partitioned too, over units whose plaintext offsets
become known as the pass goes, is slice 8's own question, asked against the
arms 2, 3 and 5 leave. Rejected: deferring it to a later phase or out-of-band,
which moves only the wrap.

**The backend figure, slice 9, precedes the spacing figure**, so that the
spacing is read on the engine that ships
([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md),
"`gz-seek`'s backend and `unsafe`, reviewed"): `gz-seek`'s checkpointing first
pass and a resumed unit decode over zlib-rs, zlib-ng and stock zlib, each a
leg under the one zlib-API module. Which build routes reach each engine, which
of them work and which is recommended are the slice's own findings
([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md),
"15.9's build routes, reviewed"). It decides the backend, whose choice is a
decision entry of its own, the upstream register keeping only why the engine
is reached through its C API.

**Checkpoints inside a static block, slice 10, follow 8**
([`../status/history/2026-10-23.md`](../status/history/2026-10-23.md),
"A writer that ends no block, reviewed"):
the forward pass walks a static block's codes itself, producing nothing, to
place a checkpoint at the first symbol boundary at or after each multiple, and
the resume primes a synthetic static-block header ahead of the byte's
remaining bits. It reads 8's native decoding first rather than writing a
second. A walker producing no output is not the inflater of our own the
backend's rejection names.

The split is a row only if `gz-seek` is functional by the wrap; otherwise it
is out-of-band work.

## Facts the grilling found

Gathered 2026-10-21 from the upstream checkouts `CLAUDE.local.md` names and
the crates' sources. Each `pg_dump` fact a decision comes
to depend on becomes an `I<n>` in [`postgres-invariants.md`](postgres-invariants.md)
in the slice that relies on it; until then it is evidence, stated here once.

**What `pg_dump` writes.**

- **`-Fp -Z` is one gzip member with no flush point, on every tested major.**
  Written through zlib's file API (`gzopen`/`gzdopen` + `gzwrite`; PG 13–15
  `pg_backup_archiver.c`'s `SetOutput`, PG 16+ `compress_gzip.c`'s `Gzip_open`
  and `Gzip_write`), never `deflate` directly. No `gzflush`, `Z_FULL_FLUSH`,
  `Z_SYNC_FLUSH` or `gzsetparams` anywhere in `src/bin/pg_dump/` on 13 through
  master; `gzwrite` compresses with `Z_NO_FLUSH` and `Z_FINISH` at `gzclose`. So
  there is no byte-aligned restart point and no window reset in the file.
- **Its header carries nothing**: flags 0 (no `FNAME`, `FEXTRA`, `FCOMMENT`),
  `MTIME` 0, zlib's `deflateInit2(…, MAX_WBITS + 16, …)` with no `gzhead`. Not
  checked: whether zlib-ng-compat writes identical bytes.
- **`pg_dump | gzip` is the same shape** — GNU gzip writes one member with no
  flushes — so the two producers a user is likely to hand us agree.
- **Directory format** writes each `.dat.gz` as one member of the same shape;
  **custom format** compresses each entry as a **zlib-wrapped** stream (2-byte
  header, Adler-32 trailer), neither raw deflate nor gzip, cut into
  length-prefixed chunks that are buffer-sized rather than flush points. Both
  are P8's.

**What a checkpoint is.** zlib's `examples/zran.c` defines it: the uncompressed
offset, the first whole compressed byte, 0–7 bits of the byte before, and up to
32 KiB of window (`zran.h`'s `point`). One can be taken at the start of **any**
deflate block, found only by decoding with `Z_BLOCK`; zlib ends a block about
every 16 K symbols at the default `memLevel`, so the choice of spacing is the
reader's, not the writer's — except at level 1 under zlib-ng's default build
and zlib-rs, whose `deflate_quick` writes one static block to the stream's
last write, `pg_dump` flushing nothing before its `Z_FINISH`
(`runs/p15-level1-writers/reading.txt`; Fedora 44's zlib-ng is built without
it). The window is the cost: a full 32 KiB a checkpoint
is about 24 GB over koji's 784 GB at 1 MiB spacing and about 0.77 GB at 32 MiB
(arithmetic, not a measurement). rapidgzip shrinks it with sparse windows. A
single member's uncompressed size above 4 GiB is learnable only by a full
decode: `ISIZE` is mod 2³² (RFC 1952, §2.3.1).

**BGZF.** Every block is a gzip member carrying `FEXTRA` subfield `BC`
(`BSIZE` = the block's total size − 1), `ISIZE` ≤ 65 536, and a fixed 28-byte
empty block marks the end. A `.gzi` is a little-endian `u64` count and that
many (compressed, uncompressed) `u64` pairs, the first block implicit. Without
one, the exact size is a walk over every block's header and trailer — the same
seek-bound shape as the multistream `.xz` footer walk.

**The crates.**

- **Resuming inflate at a bit offset with a primed window**: `miniz_oxide`'s
  `block-boundary` feature (in 0.9.1; present by 0.8.9) is the only safe-Rust
  API — `TINFL_FLAG_STOP_ON_BLOCK_BOUNDARY`, a serde-able `BlockBoundaryState`
  and `from_block_boundary_state`. `zlib-rs` has `prime` and `set_dictionary`
  only under its unstable `__internal-api` feature; `libz-rs-sys` exports the C
  ABI (`inflatePrime`, `inflateSetDictionary`, `Z_BLOCK`) as `unsafe extern`.
  `flate2` exposes no `Z_BLOCK`, `data_type` or prime, so it cannot build an
  index; `libdeflater` is whole-buffer only. zran's `NOPRIME` trick primes the
  bits without `inflatePrime`, on any raw inflater that takes a dictionary.
- **Indexed gzip**: no mature pure-Rust zran or `indexed_gzip` port exists.
  `rapidgzip` 1.2.1 (MIT OR Apache-2.0) binds the C++ engine — CMake, C++17,
  nasm — and its parallel path wants a cloneable reader; its own technique
  (speculative block finding, decode against a marker window resolved once the
  predecessor's is known) builds a seek-point index on the first pass and has
  no pure-Rust port and no C ABI.
- **The `rapidgzip` binding, evaluated** (a third party's,
  `alekseizarubin/rapidgzip-rs`; run and sources in the 2026-10-21 history
  entry): it decodes correctly, BGZF and concatenated members included, and N
  readers each at parallelism 1 over an imported index decode independently
  on their callers' threads, over any `Read + Seek`. But an imported index
  holds every 32 KiB window resident in each reader; the index carries no
  CRC, a member's check being compared only on a contiguous read from its
  start, so a sub-range read over a corrupted trailer succeeds; a file
  truncated mid-member and a wrong `ISIZE` read as `Ok`; checkpoints fall by
  compressed offset at a chunk size the constructor shrinks with parallelism
  on a small file, so the index differs by thread count there, and exporting
  forces the rest of the pass, so an index is never partial; its chunk cache
  is sized by the parallelism, not configurable, and allocated by rpmalloc
  and the C++ runtime; and the vendored engine's GCC IPO objects leave its
  symbols undefined under a release-profile link.
- **BGZF**: `noodles-bgzf` has a `.gzi` type but its frame parsing and decode
  are `pub(crate)`, so it drives only a `Read + Seek`; `bgzip` 0.3.1 has
  `decompress_block` over a caller's compressed bytes, close to sans-IO.
- **Parallel decode without an index**: `gzp` and `lgz` parallelize only
  block-structured or full-flushed streams (BGZF, pigz), and fall back to one
  thread on exactly what `pg_dump` writes.
