# P16 inbox — facts filed for its grilling

Evidence for P16 (parallel scan and extraction), the phase carved out of the
scan-performance work when the parallel half became its own. **This is a queue,
not a document**: when P16 is grilled, walk every entry, fold it into that
phase's spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged. The
scan-performance work has since **finished**, so every entry here describes
something that is true of the code as it stands rather than something still
moving.

---

## `stream::splice` assumes coverage is a contiguous prefix

**Fact.** A query's mapping pass rebuilds `DumpIndex::spans` as `prefix ++
built ++ [Unscanned tail]`, where `prefix` is every span with `end <=
seg_start`. The seam between the two is closed by **extending the last prefix
span** to where the first newly-built span starts. Both halves of that depend
on the prefix being a complete tiling of `[0, seg_start)`: if coverage had an
interior hole, `prefix` would not tile, and extending its last span across the
seam would silently paper over the wrong range.

**Why this phase cares.** This is the *one* place the P3 spec's "coverage is
a prefix, by construction" stopped being an observation and became an
assumption in code. That spec section already names device-aware
parallelism as "the plausible future source of interior holes" and argues a
span list (rather than a watermark) is what makes them expressible. It is
right that the *format* allows them — but `splice` does not, and out-of-order
NVMe scanning is exactly what would produce them. Whatever this phase does about
scan ordering has to either keep coverage prefix-shaped or rework `splice`'s
seam rule, and that should be a decision, not a discovery. **The answer already
given is the entry "coverage is left prefix-shaped" below**, which says which of
the two the scan-performance work chose and what it added to the assumption
while it was there.

**Origin.** 2026-08-24. See
[`architecture.md`](architecture.md),
"Query: mapping and streaming are separate passes".

---

## A seekable compressed source hands this phase parallel discovery for free

**Fact.** This phase's "Parallelism" section splits the problem in two:
row extraction parallelizes trivially once boundaries are known, while
*discovery* is hard, because from a cold start you cannot tell whether a random
offset is inside a `COPY` block — which is why the speculative scheme is
prototype-and-measure work rather than a plan.

A seekable compressed source removes exactly that half. An `.xz` stream's index
gives every block's uncompressed offset before a byte is decoded, so workers can
be handed real, self-contained ranges rather than speculative ones. Measured on
the koji `.xz` (31,150 self-contained streams, ~24 MiB uncompressed each): one
core decodes ~446 MB/s of plaintext, four concurrent per-stream decodes reach
~1.48 GB/s at 397% CPU, and `xz`'s own `-T8` on that file gains nothing, its
threaded decoder parallelising blocks within a stream where each stream holds
one. Numbers and method:
[`roadmap-P13-compressed-input.md`](roadmap-P13-compressed-input.md),
"Evidence this phase rests on" — probes, not figures.

**Why this phase cares.** P13 lands the decompressing source and deliberately
does *not* take parallel decode, leaving it to this phase. So this phase inherits a second parallelism case with a
different bottleneck — CPU-bound at ~450 MB/s a core where the plain path is
device-bound at ~240 MB/s on the same HDD — and a different unit of work: a
compressed block rather than a byte range resynced to the next LF. A device-awareness rule therefore needs a second axis, since the right worker count for a
compressed source is set by cores and decode rate, not by
`/sys/block/<dev>/queue/rotational`.

**Origin.** 2026-09-02, sketching P13.

**Contingent on** P13 landing first, which is this table's order. If it slips
behind this phase, the entry becomes a constraint on defaults rather than a
case to implement.

---

## Parallel xz decode is worth ~3.3x, and the seekable-xz crate is being shaped to allow it

**Fact.** On a 300 MiB stream-aligned slice of the koji `.xz`: one `xz -dc -T1`
process reaches ~446 MB/s of plaintext at 108% CPU; `xz -dc -T8` on the same
slice reaches **the same 446 MB/s**, because xz's threaded decoder parallelises
blocks *within* a stream and every stream in that file holds one block; four
`xz -dc -T1` processes on four stream-aligned pieces reach **~1.48 GB/s** at
397% CPU, near-linear. Probes, not figures — no harness, no `drop_caches`.

**Why this phase cares.** A compressed source inverts the arithmetic behind
"device-bound": it reads 19x fewer bytes and pays for them in CPU, so the
readahead, chunk-size and parallelism defaults this phase sets have two source
shapes to satisfy rather than one. The discovery half of parallelism is already
solved for a seekable stream — block boundaries are known up front from the
index — so this is the case where only the easy half is left. The external `xz-seek` crate P13 is blocked on is being specified to
*defer* parallel decode but not design it out: its requirements say the block
decoders stay independent of one another and of any shared cursor, behind the
same positioned-read interface.

**Origin.** 2026-09-02, grilling P13. Re-check
`/mnt/wd12t/fedora/experiments/xz-seek/docs/design/historical/initial.md` for what
the crate actually committed to.

---

## The sparse row index is this phase's to build, and P10 needs the same interval

**Fact.** The index — the byte offset of every Nth row — was the
scan-performance work's until its grilling found it had no consumer there. `ResumeToken` already carries a byte
offset and an in-block row count, so resuming a query does not rescan a block
from its start, and nothing in the library or the CLI exposes a row-range seek.
koji puts the scale on it: 19,575,829,920 rows is ~2.4M checkpoints at ~19 MB
against ~157 GB for a dense 8-byte-per-row index. `CopyBlock::sparse_index` is
reserved and always `None`, so adding it is not a cache-format break.

**Why this phase cares.** It is the first phase that reads one: known row
boundaries turn a speculative split into a real one and remove the resync scan
entirely. The interval is not a free choice — P10 attaches per-row-group
statistics to it, so whichever of the two phases runs first settles it and the
other inherits it. Decide it against both, not against splitting alone.

**Origin.** The scan-performance grilling, 2026-09-03
([`../status/history/2026-09-03.md`](../status/history/2026-09-03.md), "P7's
grilling").

---

## An `INSERT` run cannot be split speculatively, and it is the shape that most wants to be

**Fact.** A splitter syncs by starting at an arbitrary offset and finding the
next record boundary. A `COPY` block admits that: its rows and its `\.`
terminator are line-anchored, so a worker landing mid-block resyncs at the next
newline. An `INSERT` run admits nothing of the kind — where a statement ends is
decided by quote, dollar-quote and comment state accumulated from the run's
start, so an offset in the middle of one is uninterpretable on its own. A
worker cannot tell a `);` inside a string literal from the one that ends the
statement without having crossed every byte before it.

**Why this phase cares.** It inverts the obvious priority. An `INSERT` run is
the most CPU-hungry shape this scanner has — 4.9× a `COPY` scan's per-byte cost
warm, the deficiency register's `KD9` — so it is where cores would pay best,
and it is the one region kind speculative splitting cannot touch. Three ways
out, and the choice belongs to this phase's grilling rather than here:
serialize each run and parallelize *across* runs (trivial, and worth nothing on
a dump whose data is one large table); make the sparse row index above cover
`INSERT` runs too, which turns a split into a lookup and is the reason that
entry and this one should be read together; or accept a resync scan from the
run's start, which is O(run) per worker and self-defeating.

Note the same fact bounds P13: a compressed-block boundary lands mid-statement
just as a speculative split does.

**Origin.** The `INSERT` statement scan and the review of its `KD9` call,
2026-09-03
([`../status/history/2026-09-03.md`](../status/history/2026-09-03.md), "`KD9`
reviewed: struck, then restored to its residual"). Found by asking what the
residual costs beyond scan time; contingent on nothing — it follows from the
statement grammar, not from the implementation.

## The read path's buffers are now pooled behind one mutex, sized for one reader

**Fact.** `io::LocalFileSource` no longer allocates a buffer per chunk. It
keeps a four-slot free list behind a `std::sync::Mutex`, hands a buffer out per
`read_range`, and gets it back when the last reference to that chunk's `Bytes`
is dropped ([`architecture.md`](architecture.md), "Execution model and API
surface"). Four slots is the single-reader steady state — one chunk in flight
and one just released, with room for the query path's retained chunk — and the
lock is taken twice per chunk read.

**Why this phase cares.** N workers reading concurrently turn that free list
into shared state on the hot path: at four slots most workers miss and fall
back to allocating, which restores exactly the per-chunk `calloc` this pooling
removed — and it is 9.4% of a warm `parse`'s user instructions. The two
questions are the slot count (which wants to be a function of the worker count,
not a constant) and whether the pool should be per-worker instead of per-source,
which removes the lock and the miss at once but multiplies the resident buffers.
Neither is decidable without knowing how this phase splits the work, which is
why it is filed here rather than guessed at now.

**Origin.** The read path's buffer pool, 2026-09-03
([`../status/history/2026-09-03.md`](../status/history/2026-09-03.md), "7.13:
the read path's allocation, and the seam that earned 7.13.1"). Contingent on
the pool surviving `7.13.1`, which reworks who copies the chunk but not who
allocates it.

## Parallel *discovery* has ~10% to win even on the fastest disk here

**Fact.** Cold on a 970 EVO Plus — the fastest device this project owns — a
single-threaded whole-file `pgdq parse` of a `COPY` dump costs **1.10×** the
time `dd` takes to read the same bytes, and a large-object region 1.20×. One
core already saturates that device on the discovery path. The `INSERT` run is
the exception at **2.66×**
([`measurements.md`](measurements.md), "Scan throughput by input shape", the
cold-NVMe table).

**Why this phase cares.** It says where the workers go. The HDD reading already
said parallel discovery wins nothing on rotational media; what was open was
whether a fast device changes that, and it does not — the whole prize for
parallelising `parse` on NVMe is the **0.076 s** by which a 1.314 s scan
exceeds its floor, and a splitter's own coordination has to come out of that.
That gap has narrowed since this entry was filed, where it read 0.125 s of
1.406 s, because the read path got faster by more than the device did: the
argument is stronger now, not weaker. Row extraction is the opposite case at
**10.9×** the warm floor untyped and 15.0× typed, having been 13.3× and 31×
when the entry was written. So a parallel scheme that speeds up discovery and not extraction is
measurable only in the regime where nothing needed speeding up. It also
sharpens the `INSERT` entry above: that shape *is* where cores would pay on a
fast device, and it is the one region kind speculative splitting cannot touch.

**Origin.** The cold-NVMe figure, 2026-09-04
([`../status/history/2026-09-04.md`](../status/history/2026-09-04.md), "The NVMe
is where the `INSERT` path stops being device-bound"). Contingent on the
device: a faster disk than this one would reopen it, and none is available here
to measure on.

---

## A read buffer that misses the pool costs 1.83× a warm scan

**Fact.** `io::BufferPool` holds **four** slots, keeps nothing above 8 MiB
except a length a read loop announced through
`ByteRangeSource::hint_read_size`, and falls back to `vec![0u8; len]` for a
read it has nothing for — which is the `calloc` the buffer pool was landed to
remove. The chunk-size sweep priced a miss directly, back when a 16 MiB chunk
was one: **0.866 s warm against 0.472 s** at 8 MiB on the same file and the
same binary ([`measurements.md`](measurements.md), "What the read chunk size is
worth"). The pool is per-`LocalFileSource`, and its bound was chosen against a
scan that holds one chunk at a time plus the query path's retained chunks.

**Why this phase cares.** Parallel extraction raises the number of chunks in
flight against one source, which is exactly what the four slots were sized
against. This is the first measurement that says what a miss costs, so "does
the pool need to scale with the worker count" is a question with a number
behind it rather than a guess — and it is a number large enough that getting it
wrong would eat a meaningful share of what parallelism buys.

**Origin.** 2026-09-04, the I/O-defaults reading; the announced read length and
the current price are from 2026-09-05
([`../status/history/2026-09-05.md`](../status/history/2026-09-05.md), "`M55`:
the pool keeps the chunk size the caller asked for").

---

## Coverage is left prefix-shaped, and the assumption was hardened rather than relaxed

**Fact.** The scan-performance work did **not** rework `splice`'s seam rule.
`stream::splice` still
rebuilds `DumpIndex::spans` as `prefix ++ built ++ [Unscanned tail]` and still
closes the seam by extending the last prefix span, exactly as the first entry in
this file describes; what changed is only *how often* it runs, by putting it
behind the save throttle's gate ([`architecture.md`](architecture.md), "`parse`
resumes, and saves as it goes"). Three things about the assumption are now more
sharply stated than they were, and all three bind a splitter:

- **A contribution's unit is a whole block, never a byte range.**
  `map::Builder::snapshot` `debug_assert!`s `Mode::Idle`, so spans can be taken
  off a `Builder` only between blocks. That coupling is why the gate could not
  be worked around locally, and it is why a worker that has scanned an interior
  byte range has no way to hand its spans back through today's interface — the
  obstacle is `snapshot` and `splice`, not the span list, which can express a
  hole perfectly well.
- **How a map was assembled must not be visible in it**, and it is a test:
  `full.spans == eager.spans` in `pgdump_query/tests/query_cache.rs`, span for
  span and census field for census field. The concrete trap is blank-line
  attribution — a `Builder` starting partway through a file opens its first span
  at its first *recognized content*, which is after its start byte whenever
  blank lines separate the two, and `splice` repairs that by extending the
  **preceding** span. A start floor on `Builder` was tried and rejected because
  it hands that blank line to the following span instead. N workers merging
  pairwise must apply the same repair at **every** seam, not only at the one
  seam a prefix has.
- **The rework and `KD5` are one change, not two.** `KD5`'s remainder — keeping
  the frontier's spans appendable rather than rebuilt — is already owned by this
  phase, and the rejected-alternative paragraph beside the mechanism refuses
  it here precisely
  because it is what a parallel splitter wants anyway. So "make spans
  appendable" and "let coverage have interior holes" are the same piece of work,
  and costing them separately will double-count.

**Why this phase cares.** This is the written statement the first entry says
that work
owes, and it is the half that was a decision rather than a discovery: coverage
is prefix-shaped by choice, so this phase inherits the
assumption whole and pays for relaxing it. The practical consequence is that a
splitter cannot be built on top of `splice` — it either constrains workers to
produce a prefix (finish block *k* before block *k+1* is published, which throws
away most of what out-of-order scanning buys) or it lands the appendable-spans
rework first and treats `KD5` as discharged by the same change.

**Origin.** The splice gate and the standing scope decision, restated
2026-09-05 ([`architecture.md`](architecture.md), "`parse` resumes, and saves
as it goes", whose `KD5` paragraph carries the rejected alternative). Contingent
on nothing: the scan-performance work has finished and left `splice`'s seam rule
as it found it.

---

## The census may be accumulated over any unit; what parallel splitting threatens is its totality

**Fact.** `ArrayShape::merge` is min-of-mins, max-of-maxes and OR of the
`[lb:ub]=` flag, with `ArrayShape::default()` as its identity — a bounded
semilattice, so the combine is **commutative, associative and idempotent**. And
`index::union_census` folds it length-tolerantly: it resizes to the longest
vector it has seen and a shorter one contributes only over its own prefix, a
missing entry reading as the default. `map::Builder::on_row` is likewise
stateless per row — one `memchr2` pre-filter for `{` or `[`, then a field split
and `observe` — so it carries nothing across rows.

Taken together: **any partition of a block's rows, censused in any order on any
number of workers and folded with `merge`, gives bit-identical results to the
single-threaded pass.** The accumulation unit is free. The one thing that is not
free is that a headered block's vector is pre-sized from the header and a
header-less block's grows by field index, so two workers on different row ranges
of a header-less block legitimately return vectors of different lengths — which
the fold already handles and a hand-rolled merge would not.

**Why this phase cares.** The obvious worry is the wrong one. The merge is safe
to parallelize; what a splitter actually breaks is the invariant that **a mapped
block always carries a *total* census** — today `scanned_through` advances only
at a `CopyEnd` watermark, so a block that reached the map was walked end to end
and there is no half-censused block for a later pass to repair
([`architecture.md`](architecture.md), "The array shape census"). A worker
owning rows *[a, b)* of a block holds a partial one, and a partial census is not
a *weaker* answer than none — it is a **wrong** one. A column holding `{1,2}`
and `{{1,2},{3,4}}` records `(1, 2)` and degrades to `Utf8View`; a worker that
saw only the 2-D rows records `(2, 2)`, which resolves to `List<List<T>>` and
then hard-fails on every 1-D value in the half it never read
([`architecture.md`](architecture.md), "What the census decides, and who may
believe it"). That is exactly the confidently-wrong schema the both-bounds
design exists to prevent, reintroduced by the split. So this phase must either
publish a block's census only once every range of it has been folded in, or make
partiality representable and keep `resolve_columns` from ever being handed one —
and "the census is a per-block accumulation" is the wrong reason to hesitate,
because it is not.

**Origin.** 2026-09-05, reading the census against
`pgdump_query/src/index.rs` (`ArrayShape::merge`, `union_census`) and
`pgdump_query/src/map.rs` (`Builder::on_row`) rather than against the prose.
Contingent on the merge staying a semilattice: an `ArrayShape` field that is
order-sensitive — a first-seen or last-seen value, a count — would falsify the
whole entry, and nothing today is one.

## The per-block cost is in memory as well as in time, and nobody had measured it

**Fact.** Peak resident set for a `parse` is flat in dump *bytes* — 1535× the
bytes of a one-block dump moves it by less than the readings' own spread — and
not flat in *blocks*: ~7.7 KB a block at 500 and ~9.9 KB at 4,000, so a
4,000-block scan sits at **43.6 MiB** where a one-block scan sits at 5.9 MiB
([`measurements.md`](measurements.md), "What a scan holds resident, per byte and
per block"). Three mechanisms could produce it and that figure separates none of
them: the span list itself, `stream::splice`'s whole-list clone, and glibc
returning little of what a churn of whole-list clones frees. It is `KD14`,
unowned.

**Why this phase cares.** `KD5` — the whole-list rebuild — is **owned by this
phase**, and until now it was priced only in time (19.1 s for 4,000 blocks under
`--dqcache none`, flattened 184× by the save throttle's gate). The rejected
"appendable spans" fix is argued in that section as structurally right but not
worth a rework of a tested core path for a series the gate has already
flattened; that argument was made against the *time* series alone. If the
memory growth turns out to be the same clone, the fix buys two things rather
than one and the balance moves — and if it does not, this phase acquires a
second per-block cost it has to budget for, because a splitter running *N*
workers multiplies whatever a scan holds. Either way the attribution is a
measurement this phase should take before it decides, and it is cheap: the same
figure re-run over `query --dqcache none` (never saves, rebuilds per block) and
over `query-nomatch` (maps, never saves the cache) separates the clone from the
cache in two readings.

**Origin.** 2026-09-05, the figure that re-homed the flat-RSS claim out
of the koji section, where the per-block axis could not be tested at all (74
blocks over 784 GB)
([`../status/history/2026-09-05.md`](../status/history/2026-09-05.md), "The RSS
row was resting a design claim on the input least able to test it"). Contingent
on the save throttle's gate: the reading was
taken under `parse` with a real cache, so it is the *gated* count of splices,
which the entry above says is 5 for this input.

---

## The xz decode pool belongs to `xz-seek`; the pieces are what this phase probably wants

**Fact.** `xz-seek` will ship both a worker pool that hands back ordered bytes
over a caller-named range, and the public pieces underneath it — a `Send`,
self-contained single-block decode plus the arithmetic that partitions a range
into blocks. Both, because this project asked for both. Three constraints were
attached to the pool and are commitments the crate is building to: an explicit
worker count with no core-count default (the right count is a property of the
device and the build, and the device-awareness rule is *this* phase's to write);
cancellability mid-range, because `parse` banks its cache on a signal and a
non-interruptible bulk read would make Ctrl-C unbounded; and one block per unit
of work drawn from a shared queue rather than a contiguous run per worker.

The reason given for wanting the pieces is a claim about **this phase's shape**,
and it should be tested rather than inherited: that the natural composition over
a compressed source is one worker per xz block that **decodes and parses in the
same thread**, so a decoded 24 MiB never crosses a channel, is never held behind
an ordering constraint, and is dropped as soon as its rows are batched — leaving
memory at *N × one block* rather than *N × one block plus a reorder buffer plus
whatever the consumer has not drained*.

**Why this phase cares.** It decides whether this phase writes a scheduler for
xz blocks at all. If the fused shape holds, the pool is not used here and the
pieces are the interface; if it does not, the pool is, and the worker count
becomes one of this phase's device-class numbers. Note the two are not
symmetric in cost: taking the pool is free, and taking the pieces means this
phase owns block scheduling on top of the row scheduling it already owns.

Two facts already in this file bear directly on the fused claim and should be
read with it: `stream::splice` requires coverage to stay prefix-shaped, and a
worker holding rows *[a, b)* of a block holds a **partial census**, which is a
wrong answer rather than a weak one.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to"). Contingent on that
crate's `P3` landing as specified; the answers were given, not yet built
against.

---

## The memory bound this phase must promise is in bytes, and the crate will take it in bytes

**Fact.** `xz-seek`'s parallel path will accept **both** a worker count and a
byte budget, whichever binds first, because this project asked for both and said
the byte budget is the one it would set. The reason is that koji runs happen in
a **512 MB cgroup** (`CLAUDE.md`'s long-running-process rule, asserted by
`scripts/test_measure.py`) and a worker count cannot be promised to one: the
same count is 192 MiB against a 24 MiB-block file and 1 GiB against a 128 MiB-block
one, and this project does not choose which file its user hands it. The crate
was also asked to make the **derived worker count observable before the read
starts**, and to account in-flight compressed buffers inside the budget.

**Why this phase cares.** This is the first number this phase has to promise
that is not about time. Whatever it does with plain input, N workers over a
compressed source multiply a decoded block, and the budget is where that lands.
It also sets a precedent the plain path will be measured against: the same
question — what does a parallel scan hold resident, and can it be stated as a
number a cgroup accepts — has no answer today for either source shape. `KD14`
(per-block RSS growth, unowned) is the nearest thing and it is an attribution
that was never taken.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to").

---

## The decode pool suits discovery and not extraction, and the rates say so before any of it is built

**Fact.** One core decodes this corpus's `.xz` at ~446 MB/s of plaintext (a
probe, not a figure). This project's own warm per-thread rates over the 3.00 GiB
single-block control file are ~7.0 GB/s for structure discovery, ~940 MB/s for
string-schema extraction and ~680 MB/s for typed extraction
([`measurements.md`](measurements.md), "Scan throughput by input shape" and "A
typed query over nested columns costs 6.5 µs a row more than a string one"). So
one consumer thread absorbs **~16** decode workers on the discovery path and
**~1.5–2** on the extraction path, while a 512 MB budget admits 19 workers
against 24 MiB blocks and 3 against 128 MiB ones.

**Why this phase cares.** It settles, arithmetically and before anything is
written, which of the two compositions belongs where. Ordered bulk delivery from
`xz-seek`'s pool is well matched to `parse`, where one consumer can keep 16
decoders busy. It is badly matched to `query`, where the consumer is
*permanently* the bottleneck and every extra worker is stalled on it — which is
the same conclusion the fused decode-and-parse shape reaches from the memory
side, now with a rate behind it rather than an intuition. Two consequences
follow: this phase should not expect the pool to speed up extraction at all, and
a delivery-buffer slack parameter (offered by that crate, declined here in
favour of an exactly-N footprint) would buy nothing in the mode where it is
cheap and nothing in the mode where it is not.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling, round two
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to"). Contingent on the decode probe,
which is not a registered figure — re-take it before resting a decision on the
ratio rather than on its order of magnitude.

---

## Whole-block decode makes the retained unit a block, not a chunk

**Fact.** The single-block decode `xz-seek` will expose fills a
**caller-supplied buffer with the block's entire uncompressed output**, check
already verified — asked for in that shape because a returned `Vec<u8>` is the
per-chunk `calloc` the buffer pool exists to remove. The consequence is that a
worker's decoded buffer is 24 MiB, or 128 MiB on a large-block file, where the
read path's buffer is 1 MiB. The zero-copy `Utf8View` path pins whatever buffer
a batch views, and `RowBatcher`'s `max_source_span` cap (64 MiB) was chosen
against chunk-sized pinning, on the arithmetic that pinned bytes never exceed
the span rounded out to chunk boundaries
([`architecture.md`](architecture.md), "Three flush triggers, and only one of
them bounds memory"). Rounded out to *block* boundaries, that bound is 24–128×
larger, and a 64 MiB span cap cannot even round out to one 128 MiB block.

**Why this phase cares.** It is a second per-worker memory term the byte budget
does not count: `xz-seek`'s budget counts the decode slot, not what this
project's batches go on to pin out of it. Two remedies exist and both are this
phase's to choose — lower the span cap to a chunk-shaped number and accept more
flushes, or copy on the extraction path and give up the zero-copy view for
compressed sources. The incremental decode form, which would let a worker fill
chunk-sized pooled buffers instead, was deliberately **not** asked for: it
forfeits verify-before-return, which is the property that keeps unverified bytes
out of the cache.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling, round two
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to").

---

## The compressed source as built is one reader behind one mutex, and the vendored copy predates the pool

**Fact.** `io::XzSource` holds a single `xz_seek::Reader` — one live decode and
one current uncompressed position — behind a `std::sync::Mutex`, because the
reader's positioned read takes `&mut self` where `ByteRangeSource`'s methods
take `&self`. Concurrent `read_range` calls therefore *serialize*; nothing calls
concurrently today because every read loop in this crate is sequential. And the
copy of `xz-seek` this repo builds against is a **frozen snapshot at
`54c7983`**, taken deliberately before that crate's parallel-block-decode work
started moving its source — so the pool, the pieces and `SeekTable::blocks_in`
described in the entries above are *not* in this tree. `scripts/vendor_xz_seek.py`
is what re-takes the snapshot, and `vendor/xz-seek/VENDORED_FROM` records which
commit is in.

**Why this phase cares.** Two things it might have assumed are false. Parallel
decode is not a matter of configuring `XzSource` or raising a worker count: the
single-reader-behind-a-mutex scheme is what this phase replaces, and until it
does, a compressed source is a serialization point in front of however many
workers sit above it. And this phase is the **first consumer that needs
post-`54c7983` upstream work**, so re-syncing the vendored copy — and deciding
whether the copy is still the right arrangement, since publication was gated on
this repo and P10 both landing — is part of this phase's own opening moves
rather than something already done for it.

**Origin.** P13's implementation and its wrap audit, 2026-09-06
([`architecture.md`](architecture.md), "The compressed source";
[`roadmap-P13-compressed-input-notes.md`](roadmap-P13-compressed-input-notes.md),
"The vendor copy was deliberately not re-synced"). Contingent on nothing in this
tree; re-check `vendor/xz-seek/VENDORED_FROM` before assuming which snapshot is
in.

---

## Ordered delivery was asked for, and the unordered case was routed to the pieces on purpose

**Fact.** `xz-seek` offered out-of-order delivery — each chunk tagged with its
uncompressed offset, no reorder buffer, materially cheaper for it to build. It
was refused, on two grounds. Today's consumer cannot take it at all:
`CopyScanner` carries partial lines across chunk seams, the map is built in
offset order, and `stream::splice` rebuilds coverage as a contiguous prefix. And
a tagged contract would not remove the reorder buffer, only move it into this
project, where it would be a `BTreeMap` of decoded blocks — the same memory,
two implementations.

The second half is the part addressed to this phase: when this phase wants
unordered work it will not want a byte stream at all, so the tagged form was
declined as a *delivery mode* and the same need routed to the pieces above. The
crate is therefore not building two delivery contracts, and asking it to later
is asking it to reverse a decision this project made on this phase's behalf.

**Why this phase cares.** It closes off an option that would otherwise look
free when this phase is grilled — "ask the crate for tagged chunks" — and says
where the equivalent capability actually lives. If the fused decode-and-parse
shape does not survive this phase's grilling, this is the entry to re-open,
because the pieces were the whole reason for declining.

**Origin.** 2026-09-06, answering `xz-seek`'s `P3` grilling
([`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"What `xz-seek` was told, and what it commits us to").
