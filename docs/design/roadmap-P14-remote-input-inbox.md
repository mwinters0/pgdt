# P14 inbox — facts filed for its grilling

Evidence that P14 (remote input) will need. **This is a queue, not a
document**: when P14 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## The trait gains `stored_size()`, and identity is deliberately the cheap number

**Fact.** `ByteRangeSource` carries `stored_size()` — the bytes as stored, as
against `size()`, which is what `read_range` can address — and the cache's
`SourceIdentity` records `stored_size` plus `modified()` rather than `size()`.
For a plain local file the two are equal; they diverge for a decompressing one.
The trait is also **dyn-compatible** (`Pin<Box<dyn Future>>` returns,
`Arc<dyn ByteRangeSource>` where a source is constructed rather than borrowed),
specifically so that local, remote and decompressing-over-either do not become a
combinatorial branch at every caller. Both are built
([`architecture.md`](architecture.md), "Execution model and API surface").

**Why P14 cares.** Both are properties of the trait this phase implements a new
backend for, and the second was decided *for* this phase's benefit. An
`object_store` source answers both sizes with the same `head`, so it inherits
the default `stored_size() == size()` and owes nothing extra — but a remote
`.xz` composes the two, and that composition is the case dyn-compatibility
exists to keep cheap. `SourceIdentity` is an opaque enum for the same reason:
this phase adds a `Remote { etag }` variant and the existing match sites gain an
arm, rather than the type being redesigned around a source with no mtime.

**Origin.** 2026-09-02, grilling the compressed-input work.

---

## A cache that describes another file is now refused, and the refusal's payload is size-shaped

**Fact.** A cache whose recorded stored size is not this source's is no longer
scanned over: the three scan entry points raise
`Error::CacheSourceMismatch { path, cached_stored_size, live_stored_size }`
before a byte of the dump is read, and there is no override
([`architecture.md`](architecture.md), "The cache"). Both the `CacheLoad`
variant behind it and the error itself name **two `u64` sizes**, because that is
what `SourceIdentity::LocalFile` compares.

**Why P14 cares.** This phase adds `SourceIdentity::Remote { etag }`, which has
no two sizes to quote — an ETag mismatch is a mismatch with nothing numeric in
it — so the arm this phase adds to the identity enum does *not* fall out at the
refusal the way the entry above says it does at the match sites. The choice is
this phase's: generalise the payload to something an ETag can fill, or give the
remote identity its own refusal beside this one. Deciding it late means deciding
it inside a message a user reads.

Related and already owned elsewhere: an `.xz` size mismatch pays its footer walk
*before* the refusal (`M62`), and over ranged GETs that walk is 31,150 round
trips on the motivating file — the next entry is where that cost is stated.

**Origin.** 2026-09-06, wrapping the cache-replacement work
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "P17 is
wrapped").

---

## A footer walk over a remote `.xz` is one ranged GET per stream

