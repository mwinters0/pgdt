# Scan Performance

High performance on local files is a core project goal, not a later
optimization pass — see "Project goals" in `docs/design/roadmap.md`. The
target is that the local-file path stays **device-bound rather than
CPU-bound**, at flat memory, across hardware from rotational disk to NVMe.

This doc is the spec for roadmap P7. It states what the phase does and why,
decided against the measured baseline below rather than against the shape the
work was first sketched in.

## Baseline: where the time actually goes

Two inputs answer this, and they answer different halves of it.

**koji** (784 GB, HDD, whole-file `pgdq parse`): **~241–243 MB/s sustained at
~33% of one core**, RSS flat at ~9 MiB, across two independent runs agreeing to
within 1%. That is device-bound — the remaining 67% of the core is waiting on
the disk, which is why **no overlap, prefetch or parallelism scheme wins
anything on rotational media**. It is a regression check, not a target.

**The 3.00 GiB synthetic control** (16 scalar columns) is where the device
stops being the excuse. Every figure below is at the `ba2fc12` stamp,
`measurements.md`, 512 MB container, glibc:

| Warm, on tmpfs | Wall | × the `dd` floor |
|---|---|---|
| `dd` → `/dev/null` | 0.332 s | — |
| `pgdq parse` — structure discovery | 0.558 s | **1.68×** |
| `pgdq query --schema-mode strings` — row extraction, zero-copy | 4.42 s | **13.3×** |
| `pgdq query --schema-mode typed` | 10.36 s | **31×** |

Cold on the SATA SSD the floor is 5.75 s and `parse` is 5.78 s — **1.01×**, so
the device hides the parse entirely, and it hides a `strings` query too.

**What that leaves for a parser change to win on `parse`.** Of the 0.53 s a
census-off whole-file `parse` takes warm, 0.19 s is user and 0.33 s is system —
and `dd` over the same file in the same container is 0.33 s, all of it system.
So the scan's kernel time is exactly the cost of handing the bytes over, and
everything the `COPY` grammar, the map and the census do together is **36% of
its own elapsed time**. Deleting all of it would take 0.53 s to 0.33 s.

**The reading that sets this phase's priority.** Discovery is already within
0.23 s of the floor warm and inside the noise cold, so the whole prize there is
a quarter-second per 3 GiB. Row extraction is 13.3× the warm floor before a
single column is typed, and 31× with them. **An order of magnitude of the
project's device-bound goal is in the row path, and a rounding error is in the
discovery path.**

## What this phase targets

**The row-extraction path**, on the two paths a user actually runs: `query`
in `strings` mode and in `typed` mode.

Discovery keeps exactly three items, and each is here on its own evidence
rather than because discovery is where the time goes:

- **`KD9`** — an `INSERT` run costs mid-teens times a `COPY` block's per-byte
  CPU (9.19 s against 0.558 s warm, 27.7× the warm floor), because every line
  is decoded into an `Event::Line` and pushed through the statement
  accumulator. A koji-scale `--inserts` dump spends ~48 minutes of CPU where a
  `COPY` dump of the same size spends ~3.
- **`KD5`** — mapping is O(blocks²): every `CopyEnd` rebuilds
  `DumpIndex::spans` whole, 19.0 s of a 20.8 s 4000-block `parse`.
- **The I/O defaults** — readahead, chunk size, `posix_fadvise` — which belong
  to whichever pass reads bytes and are measured on their own device class
  (below).

## What ends this phase, and what it deliberately does not commit to

**No numeric target.** The phase's two goals are to take the obvious, cheap
optimizations and to come out of it with a **solid understanding of where the
time goes** — so that a later phase, if one wants a specific target, can set
one against evidence rather than against a guess. A threshold picked before the
decomposition exists would either be met by the first cheap win or be
unreachable, and neither teaches anything.

So the phase is done when both hold:

