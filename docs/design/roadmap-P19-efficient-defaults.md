# P19 — Efficient defaults for a parallel scan

The parallel scan mechanism exists. This phase ships it *set correctly*: what a
person who states no flag gets, on the file they actually have, inside the
memory they were actually allocated.

## What is settled going in

Three decisions predate the grilling and are not reopened here
([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md)).

**The tool discovers its own allocation.** Reading the cgroup limit was
rejected when the mechanism was built; that is reversed, on the deployment case
the rejection did not weigh — under an orchestrator the process is *given* an
allocation, is the only party that knows it, and cannot be asked to have it
restated on a command line. The posture to match is `xz -T0`. Half of it is
already `std`'s: `available_parallelism` reads the cgroup CPU quota, so the CPU
side needs no work — though it does earn a register entry, against what the
inbox filed; see below.

**The parallel default is source-dependent**, registered as a source's own
answer rather than as a test for `.xz` — the gzip, zstd/lz4 and format-coverage
phases each add a source whose right default is its own, and a conditional
written against the one compressed source in the tree today would be reopened
by each of them.

**The order is bound**, and it is not the obvious one: the process's own
overhead is settled first, *then* discovery, *then* the default set from what
was discovered. Until the resident set is a bounded function of the stated
budget, no fraction of a discovered limit is defensible and any constant is
this machine's.

Two further constraints ride along. **An unlimited environment falls back to
today's constant** rather than to a fraction of physical RAM, which keeps
discovery strictly additive and keeps the register's claim at "how a stated
limit is read". And **the library offers discovery without taking it**: the
library's own default stays `Serial`, the mechanism sits beside `Parallelism`
for an embedder who wants it, and `ParallelArgs::resolve` is this repo's one
caller. **What an embedder calls is two primitives and one convenience over
them**: a free `discover_memory_limit() -> Option<u64>` beside `Parallelism`
(the CPU side already being `available_parallelism`), and
`Parallelism::discover()`, which composes them through the reserve rule. The
primitives serve an embedder who has already made half their allocation
decision — the case that decision exists to protect — and the convenience keeps
a caller who wants the whole answer from reimplementing the reserve arithmetic,
which is this phase's finding rather than a detail. `ParallelArgs::resolve`
calls the convenience.

## The arena cap is a deployment setting, not a mechanism this binary ships

`pgdq` will **not** call `mallopt(M_ARENA_MAX, …)`. The cap is published as an
environment setting — `MALLOC_ARENA_MAX`, valued at the worker count the
deployment intends, or that count plus one — stated by the measurement
harness's apparatus and recommended in the manual. `architecture.md`'s existing
refusal of an in-binary cap therefore stands rather than being reversed, and
the roadmap's `Future` item that this phase absorbed is **declined** rather than
adopted.

**The reason is mechanical, and it is what closes the one shape that looked
right.** The attractive version keys the cap to the resolved worker count, so
that a many-core host capping to a small constant cannot become a contention
regression on the very parallel shapes the cap is for. It is unreachable:
`mallopt` bounds arena *creation* and frees nothing already made, glibc seeds an
arena per thread at that thread's first allocation, and `#[tokio::main]`'s
worker threads all allocate at startup — so by the time any argument is acted
on the arenas exist, and a cap set then removes none of them. A cap keyed to the
**stated** `--jobs` *is* reachable early (parse the flag, build the runtime by
hand, `mallopt` before any thread), but the count this phase resolves is the
**source-dependent** one, which is downstream of source recognition, which is
I/O. The number that matters arrives too late by construction.

What is left in the binary would be a constant, and a constant is the worst of
the three: it re-bases every registered figure's apparatus in exchange for a
value that cannot follow the worker count, chosen against a resident probe with
no contention reading behind it. The environment setting has neither problem —
it is in place before the first allocation, and it is free to be a function of
the deployment's own worker count.

**What this gives up is stated rather than hidden**: a user who sets nothing
still gets one arena per visible CPU, so "the stated budget bounds the process"
is delivered by an operator who read the recommendation and not by pgdq. What
the discovered default does about that is settled below.

