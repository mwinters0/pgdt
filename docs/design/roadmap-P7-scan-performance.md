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

| Warm, on tmpfs | Wall | × the `dd` floor | × the floor, library only |
|---|---|---|---|
| `dd` → `/dev/null` | 0.332 s | — | — |
| `pgdq parse` — structure discovery | 0.558 s | **1.68×** | ~1.7× — essentially all of it |
| `pgdq query --schema-mode strings` — row extraction, zero-copy | 4.42 s | **13.3×** | **~6.1×** |
| `pgdq query --schema-mode typed` | 10.36 s | **31×** | **~10.0×** |

Cold on the SATA SSD the floor is 5.75 s and `parse` is 5.78 s — **1.01×**, so
the device hides the parse entirely, and it hides a `strings` query too.

**The fourth column is what an embedder pays, and it is derived rather than
measured.** The wall column is `pgdq`, and for a `query` two-thirds of a typed
one is the CLI writing the batch back out as TSV
([`architecture.md`](architecture.md), "The library's own per-row budget") — so
the library's own cost is that budget's per-row total, 2.48 µs and 4.06 µs,
over the control's 814,362 rows against this table's own floor. It is a profile
share applied to a figure, not a figure, which is why it is approximate and
carries the ~8% a warm absolute resolves to across sessions
([`measurements.md`](measurements.md), "A move smaller than the apparatus
resolves is not a finding"); both still include the mapping pass, since every
`query` figure and every profile here is `--dqcache none`. `parse` needs no such
correction — the CLI's event callback is 4.2% of its user time and nothing else
of it is the CLI's.

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

**That conclusion is the same in both readings, and only the magnitudes
change.** Strip the CLI and row extraction is ~6.1× the floor untyped and
~10.0× typed against discovery's 1.7× — still an order of magnitude against a
rounding error, and still the same phase. What moves is what may be *quoted*:
the 31× belongs to a `pgdq query` and not to the library, so a claim about what
this phase won is read off the library column or off the per-row budget behind
it.

## What this phase targets

**The row-extraction path**, on the two paths a user actually runs: `query`
in `strings` mode and in `typed` mode.

Discovery keeps exactly four items, and each is here on its own evidence
rather than because discovery is where the time goes:

- **`KD9`** — an `INSERT` run costs mid-teens times a `COPY` block's per-byte
  CPU (9.19 s against 0.558 s warm, 27.7× the warm floor), because every line
  is decoded into an `Event::Line` and pushed through the statement
  accumulator. A koji-scale `--inserts` dump spends ~48 minutes of CPU where a
  `COPY` dump of the same size spends ~3.
- **`KD5`** — mapping is O(blocks²): every `CopyEnd` rebuilds
  `DumpIndex::spans` whole, 19.0 s of a 20.8 s 4000-block `parse`.
- **The I/O defaults** — readahead, chunk size, `posix_fadvise` — three
  tunable constants, measured on their own device class (below). Slice 7.8.
- **The read path's per-chunk zero and copy** — 54.6% of a warm `parse`'s
  *user* time, and the largest single lever on the discovery path. Slices 7.13
  and 7.13.1.
  **Its prize is inside the quarter-second above, not additional to it**: the
  0.158 s it names is most of the 0.20 s that deleting the grammar, the map and
  the census entirely would win, because zeroing and copying the chunk is part
  of what that 36% of elapsed time *is*. So it does not reopen the priority
  reading — a lever can be the biggest one on a path whose whole prize is a
  rounding error, and this is that. It is here rather than under row extraction
  because `parse` is where it dominates; a `strings` query pays ~9% of it.

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
from landing. The distinction matters most for the viewing builder, the most
delicate change on the list: it is measured like everything else, and it lands
only if the measurement 7.11's row names confirms the prize and it can be done
as its own final slice with its own review. It was admitted as the largest
single prize on the list, on a subtraction the decomposition then took apart;
what it is worth is now an open reading rather than a large number, and 7.11's
gate is what is owed on it.

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
- **No library-only figure is taken, and the per-row budget is what stands in
  for one.** Every figure this doc quotes times `pgdq`, so none of them says
  what an embedder pays; the honest instrument would be a figure that stops at
  the batch, and this phase deliberately does not build one — it would need
  either a published bench through push mode, whose only caller is tests, or a
  measurement-only flag in the shipped binary. What it does instead is keep the
  library's cost *legible*: [`architecture.md`](architecture.md), "The library's
  own per-row budget", states it as profile shares, and **a lever that lands
  re-reads that budget in the same change that re-takes its figure**. The
  obligation is what makes the decision safe rather than convenient — a library
  number is the more stable of the two and the one the embedding work will
  actually be asked for
  ([`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md)),
  so it may not be left to be re-derived from a profile nobody re-took.
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
| **One field split per row**, shared by the predicate's terms and `push_row` | the same five-way disjunction against a 16-column table's thirteenth column and against its first differ by **49% of the deep one's user instructions**, and that difference is the walk. The census re-splits every brace-bearing row too, but in the **mapping** pass, so it is not this row's to win — see "What this lever cannot reach" below |
| **Readahead, `fadvise`, chunk-size defaults** | ≤38% of `parse` wall — NVMe only, zero elsewhere |
| **The read path's per-chunk zero and copy** | 54.6% of a warm `parse`'s *user* time on the control, 0.158 s of a 0.60 s scan; ~9% of a warm `strings` query |
| **`decode_array`'s `Vec<Option<String>>` intermediate** | 4.14 µs/row on the arrays file — the micro figure's *decode* column for both array literals (290 ns + 3.85 µs), library-only and paid before the Arrow build is reached |
| **`needs_quote`'s per-byte `force_quote` scan** | 14.1% of a typed `--arrays --composite` run, of which **10.1% is `memchr` inside `slice_contains`** — `force_quote` is a `&'static [u8]`, so every byte of every token costs a linear walk of a 4–6 byte slice. Split 5.28% decode (`scan_token`) and 4.82% render (`push_token`), which are the same predicate reached from two directions |
| **Scalar decode and the typed column build** | 2.61 µs/row of the library's 4.06 on a typed control read — `append_typed` 1.66 and `decode_field` 0.95, which is 2.13 s of the 9.81 s of user time; the builder half is the larger and exceeds the whole typed premium over `strings` |
| **A viewing builder for `List<Utf8View>`** | to be re-derived: the profile puts the whole library batch stream at 33.7% of a typed read, so the Arrow build's share is bounded well below the 7.3 µs/row this row once claimed |
| **The mapping pass's double read** | exactly one extra pass — 2.0000× the file's bytes with `--dqcache none`, 1.0000× with a cache |
| **`attach_text`'s O(blocks × DDL) re-slice** | nothing: zero samples in a 100,000-sample profile of a 4000-block `parse` |

**The scalar row is the largest library bucket and the newest to the list.**
Every other lever targets the shared row machinery or the nested path; this one
is where the time goes for the shape most dumps actually have. The profile
splits it in two, because the halves have different remedies: a per-type cost
in `decode.rs` is a parser problem, and a builder-append cost is the same
family of fix as the viewing builder.

**Four rows aim inside `poll_next`, and the per-row budget says they do not
overlap.** [`architecture.md`](architecture.md), "The library's own per-row
budget", splits a typed control row's 4.06 µs four ways, and each part has
exactly one owner: `append_typed` at 1.66 µs and `decode_field` at 0.95 are
7.10's and 7.10.1's; the field split and row walk inside `push_row`, 0.98 µs, is
**7.7.1's** and not 7.10's; and the 0.47 µs of stream and scan machinery around
them is nobody's row. Stating it matters in one direction in particular — 7.10
was written claiming the whole 3.31 s of `poll_next`, which is 55% more than
its own two halves are worth, and a session landing 7.7.1 afterwards would have
found the split already gone with no record of which row had claimed it. Where
two rows name one function, the order is stated: **7.6 acts on `decode_field`
before 7.10 does**, since the unchecked borrow path for an escape-free field is
a precondition for whatever per-type work is left, not a competitor for the
same microseconds.

### What this lever cannot reach, and what it must measure first

**Owning the `push_row` split is an accounting claim, not a prize.** A query
with no predicate splits each row exactly once, in `push_row`, and every
decoder already takes its field out of *that* split — `RawRow::decode` is
handed the range `push_row` computed, which 7.6 delivered. So the budget's
split-and-walk row is irreducible on that shape: there is nothing for it to
share with. What 7.7.1 can actually remove is the predicate's **redundant**
walks, one `field_ranges(..).nth(i)` from the front of the row per term, which
is why a five-way disjunction is the stake's worked example and a bare
`--table` query is not.

**Nothing published measured that, so the reading comes first and it is its own
slice.** No registered figure passed `--where` or `--filter`, so the shape this
lever pays off on was absent from the whole apparatus; the phase's rule is that
a row lands when its *measured* prize is worth the change, and this row had no
measurement to be worth anything against. The reading is `predicate-terms`
([`measurements.md`](measurements.md), "What a filter term costs"), and the
lever is `7.7.1`. They are two slices because they ask two different review
questions — *does this instrument measure the shape the lever pays off on*
against *is this rework of the replay loop correct* — and because the evidence
half has to land first, so that the mechanism is checked against a figure it
did not produce.

**The census is in the other pass, so no arrangement of this lever reaches
it.** The splitter's three production callers are `map::Builder::on_row`,
`Predicate`'s per-term walk and `push_row`; the first runs in `map_forward` and
the other two in the replay, across the hard boundary
[`architecture.md`](architecture.md), "Query: mapping and streaming are separate
passes" describes. Sharing one split across that boundary *is* the interleaved
form that section records as deliberately deferred — a different item, and a
much larger one. The census's own re-split was made cheap instead, by 7.6's
splitter, and that is the whole of what was available here. Reasoning:
[`../status/history/2026-09-04.md`](../status/history/2026-09-04.md), "7.7 is
smaller and narrower than its rows said".

**Only two of these rows move the `strings` path at all.** `decode_field` and
the field split are each ~1 µs a row and neither changes when typing is
switched on, so together they are 80% of the library's whole 2.48 µs `strings`
row; `append_typed` and the viewing builder do not exist in that mode. So 7.6
and 7.7.1 are the phase's only levers on the zero-copy path — the one the
baseline puts at 13.3× the `dd` floor — and 7.10 and 7.11 cannot touch it.

**Three of those stakes are corrections, not fresh estimates, and they all
have one cause.** The scalar, viewing-builder and `decode_array` rows were
sized off differences taken **through the CLI** — `typed` − `strings`, and the
projection table's per-column deltas — every one of which carries
`pgdq::print_batch` inside it. On the control that subtraction attributed
7.3 µs of a 12.7 µs typed row to decode plus the Arrow build, and the profile
says **79% of the gap is `print_batch`**: the CLI turning the batch back into
TSV, which no embedder pays and no library change removes, leaving the
library's whole typed extraction at 4.06 µs a row against `strings`'s 2.48
([`architecture.md`](architecture.md), "Where a scan's time goes"). The
`decode_array` row is the same error one level down — its 6.2 µs mixed the
micro figure's `decode` and `render` columns, and `render_field` is the CLI's
caller — so it is re-stated at the decode column alone. **A stake for this
table is a library number**, and the rule that keeps the next one honest is
[`measurements.md`](measurements.md), "A mode difference and a per-column delta
are CLI numbers". Reasoning:
[`../status/history/2026-09-03.md`](../status/history/2026-09-03.md).

**One row is admitted that this table did not name**, on the terms the
paragraph below sets: `io::LocalFileSource::read_range` allocates a fresh
`vec![0u8; len]` per chunk — which the kernel zeroes and `read_exact_at` then
overwrites — and every read loop copies it into a second buffer it owns.
Between them that is more than half of a warm `parse`'s user time, and it is
neither zeroing nor copying the problem requires.

**It is slice 7.13 and not 7.8's**, which is where this paragraph first put it.
7.8 is three tunable constants measured against one figure; this is a rework of
who owns the bytes between the kernel and the scanner, and it carries three
decisions that an `fadvise` review would not ask: the copy appears in **three**
read loops (`scan.rs`, and `stream.rs` twice), each with its own `drain(..used)`
carry; `ByteRangeSource::read_range` is deliberately shaped to mirror
`object_store::get_range` so that backend is additive, so removing the
allocation either departs from that shape or pools behind it; and in query mode
the chunk is *also retained* in `batch.rs`'s `VecDeque<SourceChunk>` for
zero-copy views, so the query path holds every chunk twice and dropping the
scanner's copy lands on `push_utf8view_field`'s straddling-field fallback and
on `invalidate_block_cache`. Being the largest single `parse` lever on the list
is why it gets a review of its own rather than why it gets a slice.

**Those three decisions split two ways, and that is where 7.13 was cut.** The
`object_store` one is about the allocation and nothing else, so it is 7.13's;
the three read loops and the query path's chunk retention are both about the
copy, so they are `7.13.1`'s.

**A lever the profile finds and this table does not name is admitted**, if it
is obvious and cheap — by amending this table, with the reasoning in a history
entry, which is what "the spec changes when a decision changes" is for. Filing
every unlisted finding forward would read as discipline and would really be a
way of not acting on the evidence the phase exists to gather.

**An unattended session discharges this by filing, not by amending**, and that
is the intended route rather than a gap in it.
[`.claude/skills/go/SKILL.md`](../../.claude/skills/go/SKILL.md) requires the
phase spec to be left untouched, so a loop running without the maintainer
cannot amend the table above; what it can do is put the finding, with its
profile, in `STATUS.md`'s "Decisions worth another look", and the next review
admits the lever or declines it. Rewriting either rule to close the apparent
conflict was considered and refused. Widening `go` to permit an amendment
wherever a spec pre-authorises one generalises a carve-out any future spec
could claim, and the amendment is exactly the step a reviewer should see;
narrowing this paragraph to make the admission the maintainer's would delete a
warning that is still correct, since the failure it names — filing a finding
*instead of* acting on it — is one a session with the maintainer present can
still commit. The worked instance is `7.14`, where the review that admitted the
lever also found that the filing entry had named the wrong mechanism and had
wrongly called the render half unreachable by a library change; an
auto-admitting session would have carried both into a scanner rework.

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
the plan slipping. This is the exception [`../process.md`](../process.md)'s
"Slice numbering" names: the general rule fixes slice order at spec time, and
an evidence-led phase cannot, because the evidence is what orders the work.

**Two orderings bind, and they are the whole of it.** The **allocator decision
before the wrap sweep**, because an allocator adopted after a figure is taken
invalidates that figure; and `7.13.1` ahead of `7.6` and `7.7.1`, because those
two rework how a row is walked inside the buffer `7.13.1` replaces.

The first is stated against the sweep rather than against `7.3` because `7.3`
has landed *without* adopting, and the hazard it names is still live. What the
early slice bought was pricing the lever while it was cheap to change; what
actually binds is the deadline. Adopting at any point before the sweep pair
costs only figures the sweep re-takes anyway; adopting after it would leave
thirteen freshly-taken tables describing a binary that is no longer shipped,
with no sweep left to repair them. The question is re-asked at `7.13`, which
removes the allocation dominating the current ranking's largest number. `7.13` is also the worked example of a slice
admitted *after* spec time — a row the profile found, on the terms "The levers"
sets — which is why its number sits past the wrap slice and says nothing about
when it runs.

| Slice | What it delivers |
|---|---|
| **7.1** | **The profiling apparatus.** `[profile.profiling]` inheriting `release` with line tables and frame pointers, the tool installed and pinned, and the invocation owned by `scripts/measure.py` — a `--profile-recipe` that prints the command with paths filled in, on the `--koji-recipe` precedent, and never runs it. First profiles of `parse`, `strings` and `typed` over the control and `--arrays --composite` files. No library code. |
| **7.2** | **The decomposition, published.** `architecture.md` gains "where a scan's time goes"; the `INSERT` +10% differential profile settles that attribution; the `INSERT` fast path's layer is chosen; the three measure-only levers (`attach_text`, the double read, the census) get their readings. No library code. |
| **7.3** | **The allocator.** glibc against `jemalloc` and `mimalloc` on the three headline figures; adopted in `pgdump_query-cli` if it wins, never in the library; the apparatus line names it. |
| **7.4** | **`KD5`** — `stream::splice` moves inside the throttle's gate, the interrupt's promise is restated in `architecture.md`, and the entry is rewritten to whatever residual the measurement leaves. |
| **7.5** | **`KD9`** — the `INSERT` fast path at the layer 7.2 chose, with the quote-aware statement-end primitive built to be reusable by P8 Track A's row reader. |
| **7.6** | **Bulk `simdutf8`** over the chunk's largest whole-row prefix, with `decode_field` gaining the unchecked borrow path for a field with no escapes. |
| **7.7** | **The predicated reading** the lever is sized against — a registered figure that passes a filter, which none did, over one file at several term counts and two field depths. No library code. |
| **7.8** | **The cold-NVMe figure** — the third device class registered as a regime of its own, and taken: the one instrument that can price a readahead, `fadvise` or chunk-size default, and the reading `KD9` is read against. No library code. |
| **7.9** | **`decode_array`'s `Vec<Option<String>>`** intermediate, replaced by borrowed slices where the literal carries no escapes. |
| **7.10** | **Scalar decode** — the `decode.rs` half, where a per-type decoder's cost is a parser problem. |
| **7.11** | **The viewing builder for `List<Utf8View>`** — conditional on **7.10.1** pricing the Arrow build, last, and reviewed alone. It lands only if that reading puts the `List<Utf8View>` build above **1 µs/row** on the arrays file, which is the phase's own cross-file apparatus floor and therefore the smallest prize this table can honestly claim. |
| **7.12** | **The sweep pair and the koji regression run**, folded in: thirteen tables re-taken in one sitting, koji's byte-identity check on a glibc build, and the written statement of what a parallel splitter needs from coverage and from the census, filed to P16. |
| **7.13** | **Who owns the bytes between the kernel and the scanner, the allocation half** — `read_range`'s per-chunk zeroed allocation, removed with the `object_store` shape settled explicitly. **Also re-takes `--figure allocator` and settles adoption** — 7.3 measured but deferred, this slice removes the allocation that dominated the ranking, so it either adopts the winner or records the refusal beside the mechanism. |
| **7.7.1** | **One field split per row**, shared by the predicate's terms and `push_row`, sized against `7.7`'s reading. **Earned, not planned**: the row above paired the instrument that measures this lever with the lever itself, which is two review cycles — *does this figure measure the right shape* is not *is this rework of the replay loop correct*, and the evidence has to land first so the mechanism is checked against a figure it did not produce. Its own review: it is a rework of an already-tested core path, and it crosses layers, so [`layering.md`](layering.md) is read before it is placed. Reasoning: [`../status/history/2026-09-04.md`](../status/history/2026-09-04.md). |
| **7.8.1** | **The I/O defaults** — readahead, `posix_fadvise` and the chunk-size constant, each landed or rejected against 7.8's figure. **Earned, not planned**: the row above paired the instrument that prices these three levers with the levers themselves, which is the seam this phase has now met four times — *is this figure measuring the right device* is not *is this rework of the read path correct*, and the evidence has to land first so the mechanism is checked against a figure it did not produce. Reasoning: [`../status/history/2026-09-04.md`](../status/history/2026-09-04.md). |
| **7.13.1** | **The copy into each read loop's own buffer**, over all three loops, with the query path's chunk retention settled explicitly. **Earned, not planned**: the row above paired a contained change to one module with a rework of three already-tested scan loops, which is two review cycles and not one, and the seam was only visible from inside. Reviewed alone, and ahead of 7.6 and 7.7.1, which both rework how a row is walked inside the buffer this replaces. Reasoning: [`../status/history/2026-09-03.md`](../status/history/2026-09-03.md). |
| **7.10.1** | **The typed column build** — the builder-append half, `append_typed`'s dispatch and the Arrow appends under it, and the `List<Utf8View>` reading 7.11's gate is read from. **Earned, not planned**: the row above named its own seam — a `decode.rs` half and a builder-append half — and the two ask different review questions, *is this decoder still the function it was* against *is this rework of the batch builder correct*. The decode half is pure functions with an oracle to check against; the build half edits the batch layer three slices of this phase have already reworked. Reasoning: [`../status/history/2026-09-04.md`](../status/history/2026-09-04.md). |
| **7.14** | **`Syntax::force_quote` as a `const` 256-bit set**, one indexed bit per byte in place of the linear `contains`. **Admitted after spec time**, on the same terms as 7.13 — a row the profile found that this table did not name. It reaches decode and render in one change, because `scan_token` and `push_token` share the predicate, and it changes `needs_quote`'s truth table not at all. **The fusion of `scan_token`'s terminator walk with `needs_quote` is not admitted**: its prize is whatever this row leaves, which nobody has measured. Re-takes `nested-decode-micro` and re-reads the library's per-row budget. Reasoning: [`../status/history/2026-09-04.md`](../status/history/2026-09-04.md). |

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