- **Every lever below is measured, and every obvious or cheap win is taken.**
  Measurement is unconditional; landing is conditional on the measured prize
  being worth what the change costs. A null or negative reading is a result,
  and the instrument's own floor is part of the deliverable (`roadmap.md`, "A
  slice row that commits to a measurement names its instrument").
- **The cost decomposition of the `strings` and `typed` paths is published**,
  as a section of `architecture.md` describing where a scan's time goes,
  carrying whatever tables `measurements.md` published to support it. That is
  the durable half: the levers may each turn out to be worth little, and the
  decomposition is worth having either way.

**No lever is excluded from measurement by its implementation cost** — only
from landing. The distinction matters most for the viewing builder, which is
the largest single prize on the list and also the most delicate change on it:
it is measured like everything else, and it lands only if the profile confirms
the prize and it can be done as its own final slice with its own review.

## The phase follows the evidence, not this document's order

Nothing here is a waterfall. The lever list below is what the phase is
committed to *measuring*; the order it takes them in, and which of them turn
into code, follow what the profile says — including following a hunch that the
profile then settles. Two consequences worth stating, because they are what
distinguishes this from a phase running late:

- **A lever row is satisfied by a rejection.** "Measured, not worth it, here is
  the reading and the instrument's floor" is a delivered row, not a skipped
  one.
- **Where this document names a hypothesis rather than a decision, it says
  so**, and the phase settles it with evidence rather than inheriting it. The
  `INSERT` fast path's placement is the worked example.

## How this phase measures: profiles first, figures rarely

A whole-file figure costs the machine, and this machine has other work.
**A sampling profile is the primary instrument**: it runs in seconds, it
attributes cost per function rather than per subtraction, and it needs no quiet
machine to be worth reading, because the answer it gives is a proportion.

- **A profile is not a figure.** It is a `runs/` artifact — no medians, no
  apparatus gate, no `measurements.md` marker. What reaches `measurements.md`
  is a table, and a table is taken only when a landed lever needs a
  before-and-after.
- **The decomposition deliverable is profile-derived.** Subtraction across
  projection widths and the census-off binary remains available and is used
  where a profile cannot answer — the device-versus-CPU split most of all —
  but it is the fallback, not the method.
- **Deterministic instruction counting is available and deliberately not the
  default.** `valgrind`/`callgrind` (or `iai-callgrind` as a dev-dependency)
  would give per-function counts immune to machine state, which is exactly what
  a shared machine wants. It is rejected as this phase's primary instrument
  because several of the levers — the allocator, zero-copy views, chunk sizing
  — change *memory behaviour* rather than instruction count, and an instrument
  that cannot see them would report the phase's headline work as free. Reach
  for it if a lever turns out to be instruction-bound.
- **The tool is `perf`**, which needs no change to this machine's
  `perf_event_paranoid = 2`: that level permits user-space sampling of one's
  own processes, and user space is what this phase is about. `samply` is the
  alternative if a richer reader is wanted, and costs a sysctl; `7.1` may take
  it and record the change in `CLAUDE.local.md`.
- **The build carries symbols without disturbing the measured one.**
  `[profile.profiling]` inherits `release` and adds line tables and frame
  pointers; `release` — what every published figure is taken with — is
  unchanged.
- **Profiles run on the host, against tmpfs input, outside the 512 MB
  container.** A profile is about proportions, and the container adds
  capability plumbing without changing them.

**The phase's whole long-run budget, stated up front**, because otherwise these
arrive one at a time as "just this once":

| Long run | When | Why it is owed |
|---|---|---|
| A full `--all` sweep **pair** | once, at the wrap, detached | the phase moves nearly every one of the thirteen tables, and there is no mechanical oracle to acknowledge library changes with — `--verify-additive` settles generator changes only |
| One koji glibc scan | once, at the wrap, detached | the byte-identity regression check over a phase that reworks the scanner, and the run that makes the musl-era koji figures comparable again |
| A `--figure` re-take | as a lever lands | the before-and-after that lever publishes |