The readings this rests on are probes on koji, not figures, and no document
quotes them as measurements: 24 arenas at ~536 MiB anonymous resident, 8 at
~476 MiB, 2 at ~328 MiB ([`architecture.md`](architecture.md), "Execution model
and API surface").

## The process's own overhead: the runtime, not the allocator

**The CLI runs a `current_thread` tokio runtime.** `#[tokio::main]` on
`rt-multi-thread` is what builds one worker thread per *visible* CPU at `t=0`,
each seeding a glibc arena before a byte is read. The library dispatches every
unit of work through `spawn_blocking` and deliberately does not require
`rt-multi-thread` ([`architecture.md`](architecture.md), "Execution model and
API surface"), so the CLI can drop it: the threads that then exist are the
blocking pool's, and tokio creates those **on demand**. The arena count follows
the concurrency actually dispatched rather than the host's CPU count — which is
the `jobs`-shaped cap `mallopt` could not be made to be, reached by not
creating the threads instead of by capping what they allocate.

**It is claimed as a thread-count result, not a memory one.** The probe cut
arenas from 24 to 8 by cutting CPUs and moved anonymous resident only ~536 to
~476 MiB, so fewer threads is not by itself fewer bytes. A slice that sells
this as a memory result owes a reading; the defensible claim without one is
that the process no longer sizes itself from a number nobody stated.

*Rejected:* announcing when a memory limit was discovered and the arena cap is
not in effect, through the `PlanNote` channel the two shipped budget messages
use. The check cannot be made complete — `GLIBC_TUNABLES=glibc.malloc.arena_max`
and `MALLOC_ARENA_TEST` reach the same setting without touching
`MALLOC_ARENA_MAX` — so a correctly configured deployment would be told it had
configured nothing, and a false alarm on the one channel this project uses for
real budget advice costs more than the silence it replaces.

## The plain path is accounted for before its default is chosen

`parallel-scan-throughput` reads two bad plain numbers, and they have two
different causes of which only one is a defaults question. A typed `query` is
flat at **0.98×** across the whole `--jobs` axis because `POOL_DEPTH` clamps the
chunk pool to four slots whatever is stated — a mechanism number, which
[`architecture.md`](architecture.md) ("Execution model and API surface") calls
one to re-derive against real holders when a scheduler lands. A `parse` *falls*
to **0.81×**, and it is already negative at **two** workers — 0.471 s serial
against 0.512 s — which is below the clamp, so the clamp is not what that
reading is measuring and nothing in the tree names what is.

**So this phase owes an account of both before it chooses a plain default, and
the account is a prerequisite slice rather than a conclusion.** A
source-dependent default whose plain arm is "serial, because the number is bad
and we do not know why" is a default nobody can defend or revisit. Where the
account lands decides what follows, and only one of the two branches is this
phase's:

- **If it lands on the pool depth** — that is, on how a stated budget is turned
  into slots — it is a defaults question and the repair is a slice of this
  phase, taken before the plain default is set.
- **If it lands on coordination the leader pays per worker**, it is a mechanism
  redesign, it takes a `KD<k>` with a named destination, and this phase ships
  the plain default honestly with the reason written down.

What is refused is committing to the fix before knowing which of the two it is.

**The two halves take different instruments, because only one of them is a
discovery.** The `parse` regression has no suspect, and "where do the extra
41 ms go" is a proportion question — so it is a **profile**, `--jobs 1` against
`--jobs 2` on the plain control through the recipe the harness prints, read for
whether the delta sits in dispatch, in `BufferPool::obtain` contention, or in
work done twice. It produces a `runs/` artifact and no median. The typed-`query`
flatness has a named suspect, so it is a **confirmation**: a scratch build with
`POOL_DEPTH` raised, timed under `--figure parallel-scan-throughput --alone`,
which marks the sitting NOT PUBLISHABLE by construction — the correct status for
a reading taken against a build this project does not ship. **A profile that
comes back inconclusive is itself the answer**, and it is the answer that sends
the repair out of this phase under a `KD<k>`.

## The compressed path is accounted for before its default is chosen

The plain path got this section and a prerequisite slice; the compressed path
never did, and it is the one whose default this phase could not set. `19.15`
measured a **~403 MiB fixed term** outside the stated budget of which roughly
280 MiB is owed to nothing anyone has named — so the precondition at the top of
this spec, that no default is defensible "until the resident set is a bounded
function of the stated budget", is unmet on exactly the path the reserve
constant exists for. That is a hole in this spec rather than a finding of
`19.15`'s, and it is filled here.

**The instrument publishes a pair, not a total.** The quantity is the **fixed
term and the per-reader term**, each with its spread, because the reserve
constant is read off the first and `XzSource::partition_advice` off the second —
a figure reporting only resident would leave the decomposition exactly as
`KD19` records it. This is the opposite end of `rss-attribution`, which holds
block count as its axis and publishes a *slope*: there the intercept is the
allocator's baseline and a nuisance, here the intercept is the answer.

**The grid is asymmetric, deliberately.** The reader-count axis runs at **both**
block sizes — `control_xz`'s 24 MiB and `control_xz128`'s 128 MiB — because the
per-reader charge bills `2 × unit`, so a unit-shaped error is multiplied by the
count and `19.14` has already been wrong about that charge once. The mechanism
legs run at **one** block size only: this figure is gated `warm-parallel`, and
crossing the mechanism legs with the block sizes buys a second cross of the
expensive axis for no question anyone asked.

**Three mechanism legs, and no scratch build.** The **allocator** legs
(jemalloc, mimalloc) separate glibc fragmentation from anything structural and
are nearly free, the harness already building and stamping those binaries for
the `allocator` figure. The **arena cap** is already an axis. And the **path
step** — a budget one byte below `reader_bytes`, so block decode is declined —
prices the whole block path against the streaming fallback, which is the
15.1 MiB against 474 MiB step and the largest single fact in `19.15`'s reading.
That leg earns its place whatever the attribution finds: it is what an operator
needs in order to decide whether `--parallel-memory` is worth setting, and no
figure states it.

A scratch build altering `POOL_DEPTH` or the retention list would attribute the
pools directly and is **refused here**, because a figure cannot publish against
a build that is not the shipped one. **If the three legs leave the remainder
unexplained, that is the finding**, and it names a follow-up experiment rather
than licensing an unpublishable build inside a figure's own sitting.

**Amended: the two paragraphs above applied a figure's rule to a diagnosis, and
the account is now introspective.** What is refused above is a build that is not
the shipped one — correctly, for anything this document publishes, and
irrelevantly for an attribution, which publishes nothing. This sitting is
`--alone` and NOT PUBLISHABLE by its own next paragraph, exactly as `19.2`'s
account was while it ran against a raised-`POOL_DEPTH` scratch build. So the
constraint that was actually binding here was never the apparatus rule; it was
that **nothing in the binary can report a term**, which left differencing whole
runs as the only instrument and made "the three legs leave the remainder
unexplained" a foreseeable outcome rather than a risk. Three mechanism legs
subtracting sums cannot name a term that none of them removes.

**What replaces it.** `19.21` builds the introspection first — the process
reports its own live bytes and its allocator's retention — and this sitting
reads it. The black-box legs are **kept and re-aimed**: they stop being the
attribution and become its check, since a term the instrument names must also
show up in the sum a `getrusage` leg measures, and two instruments that share
no mechanism is the independence
[`.claude/skills/evidence/SKILL.md`](../../.claude/skills/evidence/SKILL.md)'s
third rule asks for and `19.15` and `19.18` did not have. The path step is
untouched: it prices the block path against the streaming fallback, which is an
operator-facing fact rather than an attribution, and it earns its place on the
grounds the paragraph above already gives it.

**The standing rule this now follows is
[`roadmap.md`](roadmap.md), "Attribution is introspective; only the gate is
blind"**, written out of this section's failure. The gate — `19.15`'s and
`19.16`'s readings of whether the shipped rule survives a real cgroup — is
unchanged and stays blind, on the shipped build under the shipped allocator.
Reasoning: [2026-09-11](../status/history/2026-09-11.md), "Attribution was being
done with the gate's instrument".

**The sitting is diagnostic, and `19.11` still publishes.** `reserve` is
registered but untaken, so it has no marker and no table to invalidate, and the
account is taken `--alone` — NOT PUBLISHABLE — exactly as `19.6` and `19.12`
took it. The reason is an ordering trap rather than thrift: `MEMORY_RESERVE`
lives in `io.rs`, which this figure declares, so a table published before the
constant changes goes stale the moment `19.13` ships the new one, and the
expensive `warm-parallel` sitting would be paid twice to publish a number about
to move. The closing sweep publishes it once, afterwards, and keeps what the
harness already assigns to whichever sweep declares the `Shared` edge onto
`peak-rss`: that edge, and the `Session.borrow` change that lets an **RSS**
reading cross a share at all, `borrow` copying wall clock only today.

**What ends the account is a name, not a residual.** Each of the three legs
reports what it moves, the remainder is reported as a number, and it carries
either a mechanism or an explicit "no name" with a follow-up. There is no
threshold to drive the residual under, because the decision downstream is a
*constant* and a constant needs only the fixed term's magnitude and spread,
which `19.15` already has to ±36 MiB. What the attribution buys is knowing
whether the term is **reducible**, and that is answered by whether any leg moves
it. A fractional threshold would also be unmeetable by construction if the answer
is glibc fragmentation, which no leg here can decompose further.

**A reducible finding earns `19.18.1`, and `19.16` is unchanged behind it.** If a
leg moves a term worth bounding, the fix is its own increment — the third level
`../process.md` grants for a slice whose remainder appears because the evidence
moved — and the constant is then chosen after it. Folding the choice into the
account is refused: it rebuilds the mechanism-plus-evidence bundle this pair was
split to avoid, and puts "ship a constant" in the same review as "attribute
280 MiB", which are different confidences.

**One hypothesis the account must be able to kill.** `BlockCache::slot`'s own
documentation says a block evicted while a `Bytes` still views it stays alive
until that view drops — a live-block count the pool does not bound, where every
other term here is bounded by `slots()`. Nothing in `19.15`'s reading
distinguishes that from allocator fragmentation, and the allocator legs are what
separate them: fragmentation moves under jemalloc, retained views do not. It is
stated as a hypothesis rather than a cause, because no reading has tested it.
**It is dead on arithmetic** — at every measured cell the slot count exceeds
what the charge bills, so the floor's unbilled share cannot be positive
([`../status/STATUS.md`](../status/STATUS.md), `19.18`) — and what stands in
its place is the paragraph below.

**A second hypothesis, and this one has a mechanism rather than a suspicion.**
**glibc's mmap threshold is dynamic, so a block-sized buffer stops being
mmap-backed after the first one is freed and is thereafter held in an arena that
never returns it.** Probed on the apparatus's own libc — glibc 2.36 in
`postgres:16` — eight `malloc`/`free` cycles of a 24 MiB buffer: the first is
mmap-backed and returns to the OS on `free`; from the second on, `hblkhd` is
**0**, `arena` is **25,305,088**, and the cycle ends with 24 MiB sitting in
`fordblks`, freed and resident
([`measurements.md`](measurements.md), "What an instrument can see"). That
predicts a retention term of about one block per thread that has ever decoded
one, unit-scaled and count-scaled and bounded by neither pool — which is the
shape of what `19.15` could not name. It is a hypothesis and not a cause: the
probe is a C program, not pgdq, and what makes it testable rather than plausible
is that `19.21`'s instrument reports `arena` and `fordblks` per arena directly.
**No allocator leg can test it**, which is why the mechanism legs were never
going to find it: jemalloc and mimalloc do not have glibc's threshold, so
swapping them removes the mechanism instead of measuring it, and the excess they
report is `MALLOC_ARENA_MAX`'s sixth all over again.

## The span term is charged per source, not universally

`plan_partitions` builds `divisor = footprint + max_source_span` and
`worker_count` answers `min(jobs, max(1, budget / divisor))`. The inbox filed
"divide one allowance among sub-streams rather than charging it to each" as a
spec question; the answer is that neither the current form nor that one is
right, because **the term is right for one source shape and double-counts for
the other**.

On a **chunk-shaped** source the span genuinely is what a batch pins, rounded
out to chunk boundaries, so the term is the honest cost of handing a sub-stream
its own batcher. On a **block-shaped** one it is not: a batch confined to one
worker's LF-split range inside one block pins exactly that block whatever the
cap says ([`architecture.md`](architecture.md), "Three flush triggers, and only
one of them bounds memory"), and `partition_bytes` has already charged for that
block. Adding the span there charges a second time for bytes the first term
covers.

**So what one sub-stream costs is the source's own answer**, which is the shape
this phase has already committed to for the worker count — the source states
what a concurrent reader costs it, and the span is added only where the
retained unit is the read chunk. **`Partitioning` gains a term naming that
unit** (the read chunk, or the partition itself), and `plan_partitions` adds
`max_source_span` only for the first. The source states a fact about itself and
the caller keeps the arithmetic, which is the division already drawn at "It
answers where and at what cost, never whether".

*Rejected:* keying it on `PartitionBoundaries` — `Anywhere` read as
chunk-shaped, `At(…)` as block-shaped. Reading an economic meaning off that arm
is the mistake `architecture.md` rejects by name, and it breaks on the first
remote source, whose honest answer is `Anywhere`. *Also rejected:* a
`Partitioning` method taking `max_source_span` and answering the whole
sub-stream cost, which puts a query concept inside the source trait and across
the layering boundary. That makes the accounting right in `N` and
right per source at once, and the "divide it among sub-streams" alternative
becomes unnecessary rather than merely unhelpful.

**Neither form reaches the reachability problem, and the spec says so rather
than letting the accounting claim it.** At the shipped defaults a **plain**
source divides 64 MiB of budget by a 1 MiB chunk plus a 64 MiB span and plans
**one** sub-stream for every `--jobs` — exactly as the compressed one does, and
`parallel-scan-throughput` does not show it only because every row of that
figure states `--parallel-memory 1073741824`, where the same arithmetic affords
fifteen. Charging the span once instead of per sub-stream gives
`N = (budget − max_source_span) / footprint`, which is **zero** there for the
same reason. What makes a parallel query reachable at the defaults is the
*budget* default, which is what discovery is for.

## What is discovered, and what the default makes of it

**The default is the limit minus a reserve, not a fraction of it.** The
evidence rules the fraction out as an instrument: 128 MiB stated measures the
same ~328 MiB resident as 256 MiB stated, because `BufferPool::slots()` clamps
to `POOL_DEPTH.max(jobs)` at either — so the overhead above a stated budget is
roughly **constant**, not proportional. A percentage under-reserves at a small
limit and over-reserves at a large one, which is backwards: the small cgroup is
where being wrong kills the process.

**That evidence holds only below the clamp, and the rule is sound only once the
block pool's ceiling is its stated budget.** The reading above was taken at
`--jobs 4`, where `POOL_DEPTH.max(jobs)` is 4 and binds at both budgets; at
`--jobs 24` the clamp is slack, `slots()` becomes `budget / unit`, and the
block pool's free list and retained list are each held at that count with
nothing shared between them — so a block-decoding source's resident set tracks
**twice** the stated budget rather than a constant above it. The overhead is
constant below the clamp and proportional above it, which no subtraction
describes. **`19.7` therefore couples the two counts** so that one stated
number bounds the pool ("[`architecture.md`](architecture.md), The compressed
source"); without that repair `limit − reserve` bounds nothing on the
compressed path.

**The coupling was necessary and not sufficient, which this spec did not
foresee.** It predicted the rule would be correct as written once the counts
were coupled. `19.12` measured the coupled build and found resident *up* a
tenth rather than halved: the pools were bounded, and what the readers
themselves hold was never in them. `19.14`'s corrected divisor is the other
half, and only with both does `limit − reserve` bound the compressed path.
Evidence: [2026-09-09](../status/history/2026-09-09.md), "The reserve entries,
reviewed: the divisor is wrong, not the rule".
Evidence: [2026-09-09](../status/history/2026-09-09.md), "The reserve rule's
two entries, closed".

So the rule is `budget = limit − reserve`, floored at today's 64 MiB constant,
and **there is no fraction**. `Parallelism::discover()` returns that, and
`ParallelArgs::resolve` narrows it by what the source says it can use.

**The fraction was specified as a ceiling and is dropped, because the ceiling
is structural.** "We do not take an allocation we cannot show we use" has an
exact expression once the divisor charges honestly — `jobs × C`, for a
per-reader cost `C` — and the mechanism already enforces it from two sides:
`stream::worker_count` is `min(jobs, budget / C)`, and `BufferPool::slots`
clamps at `POOL_DEPTH.max(jobs)`. So resident saturates at `180 + jobs × C` and
every byte of budget above that is inert, at any limit and on any machine — at
256 cores against a 128 GiB limit it is ~15 GiB, bounded by the worker count
and not by the budget. A number that provably never binds is worse than no
number, because it reads as a safety margin and is not one.

**What that costs is a coupling, stated rather than hedged against.** The
inertness rests on `slots()` clamping at `POOL_DEPTH.max(jobs)`; a change that
unclamps it makes a large budget suddenly real. That consequence is recorded
beside the clamp ([`architecture.md`](architecture.md), "Execution model and
API surface") rather than guarded by a fraction hedging against a change nobody
has proposed.

**The margin this leaves is thin in one band, and `19.15` is what settles
it — knowingly, and after `19.13` ships.** The argument above is about *mean*
resident; it says nothing about variance, and variance is what a cgroup kills
on, since it kills on one run's peak rather than on the median of three.
`19.12`'s per-rep spreads across the compressed uncapped leg are **10.1%,
6.5%, 19.8% and 7.0%** — proportional to resident rather than a fixed jitter,
the absolute spread growing 27 MiB → 160 MiB across the axis. A *constant*
reserve leaves *constant* headroom against that: 18.4% at a 512 MiB limit,
falling to **7.0%** through the 1.25–1.5 GiB band before the worker-count cap
lifts it again. In that band the margin is below the median observed spread.

Raising the constant does not fix the shape — 20% at 1.8 GiB needs a ~540 MiB
reserve, which puts a 600 MiB cgroup below the floor — so the alternative is a
proportional term reintroduced for a *measured* reason rather than as the
unmeasured ceiling dropped above. That is not taken now: it would pick a number
from four cells of a diagnostic sitting on a build `19.14` is about to replace.
**`19.15` is the gate, not a follow-up.** It runs at 256 MiB, 512 MiB and
1 GiB, either side of the worst band, against a **scratch build** of the rule
and *before* `19.13` lands; if the headroom does not survive, the constant is
chosen differently or the proportional term returns. Either way `19.13` ships a
number this phase has watched survive a real cgroup, which under the container
premise is the case it will actually meet.
Reasoning: [2026-09-09](../status/history/2026-09-09.md), "The reserve's
headroom is thin where variance is widest".

**The gate failed, and three of the decisions above are amended by what it
found.** `19.15` ran and the headroom did not survive — at the opposite end of
the axis from the prediction, 7.3% at 512 MiB against 33.4% at 1.5 GiB, because
the paragraph above reasons from a fit taken with the count pinned at
twenty-four while a flagless run derives the count *from* the limit. The three
amendments:

- **The margin is now stated, where before it was only called "thin".** The
  criterion is that the **worst observed rep leaves at least 20% of the limit**,
  the median being context and not the gate. Until this, `budget = limit −
  reserve` aimed resident at the limit by construction and every margin it left
  was an accident of two over-estimates that both scale with the reader count —
  which is why the thin point is the *smallest* limit reaching the block path.
- **The proportional term does not return, and is withdrawn as this spec's
  named reopening.** It is smaller than a constant exactly where the failure is,
  so it is the wrong instrument on the measurement as well as on the argument
  this spec already made against a fraction twice over. Calibrating it at a
  large limit instead relies on the source's own recommendation binding before
  the allowance does, which trades an ordinary host's worker count for a small
  host's safety.
- **The constant is chosen by `19.16`, not by this paragraph, and it also sets
  the floor.** Every candidate near the measured fixed term produces a
  *one-reader block path* that no sitting has measured, so the number is taken
  from a reading rather than extrapolated across the discontinuity that broke
  the last extrapolation. And because `BlockCache::affordable` reads off the
  budget, the reserve and the limit below which a compressed scan goes serial
  are one knob; the floor stays implicit and the decline is **reported**, naming
  the limit that caused it.

Reasoning: [2026-09-11](../status/history/2026-09-11.md), "The budget rule's
headroom fails at 512 MiB, and not where it was predicted to", and
[`roadmap-P19.15-budget-probe-notes.md`](roadmap-P19.15-budget-probe-notes.md).

**A fourth amendment: the reserve is not one constant, because the term it
covers scales with the block unit.** Everything above argues about whether the
reserve should scale with the *limit*, and refuses that twice. Neither argument
considered the **block size**, and that is the axis it actually varies on:
`19.18`'s probe refutes a unit-independent fixed term outright — at equal
reader counts the two block sizes differ by far more than the `2 × unit` the
per-reader charge accounts for, by an excess that *falls* as the count rises,
which is the signature of a unit-scaled term that is not per-reader. So:

- **The repair is a second per-file term, subtracted once — not a bigger
  `reader_bytes`.** The measurement decides the shape rather than leaving it to
  the repair: the excess *falls* as the reader count rises, so the term is not
  per-reader, and `reader_bytes` is multiplied by the count. Charging it there
  would over-bill at high counts by exactly the factor the reading rules out —
  the mirror image of the error `19.14` corrected — and because
  `BlockCache::affordable` reads off the budget, an over-billed per-reader term
  declines the block path on allocations that could have afforded it, trading
  an OOM for a silent serial scan. So the budget loses the per-file term once,
  before it is divided, and `reader_bytes` keeps its shape. After that what
  remains may be constant and `19.16`'s question becomes answerable. A
  per-source *reserve* is the same arithmetic filed in the one place the rule
  cannot read it, which this spec already refused above.
- **The decline the repair widens is accepted, and reported.** A per-file term
  taken off the top leaves less for readers, so a compressed scan goes serial
  on allocations where it takes the block path today — at 128 MiB blocks with
  `POOL_DEPTH` units of floor that is roughly 512 MiB, most of a small
  container. That is the correct answer rather than a regression: it is the
  first arrangement in which the stated number is true for a large-block file,
  the decline's report (`19.13`) names the limit that caused it, and
  `--parallel-memory` reverses it. The cheaper alternative is named and **not
  taken here** — the floor is `POOL_DEPTH` units only because `POOL_DEPTH` is
  four, and a block cache whose floor tracked the reader count would charge
  less at low counts. That reworks a pool's sizing rule on evidence this
  account has not produced, so it is a `Future` item
  ([`roadmap.md`](roadmap.md)) rather than a slice of this phase.
- **`19.16` does not run until the account completes.** Its candidates were
  picked against a fixed term measured at one block size; choosing a constant
  against a quantity now known not to be constant is the extrapolation that
  broke `19.15`, one level up. The spec's binding order — the compressed
  account, then the constant — is unchanged and is the reason.
- **The named candidate is the block cache's own retention.**
  `BufferPool::slots` clamps at `POOL_DEPTH.max(jobs)`, so the cache holds up
  to `POOL_DEPTH` units regardless of the reader count, and `reader_bytes`
  bills two units *per reader* and nothing for it. `KD19`'s evicted-but-viewed
  block is the second candidate. The three mechanism legs separate them, and
  they are what the aborted sitting never reached.
- **The repair takes the next free slice number once the account names it, and
  not before.** What the amendment fixes is *where* the repair goes, not what
  it is; that is the re-taken `19.18`'s reading. A slice allocated ahead of it
  would be a box the unattended loop picks up and cannot land, the checklist
  being that loop's work queue — allocation on discovery is the rule for a
  phase and for an out-of-band item, and a slice is allocated when it can be
  sliced.
- **The 128 MiB flagless family is the repair's acceptance gate, not a source
  of numbers.** Under the current rule it produces none at any registered
  limit: it declines the block path at `512m` and is killed at `1g` and above.
  After the repair every leg in it must either take the block path and survive
  its allocation or decline it, and none may be killed — which is what that
  family is uniquely able to say, being the one reading in the phase taken on a
  block size the machine did not choose for itself. The margin criterion above,
  worst rep leaves ≥20% of the limit, is unchanged and applies to the legs that
  survive.

Reasoning: [2026-09-11](../status/history/2026-09-11.md), "A unit-independent
fixed term is refuted, so the reserve cannot be one constant".

**A fifth amendment: the repair runs *before* the account, the named candidate
is refuted, and the fixed term the first four amendments argue about does not
exist.** Three of the four bullets above survive — the repair's shape, the
accepted decline, the 128 MiB family as acceptance gate. Three things change,
and each is established without a new sitting:

- **There is no fixed term to choose a reserve against.** `19.15` and `19.18`
  independently fitted `403 + 31.2` and `406 + 31.1` a reader, and agreeing
  with each other is why it stood for three sessions. Both fitted over three to
  twenty-four readers; resident is **concave** in the count, so the intercept is
  the concavity the window skipped. The same file over one to six readers fits
  `62 MiB + 93.3`, and at one reader the process holds **62.9 MiB** where
  `403 + 31.2` predicts 436. The concavity's mechanism is `BufferPool::slots`
  running 2, 4, 4, 4, 5, 6 over one to six readers — so the clamp this spec
  cites as the reason overhead is *constant* is the reason it is not.
- **The named candidate is refuted arithmetically, not by measurement.** At
  every measured cell `(budget − chunk_held)/unit` exceeds
  `POOL_DEPTH.max(jobs)`, so slots are 2/4/4/6 against a bill of `2 × readers`
  = 2/4/8/12: the retention floor's unbilled share is zero at two readers and
  negative above, and it cannot produce a positive excess at any count. The
  three mechanism legs registered to separate it from the evicted-but-viewed
  block would have spent an hour establishing that.
- **The term is a defect, so the repair is `19.19` and it goes first.**
  `partition_bytes` is both the memory charge and `run_region`'s cut size, so
  `19.14` raising it to `reader_bytes` made every partition 2.08–2.42 blocks;
  `scan_partition`'s first read is the whole piece, so it takes
  `read_by_blocks`' multi-block arm and assembles into an unpoolable buffer of
  partition length, held through the parse and billed one chunk. A charge that
  inflates what it charges for. It is also a throughput regression — the chunk
  pool's four slots then cap the fused workers, flat at 8.83 → 8.77 s from four
  readers where the pre-change figure scaled to twenty-four jobs.

**So the binding order inverts for this one slice.** The fourth amendment held
the repair behind the account on the ground that the account is what names the
term. It is named, by a failing unit test rather than a sitting — build the
four-block `SeekTable` fixture, call `partition_advice`, feed `stream::cut`,
assert every range spans one block — and a slice whose evidence is a unit test
is allocatable now, which is what that bullet was waiting for. Every reading
taken before it lands prices the defect, so the re-taken `19.18` follows it.
`19.16` still runs after the account, unchanged.

`19.19` also carries `fit`'s divisor: it divides the allowance by the source's
*recommendation* rather than by the affordability charge, so a 512 MiB
allocation resolves three readers where four fit. The two are one arithmetic.

Reasoning: [2026-09-11](../status/history/2026-09-11.md), "`M81` and `M82` are
both withdrawn, and the account they were guarding is wrong".

**The reserve is one constant, taken from the compressed leg, and it
over-reserves the plain path by roughly the difference.** The two paths' fixed
terms are a factor of thirty apart — a plain `parse` holds 5.86 MiB above its
pool, which is `peak-rss`'s published 5.85 and the cheapest check that the
instrument is the right one, where the block-decoding path's fixed term is a few
hundred megabytes. A per-source reserve is not available where the number is
needed: `Parallelism::discover()` is the primitive an embedder calls with
nothing open, and a source's own answer is downstream of recognition, which is
I/O — the same argument that makes the *worker* default source-dependent makes
the reserve not. Over-reserving is the safe direction, and an operator who wants
a plain scan's real headroom states `--parallel-memory`. *Rejected:* having
`discover()` answer a range, or a closure over a source, which puts recognition
inside the primitive that exists to serve a caller who has already made half
their allocation decision.

**What is read is the minimum over every limit that binds**: cgroup v2
`memory.max` and `memory.high`, v1 `memory.limit_in_bytes`, and every ancestor
cgroup rather than the nearest. `memory.high` throttles where `memory.max`
kills, and sustained reclaim ends a scan's throughput as surely as an OOM ends
the run; an ancestor's limit binds whichever level states it.

**An unlimited environment falls back to today's constant**, as settled going
in.

**A source recommends a budget as it recommends a worker count, and that is
what keeps the fallback from meaning "serial".** A corrected charge is ~59 MiB
a sub-stream, so the 64 MiB constant admits **one** — which would override
`XzSource::default_workers`' `available_parallelism()` to serial and ship a
source-dependent worker default that never fires on an unlimited host, the
machine most likely to run this. So `ParallelArgs::resolve` asks the source
when `--parallel-memory` is absent exactly as it already asks when `--jobs` is:
`XzSource` answers with what **one** of its workers holds — ~59 MiB, which at
its recommended twenty-four is ~1.4 GiB — and `LocalFileSource` answers with no
budget, as it answers with one worker today. A count nothing can afford is not a
recommendation, which is the general form
([`roadmap.md`](roadmap.md), "A default runs as fast as the allocation
permits").

**Per worker rather than as a total, and the composition multiplies.** A total
would have to be a total for the source's own count, so a caller that replaced
that count with `--jobs` could recover the per-worker cost only by dividing by a
number no longer in play — six times too large at `--jobs 4` against a
twenty-four-worker recommendation, which reduces a run four readers fit to one.
Stated per worker the answer is independent of every count. Reversed on review,
which affirmed the reasoning above and moved only where the multiplication
happens ([2026-09-10](../status/history/2026-09-10.md), "The recommended pair
has to be consistent").

*Rejected: detecting a container and softening the default when we are not in
one.* It sounds like the modest unlimited default and is a weaker version of
it. What binds a process is the **limit**, not the namespace, and "was an
allocation stated" is already read completely (`RT1`–`RT6`) and is right in all
four combinations: in a container with a limit, fill it; in a container
*without* one, stay modest, since nothing said what we may take; on a bare host
with a systemd `MemoryMax`, fill it, since somebody did; on a bare host with
nothing, stay modest. Container-ness changes the answer only in the two middle
rows, and in both it changes it to the wrong one. As a *diagnostic* it is the
incomplete-environment check this spec already refused for the arena cap —
`/.dockerenv` is docker's and absent under containerd, `/proc/self/cgroup`
reads `0::/` in some containers and a real path in others — and a false alarm
on the budget channel costs more than the silence. The actionable sentence
needs no detection: **no limit found already means no limit is enforced**,
which is complete and true however the process was started, and the status line
is where it goes.

**Where nothing is discovered, the budget is capped at half of
`MemAvailable`.** `XzSource` recommends `available_parallelism()` for the count
and `C` for what one of those workers holds, so the ask is `jobs × C` — and on
the no-limit path the cap applies to the **pair**: the count comes down to what
half of `MemAvailable` affords, `jobs' = min(jobs, ½ × MemAvailable / C)`, and
the budget is what that many spend, `jobs' × C`, never the cap itself. Because
resident saturates at `jobs × C`, the cap **costs nothing wherever there is
room**: half of this host's ~20 GiB available is ~10 GiB, which plans the same
twenty-four workers a 1.4 GiB budget does, every byte above `jobs × C` being
structurally inert. Fast where the machine allows it, bounded where it does not.

**It binds wherever the recommended count costs more than half of what the host
has free**, which is not only a small machine: `C` scales with the block size,
so a large-block file reaches it on a large one too. The cap on
`XzSource::default_workers` — this file's own block count — is the other half of
the same bound, and it binds independently of memory.

**Half, and `MemAvailable`, and only after no limit was found — each for its own
reason** ([`runtime-invariants.md`](runtime-invariants.md), `RT8`).
`/proc/meminfo` is the **host's** even inside a container, so consulting it
before `RT1`–`RT6` have ruled out a limit would read 20 GiB against a 512 MiB
allocation. `MemFree` is unusable: it excludes reclaimable page cache and reads
7.4× lower than `MemAvailable` on an idle host, so pgdq would throttle itself
for memory the kernel would hand straight back. And half rather than all
because `MemAvailable` is an estimate that two processes reading at once each
see the whole of — it is a ceiling to plan under, never a reservation.

**This reverses the going-in refusal to size from physical memory, deliberately
and with an invariant behind it.** That refusal kept discovery "strictly
additive" when the fallback was a 64 MiB constant that could not hurt anyone.
The fallback now scales with the core count, so an unbounded one would size
pgdq's appetite from hardware nobody granted — and the cheapest thing that
bounds it is the machine's own answer about what is left. `RT8` is what makes
that a read with stated semantics rather than a guess.

*Rejected: a modest constant instead — `min(cores, 4) × C`, about 236 MiB.* It
was taken briefly and is wrong on the "fast" half of the posture: it caps a
roomy twenty-four-core host at four readers with nothing contended, buying
safety that the `MemAvailable` cap provides for free. A constant cannot tell a
128-core server with 256 GiB from a 128-core one with 8.

*Rejected: detecting a container and softening the default when we are not in
one.* It sounds like the same idea and is a weaker version of it. What binds a
process is the **limit**, not the namespace, and "was an allocation stated" is
already read completely (`RT1`–`RT6`) and is right in all four combinations: in
a container with a limit, fill it; in a container *without* one, fall to the
`MemAvailable` cap, since nothing said what we may take; on a bare host with a
systemd `MemoryMax`, fill it, since somebody did; on a bare host with nothing,
the cap again. Container-ness changes the answer only in the two middle rows,
and in both it changes it to the wrong one. As a *diagnostic* it is the
incomplete-environment check this spec already refused for the arena cap —
`/.dockerenv` is docker's and absent under containerd, `/proc/self/cgroup`
reads `0::/` in some containers and a real path in others — and a false alarm
on the budget channel costs more than the silence. The actionable sentence
needs no detection: **no limit found already means no limit is enforced**,
which is complete and true however the process was started, and the status line
is where it goes.

**The composition is not a `min`, and the difference is one `Option`.** Written
as `min(source_recommendation, discovered)` it breaks the case it exists for:
on an unlimited host `discover()` falls back to today's constant, so the `min`
is 64 MiB and the `.xz` scan is serial again. What distinguishes *no limit
found* from *a small limit* is that the first has no cap at all:

- `environment_cap` = the discovered limit minus the reserve, or **`None`**
  where nothing was discovered;
- `want` = the source's recommendation, or `DEFAULT_MEMORY_BUDGET` where the
  source offers none — which is the plain path, and which stays serial anyway;
- `budget` = `environment_cap.map(|cap| want.min(cap)).unwrap_or(want)`.

So `min` governs where a limit exists, which is "do not take what you cannot
use", and the fallback is the source's own answer. That is also what keeps "an
unlimited environment falls back to today's constant" true: the constant is
what a source with *no* recommendation gets.

**Below the reserve the arrangement is named rather than emergent.** At a
256 MiB limit `limit − reserve` is zero, and three independent floors would
otherwise produce the behaviour between them — `worker_count`'s `.max(1)`,
`BufferPool::slots`' clamp to one, and `affordable()` refusing block decode. So
state it: below the reserve the budget is one reader's worth on the *streaming*
path, and the below-floor `PlanNote` (`19.9`) says the allocation bound the
scan. It gets a test, so that a later change to any of those three floors
cannot silently alter it.

**This qualifies the settled sentence above rather than reversing it.**
Discovery still falls back to today's constant; what changes is that a *source*
may then recommend more, through the mechanism `19.8` already built. A stated
flag still wins outright, a discovered limit still binds over the top, and the
library's own default stays `Serial`. What it costs is that an unlimited host
reading a compressed file hands ~1.4 GiB to the pools where it hands 64 MiB
today — the `xz -T0` posture this phase opened with, reversible with one flag.
Reasoning: [2026-09-09](../status/history/2026-09-09.md), "The reserve entries,
reviewed: the divisor is wrong, not the rule".

**`discover_memory_limit` takes its filesystem root as a parameter**, so that
the v1 arm can be driven from a fixture tree. This machine runs a pure v2
hierarchy and cannot produce a v1 memory controller at all
([`runtime-invariants.md`](runtime-invariants.md), `RT4`), so without the seam
the v1 branch ships never having been executed. The seam is 19.7's, the fixture
tree is 19.9's, and what it establishes is that the reader handles the shape
`RT4` claims — not that the kernel still produces it, which only a v1 host can
say. Reasoning:
[2026-09-09](../status/history/2026-09-09.md), "The runtime register cites by
path and tag".

**Where the reserve leaves less than the floor, the reserve wins and pgdq says
so.** On a small allocation the 64 MiB floor and the reserve fight — a 256 MiB
cgroup against an uncapped reserve of ~200 MiB leaves ~56 MiB — and letting the
floor win would reassert today's constant under a new name in the one case
discovery was reversed for. So the budget goes below the floor, and where
`limit − reserve` falls under what a single slot needs, the `PlanNote` channel
carries it: a user in a tight cgroup is told that their *allocation* bound the
scan, not their flags. **This is not the check refused above** — that one
inferred a setting from an environment variable and could be wrong about it;
this one reports arithmetic pgdq performed itself.

## What this phase does not specify, because it already ships

**Both budget announcements exist.** One names a `--jobs` the stated budget
refuses; the other names a budget-declined `.xz` block path, carrying the file's
largest block beside the budget, and adds the container's shape to
`info --detail`/`--json` off the persisted seek table. Both are `PlanNote`s on
`TableStream::plan_notes` rather than `DiagnosticKind`s, because every
`DiagnosticKind` is a property of the *file* and a budget decline is not. **No
slice here may re-specify them**: committing to an announcement that already
ships would make this phase unmeasurable against what it delivered. What is left
for this phase is the numbers, and both messages now argue for it with something
a user can act on rather than with silence.

**Changing the `--jobs` default is free of the figure register.** Every
registered figure's command shape, the profile recipe and the koji recipe state
`--jobs` explicitly, and `--check` refuses a shape that pins no count
([`measurements.md`](measurements.md), "The apparatus"). `DEFAULT_JOBS` is 1
today, and the reason given was that a person stating no flag gets "the
arrangement every published figure was taken under" — a measurement rationale
standing in for a user-facing default. **That rationale is spent**: this phase
may set the default from what the evidence says a user should get, and no figure
moves underneath it.

## The source states its own worker default

A defaulted `ByteRangeSource` method answers a *recommendation*, `Serial` by
default — the same convention `partitions` already follows, so a source that has
not thought about concurrency is not split by a caller who read silence as
consent. `LocalFileSource` inherits the default until the plain account says
otherwise; `XzSource` overrides it.

**`.xz` takes the cores**: `available_parallelism()`, which already reads the
cgroup CPU quota, clamped by what the discovered budget affords through the
divisor that exists. That is the `xz -T0` posture the discovery decision
adopted, and the budget is now a real number rather than a constant, so the
clamp means something. *Rejected:* a small constant such as four. The standing
rule's baseline already provides predictability in a different way ("Four
workers is the optimization baseline; twenty-four is the guard"), and a constant
would leave the one shape that demonstrably scales — a compressed `parse` at
**5.82×** at twenty-four workers, still climbing — running at a fifth of what
the machine offered it.

**The asymmetry this ships is stated rather than smoothed over.** The same
default gives a compressed `parse` 5.82× and a compressed typed `query` only
**1.60×**, its sub-stream count capped by the divisor. That is a count asked for
and partly not delivered, and the two shipped budget messages are what say so to
the user — which is why this phase inherits them built rather than specifying
them.

## The user surface: absence means discover

**A stated flag wins outright; absence is what triggers discovery.** No new
spelling is added. *Rejected:* `--jobs 0` as an explicit "discover" — zero
already reads as one in `Bulk::new` and in `Parallelism::workers`, and giving it
a third meaning at the CLI while the library reads it as one is the divergence
the two-numbers paragraph warns against.

**The status line reports the value and its provenance**, and the provenance is
a **second line the CLI emits**, not the `memory_bytes=` slot `scan started`
already prints. All four spellings ship — `(stated)`, `(discovered: …)` naming
which limit was read, `(no limit found: what this source asks for)` and
`(default: no limit found)` — on a line whose layer can produce them. The slot
cannot: discovery hands the library a number byte-identical to a stated one, so
the distinction is gone before `scan started` runs, and reaching it would mean
widening `Parallelism` or `ScanOptions` for a display fact nothing branches on.
The library's slot keeps saying what bound applies, which is what an embedder
that hands over `Parallelism::default()` is owed. Both widenings are filed as
rejected beside the mechanism
([`architecture.md`](architecture.md), "Status output").

## The default is pinned by a test, not by a figure

Every registered command shape states `--jobs` explicitly — `--check` refuses
one that pins no count, which is the apparatus rule. The consequence is that
once a source-dependent default ships, **no figure exercises it**: this phase's
whole deliverable is what a person who states nothing gets, and the measurement
register is by construction blind to it.

**That is closed with a test, not with a carve-out.** What can go wrong with a
default is that it *resolves* wrongly — reads the wrong limit, picks the wrong
source arm, ignores a stated flag — which is a correctness property a test pins
exactly and a median pins badly. `pgdump_query-cli/tests/` asserts the resolved
arrangement against a stated cgroup limit, per source.

*Rejected:* a figure whose shapes deliberately state no count, with the
carve-out written into `measurements.md`. It would put an unpinned shape into a
register whose whole discipline is that a shape states its count, to buy a
number the pinned rows of `parallel-scan-throughput` already carry — the
throughput of whatever arrangement the default lands on is measured there, at a
stated count, which is the honest way to measure it.

## `runtime-invariants.md`

The register is this phase's first slice: `postgres-invariants.md`'s sibling,
named for the runtime environment rather than for Linux, so that a
container-less or non-Linux answer has the same home. `CLAUDE.md` gains a
read-trigger beside the Postgres one.

Entries: `/proc/self/cgroup` for locating the process's own cgroup; v2
`memory.max` and `memory.high` with the literal `max` sentinel; v1
`memory.limit_in_bytes` with its large-number sentinel; the ancestor walk and
why the effective limit is the minimum; the hybrid v1/v2 hierarchy.

**The Re-verify field is a container invocation**, not a citation:
`sudo nerdctl run --memory <N>` asserting that the value read back equals the
value requested. That is the same ritual `postgres-invariants.md` runs across
six images, and an entry without one is an entry nobody checks.

**`available_parallelism` reading the cgroup CPU quota gets an entry**, against
what the P19 inbox filed. The inbox's test was that the behaviour is `std`'s
and therefore not ours; the register's trigger is a decision depending on
external behaviour *we do not control*, and someone else's code is more
external, not less. This phase's whole CPU default rests on it, the proof is
cheap, and without an entry a future toolchain moves that default silently.

## `KD16` is absorbed: `Serial` carries a budget

`KD16` — the CLI drops a stated `--parallel-memory` at the default `--jobs 1`,
because `Parallelism::workers(1, …)` collapses to `Serial`, which states no
bytes — moves from `(c) unowned` to **`(b)`, owned by this phase**, and is
paired with a named slice in the change that writes the slice list. A value
shape that silently discards a stated budget at one worker is incoherent with a
phase whose whole subject is that the budget is real, and the recourse both
shipped budget messages name ("raise `--parallel-memory`") is inert without
`--jobs 2` beside it — a manual telling a user to work around a defect.

**Of the two fixes named beside the mechanism, `Serial` carries an optional
budget.** `workers(1, …)` no longer collapsing is the alternative and is
refused: the collapse is what keeps "is this parallel" a match on the variant
rather than a comparison against the number one.

**The figure moves, and this spec says so rather than letting the sweep discover
it.** `parallel-scan-throughput`'s one-job row runs at the library's 64 MiB
default *precisely because* of this collapse, and that figure's own prose states
it as the serial arrangement a speedup is a speedup over. Closing `KD16`
re-bases that row. It lands inside the closing sweep below, so it costs a
paragraph rather than a sitting of its own.

## The reserve is a figure, and it is measured uncapped

The budget rule rests on one number, and a number the docs state is one someone
can re-take. Neither existing figure answers it: `peak-rss` measures resident
and `rss-attribution` decomposes per-block growth, where the reserve is
**resident above what the process was told it could have**. So it is a
registered figure, taken in the closing sweep beside those two. What it shares
with them is **a sitting, and one reading** — all three read resident, so `M74`'s
sweep takes them together, which is the collapse that sweep exists to make, and
the one borrow is `peak-rss`'s `control` row, which this figure's serial baseline
repeats spec for spec. It shares **nothing** with `rss-attribution`, whose every
leg runs the two block-count shapes that are its axis where none of this figure's
do: the two measure different paths and publish a slope against an intercept, so
there is no reading to borrow ([2026-09-11](../status/history/2026-09-11.md),
"The reserve shares a sitting with two figures and a reading with one").

**It is measured both ways and the shipped default uses the uncapped number.**
Having declined the in-binary cap there are two reserves — one with
`MALLOC_ARENA_MAX` at the recommended value, one without — and the shipped
number comes from the uncapped leg because that is the **safe direction**: the
default has to survive the case where the recommendation was not followed,
because that is the case that kills the process. It is measured both ways so
that the difference is a reading rather than an assumption, not because the
difference is expected to be large.

**The two legs were expected to differ by roughly the 200 MiB the arenas hold,
and on the one shape measured so far they do not differ measurably at all.**
The 200 MiB came from a koji probe under `rt-multi-thread`, where twenty of the
process's twenty-four arenas belonged to runtime threads that did no work;
`19.3` deleted those threads. So the price this section used to state — that an
operator who capped arenas gets a budget sized for one who did not — is
proportional to a difference nobody can currently measure, and it is **not** an
argument for reversing the decision at the top of this spec. `M76` takes the
controlled reading that would restore a number to it, and `19.10` writes
whatever that reading says. Evidence:
[2026-09-09](../status/history/2026-09-09.md), "The reserve rule's two entries,
closed".

**The published figure carries two arena legs, not three.** `19.6` registered
uncapped, the worker count plus one, and `2`; the middle leg is dropped at
`19.12`, and the argument against it does not wait on `M76`: a cap set at or
above the concurrency actually dispatched cannot bind, which after `19.3` is a
mechanism rather than a reading, so that leg measures a setting inert by
construction. The pair that remains is what the default is sized for against the
tightest value an operator would plausibly set. **Both are kept even if `M76`
finds no effect at all** — a figure with one leg cannot report a null, and it is
what would notice the day a later allocator or runtime change brings the effect
back. That is the second time in this phase an arena claim has outlived the
build it was measured on, which is the whole argument for keeping the
instrument pointed at it.

**The figure states the budget and the worker count, and that is not the
arrangement the default produces — so it gains flagless legs.** Every reserve
reading this phase has taken pins `--jobs` at this machine's
`available_parallelism()` and states a budget, which is what `19.12` measured
and what `19.15` then showed does not predict a flagless run: under discovery
the count comes *down* with the budget, so the fit's two terms trade places and
the thin point moves to the other end of the axis. A figure whose table is cited
for what the shipped default holds must therefore measure the shipped default.

**A killed leg is a reading, and a killed leg bars publication.** The rule aims
resident at the limit by construction, so a flagless leg that crosses its
allocation is the most informative cell on that axis and the sweep records it
as `killed at <maxrss>` and carries on — losing the whole figure to it, as the
harness does today, throws away every other family for the one reading that was
expected. What it may not do is reach `measurements.md`: a published table whose
own axis says the shipped default kills the process cannot be told, by a reader
who was not at the sitting, from a broken apparatus — which is the ambiguity the
NOT PUBLISHABLE banner exists to remove for a diagnostic sitting and nothing
removes for a published one. So a sitting that publishes `reserve` refuses while
any leg is killed, enforced by the harness as `--figure` already refuses an
entangled figure outside a sweep, rather than remembered by the session that
publishes. This binds `19.11`.

**The stated legs stay, because they are the only ones that can separate the
two terms.** Under discovery `budget = jobs × per_worker` exactly, so the
budget axis and the count axis are the *same* axis and no fit over flagless legs
alone can say whether a byte of resident is owed to a reader or to a budget
byte — which is the question the fixed term turns on. Flagless legs say what the
default holds; stated legs are what makes the number decomposable. Dropping
either leaves a figure that cannot answer one of the two questions asked of it.

Reasoning: [2026-09-11](../status/history/2026-09-11.md), "The budget rule's
headroom fails at 512 MiB, and not where it was predicted to".

## Slice order: allocation, not schedule

**This phase's slice numbers after its evidence slices are allocation order
rather than schedule** (`../process.md`, "Slice numbering"). Its first slices
exist to produce the evidence that decides what the later ones are worth — the
plain-path account decides whether the pool-depth repair belongs to this phase
at all, and the reserve measurement decides the budget rule's one number — so a
list ordered in advance would order the work against the guesses the phase was
convened to replace.

Five orderings bind, and nothing else does:

- **the runtime-invariants register before anything reads `/sys/fs/cgroup`** —
  it is the condition the original rejection named, and it survived the reversal
  as work;
- **the `current_thread` runtime before the reserve is measured**, since it
  changes the thread count the reserve is a reserve for;
- **the plain-path account before any plain default is set**;
- **the closing sweep last, and after `KD16`**, since closing `KD16` re-bases a
  row that sweep publishes;
- **the pool accounting, then the reserve re-take, then the divisor, then the
  container gate, then the compressed account, then the constant, then the
  budget rule** — `19.7`, `19.12`, `19.14`, `19.15`, `19.17`, `19.18`, `19.16`,
  `19.13`. The account precedes the constant for the reason the plain account
  precedes the plain default, and `19.17` precedes `19.18` because an instrument
  is registered before it is read, and **`19.21` precedes `19.18` for the same
  reason one level down** — the harness leg is registered before it is read, and
  the thing it reads has to exist before either. `19.15` and `19.16` joined this chain when
  the gate failed: the rule's one constant now comes from `19.16`'s reading of a
  one-reader block path rather than from `19.12`'s fit, and `19.13` cannot ship
  a number that reading has not produced. The accounting change
  moves every cell of the reserve reading, and the rule's one constant comes
  from that reading, so taking either out of order prices a build about to be
  replaced or ships a number nothing measured. `19.14` joined that chain when
  `19.12` found the compressed charge out by 2.4×: a rule whose constant is
  read off a plan that under-charges is a rule compensating for an accounting
  error with a safety margin, which is the shape this phase exists to remove.
  **`19.15` runs before `19.13` lands, against a scratch build of the rule** —
  the pattern `19.2` used against a clamp-lifted build and `19.6`/`19.12` used
  `--alone`. The reserve's headroom is thinnest through the 1.25–1.5 GiB band,
  which under the container premise is an ordinary allocation size rather than
  a point on an axis, so checking after the rule ships puts the check behind
  the risk. `19.13`'s code is written first and its box ticked second.
  **`19.14` owes no reserve sitting of its own**, which is what keeps it out of
  that chain's evidence half: `19.12` fitted `resident(n) = 180 + 59.4n` over
  sub-stream *counts*, and `19.14` changes only the map from budget to count,
  not what one reader holds. So `resident(budget)` is re-derived from that fit
  rather than re-measured, the corrected counts interpolate inside its range
  rather than extrapolating past it, and admitting fewer readers can only lower
  resident by lowering the thread and arena count — so the derivation errs
  conservative. `19.15` is what would falsify it.
  **`19.14` waits on `xz-seek` and the re-vendor, and so therefore does the
  rest of the phase** — a charge assembled from a constant standing in for the
  decoder's own retention is the same approximation one slice further along,
  and the phase is for removing it rather than relocating it. What that costs
  is a cross-repo dependency and an idle frontier; what it buys is that the
  rule's constant is read off a plan that charges what the file actually
  costs. Reasoning: [2026-09-09](../status/history/2026-09-09.md), "The
  reserve entries, reviewed: the divisor is wrong, not the rule".

## What the reserve actually covers, and what it does not

**The per-reader charge is right; the reserve covers a flat term.** Measured
over `19.16`'s block-path regime the worst-resident slope is 57.28 MiB a reader
against the 58.03 `block_reader_bytes` bills — 0.987 — so glibc's arena
retention is inside the charge, not above it. What is above it is flat:
`worst − 58.03 × jobs` runs 83.6–214.6 MiB with no trend across jobs 2…24.
`19.18`'s reading that retention "rises with the count the allowance affords"
is true of `fordblks` and does not reach the reserve.

**The constant is 384 MiB and it is forced, on this machine's grid.** The
criterion caps the count at 11 at `1g` and 19 at `1536m`, which need a reserve
in (322, 381] and (367, 425] respectively; 384 is the only candidate inside
both. 512 MiB is declined — the criterion was registered before the sitting.

**Two defects the reserve cannot fix are separated out here**, because both
were being read as reasons to raise it and neither is. The charge under-bills
the block pool's floor below `POOL_DEPTH` readers, which is unbounded in the
block size and is `19.22`'s; and the margin at `2g` is bought by this box's
24-core clamp rather than by the reserve, which is `19.23`'s. Reasoning:
[2026-09-12](../status/history/2026-09-12.md), "The reserve entry closes on 384,
and the grilling found the charge wrong below four readers".

## The closing sweep

The phase ends in a sweep, and that sweep is where `M74` closes: it takes
`rss-attribution` alongside `peak-rss`, declares the `Shared` edge that
collapses their disagreeing readings of one command shape, pastes the emitted
section, deletes the `outside-register` marker and the `measure.NOT_OURS` row,
re-generates the stamp's accounting sentence and fills the ledger row's Date.

**Why a sweep is owed is now a different reason than when the phase was
allocated.** It was owed because a `mallopt` in the binary re-bases every
figure's apparatus; that is declined above. The apparatus gaining
`MALLOC_ARENA_MAX` was the replacement reason and is declined too — reviewed
against `M76`'s reading and refused, because capping arenas would buy resident
bytes at the cost of contending the allocator under the parallel figures
([2026-09-09](../status/history/2026-09-09.md), "The arena cap stays off the
apparatus"). So what owes the sweep is whatever the defaults themselves move,
and nothing else.

## Slices

**These numbers after the evidence slices are allocation order, not schedule**
— see "Slice order: allocation, not schedule" above for the five orderings that
bind. A slice admitted after this spec takes the next free number rather than
being inserted.

| Slice | What it does |
|---|---|
| **19.1** | `docs/design/runtime-invariants.md` — the register, and `CLAUDE.md`'s read-trigger beside the Postgres one. No code. |
| **19.2** | The plain-path account: a profile of `--jobs 1` against `--jobs 2`, and a `--alone` sitting against a raised-`POOL_DEPTH` scratch build. No shipped code. |
| **19.3** | The CLI runs a `current_thread` runtime; `rt-multi-thread` leaves its manifest. |
| **19.4** | `Serial` carries an optional budget — closes **`KD16`**. |
| **19.5** | `Partitioning` states its retained unit; `plan_partitions` adds `max_source_span` only for a chunk-shaped source; and a plain source's partition stops being exactly one read chunk, which is 19.2's repair 1 folded in. |
| **19.6** | The reserve figure is registered in `scripts/measure.py` and taken diagnostically to choose the constant. No published table. |
| **19.7** | `BufferPool`'s accounting, and nothing else: the under-report whenever `keeps` admits a buffer larger than `slot_bytes` ([`../status/history/2026-09-09.md`](../status/history/2026-09-09.md), "The plain partition's cap, reviewed"); the **coupling of the block pool's free and retained counts**, without which a stated budget bounds half of what that pool holds; and `BlockCache::affordable` requiring room for **two** units rather than one, since a coupled count of one is the un-poolable shape that pool rejects by name. Re-scoped after the spec was written — see the row below and [2026-09-09](../status/history/2026-09-09.md), "The reserve rule's two entries, closed". |
| **19.8** | The source's own worker default: the trait method, `XzSource`'s override, `ParallelArgs::resolve`, and `DEFAULT_JOBS` removed. |
| **19.9** | Resolution tests, the status line's provenance — `(default: no limit found)` saying what it *means*, that no limit is being enforced — the below-floor `PlanNote`, the v1 fixture tree that tests `RT4`'s shape against the reader — carrying a `meminfo` too, so the no-limit branch's `MemAvailable` cap is driven from the same seam — and a test pinning the below-reserve arrangement so no floor can silently change it. |
| **19.10** | The manual: the `MALLOC_ARENA_MAX` recommendation as `M76`'s reading leaves it, the new defaults, both flags' help text, and the moved whole-block-decode threshold — `19.7` declines a file whose blocks exceed half the budget, where today it declines one whose blocks exceed the budget. |
| **19.11** | The closing sweep — publishes the reserve figure and `rss-attribution`, closing `M74`, and re-takes both `parallel-*` figures against `19.14`'s raised `PARALLEL_BUDGET`. It publishes `reserve` for the first time and so owes the two things the harness assigns to whichever sweep declares its `Shared` edge onto `peak-rss`: the edge, and the `Session.borrow` change that lets an **RSS** reading cross a share, `borrow` copying wall clock only today. |
| **19.12** | The reserve re-taken diagnostically against `19.7`'s build, and the constant chosen from it; `RESERVE_ARENAS` drops its worker-count-plus-one leg. No shipped code, exactly as `19.6`. |
| **19.13** | `discover_memory_limit`, `Parallelism::discover`, and the budget rule, carrying `19.12`'s constant — plus the source's own budget recommendation, which `ParallelArgs::resolve` asks for when `--parallel-memory` is absent as it already asks for a worker count when `--jobs` is. One slice because they are one review question: what a flagless invocation ends up with for a budget. |
| **19.14** | `XzSource::partition_advice` charges a sub-stream what a reader holds — **two** units, the chunk, and `xz_seek::Reader::decode_footprint()` — rather than one; and `BlockCache::affordable` is restated against that same cost, so affording block decode and admitting a reader stop being two sentences. Raises `measure.PARALLEL_BUDGET` 1 GiB → 2 GiB with the harness prose that explains it, the old value no longer admitting the widest row's twenty-four workers; the readings follow at `19.11`. Corrects the manual's "on a compressed file the headroom you need is a multiple of the budget rather than a fixed margin", which is true of the shipped build and false the moment the charge is right — the falsified-claim rule puts it in this change, not in `19.10`. **Blocked on `xz-seek`**: `BlockTask::decode_into` takes only an output slice, so the decoder's own per-decode retention is not visible from here, and the phase waits for the crate to answer rather than shipping a constant standing in for it. |
| **19.15** | The budget rule run in containers at 256 MiB, 512 MiB and 1 GiB with nothing stated, reporting what each discovers and holds, plus one leg stating `--jobs` on a *plain* file — the shape `KD18` makes expensive, which no reading has ever put against a real limit. Its unlimited arm is exercised **without an unbounded run**: the `None` branch is a unit test over a fixture root carrying no limit files (`19.9`'s tree), and the at-scale reading uses a limit set high enough that the source's recommendation is what binds, which is the same arithmetic outcome with a bounded blast radius. **Its first job is the reserve's headroom**, thin at ~7% through the 1.25–1.5 GiB band against per-rep spreads of 6.5–19.8%; a failure there reopens the reserve and a proportional term is what it reopens to. A `runs/` probe, not a figure. |

| **19.16** | The reserve constant, chosen from a reading rather than a fit: candidate reserves **derived from the re-taken account's fixed term** rather than named here, ten reps each, against the **one-reader block path** that every candidate near that term produces and that no sitting has measured. The 512 MiB and 768 MiB this row first named were picked against a fixed term since shown not to be one — see the fourth amendment under "The gate failed" — and a session that measured them would be pricing bytes the charge repair has already moved. It reports the resolved count and the worst-rep headroom against the stated criterion — worst rep leaves at least 20% of the limit — and the constant it picks is what `19.13` then ships. A `runs/` probe on `19.15`'s apparatus, not a figure. |

| **19.17** | The compressed account's instrument, and no library code: `reserve` gains flagless legs beside its stated ones, the reader-count axis is registered at both block sizes, the three mechanism legs — allocator, arena cap, path step — are registered at one, and what it shares with `peak-rss` and `rss-attribution` is re-declared — **one edge and one stated non-edge**, per the section above. Reviewable cold against `scripts/test_measure.py`. |
| **19.18** | The sitting, **diagnostic** — `--alone`, NOT PUBLISHABLE, as `19.6` and `19.12` were: the compressed path's **fixed term and per-reader term**, each with its spread, on a quiet machine. It either attributes the ~280 MiB `19.15` left unexplained or reports that the three legs could not, naming the follow-up experiment. Closes **`KD19`** or rewrites it to what is still true. **Amended: it runs on `19.21`'s instrument, and the black-box legs stop being the attribution.** As written, the row's only instrument was a subtraction between whole runs, which cannot name a term no leg removes — and its named hypothesis then died on arithmetic, leaving a row whose foreseeable outcome was "the three legs could not". The attribution now comes from the process reporting its own live bytes and its allocator's retention; the `getrusage` legs are kept as the **check** on it, two instruments sharing no mechanism being the independence the account has never had. The mechanism legs are **re-aimed or dropped** rather than run as registered: the allocator pair cannot test the live hypothesis, since swapping glibc removes the threshold behaviour instead of measuring it. Reasoning: [2026-09-11](../status/history/2026-09-11.md), "Attribution was being done with the gate's instrument". |

| **19.17.1** | A killed leg is recorded as a reading and the sitting continues; a killed leg bars publication of the figure. An **earned third level**: `19.17` shipped the wrong contract, registering a `RESERVE_LIMITS` docstring that calls an OOM kill "a reading rather than an apparatus failure" beside a sweep that raises and loses the whole figure. Re-filed from the withdrawn `M82`, which was not out-of-band because it reverses that decision. Two facts the row did not have: the harness **cannot tell an OOM kill from any other failure**, `rss_wrapper` collapsing every signal death to exit 1 and `--rm` destroying the container before `inspect` could say — so the oracle is `/sys/fs/cgroup/memory.events`' `oom_kill` count, read inside the container after the timed command; and the licence is **per family**, never harness-wide, since `parallel-peak-rss` once measured 3067 MiB in a 3072 MiB container and a kill there is the apparatus failure that being loud caught. A killed cell is a *censored* reading and needs a third cell state rather than a number. |
| **19.19** | The per-file term, billed where the file is open, and `fit`'s divisor — the repair the fifth amendment names, and the one slice in this phase whose evidence is a **unit test** rather than a sitting. `partition_bytes` is both the memory charge and `run_region`'s cut size, so `19.14` raising it to `reader_bytes` made every partition 2.08–2.42 blocks and sent `scan_partition`'s first read down `read_by_blocks`' copying arm, into an unpoolable buffer of partition length held through the parse and billed one chunk. Restore the one-block partition (or make that first read chunk-sized like its tail) and bill what `reader_bytes` misses where the source is open; in the same slice, divide the allowance by the affordability charge rather than by the recommendation, which costs a reader at every allocation. Acceptance is two-sided: the 128 MiB flagless family survives or declines with nothing killed, **and** the pre/post probe shows the four-worker throughput ceiling lifted. Owns **`KD19`**. |
| **19.20** | The cut width, decided by measurement — the route `19.19` took on a judgement call, reopened. `19.19` fixed a partition-length first read by shrinking the cut to one block, which re-couples the cut to the source's retained unit and costs throughput below about five stated readers (2 readers: 11.2 s → 17.9 s on the 24 MiB control). The waste is one successor-block decode **per piece** (`KD20`), so the cut width is what amortises it: one block pays 100%, `19.14`'s 2.42-block piece paid 41%, eight blocks would pay 12.5% — and retention is capped at `BufferPool::slots`, not by piece width, so a wider cut does not widen the charge. Make `scan_partition`'s first read chunk-sized like its tail, widen the cut past one block, and time it against both existing builds at stated and flagless counts; keep whichever wins. **The outcome is open** — `19.14`'s flat-above-four ceiling is unexplained and may survive the read-shape fix, in which case what is shipped stays. Re-pin the invariant on the **read size** (no read exceeds `chunk_size`), which is what keeps every buffer poolable and survives either outcome; `a_block_decoding_partition_never_crosses_a_block_boundary` and `window_end`'s "every piece lies within a single unit" pin the arrangement under review and must not be left as settled intent. **Amended by the sitting, in two places.** The chunk-sized read is not one decision: it was kept on the plain path, where it holds 9.4 MiB at `--jobs 24` against 209.2, and refused on the compressed one, where it costs 76 MiB of median resident and crossed a 1 GiB limit once in three — so the read shape is stated **by the source** (`io::PartitionRead`) rather than by the leader, which is a mechanism this row did not name. And the read-size invariant is **refused rather than deferred**: "no read exceeds `chunk_size`" is not a property of the arrangement that won, so the clause's stated ground — that it "survives either outcome" — is exactly what the measurement falsified. What is pinned instead is the **cut width**, a piece spanning at most `BOUNDARIED_PARTITION_UNITS` of the source's own units, plus the chunked half over the plain source. Reasoning: [2026-09-11](../status/history/2026-09-11.md), "`19.20`'s refused clause, and the read shape as the source's statement". Does **not** close `KD20`: a wider cut amortises the wasted decode, only an in-flight map removes it. |
| **19.21** | **The introspection the account needs, built before the account is taken.** One cargo feature on `pgdump_query-cli`, off by default and joining `alloc.rs`'s existing at-most-one guard, under which the binary reports what it holds: a counting `#[global_allocator]` over `System` maintaining **live bytes and their high-water**, allocator-independent and exact; and glibc's own statistics at exit — `mallinfo2`'s `arena`, `hblkhd`, `uordblks`, `fordblks`, and `malloc_info`'s per-arena `system current` and `system max`. Emitted as `key=value` lines, which `measure.parse_reported` already reads. **Neither quantity needs a sampler**: the counter keeps its own high-water and `malloc_info` keeps each arena's, which is what makes this minutes rather than an instrumented rerun of the sweep. **No library code** — the allocator is the binary's choice and never the library's ([`architecture.md`](architecture.md), "The allocator is the binary's choice"), which is also what keeps `P6`'s embedder unburdened. **Not a fourth `ALLOCATOR_LEGS` member**: that tuple is the `allocator` figure's published table, and a binary carrying an atomic per allocation does not belong in a timing comparison with three that do not. The perturbation is stated rather than bounded — **this build never times anything**. The instrument owes its own check, an instrument nobody can falsify being the trap a figure nobody can re-take already is: the feature build must resolve the same `jobs=`/`memory_bytes=` pair as the default build on the same input, so the instrument is shown not to have moved the plan it reports on. |
| **19.22** | **The charge bills the pool floor.** `block_reader_bytes` charges `2 × unit` a reader, which assumes `slots == jobs`; `BufferPool::slots` clamps the block pool at `POOL_DEPTH.max(jobs)`, so below four readers the pool holds units nobody paid for — `(POOL_DEPTH − jobs) × unit`, confirmed to 1.4 MiB over five cells of `19.16`'s readings. The cost is no longer linear in `jobs`, so `Parallelism::fit` **solves** for the largest affordable count rather than dividing a cap by a per-worker scalar. Unbounded in the block size (96 MiB at 24 MiB blocks, 384 at 128, 2 GiB at 512), which is why no reserve absorbs it. |
| **19.23** | **The count answers to the criterion, not to the core count.** `2g` clears the margin only because `available_parallelism` clamps 28 readers to this machine's 24; a 64-core host resolves 28 and is predicted to breach. Once `19.22` makes the cost a model, `fit` refuses a count whose predicted resident breaches the stated margin, which removes the dependence rather than documenting it. |
| **19.24** | **The harness checks the model, rather than searching for a constant.** `19.16` spent 400 runs to pick one integer and the account that mattered came out of arithmetic over its `readings.json` afterwards. A registered check predicts held bytes per cell from the charge model, measures, and asserts the residual is small and non-negative — which is what surfaces an under-bill, and what a grid search structurally cannot report. Seeded from readings already in the tree; no new sitting. |


**`19.14` through `19.21` were admitted after this spec was written**, and take the
next free numbers rather than being inserted. `19.14` reverses a repair this
spec's evidence section rejected: charging a compressed worker what it holds
was refused because "it would admit fewer readers for exactly the same resident
set", which rested on resident being a function of the stated budget rather
than of the worker count — falsified by `19.12`'s plain leg, which saturates at
twenty-four workers and goes flat while the budget doubles. `19.15` exists
because every reserve reading this phase took was made at `-m 3g` with the
budget *stated*, while discovery's whole purpose is the small allocation
nothing has ever run in. `19.16` exists because `19.15`'s failure left the
constant needing an arrangement nothing had measured, and the phase had already
paid once for extrapolating a fit onto an arrangement it never took. `19.17` and
`19.18` exist because this spec gave the plain path an account before its default
and never wrote the compressed half — they are the missing prerequisite, not a
follow-up, which is why they run **ahead of** `19.16`: a constant chosen before
the attribution is chosen against the same unexplained term that broke the last
one. They split at the seam `../process.md` names, the instrument being
reviewable cold and the sitting being a machine-quiet reading whose review
question is whether its numbers support a constant. Reasoning:
[2026-09-09](../status/history/2026-09-09.md), "The reserve entries, reviewed:
the divisor is wrong, not the rule".

**`19.21` exists because `19.17` and `19.18` split at that seam and both landed
on the wrong side of a different one.** The instrument `19.17` registered is a
*harness* instrument — more legs of the same subtraction — and the account it
serves is an attribution, which a subtraction cannot produce. That was not
visible at the seam the pair was split on, because both halves were correct
about the sitting and neither asked whether the sitting could answer the
question. `19.21` is the instrument the account actually needs, and it is a
third slice rather than a re-scope of `19.17` because `19.17` shipped what its
row promised and its legs are kept: what changed is that they are now the check
rather than the answer. Reasoning:
[2026-09-11](../status/history/2026-09-11.md), "Attribution was being done with
the gate's instrument".

**`19.7` was re-scoped and split after this spec was written, and the two new
rows take the next free numbers rather than being inserted.** As specified it
carried the accounting change, the discovery primitives and the budget rule at
once; the accounting change grew when the block pool's coupling joined it, and
the rule cannot pick its constant until the reserve has been re-read on the
build the accounting change produces — a loop the row could not contain. The
three now land in order: `19.7`, then `19.12`, then `19.13`. The alternative
was one slice taking its own mid-slice diagnostic sitting, which keeps the
review question whole ("does the stated number now bound the pool") at the cost
of asking it over a diff that also introduces discovery; the split was taken
because the accounting half is a rework of an already-tested concurrent
structure and earns its own review. Reasoning:
[2026-09-09](../status/history/2026-09-09.md), "The reserve rule's two entries,
closed".

**19.2's account has been taken, and it lands on both branches rather than one.**
The fork above asks which of two causes the account lands on; it names three,
and they route differently. The **sizing** term (a plain partition is exactly one
read chunk) and the **depth** term (four slots whatever `--jobs` states) are the
first branch — both are arguments of `pool.set_limits` — and their repair is
**folded into 19.5**, which already opens `Partitioning` to state its retained
unit, rather than admitted as a number of its own. The **unattributed 4→8 step**
and the **refuted typed-`query` suspect** are the second branch: out of this
phase, `KD17` allocated for the second of them, and 19.8 ships the plain default
with the reason written down.

**A repair reaching `leader::scan_partition` was ruled out of this phase and is
now admitted, in `19.20`.** The original reason was that it changes what the
leader does per piece for every source. That is true and is not the deciding
consideration: which repair is right is decided by what the repairs do, not by
which phase they are filed under, and `scan_partition` has exactly one caller —
the cold interior split, whose plain arm a user reaches only by stating
`--jobs`. Reasoning:
[2026-09-11](../status/history/2026-09-11.md), "The route is judged on the
repair, not on which phase it belongs to".

**The plain source's own worker default is `Serial`, and 19.8 is not blocked on
anything further.** A plain `parse` is slower than serial at every worker count
in both builds the sitting timed, reaching 0.87× at its best point with the
pool-depth clamp lifted; the first branch's repair shrinks a cost rather than
making the shape scale. That is the defensible default this section demanded in
place of "serial, because the number is bad and we do not know why". Only a
reading showing a plain parallel `parse` beating serial reopens it.
([`../status/history/2026-09-09.md`](../status/history/2026-09-09.md), "The
plain-path fork routes to both branches".)
