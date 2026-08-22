# Scan Performance

High performance on local files is a core project goal, not a later
optimization pass — see "Project goals" in `docs/design/roadmap.md`. The
target is that the local-file path stays **device-bound rather than
CPU-bound**, at flat memory, across hardware from rotational disk to NVMe.

This doc is the design sketch for the concentrated work in roadmap Phase 5,
plus the small number of constraints it places on work happening *now*. It is
a plan, not a description of implemented behaviour; nothing here is built.

## Baseline

From the koji sample (784GB, HDD, `pgdq info --verbose`): **243 MB/s sustained
at ~33% of one core**, RSS flat at ~9 MiB. That is device-bound, not
parse-bound — the HDD is the limit and the current scanner has roughly
3× headroom over it, implying a single-core parse ceiling near **700 MB/s**.

That number is the thing to beat, and it also sets the priority: on rotational
media there is nothing to win, so every technique below has to justify itself
on NVMe or on page-cache-warm data, where the device stops being the excuse.

## Two workloads, two algorithms

Conflating these is the main way to get this wrong.

**Structure discovery** (`pgdq parse`, cache building) reads the whole file to
find `COPY ... FROM stdin;` headers and `\.` terminators. Essentially all of
those bytes are row data we do not care about. The goal is to *skip* as fast
as possible.

**Row extraction** (answering a query against a known block) touches every
byte of the rows it returns and must produce Arrow arrays. The goal is to
minimize work *per byte*, not to skip.

## Structure discovery: search for the terminator, don't enumerate lines

The natural sketch — SIMD-scan for every `\n`, record the offsets, then look
around each one — does more work than the problem needs, in two ways.

**Don't store the offsets.** koji has 19,575,829,920 rows. Eight-byte offsets
for all of them is ~157 GB of index for a 784GB file: a fifth of the input
written back out, and more write bandwidth than the read itself. The right cache
granularity is per-block (what `DumpIndex` already stores) plus a **sparse** row
index — see below.

**Don't enumerate the newlines either.** Inside a `COPY` block the only thing
that ends the block is a line containing exactly `\.`, and that line is
*unambiguous*, which is what makes a direct substring search safe:

> A field whose value contains a backslash is emitted with the backslash
> doubled, so a data row whose content is `\.` appears in the file as `\\.`.
> The byte sequence `LF 5C 2E` (newline, backslash, dot) therefore never
> occurs at the start of a data line — in `LF 5C 5C 2E` the dot is not
> preceded directly by newline-backslash. Within a `COPY` block, a match is
> always the real terminator.

So the inside-block path becomes a single `memchr::memmem` search for the
3-byte needle `LF \ .`, followed by a check that a line terminator (LF, CRLF,
or EOF) follows. Anchoring on LF rather than CR handles CRLF files with one
search instead of two. `memmem` is already SIMD with a rare-byte prefilter;
this is an order of magnitude faster than the device on any hardware we'd read
from, which is exactly the point — discovery should cost effectively nothing
beyond the read.

**Leave the outside-block path alone.** Between blocks the bytes are DDL:
a few megabytes out of 784GB. Optimizing it would be invisible, and keeping it
line-oriented preserves the option of adding dollar-quote tracking to close
the known gap in `docs/design/pg-dump-compatibility.md` — which a
needle-skipping scan would make awkward, since it deliberately never looks at
the bytes it jumps over.

## Row extraction

**Raw LF is an unambiguous row terminator.** COPY TEXT escapes an embedded
newline as the two characters `\n`; a raw `0x0A` byte never appears inside a
field. This is the load-bearing property for everything below — it means row
boundaries are findable with a single `memchr`, and that *any* byte offset
inside a block can be resynchronized to a row boundary by scanning forward to
the next LF.

**Zero-copy the common field.** Most fields contain no backslash at all. Per
row, `memchr::memchr2(b'\t', b'\\')`: if a field has no backslash, it is a
direct slice of the read buffer — no allocation, no unescape pass. Only
escaped fields take the decode path, into a separate owned buffer.

**Build `Utf8View` over the read buffer.** For that slice to stay zero-copy
all the way into Arrow, the chunk must be read into an Arrow `Buffer` and the
views must reference it (arrow-rs exposes block-based view construction on
`StringViewBuilder` — confirm the exact API shape when implementing). Two
consequences to design around:

- A batch then pins its whole source chunk in memory. Harmless at full
  selectivity; wasteful once Phase 3 pushdown filters aggressively. Compact
  the view array when the selected fraction drops below a threshold.
- Fields ≤12 bytes are stored inline in the view and don't reference the
  buffer at all, so short-column tables get this for free either way.

**Validate UTF-8 in bulk, once.** Per-field `std::str::from_utf8` is ~1-2
GB/s; `simdutf8` validates at roughly an order of magnitude more. Validate the
largest whole-row prefix of the chunk in one call, then use unchecked
conversion for the zero-copy fields. This is sound because field and row
delimiters are ASCII (`0x09`, `0x0A`) and ASCII bytes never occur inside a
multi-byte UTF-8 sequence — so splitting a validated buffer on them always
yields valid pieces. Fields taking the *decode* path still need validation,
since `\xNN` and octal escapes can synthesize invalid sequences.

## I/O: keep `pread`, don't mmap

mmap is the intuitive choice here and it is the wrong default.