**The `INSERT` +10% attribution is a differential profile, not a bisect.**
Profiling `insert_run.sql` at `b70589f` and at `ba2fc12` and diffing by
function answers the same question in seconds — and if the two profiles are
indistinguishable, that is the answer, since the cost is then not in user-space
and no bisect over those commits was going to find it.

## The levers, and what each is worth before it is touched

Every row is measured; a row lands when its measured prize is worth what the
change costs. The stakes are the `ba2fc12` figures, and they are what the
profile is read against.

| Lever | Measured stake |
|---|---|
| **Allocator** — glibc against `jemalloc` and `mimalloc`, chosen in the CLI | 1.8–2.4× between two stock libcs on everything that moves real bytes |
| **`INSERT`-run fast path** (`KD9`) | 16.5× a `COPY` block's per-byte CPU; ~48 CPU-minutes per TB against ~3 |
| **The map's per-`CopyEnd` rebuild** (`KD5`) | 19.0 s of a 20.8 s 4000-block `parse` |
| **Bulk `simdutf8`** in place of per-field `std::str::from_utf8` | 16 validation calls per row of the control today, one per borrowed field |
| **One field split per row**, shared by the predicate's terms, `push_row` and the decoders | a five-way disjunction walks the row five times; the census re-splits every brace-bearing row |
| **Readahead, `fadvise`, chunk-size defaults** | ≤38% of `parse` wall — NVMe only, zero elsewhere |
| **`decode_array`'s `Vec<Option<String>>` intermediate** | 6.2 µs/row of the arrays file's 13.5, paid before the Arrow build is reached |
| **Scalar decode and the typed column build** | 7.3 µs of a 12.7 µs typed row on the control — 57% of a typed read, over sixteen ordinary columns with no nesting |
| **A viewing builder for `List<Utf8View>`** | ~7.3 µs/row — the Arrow build share, the largest single prize on the list |
| **The mapping pass's double read** | a cold query reads the target block twice, the first read far cheaper per byte |
| **`attach_text`'s O(blocks × DDL) re-slice** | nothing on koji (~200 × 154 KB against an hour); unbounded in principle on a block-rich dump |

**The scalar row is the largest bucket and the newest to the list.** Every
other lever targets the shared row machinery or the nested path; this one is
where the time goes for the shape most dumps actually have. The profile splits
it in two, because the halves have different remedies: a per-type cost in
`decode.rs` is a parser problem, and a builder-append cost is the same family
of fix as the viewing builder.

**A lever the profile finds and this table does not name is admitted**, if it
is obvious and cheap — by amending this table, with the reasoning in a history
entry, which is what "the spec changes when a decision changes" is for. Filing
every unlisted finding forward would read as discipline and would really be a
way of not acting on the evidence the phase exists to gather.

The last three are the ones whose implementation would be expensive: the
viewing builder honours three sharp edges recursively at every level of `List`
and `Struct` nesting, eliminating the double read means carrying the live
segment's in-flight spans in the opaque `ResumeToken`, and `attach_text` is
correct as written rather than lazy. They are measured on the same terms as
the rest, and each lands only if the measurement says the prize is real and the
change stays contained.

## Where the `INSERT` fast path lives is for the profile to settle

`KD9`'s inbox entry called the fix "a second scanner-level fast path — the same
mechanism `State::InLargeObjectRegion` already is". That names a layer the
evidence does not yet support, so it is carried here as a **hypothesis with two
candidates**, and the profile decides.

**The hypothesis.** The scanner emits `Event::Line` cheaply; the expensive part
is `map.rs`'s `Mode::InsertRun`, where `push_stmt_line` pushes every line into
a `String` and `statement_complete` re-walks it, per statement, per row. The
supporting reading is the large-object region: skipping lines *unread* in the
scanner costs 0.442 s per 3.00 GiB against `COPY`'s 0.558, so a path that still
emits lines but stops accumulating them should land in that band — which would
be nearly the whole 8.6 s gap, for a diff contained to L2.

