# P14 inbox — facts filed for its grilling

Evidence that P14 (remote input) will need. **This is a queue, not a
document**: when P14 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## The trait gains `stored_size()`, and identity is deliberately the cheap number

**Fact.** P13 settles that `ByteRangeSource` grows `stored_size()` — the bytes
as stored, as against `size()`, which is what `read_range` can address — and
that the cache's `SourceIdentity` records `stored_size` plus `modified()`
rather than `size()`. For a plain local file the two are equal; they diverge
for a decompressing source. P13 also makes the trait **dyn-compatible**
(`Pin<Box<dyn Future>>` returns, `Arc<dyn ByteRangeSource>` at the call sites),
specifically so that local, remote and decompressing-over-either do not become
a combinatorial branch at every caller.

**Why P14 cares.** Both are decisions about the trait this phase implements a
new backend for, and the second was made *for* this phase's benefit. An
`object_store` source answers both sizes with the same `head`, so it inherits
the default `stored_size() == size()` and owes nothing extra — but a remote
`.xz` composes the two, and that composition is the case D3 exists to keep
cheap.

**Origin.** 2026-09-02, grilling P13.

---

## A footer walk over a remote `.xz` is one ranged GET per stream

**Fact.** Building an xz seek table means reading each stream's footer, and the
koji upstream download is **31,150 concatenated streams**. Locally that walk is
85 s of HDD seeks; over ranged GETs it is 31,150 round trips. P13's answer is to
persist the table in the `.dqcache` so only a cold source ever pays, and to
require of the external `xz-seek` crate that a reader can be constructed *from*
a previously obtained table with no walk at all.

**Why P14 cares.** It is this phase's worst latency case and it arrives the
moment remote and compressed compose. It also sharpens a fact this phase depends
on independently: with a complete structural cache, a query reduces to the cache
read, `stored_size()`/`modified()` for the identity check, and the replay read of
the target block — so the round-trip count for a warm remote query is small and
countable, and that is the number this phase should be designed against.

**Origin.** 2026-09-02, grilling P13.

---

## The seekable-xz crate reads its compressed bytes through a trait, on purpose

**Fact.** `/mnt/wd12t/fedora/experiments/xz-seek/docs/design/historical/initial.md`
requires (R2) that the crate never open files, taking its compressed input
through a caller-supplied positioned-read trait instead — explicitly so that the
same crate serves a local file, an `mmap`, an in-memory buffer, and this phase's
ranged GETs.

**Why P14 cares.** It means "remote compressed input" needs no new work in that
crate: the composition is P13's `XzSource` wrapping this phase's
`object_store` source. If that requirement is dropped or the crate ships owning
its I/O, this phase inherits the problem.

**Origin.** 2026-09-02, grilling P13.

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