- It bypasses `ByteRangeSource`, so it cannot be the path for Phase 4's
  `object_store` backend — adopting it means maintaining two readers.
- Page faults on a 784GB file on slow media are synchronous and
  uninterruptible, with no way to bound prefetch depth or time out.
- I/O errors arrive as `SIGBUS`, not `Result`. For a tool whose whole premise
  is "read a file bigger than memory," turning read failures into signals is
  a bad trade.

The current design — positioned reads into a reusable caller-owned buffer —
already delivers the flat ~9 MiB RSS and 243 MB/s, behind an abstraction that
survives into Phase 4. The wins available are within it:

- **Double-buffered readahead**: issue the next chunk's read while parsing the
  current one, so parse time and I/O time overlap instead of alternating.
  Cheap, and the single most likely reason the current scan sits at 33% CPU.
- **`posix_fadvise(SEQUENTIAL | WILLNEED)`** on the local backend, to let the
  kernel read ahead deeply on the sequential scan.
- **Tuned chunk size**, page-size-aligned: large (1-4 MiB) on rotational media
  to amortize seeks and syscalls, smaller on NVMe. `ScanOptions::chunk_size`
  already exposes this; the work is measuring the right defaults per device
  class rather than adding a knob.

mmap stays available as a possible local-only fast path, gated on a
measurement showing it beats double-buffered `pread` by enough to justify a
second code path. Not assumed.

## Parallelism

Split by workload, and by device.

**Row extraction parallelizes trivially, once boundaries are known.** Given a
block's byte range, hand each worker a sub-range; each resyncs forward to the
next LF and stops after crossing its end boundary — the standard
split-anywhere pattern, made correct here by the raw-LF property above.
Batches are reassembled in range order to preserve row order. This is the
high-value case, because it is what a query actually runs.

**Discovery is harder and worth less.** From a cold start you cannot tell
whether a random offset is inside a block. A speculative scheme is possible —
workers scan from fixed offsets recording *candidate* header and terminator
positions, then a single linear pass over the ordered candidates decides which
are real, since a terminator can only close a block opened earlier. An
inconsistency (a terminator with no open block) means falling back to a
sequential rescan of that region. Worth prototyping, but it should be gated on
a measurement: discovery is a one-time per-file cost that the cache then makes
free, so parallelizing it competes with simply having run `pgdq parse` once.

**Be device-aware.** On the koji HDD the scan is already device-bound;
concurrent readers there would add seeks and make it slower. On NVMe, queue
depth is required to saturate the device at all. So parallelism must be
configurable with a conservative default, ideally informed by whether the
backing device is rotational (`/sys/block/<dev>/queue/rotational` on Linux)
rather than by core count, which is misleading on a CPU-heavy machine like
this one.

## Cache: a sparse row index

This is where the newline-offset idea belongs, at the right granularity.
Record the byte offset of every Nth row (N matching the default batch size,
8192). koji's completed scan puts a real number on it: 19,575,829,920 rows is
~2.4M checkpoints at ~19 MB, against ~157 GB for a dense 8-byte-per-row index.
It buys two things that matter:

- **Seek to a row range** without scanning the block from its start.
- **Parallel splits at known row boundaries**, removing even the resync scan.

Reserve room for it in the serialized `DumpIndex` from the start; it is
optional data, so a cache without it stays valid. The checkpoint interval also
defines the row-group boundary that per-column statistics attach to
(`docs/design/roadmap.md`, Phase 3 companion), so the two features share one
addressing scheme.

## Measurement discipline

Every item above is a hypothesis until measured, and the ordering of the work
should follow the measurements rather than this doc's ordering.

- **Separate I/O-bound from CPU-bound** by benchmarking from tmpfs or a warm
  page cache, not just from disk. A change that improves parse throughput is
  invisible in the koji number and will be dismissed wrongly.
- **Measure on fast media, with a dataset that fits on it.** koji lives on an
  HDD and cannot answer a CPU-cost question. `scripts/generate_perf_data.py`
  (introduced in Phase 2) generates a synthetic dump of any size onto the SSD
  or NVMe volume — ~100 GB is the target here — with stress sections for the
  paths whose cost is expected to move. Generated, never committed, and
  deliberately not reproducible: this measures throughput, not correctness.
- **`criterion` microbenchmarks** for the decoder and the needle search;
  **whole-file runs** for the end-to-end number. Both, not either.
- **Track bytes/second and CPU%**, not wall clock alone — 243 MB/s at 33% of a
  core and 243 MB/s at 95% mean opposite things about where to work next.
- Follow the long-running-process rules in `CLAUDE.md` for anything at koji
  scale.

## What this constrains before Phase 5

Three rules that earlier phases hold to, because retrofitting them is expensive
and pre-empting them is nearly free. The first two are already satisfied by
Phase 1 and must survive any change to those layers; the third is standing.

1. **The batch layer reads chunks into an Arrow `Buffer` and builds `Utf8View`
   arrays as views over it**, rather than copying field bytes into fresh
   allocations. This is the one item with a real cost to deferring. See the
   sharp edges around that path in
   `docs/design/roadmap-phase1-mvp-notes.md`.
2. **The serialized cache leaves room for the optional sparse row index**, so
   adding it later is not a format break — `CopyBlock::sparse_index`, reserved
   and always `None`.
3. **`ScanOptions::chunk_size`** stays tunable, and defaults are treated as
   measured values, not constants.