**The two candidates.** In `map.rs`: find each statement's end by scanning the
raw line bytes quote-aware, which is what `in_open_quote`/`statement_complete`
already express, without the copy. In `scan.rs`: a third region state skipping
lines unread. The second is the larger claim — the scanner cannot know a run
has begun without `parse_insert_target`, which is statement classification and
therefore L2's, and unlike `BEGIN;`/`COMMIT;` (I12) an `INSERT` run's
boundaries rest on no line-anchored invariant.

Whichever the profile picks, `KD9`'s entry is corrected in the same change, and
the quote-aware statement-end primitive is built to be reusable: P8 Track A's
row reader needs the same one.

## Discharging `KD5`: the gate the throttle already owns

`stream::splice` moves inside `if settled || cancelled || throttle.due()`,
firing a few dozen times per scan instead of once per block — roughly 19.0 s to
1 s at 4000 blocks, on machinery that already exists.

**This is a deliberate change to what an interrupt promises, and it is
quantified.** Today an interrupt banks the last completed block, because
`index` is spliced at every `CopyEnd` and the chunk-top save is therefore
unconditional. Under the gate it banks the last *spliced* watermark. The loss
is bounded by time rather than by block count, since the throttle is
self-tuning against the last save's own duration: at 4000 blocks that is 105
saves across a 20.8 s scan, so **~0.2 s of scanning**. Where blocks are far
apart the gate clears every time — koji's are ~45 s apart with sub-second saves
— so on the shape where a lost block is expensive, **nothing changes at all**.
The degradation is confined to the shape where the lost blocks are small.

The byte-identical-resume property is untouched: the resume point moves, the
structural record it reproduces does not.

*Rejected:* the appendable-spans rework — keeping the frontier's spans
appendable rather than rebuilt. It is a rework of an already-tested core path
for a series the gate has already flattened by 20×, and it is what a parallel
splitter wants anyway, so it goes to P16 if the residual still matters.
`map::Builder::snapshot` `debug_assert!`s `Mode::Idle`, so the chunk-top check
cannot re-derive the spans mid-block; that coupling is why the gate cannot be
worked around locally.

`KD5` is rewritten to whatever residual the measurement leaves, in the change
that lands this.

## The slices

**7.1 and 7.2 are ordered; everything after them is allocation order, not
schedule.** The evidence slices go first because they de-risk every row of the
lever table while it is still cheap to change, and after them the phase follows
the profile — so a slice landing out of numeric order is the plan working, not
the plan slipping. The one ordering that does bind is `7.3`: an allocator
adopted after a figure is taken invalidates that figure.

| Slice | What it delivers |
|---|---|
| **7.1** | **The profiling apparatus.** `[profile.profiling]` inheriting `release` with line tables and frame pointers, the tool installed and pinned, and the invocation owned by `scripts/measure.py` — a `--profile-recipe` that prints the command with paths filled in, on the `--koji-recipe` precedent, and never runs it. First profiles of `parse`, `strings` and `typed` over the control and `--arrays --composite` files. No library code. |
| **7.2** | **The decomposition, published.** `architecture.md` gains "where a scan's time goes"; the `INSERT` +10% differential profile settles that attribution; the `INSERT` fast path's layer is chosen; the three measure-only levers (`attach_text`, the double read, the census) get their readings. No library code. |
| **7.3** | **The allocator.** glibc against `jemalloc` and `mimalloc` on the three headline figures; adopted in `pgdump_query-cli` if it wins, never in the library; the apparatus line names it. |
| **7.4** | **`KD5`** — `stream::splice` moves inside the throttle's gate, the interrupt's promise is restated in `architecture.md`, and the entry is rewritten to whatever residual the measurement leaves. |
| **7.5** | **`KD9`** — the `INSERT` fast path at the layer 7.2 chose, with the quote-aware statement-end primitive built to be reusable by P8 Track A's row reader. |
| **7.6** | **Bulk `simdutf8`** over the chunk's largest whole-row prefix, with `decode_field` gaining the unchecked borrow path for a field with no escapes. |
| **7.7** | **One field split per row**, shared by the predicate's terms, `push_row`, the census and the decoders. Its own review: it is a rework of an already-tested core path, and it crosses layers, so [`layering.md`](layering.md) is read before it is placed. |
| **7.8** | **The I/O defaults** — the cold-NVMe figure, then readahead, `posix_fadvise` and the chunk-size constant, each landed or rejected against it. |
| **7.9** | **`decode_array`'s `Vec<Option<String>>`** intermediate, replaced by borrowed slices where the literal carries no escapes. |
| **7.10** | **Scalar decode and the typed column build**, split by the profile into a `decode.rs` half and a builder-append half. |
| **7.11** | **The viewing builder for `List<Utf8View>`** — conditional on 7.2 confirming the prize, last, and reviewed alone. |
| **7.12** | **The sweep pair and the koji regression run**, folded in: thirteen tables re-taken in one sitting, koji's byte-identity check on a glibc build, and the written statement of what a parallel splitter needs from coverage and from the census, filed to P16. |