**Fact.** Building an xz seek table means reading each stream's footer, and the
koji upstream download is **31,150 concatenated streams**. Locally that walk is
85 s of HDD seeks; over ranged GETs it is 31,150 round trips. The **warm** half
is built and no longer this phase's to do: a cache's persisted table is read
back before any source exists and handed to recognition, which builds the source
from it and walks nothing ([`architecture.md`](architecture.md), "The compressed
source"). What remains is the **cold** case — the first command against a remote
`.xz`, where there is no cache to read a table out of and the walk is 31,150
round trips before a byte is served.

Two details of the warm path are shaped for a local file and are exactly what
this phase has to replace. The entry point that loads the table
(`cache::claim`) checks the envelope's identity against a plain
**`stat` on the dump path**, which a remote source does not have — the same
no-mtime problem the phase already owns for `SourceIdentity`, arriving one layer
earlier and before any source exists to ask. That check now decides a *refusal*
as well as the table: a cache recorded against a file of another stored size is
answered there, so all three commands report it without opening anything. A
remote source with no cheap identity to read loses both at once — the spared
walk and the free refusal — which is why whatever replaces the `stat` has to be
answerable before a source exists, not merely from one. And recognition sniffs the file's
magic with a small read from the **path**, not through a `ByteRangeSource`, so a
remote source needs that first read to be a ranged GET or needs recognition to
be told what it holds.

**Why P14 cares.** The cold case is this phase's worst latency case and it
arrives the moment remote and compressed compose. Everything else in a warm
remote query is a cache read, an identity check and one block's bytes — which is
the number this phase should be designed against — so what stands between the
design and that number is now the cold walk alone, plus the two local-file
assumptions above, which are on the warm path and must not be inherited
unnoticed.

**Origin.** 2026-09-02, grilling the compressed-input work; the half-built state
found by its wrap audit and closed on 2026-09-06 by the seek-table readback
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "The seek
table is read back, and a claim the file contradicts condemns the cache"), and
the refusal put on that same check the same day
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "`M62`:
the cache path settles the size mismatch before anything is opened").

---

## The seekable-xz crate reads its compressed bytes through a trait, on purpose

**Fact.** `/mnt/wd12t/fedora/experiments/xz-seek/docs/design/historical/initial.md`
requires (R2) that the crate never open files, taking its compressed input
through a caller-supplied positioned-read trait instead — explicitly so that the
same crate serves a local file, an `mmap`, an in-memory buffer, and this phase's
ranged GETs.

**Why P14 cares.** It means "remote compressed input" needs no new work in that
crate: the composition is the existing `XzSource` wrapping this phase's
`object_store` source. If that requirement is dropped or the crate ships owning
its I/O, this phase inherits the problem.

**Origin.** 2026-09-02, grilling the compressed-input work.

**Contingent on** the crate honouring R2; re-check the requirements file.

---

## The 1 MiB read chunk and the refusal of double-buffering are both facts about local disks

**Fact.** `scan::DEFAULT_CHUNK_SIZE` is 1 MiB, chosen by measurement over six
sizes from 64 KiB to 16 MiB on a SATA SSD, an NVMe drive and tmpfs
([`measurements.md`](measurements.md), "What the read chunk size is worth"). Two
things travel with it. `io::BufferPool` **keeps nothing above 8 MiB unless a
read loop announced that length** through `ByteRangeSource::hint_read_size`, so
an unannounced read above the ceiling is a fresh zeroed allocation every time —
worth **1.83× a warm scan** at 16 MiB, back when a chunk was one. And
double-buffered readahead was refused on the arithmetic that a cold scan cannot
go below the device's own delivery time, which on the fastest local disk we own
leaves a 5.8% envelope for the whole overlap idea
([`architecture.md`](architecture.md), "Execution model and API surface").

**Why P14 cares.** Both premises fail over a network. A ranged GET has latency
a local `pread` does not, so overlapping the next request with the current
parse is worth something here even though it was refused there — the refusal is
a local-disk result and must not be read as a decision about the trait. And a
1 MiB range is almost certainly too small for `object_store`, where per-request
overhead dominates; the first size that looks right will be several MiB. A
local-file read loop keeps its pooling at that size because it announces it,
but a P14 source that recycles buffers of its own has to implement
`hint_read_size` to get the same — a defaulted trait method does nothing, and
doing nothing is exactly the silent pooling loss the ceiling used to produce.

**Origin.** 2026-09-04, the I/O-defaults reading; the announced read length is
from 2026-09-05
([`../status/history/2026-09-05.md`](../status/history/2026-09-05.md), "`M55`:
the pool keeps the chunk size the caller asked for").

---

## The fetch policy over a remote source is this phase's to own, and the crate ships no knob for it

**Fact.** `xz-seek`'s parallel path splits fetching from decoding — a fetch
stage, a queue of compressed buffers, N decoders — with **one fetcher, no
coalescing and no fetcher-count parameter**. This project asked for the
parameter on this phase's behalf and then **withdrew it**, on two grounds: a
fetcher inside a synchronous crate reaches an async object store only by
blocking on a future from a blocking thread, so a count multiplies parked
threads rather than fixing them; and the measurement that motivated coalescing
turned out to be one thread issuing *block-sized* reads in file order, already
at the HDD's ceiling, so coalescing has nothing measured to buy locally.

What replaces the knob is **a public `CompressedSource` over a pre-fetched
window** — a base offset plus bytes, generic over its backing store so a
`Bytes` or an `Arc<[u8]>` goes in without a copy, reporting the whole file's
size and erroring rather than short-reading outside its own range. Two
compositions fall out of it, and choosing between them is this phase's:

- **Take the fetch entirely.** Async, concurrent, coalesced ranged GETs on the
  runtime this project already has, with `xz-seek` reduced to a CPU-side decoder
  over buffers handed to it. No blocked threads at all.
- **Keep the pool, and prefetch underneath it.** This phase's own
  `CompressedSource` impl issues concurrent GETs ahead of the fetcher's cursor
  and answers `read_at` out of what has landed. `xz-seek` was asked to
  **guarantee that its single fetcher requests blocks in ascending order within
  a range**, which is what makes that anticipation possible; if that promise is
  absent when this phase is grilled, this composition is not available and the
  first one is the only one.

The measured tension behind the original request is still worth knowing: on the
HDD one sequential fetcher delivers 249.6 MB/s of compressed input against 190.8
at eight concurrent. That is an argument for *fetch concurrency = 1 on a
spindle*, and this phase's device is the one where the number goes the other
way — which is now expressed by owning the policy rather than by a parameter.

**Why this phase cares.** It is the difference between remote compressed input
being a composition that already works and one that is capped in a dependency,
and the answer moved: nothing is capped, but nothing is provided either. The
concurrency is this phase's to write.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling, rounds one and two
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to"). Contingent on that
crate's `P3` landing as specified.

---

## The sync/async seam bites here and nowhere else, and the window type is the escape

**Fact.** `xz-seek` is synchronous and intends to stay so — an async API there
would put an executor in the dependency graph of a crate whose distinguishing
claim is a four-crate unsafe-free build. That costs this project nothing today,
because every byte it reads already crosses a `spawn_blocking` boundary
(`io::LocalFileSource::read_range` *is* a blocking `read_exact_at` on a blocking
thread) and the library depends on `tokio` with `rt` + `sync` only, leaving
`rt-multi-thread` to the binary.

It costs **this phase** something specific: `CompressedSource::read_at` is
synchronous and an `object_store` backend is async, so composing them means
blocking on a future from inside a blocking thread — **one parked OS thread per
in-flight fetch**. Workable, not good, and it is the reason the entry above
withdrew the fetcher-count parameter: a count multiplies parked threads instead
of removing them.

The escape is the pre-fetched window: with it, this phase issues its own async,
concurrent, coalesced ranged GETs on the runtime it already has, and hands
`xz-seek` filled buffers. That composition has no blocked threads in it.

**Why this phase cares.** It is the one place in the whole compressed-plus-remote
composition where the layering does not simply fall out, and the mitigation has
to be asked for in `xz-seek`'s `P3` rather than discovered here. Whether to take
the fetch back is a decision this phase makes, not one the crate makes for it.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to").

---

## `Window`'s bound, its constructor shape, and footer-walk composition are answered from this side

**Fact.** `xz-seek`'s `P3.3` landed `Window<B>`, asked three questions about it
from this project's chair, and got these answers:

- **`impl<B: AsRef<[u8]> + Send + Sync> CompressedSource for Window<B>`'s
  `+ Send + Sync` buys nothing and should come off.** Every backing this
  project will hand it — `bytes::Bytes` out of an `object_store` GET, `Arc<[u8]>`
  or `Arc<Vec<u8>>` where one coalesced fetch backs several block decodes, a
  pooled `Vec<u8>` — is already `Send + Sync`, and `Window<B>`'s own auto-trait
  status follows from `B`'s regardless of what the impl's `where`-clause says.
  Nothing here holds compressed bytes in a non-`Sync` handle (an `Rc`); every
  path in this project crosses `spawn_blocking` or a `tokio` task, so there is
  no single-threaded stage that would want one.
- **`Window::new(base, file_size, bytes)`'s swappable `u64`s are left alone.**
  The mitigation this project owes is on its own side of the boundary: the
  fetch stage should hold `file_size` once (a `head()`/content-length, cached)
  as fixed state rather than threading it positionally through each `Window`
  construction in a loop — that discipline, not a type change in `xz-seek`,
  is what actually removes the swap risk. Precedent: `stored_size()`/`size()`
  ([`architecture.md`](architecture.md), "The cache") is the same shape of adjacent-`u64`
  risk, and this project's answer there was legibility (both surface at the
  accessor and in error text) over a type-level guard.
- **Composing a tail window with an offset-0 window for a remote footer walk
  stays out of `xz-seek`.** Confirmed this is wanted: a cold `.xz` seen for
  the first time remotely is exactly the case
  ["A footer walk over a remote `.xz` is one ranged GET per
  stream"](#a-footer-walk-over-a-remote-xz-is-one-ranged-get-per-stream) above
  worries about, and the single-block-per-stream shape needs the six magic
  bytes at offset 0 *and* the tail. But merging windows needs a policy for
  overlaps and gaps that only this phase can choose, which is the same reason
  the fetcher-count knob was withdrawn above — this project owns fetch policy,
  the crate ships primitives. A composing `CompressedSource` over several
  windows is this phase's own ~10 lines against the public trait.

**Why P14 cares.** These are exactly the three unresolved questions the crate's
`Window` type carried into its own grilling (`xz-seek`'s `STATUS.md`, the
`Send`/`Sync` entry was open); P14 is the type's only planned consumer, so the
answers are effectively P14 design decisions arrived at early, not just
feedback to another crate.

**Origin.** 2026-09-06, `xz-seek` `P3.3` grilling, cross-session exchange with
the `xz-seek-1d` session.
