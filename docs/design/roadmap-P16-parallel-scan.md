# P16 — Parallel scan and extraction

What this phase does and why, decided before any of it exists. How it landed is
the notes doc; what has landed is [`../status/STATUS.md`](../status/STATUS.md).

**This phase's inbox has been drained into this document and deleted**
(`../process.md`, "Inboxes: facts filed by destination"). Two of its entries
were discarded as stale rather than folded in, and both are named where the
decision that stranded them is written: the claim that a seekable compressed
source hands this phase parallel discovery *for free* — it hands over byte
ranges, not scanner state — and the claim that `KD5`'s rework and this phase
are one change, which was predicated on relaxing prefix-shaped coverage.

## The thesis: parallelism is what stops CPU becoming the compressed source's new floor

A `.xz` dump trades device bytes for CPU at roughly **19.4×** — koji is 784 GB
plain against 40,397,009,888 bytes compressed. Reading the compressed file
therefore offers the parser 19.4 bytes of plaintext for every byte the device
delivers, and the question the phase exists to answer is what stops that offer
from being taken.

Today the answer is *serial decode*. `io::XzSource` holds one `xz_seek::Reader`
behind a `std::sync::Mutex` (see [`architecture.md`](architecture.md), "The
compressed source"), so a compressed scan runs at one core's decode rate —
**~435 MB/s** of plaintext on koji's 15.70× bytes — whatever the device
underneath could have offered.

### What each stage costs, per thread

| Stage | Rate | Source |
|---|---|---|
| xz decode, one core | ~435 MB/s plaintext at 15.70× | [`measurements.md`](measurements.md), "What a second decode worker buys" |
| xz decode, 24 cores | ~3,388 MB/s plaintext at 15.70× | the same figure, koji leg's 24-worker row |
| Structure discovery (`parse`) | **7049 MB/s** | [`measurements.md`](measurements.md), "Scan throughput by input shape", warm `COPY` row |
| Extraction, `--schema-mode strings` | ~940 MB/s | [`measurements.md`](measurements.md) |
| Extraction, typed | ~680 MB/s | [`measurements.md`](measurements.md) |

And what each device offers, as a `dd` floor and as the plaintext that floor
implies at koji's ratio:

| Device | `dd` floor | × 19.4 = plaintext offered |
|---|---|---|
| HDD (koji) | ~240 MB/s | ~4,656 MB/s |
| SATA SSD | **561 MB/s** | ~10,883 MB/s |
| NVMe (970 EVO Plus) | **2602 MB/s** | ~50,479 MB/s |

### The two conclusions the table forces

**Parallel discovery is worth nothing on a plain file and everything on a
compressed one.** Discovery runs at 7049 MB/s, which is above every device this
project owns, so a plain-file `parse` is device-bound on all of them — cold on
the NVMe it is **1.06×** the `dd` floor, and the entire prize for parallelising
it is the **0.076 s** by which a 1.314 s scan exceeds that floor, before a
splitter's own coordination comes out of it. On a compressed file the same scan
is bounded by ~435 MB/s of decode, which is *below* every device's offer, so
every worker added converts into throughput — but at a falling rate, and the
device is never reached. Twenty-four decode workers on koji's bytes reach
~3,388 MB/s against the HDD's implied ~4,656 MB/s, and scaling has flattened
long before that: 7.78× one worker at twenty-four, where the twenty-fourth buys
essentially nothing over the sixteenth. Decode stays the bound on a compressed
file at every worker count this machine can offer, which is a stronger
statement of the case for parallelising it than the linear reading was.

**Parallel extraction is worth something everywhere, plain input included.**
Extraction runs at 680–940 MB/s, which is *below* the SATA SSD's floor and far
below the NVMe's. So a typed `query` is CPU-bound on every device we own, on
plain and compressed input alike, and it is where cores pay regardless of the
source.

### The fused worker is self-balancing, and that is why it is the shape

A worker that decodes *and* parses its own range spends its time in whatever
proportion the two stages demand, with nobody computing the ratio. Its
throughput is the harmonic combination of the two rates:

| Work | Fused rate per worker | Decode's share of the worker |
|---|---|---|
| `parse` (discovery) | **~419 MB/s** | 94% |
| `query --schema-mode strings` | **~302 MB/s** | 74% |
| `query`, typed | **~269 MB/s** | 60% |

The ratio a split decode-pool/parse-pool arrangement would have to be tuned to
is **~16 decode workers per discovery thread** and **~1.5 per typed-extraction
thread** — an order of magnitude apart, and a function of the command rather
than of the machine. The fused shape gets both right for free.

What that buys, at 12 physical cores / 24 threads:

- **HDD**, `parse` on `.xz`: ~12 fused workers reach the device's 4,656 MB/s
  offer. Against a plain-file HDD scan's ~240 MB/s that is the full **19.4×**.
- **HDD**, typed `query` on `.xz`: ~17 workers reach the same offer — again
  **19.4×** over the plain path, which is device-bound at 240 MB/s.
- **SATA SSD**: CPU-bound at roughly 24 × 419 ≈ **10.1 GB/s** for `parse`,
  which is within a few percent of the 10.9 GB/s the device offers.
- **NVMe**: CPU-bound well below the device's 50 GB/s offer, at roughly
  **10 GB/s** for `parse` against the plain path's 2451 MB/s, and roughly
  **6.5 GB/s** for a typed query against the plain path's CPU-bound 680 MB/s.

These are arithmetic, not readings. The decode rate they rest on is now a
registered figure — [`measurements.md`](measurements.md), "What a second decode
worker buys" — and it says the scaling is **sublinear past four workers**, so
every projection above that multiplies a one-core rate by a worker count is a
floor on what will be needed rather than an estimate of what will suffice. The
remaining probe is `parallel-scan-throughput`, which 16.13 takes.

## Settled decisions

### P16 runs next, and the compressed path is its headline

The alternative considered and declined was running **P15 (gzip)** first, on
the grounds that `pg_dump -Fp -Z9` writes a file this build cannot read at all
and has done for the life of the project — a compatibility gap against a
throughput gain. It is declined because the vendored `xz-seek` copy now carries
parallel block decode whole (`roadmap.md`'s out-of-band ledger, `M64`) and
nothing here reads it: exercising that interface is what vets it, and the
serialization point it removes is real and measured. `KD5` is also live, owned
by this phase, and unowned by anyone else.

### Parallelism is a caller-controlled knob, and it can be turned off entirely

An embedded or otherwise core-starved consumer must be able to ask for the
single-threaded path, and get it — not a pool of one, but the serial code path
this build has today. So the parallel arrangement is selectable by the caller
and a fully-serial mode is a supported configuration rather than a degenerate
case of the parallel one.

### What this phase parallelizes is what is CPU-bound

Not "the compressed path" and not "the scan" — the rule is one line, and the
three cases fall out of it rather than being enumerated:

- **Decode: always.** It is ~435 MB/s on one core against every device's offer,
  and still below the HDD's at twenty-four.
- **Extraction: always, plain input included.** It is 680–940 MB/s against a
  561 MB/s SATA floor and a 2602 MB/s NVMe floor, so a typed `query` is
  CPU-bound on every device this project owns whatever the source is.
- **Discovery: only with a decoder in front of it.** At 7049 MB/s it is above
  every device, so on a plain file it is device-bound and there is nothing to
  win.

**Parallel plain-file discovery is therefore worth nothing at the default, and
that is where the refusal lives.** The evidence is the cold-NVMe figure — 1.06×
the `dd` floor, a 0.076 s prize on a 1.314 s scan, against which a splitter's
own coordination is charged.

What the evidence does *not* license is a branch in the library refusing to cut
a plain interior. The leader is built for the compressed case regardless, and
once it exists, running it over a plain interior is the same `scan_piece` and
`merge` with cuts that happen to be device reads — so a refusal would be a
second knob beside `Parallelism`, disagreeing with it, and would put the plain
path beyond the reach of `16.13`'s own figure. The economics are the caller's
and are already spelled: `--jobs`, which `16.16` defaults to 1. A user who
states `--jobs 8` on a plain `parse` buys coordination for ~6% and is entitled
to; the shipped default is the refusal, stated as a number rather than as a
branch. This amends an earlier reading of this section — the reasoning is
[2026-09-07](../status/history/2026-09-07.md), "What `partitions()` answers, and
where the plain-file refusal actually lives".

**The speculative-split scheme is refused on its own grounds** and does not
depend on this: the leader makes speculation unnecessary rather than cheap, so
there is no validation path and no rollback ("A serial leader opens each region"
below).

### A serial leader opens each region; workers never guess

A seekable compressed source hands this phase **byte ranges**, not **scanner
state**: an xz block boundary lands mid-row and mid-statement exactly as a
speculative split does, so "the index gives every block's offset before a byte
is decoded" does not by itself say whether a worker starts inside a `COPY`
block. Two arrangements answer that, and the phase builds both because they
serve different states of the cache:

**Cold — the leader opens the region.** One thread scans from the frontier
keeping exact scanner state. On reading `COPY … FROM stdin;` it knows every
byte until `\.` is line-structured rows, and hands that interior out in
LF-split ranges. Workers decode their own blocks, count rows, census arrays and
decode fields; they never parse *structure*, because the leader has already
proved there is none to find inside the region. The leader is serial only
across the preamble and inter-block DDL, which is bounded by schema size rather
than file size (`roadmap.md`, "Project goals") — koji is 74 blocks over 784 GB.

**Cached — partitioned replay.** With a complete map a block's extent is
already known, so `query` splits `[header_offset, end_offset)` with no state to
establish at all. This is where the extraction win lands for a file that has
been parsed once, it is the same code for a plain source and a compressed one,
and it is a deliverable in its own right rather than a special case of the
cold path.

*Rejected:* **speculative splitting** — a worker guessing it is inside a `COPY`
block, resyncing at the next LF, and being validated when the serial prefix
catches up. The leader makes speculation unnecessary rather than cheap: a
worker is never wrong, so there is no validation path, no rollback, and no
window in which a bad guess sits in the map. What it costs is a dump of many
small blocks, where the leader crosses a boundary per block and parallelism
collapses to serial — that shape is the 4000-block fixture, and being serial on
1.9 MB is not a cost.

### A worker decodes and parses in one thread

The composition over a compressed source is one worker per block that decodes
*and* parses, so a decoded block never crosses a channel and is dropped as soon
as its rows are batched. `xz-seek`'s **pieces** are the interface —
`Reader::block_task`, `BlockTask::decode_into`, `SeekTable::blocks_in` — and
its `Bulk`/`Reader::read_range` **pool is not used here**.

The reason is that a split decode-pool/parse-pool arrangement has a ratio to be
tuned to, and the right ratio is a property of the *command*: **~16 decode
workers per discovery thread** against **~1.5 per typed-extraction thread**, an
order of magnitude apart. No caller can set that, and a fused worker lands on
both without anyone computing it.

Two costs are accepted with it. This phase owns block scheduling on top of the
row scheduling it already owns — the pool would have been free. And
`BlockTask::decode_into` fills a caller-supplied buffer with a block's **entire**
uncompressed output, so a worker's retained unit is 24 MiB, or 128 MiB on a
large-block file, where the read path's is 1 MiB.

### Memory is bounded by a block pool with backpressure, and zero-copy survives

A worker decodes into a slot taken from a pool of **block-sized** buffers, and
blocks until one is free; the slot returns when the last batch viewing it
drops. The bound is therefore *slots* × (decoded block + dictionary), stated
exactly rather than dependent on how fast a consumer drains, and the zero-copy
`Utf8View` path is preserved for compressed sources as it is for plain ones.

Two things follow and are this phase's to do. `RowBatcher`'s `max_source_span`
cap is **re-derived against block-shaped pinning** — its 64 MiB was chosen on
the arithmetic that pinned bytes never exceed the span rounded out to *chunk*
boundaries, and a 64 MiB cap cannot round out to one 128 MiB block at all. And
`io::BufferPool` is reworked off its four fixed slots behind one mutex, which N
concurrent readers turn into shared state on the hot path — a miss costs
**1.83×** a warm scan, priced when a 16 MiB chunk was one (0.866 s against
0.472 s).

*Rejected:* **copying field bytes on the compressed extraction path**, so that a
batch owns its bytes and the block is freed the moment its worker is done. It
bounds memory just as exactly and costs roughly +7% at extraction's 680 MB/s,
but it gives up the zero-copy view for an entire source class — the fifth of
`roadmap.md`'s "Four decisions that keep later phases additive" — to solve a
bounding problem that backpressure solves without giving up anything.

**Per-worker footprint on koji's multistream `.xz`**: 8 MiB LZMA2 dictionary
(`xz -6`, read from the file header, and excluded from `xz-seek`'s own
`footprint()` by decision) + ~24 MiB decoded block + ~3 MiB compressed window
and input chunk ≈ **35 MiB**. A 512 MB cgroup admits **~14** workers — above
the ~12 that saturate the HDD for `parse`, below the ~17 for a typed query.

**Block size dominates achievable parallelism under a fixed budget.** The same
cgroup admits **3** workers against the locally recompressed
`koji-…blocks128.xz` (128 MiB blocks) versus 14 against the 24 MiB-block
upstream download — so the multistream file is the better shape for a memory-
bounded parallel read despite costing an 85 s footer walk once per file, and
the recompressed one is worse at exactly the thing it was made to be better at.

### The caller sets workers or bytes, whichever binds first

Both, neither defaulted, mirroring `xz_seek::Bulk::new(workers, budget_bytes)`.
The defaults split by audience:

- **The library defaults to serial.** An embeddable component does not spawn
  threads by surprise, and embeddability is a project goal
  (`roadmap.md`, "Project goals"), so parallelism is opted into. The surface is
  a `Parallelism` enum, so `Serial` is a state rather than a magic number — and
  it selects the serial code path this build has today, not a pool of one.
- **The CLI defaults to parallel**, with `--jobs <n>` and `--parallel-memory
  <bytes>` capping it. `--jobs 1` is the serial path.

**Amended: the stated bytes bound both memory terms, and the CLI's parallel
default waits on the figure that justifies it**
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "What a
default `pgdq query` may hold"). Two things were true when the split above was
written and stopped being true at `16.9`. The budget was a bound on what the
*source* retained, and the replay's sub-streams now multiply what a query
**pins** as well — so `worker_count` divides the stated bytes by what one
sub-stream costs in both terms, `partition_bytes + max_source_span`, and one
number bounds a query rather than one number bounding half of it. And the CLI's
parallel default was set when `--jobs` bought I/O depth at no memory cost;
until `16.10.2` there is no CPU parallelism to buy, the I/O depth is unmeasured on
every device, and on a HDD N sub-streams read at N separated offsets at once.
So **`--jobs` defaults to 1 until `16.13` measures a gain**, and the sentence
above states the intent the figure is asked to license rather than a default
shipped ahead of it. `--jobs` and `--parallel-memory` are unchanged as flags:
what moves is only what a person who states neither gets.

**Amended again: the flip is a correction to execute, not a position at the end
of the phase, and the worker count is apparatus**
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The
parallel default is a correction the tree has not made"). The first of the three
reasons above is spent — `16.10.2` landed, so there *is* CPU parallelism to buy —
and the two that remain argue for landing the correction sooner rather than
holding it. So **`16.16` runs ahead of `16.11`** rather than last; the numbers
are identifiers and the index table is the schedule, so nothing is renumbered.
Two further things move with it. Every registered figure's command shape, the
profile recipe and the koji recipe **state `--jobs` explicitly** instead of
inheriting whatever the CLI default is — worker count is part of the apparatus
on the same argument [`measurements.md`](measurements.md) already makes for the
allocator, and no shape stated it while the default changed meaning underneath
the register. And `--jobs`' own help text stops calling itself a ceiling on
concurrent workers, which on a plain source is `min(jobs, 4)`.

*Rejected:* deriving the byte budget by reading the cgroup limit. It is
attractive given that every koji run here is in a 512 MB cgroup, but it makes
memory behaviour depend on a kernel interface nothing in this project has
verified, and it would need an entry in `postgres-invariants.md`'s sibling
position — an assumption register entry for something outside PostgreSQL
entirely — before it could be honest.

### The sparse row index and `KD5` both leave this phase

**The sparse row index goes to P10.** The leader arrangement removes this
phase's need for one: splitting an open `COPY` block's interior at LF
boundaries costs a single row's resync, so known row boundaries buy nothing
`memchr` does not already give. Its remaining consumers are P10's row groups
and a row-range seek nothing exposes, so P10 owns it outright and the roadmap's
"whichever of the two runs first settles the interval" is amended to say so.

**`KD5` drops to `(c)` unowned**, promoted by a dump with thousands of blocks
scanned under `--dqcache none`. It is the whole-list splice rebuild, and the
leader keeps coverage prefix-shaped, so `splice` runs once per `CopyEnd`
exactly as it does today and parallelism does not change the splice count.

**`KD14`'s attribution is taken here**, because it is cheap and this is a phase
whose central promise is a memory bound: two readings, `query --dqcache none`
against `query-nomatch`, separating the span list from the whole-list clone
from what glibc returns. A phase running N workers multiplies whatever a scan
holds, so it has to know what that is.

### The source advises its own partitioning, and L4 never learns what it is

A fused worker calling `xz-seek`'s pieces from L4 would put decode scheduling in
the query layer and make L4 name its source. Two changes keep every layer where
[`layering.md`](layering.md) puts it:

- **`XzSource` becomes internally concurrent** — the single `xz_seek::Reader`
  behind a `std::sync::Mutex` gives way to per-call block decode over a shared
  immutable `SeekTable` and the block pool above, so concurrent `read_range`
  calls genuinely run concurrently. That is the serialization point this phase
  exists to remove, and removing it is entirely an L1 change.
- **`ByteRangeSource` gains a partitioning advisory** — how this source would
  like a range split, and what each partition costs resident.
  `LocalFileSource` answers "anywhere, one buffer each"; `XzSource` answers "at
  these block boundaries, 32 MiB each". It is a defaulted method, joining the
  others that exist for a source the local file is not
  ([`architecture.md`](architecture.md), "Execution model and API surface").

**The advisory is read off the read path the source actually took, never off
the seek table alone.** An `XzSource` whose largest block is above
`BLOCK_DECODE_MAX_BYTES` still has a seek table and still has block boundaries,
but every read goes through the mutex-guarded streaming reader, where reaching
an offset inside a block has one route — restart at that block's start and
discard forward (`vendor/xz-seek/src/reader.rs`: *"a backward move within a
block restarts"*). Two workers on different partitions of such a file would
each force the other's restart, so parallel mode over it is **worse than
serial**, not merely unaccelerated. That source advises **one partition**, and
nothing is given up by it: the shape this reaches in practice is a single-block
file, whose one block is the parallel unit entire — LZMA2's dictionary runs the
length of a block, so no budget buys a second worker anything
([`architecture.md`](architecture.md), "The compressed source").

L4's scheduler asks the source how to split, runs N workers that each
`read_range` and parse, and never learns what is underneath. **P14 inherits the
question correctly**: a remote source's natural partitioning is a ranged-GET
size, which is the same method with a different answer.

**The leader's LF-splitting rests on two registered invariants and needs no new
one.** I15 escapes `\n` among exactly seven things in COPY TEXT output, so a
literal LF byte inside a data region is always a row boundary; I7 says
`LF 5C 2E` cannot open a data line, so `\.` is unambiguous. Both gain a
"Relied on by" line naming this mechanism.

### The library hands out partitions; the CLI merges them

`TableStream` splits into N sub-streams, each internally in file order, and the
caller runs them. That maps onto DataFusion's `TableProvider::scan` partitions
directly — the destination the async core and the `Utf8View` choice were made
for — and running the partitions sequentially *is* the serial path, so the
knob's "off" setting is not a second implementation.

**`pgdq query` merges them back into file order**, k-way on source offset. Each
partition yields batches in order, so the merge holds **one batch per
partition** — bounded at N × batch rather than an open-ended reorder buffer. The
engine gets partitions, the human gets the order they expect, and neither pays
for the other.

*Rejected:* reordering inside `TableStream` into one merged stream, and
emitting unordered with the contract relaxed. The first puts a buffer the
consumer controls inside the library; the second is permitted pre-1.0 and gives
up file order for the CLI's reader to buy nothing an engine wanted.

### An interrupt banks the last completed `CopyEnd`, exactly as today

Workers' partial ranges are discarded. Banking them would record a **partially
censused block**, and a partial census is a *wrong* answer rather than a weak
one: a worker that saw only a column's 2-D rows records `(2, 2)`, resolving to
`List<List<T>>` and then hard-failing on every 1-D value in the half it never
read — the confidently-wrong schema the both-bounds census design exists to
prevent ([`architecture.md`](architecture.md), "What the census decides, and
who may believe it").

So the census totality invariant holds by construction, `splice` is untouched,
and byte-identical resume survives with no new machinery. What an interrupt
costs is unchanged — the block in flight, which on koji is hundreds of
gigabytes — and this phase *improves* it in wall-clock terms, since re-scanning
that block is now up to 19× faster. Wiring is small: `ScanOptions::cancel`
reaches the workers, and `xz-seek`'s block tasks already poll a flag inside the
decode loop, so a join is bounded by a decoder call rather than by a 128 MiB
block.

*Rejected:* banking a partially scanned block to save an interrupted koji scan
its remaining hours. It requires census partiality to be representable and kept
away from `resolve_columns`, which is a larger change than this phase's, and it
belongs to whoever wants it rather than here.

### A parallel scan's cache is byte-identical to a serial one

`pgdq parse --jobs 1` and `--jobs 8` over one input produce byte-identical
`.dqcache` files, and both equal what this build produces today. That is the
phase's central correctness test, and it extends an argument the project has
already made: `full.spans == eager.spans` holds because **how a map was
assembled must not be visible in it**, and worker count is a second assembly
route.

For the census it is true by construction rather than by care — `ArrayShape`'s
merge is min-of-mins, max-of-maxes and OR of the `[lb:ub]=` flag over
`ArrayShape::default()`, a bounded semilattice, so it is commutative,
associative and idempotent, and `index::union_census` folds length-tolerantly.
The test's value is that it catches a merge bug in spans, census, row counts,
diagnostics and the serialized envelope at once, and that it is the regression
oracle for the leader: a worker that somehow began outside a region would be
caught here. It also settles resume across worker counts — a cache written
under `--jobs 8` and resumed under `--jobs 1` is the same cache — and it is why
the worker count is **not** recorded in the cache. What it cannot cover is
`INSERT` runs and preamble DDL, which stay serial by I17 and by the discovery
refusal.

### Evidence first: the slice order after the evidence slices is allocation order

This phase's opening slices produce the evidence that decides what the later
ones are worth, so — per `../process.md`, "Slice numbering" — its numbers past
those are **allocation order rather than schedule**. The thesis table at the
top of this document is arithmetic, and one of its two load-bearing inputs is
now measured.

Two figures are load-bearing and are taken before the design is committed to:

- **`xz-decode-scaling`** — decode rate against worker count, taken by
  `scripts/measure.py`, over the koji `.xz` and a generated control. **Taken**:
  the near-linear reading holds to four workers and not past it — 7.78× at
  twenty-four on koji, 10.80× on the control — so the worker counts this
  document's arithmetic names are floors rather than estimates, and the slices
  that price parallelism read the figure rather than the table.
- **`parallel-scan-throughput`** — `pgdq parse` and `pgdq query` against
  `--jobs`, on the HDD, the SATA SSD and the NVMe, compressed and plain.

The orderings that **do** bind regardless of what those say: the block-pool
rework precedes anything that runs N readers, and `XzSource`'s internal
concurrency precedes the scheduler that depends on it.

**A `--jobs` figure needs an apparatus gate of its own.** Every figure in the
register today is single-threaded and its gate assumes a quiet machine; a
figure that deliberately occupies 24 threads inverts that assumption, so it
cannot inherit the sweep's gate.

### `--jobs` is a ceiling, not a request

Three shapes admit no parallelism, and they get three different treatments:

- **A plain-file `parse`** — silent. Discovery on plain input is refused above,
  so one worker is the correct answer rather than a condition to report, and
  warning here would fire on the most common invocation this tool has.
- **A single-block `.xz`** — the existing `DiagnosticKind::NonSeekableCompressedSource`,
  unchanged, which already names `xz -T0` and `--block-size=<size>`.
- **An `INSERT` run** — nothing in the code, one sentence in the manual. By I17
  it has no line-anchored statement boundary, so the leader can never open it
  to workers; it is the shape that would gain most (**4.9×** a `COPY` scan
  warm, `KD9`) and the one that structurally cannot, which a reader of `--jobs`
  deserves to be told once.

### Workers come from `spawn_blocking`, and the module map gains one L4 entry

The library keeps `tokio = { features = ["rt", "sync"] }` — **no
`rt-multi-thread` is added** — and dispatches workers through
`tokio::task::spawn_blocking`, the mechanism `io.rs` already uses for every
positioned read. It works under a `current_thread` runtime, since the blocking
pool is separate from the reactor, so an embedder's runtime flavour stays the
embedder's choice and the caller's runtime governs how much parallelism exists.

*Rejected:* a `std::thread` pool of our own, which spawns threads an embedder
did not ask for and needs channels to bridge back to the async `Stream`; and
`rayon`, a new dependency that mixes awkwardly with async.

**A `spawn_blocking` task cannot be cancelled from outside**, so the interrupt
guard rests entirely on the cooperative flag — which is why `xz-seek` was asked
to poll one inside its decode loop. Nothing else in the design wants an
abortable task.

Placement, recorded in [`layering.md`](layering.md)'s table in the same change:
the block pool and the partitioning advisory are **L1** (`io.rs`), the leader
and the worker scheduler are a **new L4 module**.

**This phase closes one of `layering.md`'s two recorded deviations.**
`stream.rs` (L4) owns the `Bytes` → `arrow::Buffer` conversion and the
chunk-retention deque, which is L3 work; that deque is being reworked here for
block-shaped pinning, which is the "work that reworks the module anyway" the
deviation was waiting for. Moving it into `batch.rs` is behaviour-preserving,
so it lands under the "Refactor when the shape stops fitting" rule, which
obliges it to say what it buys: it survives P10 and P14, and it makes the
pinning rework happen in one layer instead of straddling two. The second
deviation — `batch.rs` exposing `read_table` — is **not** touched, since
nothing here reworks it.

### The leader opens one region at a time

Workers cover the interior of one open `COPY` block; whichever finds `\.`
reports the end and the leader resumes from there. So blocks are processed one
at a time with N-way parallelism inside each.

**The degradation is a property, not a deficiency** — the remedy is in the
user's hands (`xz --block-size=`), and the shape degrades gracefully rather
than failing. A block smaller than *jobs* × *compressed block size* — roughly
**336 MiB** at 14 workers over 24 MiB blocks — cannot fill every worker, and a
block under one compressed block gets no parallelism. koji's 74 blocks over
784 GB fill every worker; a 500 GB dump of 5,000 tables at ~100 MB each gets
~4 workers a block, keeping the whole 19.4× compression win and about 4× of
the 14× parallel one.

*Rejected:* cross-block pipelining, keeping workers busy on block *k* while the
leader gets ahead to *k+1*. Finding block *k*'s end **is** the work the workers
are doing, so it needs either speculation past an unscanned block or a second
leader.

### The lowest-offset error wins

A worker that fails records its error and stops; the scheduler waits for the
partitions *before* the failing one to finish or fail, then raises the
lowest-offset error. Serial scanning gives the earliest error in the file by
accident of having only one order, and a user re-running to confirm a failure
relies on getting the same message — so under N workers that has to be arranged
rather than assumed, or a typed `query` over a column with one bad value per
million rows would name a different row on every run over an unchanged file.

It costs nothing where there is no error, and the ordering key is the one the
CLI's k-way merge already sorts on.

*Rejected:* first-to-fail-wins with the nondeterminism documented. A
reproducible error message is worth more than the sibling drain it costs.

### Two memory figures, and `peak-rss` re-taken

This phase's central promise is a number in bytes, and nothing measures resident
set at more than one thread today. `peak-rss` — the register's one figure taken
outside a sweep, at `7ee5db5`, standing in no borrow edge, and genuinely stale
since — is re-taken here, and **`parallel-peak-rss`** joins it: RSS against
`--jobs`, **at two block sizes**, because the same 512 MB cgroup admits 14
workers at 24 MiB blocks and 3 at 128 MiB, so a figure at one block size would
state a number that is really a function of the file. `KD14`'s attribution
rides in that sitting rather than as separate work, being the same instrument
pointed at `query --dqcache none` against `query-nomatch`.

Both stand outside a borrow edge, as `peak-rss` does, so both may be published
outside a sweep with their sitting commit in their own markers — which this
phase needs, since a full sweep is an hour of a *quiet* machine and these
figures deliberately occupy all of it.

### The headline claim is stated against koji; the mechanism against generated inputs

The registered figures — `xz-decode-scaling`, `parallel-scan-throughput`,
`parallel-peak-rss` — are generated-input work and go through
`scripts/measure.py`. They cannot show the 19.4× device trade: the ratio is a
property of koji's data, and the HDD is deliberately not a sweep regime
([`measurements.md`](measurements.md), "The HDD is not a fourth regime").

So the claim is verified on koji, **outside the register**, filed the way the
existing koji section is — an `<!-- outside-register: … -->` marker reconciled
against `measure.NOT_OURS` both ways, carrying no figure marker. It is one
detached `pgdq parse` of the `.xz` under `--jobs` in a 512 MB cgroup, launched
per `CLAUDE.md`'s long-running-process rule and read by a later session, and it
doubles as the correctness check that matters most: **the resulting `.dqcache`
must be byte-identical to the one the serial 784 GB scan already produced.**
That is the determinism property at the only scale where it is interesting, and
it costs one detached hour rather than a new apparatus.

## Slices

**The order after the evidence slices is allocation order, not a schedule.**
This phase's opening slices exist to produce the evidence that decides what the
later ones are worth, so a list ordered in advance would be ordering it against
the guesses the phase was convened to replace (`../process.md`, "Slice
numbering"). A slice admitted after this document was written takes the next
free number rather than being inserted.

**The orderings that bind regardless of what the evidence says:** 16.1 and 16.2
first; **16.3 before 16.4**, so the pinning rework happens in one layer rather
than straddling two; **16.4 before 16.5, 16.8 and 16.10.1**, all three of which
run N readers; **16.5 before 16.10.1**, which depends on it; **16.7 before
16.7.1**, which reads the value it defines; **16.7.1 before 16.9**, which needs
the flags; **16.10 before 16.10.1**, which is what "the evidence half lands
first" means here; **16.10.1 before 16.10.2**, and **16.10.2 before 16.12**, whose
determinism test has nothing to compare until a `--jobs` parse takes a
different route through the file. **16.4.1 lands with or after the
first of 16.5 and 16.10.1**, whichever runs first, since a wait with no second
holder is a
deadlock rather than a bound — **and after 16.7.1**, which is a second binding
found while 16.6 was being written
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.4.1
waits on 16.7"): a wait has no number to wait on until a caller states a
budget *and the pool reads it*, so wired to today's constant it would be a wait
on the serial path's number, and against a value nothing consumes it would be a
wait on a number that never arrives. The condition that once also named
`Parallelism` is gone — the exemption is by holder class (this document's
`16.4.1` row) — but the budget binding stands on its own.

### Evidence

| Slice | What it commits to |
|---|---|
| **16.1** | **`xz-decode-scaling`** — the 446 MB/s decode probe becomes a registered figure: plaintext decode rate against worker count, over the koji `.xz` and a generated control, at 1/2/4/8/12/16/24 workers. Instrument: `scripts/measure.py`, a new figure in the register with its own `depends`/`quoted_by`/`shares` edges. No library code. |
| **16.2** | **`KD14`'s attribution, and `peak-rss` re-taken at HEAD** — two readings, `query --dqcache none` against `query-nomatch`, separating the span list from `stream::splice`'s whole-list clone from what glibc returns to the OS. Instrument: the existing in-container RSS harness. No library code. |

### Mechanism

| Slice | What it commits to |
|---|---|
| **16.3** | **`layering.md`'s L3 deviation closed** — the `Bytes` → `arrow::Buffer` conversion and the chunk-retention deque move from `stream.rs` (L4) into `batch.rs` (L3), behaviour-preserving, with that doc's table and deviation list updated in the same change. |
| **16.4** | **The block pool's sizing** — `io::BufferPool` from four fixed slots to a byte budget the slot count is derived from, so a slot may be a decoded xz block, and `RowBatcher::max_source_span` re-derived against block-shaped rather than chunk-shaped pinning. |
| **16.4.1** | **Backpressure** — a slot acquisition that waits for a free slot instead of allocating, which is what turns the slot count into a bound on what is outstanding. **Earned, not planned** ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.4 split: backpressure has no test without a second holder"): the pool cannot tell a serial reader legitimately holding `(max_source_span / chunk) + 1` buffers — unbounded under `max_source_span: None` — from a worker holding one slot, so a wait is backpressure for the second and a deadlock for the first. The **exemption** is what makes a wait safe, not the budget: at the defaults the serial reader reaches 65 MiB against a 64 MiB budget, and under `None` no finite budget is above its reach. It lands with the first concurrent consumer because that is what a test needs — a blocking acquire has no behaviour except its interaction with holders, and against the exempt holder alone the strongest assertion is "it did not block", which the non-waiting `take` already guarantees. Two commitments come with it, and the second replaced an option-validation error the block pool made unsound ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The wait is exempted by holder class, not validated by an option pair"): **one slot per waiting holder**, and the exemption stated by **holder class** — a read that will be retained into a batch never waits, whatever `Parallelism` and `max_source_span` say, because the batch is yielded to the caller and how many slots are outstanding is then a property of consumer code. The stated bound is two terms: the waiting holders' slots, and what in-flight batches pin. |
| **16.5** | **`XzSource` internally concurrent** — the single `xz_seek::Reader` behind a `std::sync::Mutex` gives way to per-call block decode over a shared immutable `SeekTable` and the block pool, so concurrent `read_range` calls genuinely run concurrently. The block decoder gets a **pool of its own**, not a second unit announced into the chunk pool ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.5 takes a second pool, because a pool describes one unit"): `BufferPool::hinted` is one atomic driving both `keeps()` and `slot_bytes()`, so a single pool serving two units drops every block on release at the chunk length, or takes the chunk path's pooling away at the block length. |
| **16.6** | **`ByteRangeSource::partitions`** — the defaulted partitioning advisory, with `LocalFileSource`'s and `XzSource`'s answers and their per-partition footprints. `XzSource` has **two** answers, read off the read path it took rather than off the seek table: block boundaries where it block-decodes, and **one partition** where it fell back to the streaming reader, whose restart-and-discard positioning makes N workers worse than serial. No consumer yet. |
| **16.7** | **The `Parallelism` value** — the library enum defaulting to `Serial`, with `Workers { jobs, memory_bytes }` mirroring `xz_seek::Bulk::new(workers, budget_bytes)` and one worker spelled as `Serial` rather than as a `Workers` of one, and its `ScanOptions`/`QueryOptions` wiring. No consumer: the surface is where a caller states the two numbers, and both mechanisms that read them are `16.7.1`'s. |
| **16.7.1** | **What the stated budget decides, and the flags that state it** — `POOL_BUDGET_BYTES` and the block pool's depth become the caller's numbers, **`BLOCK_DECODE_MAX_BYTES` becomes a consequence of the stated budget rather than a constant beside it** (the 256 MiB cap and the 64 MiB budget are unrelated numbers today, and the latter is not a bound above its own slot size, since `BufferPool::slots` clamps to at least one; a file with more than one block is seekable for a client willing to allocate a block, so what the cap declines is memory — and the budget is where a caller says how much of it they have, [`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The block-decode cap declines memory, not seekability"), and the CLI's `--jobs` / `--parallel-memory` with the manual page: the degradation curve, and the one sentence saying why an `INSERT` run cannot be parallelized. **Earned, not planned** ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.7 split: a value, then what reads it"): the row paired a value with a rework of the two pools every timed path runs through, and the flags are only honest once the number they state changes something. |
| **16.8** | **Partitioned replay** — `TableStream` splits into N sub-streams over a complete map, each internally in file order, for a plain source and a compressed one alike. |
| **16.9** | **The CLI's k-way merge** on source offset, holding one batch per partition, so `pgdq query` keeps file order at N × batch rather than an open-ended reorder buffer. |
| **16.10** | **The interior split** — what one LF-split piece of an open `COPY` block's interior answers on its own (rows, its own census, the `\.` terminator if it held one), and how the pieces fold back into the block's totals. Pure and synchronous: the body a fused worker runs, and the merge the leader performs, with no source, no dispatch and no consumer. Adds a "Relied on by" line to I7 and I15, which are what make LF-splitting sound. |
| **16.10.1** | **The leader's scheduler** — `leader::scan_region` cuts an open `COPY` region's interior where the source advises, runs the fused decode-and-parse workers over the pieces, folds their answers with `16.10`'s `merge` until one holds the terminator, and is the read loop that grants `WaitPolicy::MayWait`. It reads `partitions()` for the shape of the cut only, never for whether to make one, and declines to cut a region smaller than one partition's own `partition_bytes()` — a floor derived from the source's answer rather than set as a constant, which is what keeps a many-small-block dump from paying scheduling per region. No consumer: what folds its answer into `map::Builder` and the `DumpIndex` is `16.10.2`. **Earned, not planned** ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.10 split: the parse, then the scheduler that feeds it"), and **rewritten to the scope that landed** when the wiring turned out to be the other half of the same seam ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.10.1 split again: the scheduler, then the loop that runs it"). |
| **16.10.2** | **The mapping pass runs the leader** — `stream::map_forward` offers each open `COPY` region to `leader::scan_region` and closes a region it took through the same path a serial `CopyEnd` takes: the census onto the `Builder`, `on_copy_end`, the splice, the throttle's save and the target check, with the serial scanner repositioned past the block. `pgdq parse --jobs` then buys something. **Earned, not planned**: `16.10.1` paired a self-contained scheduler with a rework of the loop every `parse` and every query's first pass runs through, which is the seam `../process.md`'s "Size a slice by its review, not by its scope" names ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.10.1 split again: the scheduler, then the loop that runs it"). The remainder of a mis-sized *third-level* slice is the next increment under its parent, not a fourth level and not the next free phase-level number — the shape `11.6.1`→`11.6.2` and `4.4.2`→`4.4.3`→`4.4.4` already set, now stated in [`../process.md`](../process.md), "Slice numbering". It was briefly numbered `16.17`, which is what commit `f65f9f6`'s message calls it. |
| **16.11** | **Error ordering** — a failing worker records and stops, the scheduler drains the partitions before it, and the lowest-offset error is the one raised. Asserted, not documented. |
| **16.12** | **The determinism test** — `pgdq parse --jobs 1` and `--jobs 8` produce byte-identical `.dqcache` files over every fixture, and both equal what this build produces today. |
| **16.13** | **`parallel-scan-throughput` and `parallel-peak-rss`, as apparatus** — both figures built, registered in `measure.UNTAKEN` and runnable, with no readings taken. `parse` and typed `query` against `--jobs`, compressed and plain, over four legs of one plaintext; RSS against `--jobs` at two block sizes. Both carry an apparatus gate of their own — `warm-parallel`, which gates on steal alone, a figure that occupies 24 threads being unable to inherit the sweep's quiet-machine one — and both declare a container memory departure, the register's 512 MB being smaller than the budget under test. **On a plain source the curve flattens at four workers because the chunk pool binds, not because the scan stops scaling** — `POOL_DEPTH` clamps `BufferPool::slots()` and a fifth fused worker waits ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "The plain-file leader is capped at four workers by the chunk pool"). The job range runs past four so that ceiling is in the table rather than inferred; `16.15` is where the ceiling itself is re-derived. **Rewritten to the scope that landed** when the sitting turned out to be unreachable from the tree the apparatus lives in ([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.13 split: the apparatus, then the sitting that needs a commit to name"). |
| **16.13.1** | **The sitting** — both figures taken at a named commit and folded into [`measurements.md`](measurements.md), each declaring that commit inside its own marker, and both entries moved out of `measure.UNTAKEN` into `FIGURES` where the doc-side checks start applying. **Earned, not planned**: a figure published outside a stamped sweep names the commit it was taken at, and a sitting run from a tree carrying its own uncommitted apparatus has none to name — the parent it could name is a tree where the instrument does not exist. It is the seam `xz-decode-scaling` already waited at, and it is not the mechanism/evidence one: both halves are one confidence and the whole of `16.13` is harness code, but the second half cannot be *taken* from the tree the first half lives in. |
| **16.14** | **koji verification** — a detached parse of the `.xz` in a 512 MB cgroup, per `CLAUDE.md`'s long-running-process rule, checked two ways because no single artifact can carry both. **Across arrangements**: a `--jobs 1` and a `--jobs N` leg of the *same* file, taken in the same run, whose two `.dqcache` files must be byte-identical to each other. **Across formats**: each leg's reported block, row and byte counts must equal the plain 784 GB scan's, in [`measurements.md`](measurements.md), "koji full scan". **Amended by the review of 2026-09-08** ([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md), "A compressed cache and a plain one were never byte-comparable"): this row asked for byte-identity against the cache the serial 784 GB scan left behind, which is an artifact nothing produces — a compressed source's cache and a plain source's cache of one dump differ in `compression` and `identity` by construction, quite apart from the format bump that had already expired that particular file. Outside the register, with an `<!-- outside-register: … -->` marker. |

**16.10 was named here in advance as the row most likely to earn a
`16.10.1`, and it did** — the prediction being that it is the only row pairing
a new scheduler with a merge into three already-tested structures, and that if
the two halves needed different confidences at review the split was at that
seam. **The seam it actually earned is one step to the left of that**, at the
recurring mechanism/evidence one: the *parse* a worker runs and the fold the
leader performs are pure, synchronous and checkable against the serial scanner's
own answer, where the scheduler is dispatch plus a restructure of `map_forward`,
which every `parse` and every query's first pass runs through. So `16.10` is the
half that can be reviewed against a reference it did not produce, and `16.10.1`
is the half that reworks a tested core path
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md),
"16.10 split: the parse, then the scheduler that feeds it").

**And the same seam ran through `16.10.1` too**, which is what `16.10.2` is. The
dispatch is a self-contained new function that can be checked against the serial
scanner's answer exactly as the parse was; the restructure of `map_forward` is
the rework. Naming a slice "the half that reworks a tested core path" turned out
not to be the same as it *being* only that — the sentence above described the
row's centre of gravity and the row still carried both confidences, which is a
thing to expect of any row a split leaves behind
([`../status/history/2026-09-07.md`](../status/history/2026-09-07.md),
"16.10.1 split again: the scheduler, then the loop that runs it").

**16.4 earned the phase's first third level, and at a seam this document did not
predict.** The row paired a sizing change with a *bound*, and a bound is only
reviewable against the holders it bounds — which is a different seam from the
mechanism/evidence one the process names, and one that only shows up once a
slice is being written.

**16.13 earned one at a seam that is not about confidence at all**, which is the
first of those in this project. Its two halves are one confidence — no library
code, one harness diff, one review — and what separates them is that the second
cannot be *performed* from the tree the first lives in: a figure published
outside a stamped sweep names the commit it was taken at inside its own marker,
so the apparatus has to be committed before the readings exist. `measure.UNTAKEN`
is the mechanism that already anticipated this and `xz-decode-scaling` is the
precedent; what is new is that the seam was predictable at spec time from the
figures being outside-the-sweep ones, and this document did not predict it.

**16.7 earned the second, at the seam between a value and what reads it.** The
row paired a plain enum with a rework of the two buffer pools every timed path
runs through, and the rework turns on a choice this document does not make —
whether the default budget stays where `POOL_BUDGET_BYTES` is, and a large-block
`.xz` therefore falls back to streaming, or rises to where the cap is, and every
compressed scan retains more. Both are visible only once the derivation is being
written. The CLI flags follow the rework rather than the value, because a flag
whose number changes nothing is surface that lies to a user, where an option
field documented as unread is not.