## What this phase is not: parallelism is P16

Parallelism is carved out into its own phase (`roadmap.md`, "P16 — Parallel
scan and extraction") and this phase stays **single-threaded throughout**.

The reason is that parallelism multiplies a loop rather than fixing one, and
the table above says the loop is what is wrong: at 13.3× the device floor per
core, threads buy a constant where the per-byte cost buys an order of
magnitude. Splitting also keeps this phase's diffs off the three mechanisms a
splitter has to rework — `stream::splice`'s assumption that coverage is a
contiguous prefix, the array-shape census's per-block accumulation, and the
interrupt guard's "bank the last completed block" promise — which is the
`process.md` seam against putting a self-contained change and a rework of an
already-tested core path in one review cycle.

**Two things this phase owes P16**, both of which belong here anyway:

- **The sparse row index**, which turns a speculative split into a real one:
  known row boundaries mean no resync scan and no guessing whether an offset is
  inside a block.
- **A written statement of what a splitter needs** from coverage and from the
  census — whether coverage stays prefix-shaped or `splice`'s seam rule is
  reworked, and what unit the census can be accumulated over. The P7 inbox
  filed both as decisions this phase must not leave as discoveries.

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

**The skip is not free today, and what it costs is two consumers.** A needle
search jumps over bytes without looking at them, and two things currently look
at every one of those rows: `CopyEnd::row_count`, which the cache stores and
`info` reports, and the array-shape census, which `map::Builder::on_row`
records from `Event::Row` for every mapping pass and which a query reads back
to retype its array columns before the first batch. Counting rows without
enumerating them is available — `memchr::count` over the block extent is the
same SIMD pass — but the census is not: it needs the bytes. So adopting the
needle search means deciding what happens to the census, and the prize is the
quarter-second per 3 GiB the baseline table puts on discovery.

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

- A batch then pins its whole source chunk in memory — and, once a filter is
  aggressive, every chunk it took a view into, in proportion to
  `1/selectivity`. Threshold compaction was the original answer here and is
  **withdrawn**: the fix is a flush trigger on the source byte span a batch
  covers — [`architecture.md`](architecture.md), "Three flush triggers, and
  only one of them bounds memory".
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

## The allocator is chosen in the binary, and measured before anything else

The same source built against two libcs differs by 1.8–2.4× on everything that
moves real bytes, which is larger than everything else on this phase's list put
together. That says allocators matter here; it does not say a replacement beats
glibc. So the phase measures it — glibc against `jemalloc` and `mimalloc`, on
the three headline figures — **early, so that every later figure is taken under
the allocator the phase ships**.

**The choice is `pgdump_query-cli`'s, never the library's.** A
`#[global_allocator]` in a library imposes it on every embedder, which is
precisely P6's audience. The consequence is stated rather than hidden: a figure
in `measurements.md` is a **CLI** figure, taken under the CLI's allocator, and
an embedder inherits whatever their own binary chose. The apparatus line names
the allocator exactly as it already names glibc.

## I/O: keep `pread`, don't mmap

mmap is the intuitive choice here and it is the wrong default.

- It bypasses `ByteRangeSource`, so it cannot be the path for P6's
  `object_store` backend — adopting it means maintaining two readers.
- Page faults on a 784GB file on slow media are synchronous and
  uninterruptible, with no way to bound prefetch depth or time out.
- I/O errors arrive as `SIGBUS`, not `Result`. For a tool whose whole premise
  is "read a file bigger than memory," turning read failures into signals is
  a bad trade.

The current design — positioned reads into a reusable caller-owned buffer —
already delivers the flat ~9 MiB RSS and 243 MB/s, behind an abstraction that
survives into P6. The wins available are within it:

- **Double-buffered readahead**: issue the next chunk's read while parsing the
  current one, so parse time and I/O time overlap instead of alternating.
- **`posix_fadvise(SEQUENTIAL | WILLNEED)`** on the local backend, to let the
  kernel read ahead deeply on the sequential scan.
- **Tuned chunk size**, page-size-aligned: large (1-4 MiB) on rotational media
  to amortize seeks and syscalls, smaller on NVMe. `ScanOptions::chunk_size`
  already exposes this; the work is measuring the right defaults per device
  class rather than adding a knob.

**Overlap's prize is `min(device time, parse time)`, which bounds it to one
device class.** On the HDD the device is the whole cost and the 67% of a core
koji leaves idle is disk wait, not something to overlap into. On the cold SATA
SSD the same arithmetic caps it at 4%, and the cold table reads `parse` at
5.78 s against a 5.75 s floor — the kernel's own readahead is evidently
already doing it. On tmpfs there is no I/O to overlap at all. So a fast NVMe
device is the only place where this work can show anything: ~0.9 s of device
against 0.55 s of `parse` CPU per 3 GiB, which serial is 1.45 s and fully
overlapped is 0.9 s. That is the measurement these three bullets are gated on,
and it is why the campaign measures a third device class at all.

**The chunk-size default is one measured constant, not a runtime probe.**
`ScanOptions::chunk_size` stays tunable and the per-device-class numbers are
published, so an operator can pick; the shipped default is the one that is
worst-case-best across the three classes. *Rejected:* choosing it at runtime
from `/sys/block/<dev>/queue/rotational`. It is Linux-only and it degrades
exactly where this tool runs — `/sys` may be masked inside a container, and on
LVM, dm-crypt, MD, NFS or an overlay, resolving a path to its backing device is
a walk with several ways to be wrong. If the measured spread turns out to be
large enough to matter, adaptivity becomes a decision with evidence behind it.

mmap stays available as a possible local-only fast path, gated on a
measurement showing it beats double-buffered `pread` by enough to justify a
second code path. Not assumed.

## Parallelism: not here

Every parallel scheme — speculative discovery, split-anywhere row extraction,
device-aware worker counts — belongs to P16, for the reasons under "What this
phase is not" above. The property that makes a split correct at all is the raw-LF
one stated under "Row extraction": any byte offset inside a block
resynchronizes to a row boundary by scanning forward to the next LF. That is
why the sparse row index this phase builds is an optimization of a working
scheme rather than its precondition.

## The sparse row index: not here either

The index — the byte offset of every Nth row, at ~19 MB for koji's
19,575,829,920 rows against ~157 GB for a dense 8-byte-per-row one — belongs to
**P16**, the first phase that reads one. Both of its buys are outside this
phase: parallel splits at known row boundaries are P16's, and "seek to a row
range" is a feature nothing exposes — `ResumeToken` already carries a byte
offset and an in-block row count, so resuming a query does not rescan a block
from its start.

*Rejected:* building it here because this phase owns the scanner. Its interval,
serialization and invalidation rules would be guessed against no reader, and
this phase could not measure the benefit against anything — which is exactly
what "every item is a hypothesis until measured" forbids. P10 additionally
needs the checkpoint interval to coincide with the row group its statistics
attach to, and that is P10's decision to make.

What stays here is what already exists: `CopyBlock::sparse_index`, reserved and
always `None`, so adding the index later is not a format break.

## Rules for a figure this phase does take

Profiles are the primary instrument ("How this phase measures" above), so the
tables this phase publishes are few. These are the rules the few obey.
`measurements.md`'s preamble carries the standing ones; what follows is what
this campaign adds.

**Three regimes, and each answers one question.**

- **Warm, on tmpfs** — what the CPU costs. A figure about parsing CPU is taken
  here and **never against a page-cache-warm filesystem**: device time and
  background I/O swamp the difference being measured, and page-cache residency
  is an assumption rather than a guarantee.
- **Cold, on the SATA SSD** — what the device hides. It is the regime in which
  a device floor means anything, and the one that says whether a CPU win is
  visible to a user on ordinary storage.
- **Cold, on NVMe** — the one device class where I/O and parse are within a
  small factor of each other, and therefore the only instrument that can price
  readahead, `fadvise` and chunk-size defaults. Nothing else is measured here:
  it exists for that one decision.

**The HDD stays koji-only and stays a regression check.** A synthetic 3 GiB
file on a rotational disk measures one file's layout, and koji already answers
the only HDD question the design has: the scan is device-bound there, so no
default this phase picks changes anything. *Rejected:* adding an HDD
throughput figure to the sweep — it would cost minutes per sweep to re-confirm
a bound that two 54-minute runs already agree on to within 1%.

**Attribute a cost within one file, not across two.** Cross-file differencing
bottoms out at about **±0.5 µs/row** — established by a seed-43 control that
should read zero and reads +0.01, against a real effect of +0.79 taken the same
way — and no number of reps moves that floor, because everything that actually
moves these numbers is held constant inside a sweep. `--column`/`--no-columns`
answers "what does this one thing cost" over identical rows of one file, with
neither that floor nor a file-dependent untyped baseline, and it is the
instrument this campaign's questions are asked with.

**Per-rep SE is a repeatability check, not an error bar.** Two builds of one
source gave the same comparison as +0.61 µs/row (t = +4.34) and −1.16 µs/row
(t = −4.81). A wide SE means the sweep is broken; a narrow one licenses
nothing. A result whose sign flips between apparatuses is a question the
instrument cannot address, and the answer is a better instrument, not more
reps.

**Name the allocator with the figure.** The same source against two libcs
differs by 1.8–2.4× on everything that moves real bytes, which is larger than
everything on this phase's list put together. Every figure is glibc, in a
glibc image, and says so.

**Anything at koji scale follows `CLAUDE.md`'s long-running-process rules** —
detached, logged under `runs/`, read by a later session, and never waited on.
A full sweep is one of these.

**A figure this phase produces is a harness figure.** `scripts/measure.py`
takes it, `measurements.md` publishes it under an `<!-- figure: -->` marker,
and it declares both edges — what invalidates it, and what it invalidates. A
one-off `runs/` script that scrapes a median out of a log is what this campaign
exists downstream of, not what it produces.

## The three constraints this phase placed on earlier phases

All three were held to, so none of them is work this phase still has to do.
They are recorded because a change to those layers must not undo them.

1. **The batch layer reads chunks into an Arrow `Buffer` and builds `Utf8View`
   arrays as views over it**, rather than copying field bytes into fresh
   allocations. Built: `batch.rs`, with the three sharp edges around that path
   — chunk retention in the deque, `StringViewBuilder` block-index
   invalidation on every flush, and the straddling-field case — described in
   `docs/design/architecture.md`.
2. **The serialized cache leaves room for the optional sparse row index**, so
   adding it is not a format break: `CopyBlock::sparse_index`, reserved and
   always `None`.
3. **`ScanOptions::chunk_size` stays tunable**, and its default is treated as a
   measured value rather than a constant.
