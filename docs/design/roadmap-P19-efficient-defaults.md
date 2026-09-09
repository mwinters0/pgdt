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
source"), and the rule below is correct as written once it does; without that
repair `limit − reserve` bounds nothing on the compressed path and the fraction
is doing undeclared work.
Evidence: [2026-09-09](../status/history/2026-09-09.md), "The reserve rule's
two entries, closed".

So the rule is `budget = limit − reserve`, floored at today's 64 MiB constant,
with a **fraction as a ceiling only** — `min(fraction × limit, limit −
reserve)`. The ceiling exists so that a very large allocation is not handed to
the pools whole; it bounds nothing anyone has measured, and it is defensible as
"we do not take an allocation we cannot show we use" rather than as a number.

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
`XzSource` answers with what its recommended count needs — twenty-four workers
at ~59 MiB is ~1.4 GiB — and `LocalFileSource` answers with no budget, as it
answers with one worker today. A count nothing can afford is not a
recommendation, which is the general form
([`roadmap.md`](roadmap.md), "A default runs as fast as the machine or the
cgroup permits").

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

**The status line reports the value and its provenance**, which is a slot that
already exists: it prints `memory_bytes=67108864 (default)` today, and gains
`(stated)`, `(discovered: …)` naming which limit was read, and
`(default: no limit found)`.

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
registered figure, taken in the closing sweep beside those two — all three read
resident on shared shapes, so it declares `shares` edges and the harness
re-takes the set together, which is the same collapse `M74` exists to make.

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
  budget rule** — `19.7`, `19.12`, `19.14`, `19.13`. The accounting change
  moves every cell of the reserve reading, and the rule's one constant comes
  from that reading, so taking either out of order prices a build about to be
  replaced or ships a number nothing measured. `19.14` joined that chain when
  `19.12` found the compressed charge out by 2.4×: a rule whose constant is
  read off a plan that under-charges is a rule compensating for an accounting
  error with a safety margin, which is the shape this phase exists to remove.
  `19.15` follows `19.13`, having nothing to run until the rule ships.
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
| **19.9** | Resolution tests, the status line's provenance, the below-floor `PlanNote`, and the v1 fixture tree that tests `RT4`'s shape against the reader. |
| **19.10** | The manual: the `MALLOC_ARENA_MAX` recommendation as `M76`'s reading leaves it, the new defaults, both flags' help text, and the moved whole-block-decode threshold — `19.7` declines a file whose blocks exceed half the budget, where today it declines one whose blocks exceed the budget. |
| **19.11** | The closing sweep — publishes the reserve figure and `rss-attribution`, closing `M74`. |
| **19.12** | The reserve re-taken diagnostically against `19.7`'s build, and the constant chosen from it; `RESERVE_ARENAS` drops its worker-count-plus-one leg. No shipped code, exactly as `19.6`. |
| **19.13** | `discover_memory_limit`, `Parallelism::discover`, and the budget rule, carrying `19.12`'s constant — plus the source's own budget recommendation, which `ParallelArgs::resolve` asks for when `--parallel-memory` is absent as it already asks for a worker count when `--jobs` is. One slice because they are one review question: what a flagless invocation ends up with for a budget. |
| **19.14** | `XzSource::partition_advice` charges a sub-stream what a reader holds — **two** units, the chunk, and `xz_seek::Reader::decode_footprint()` — rather than one; and `BlockCache::affordable` is restated against that same cost, so affording block decode and admitting a reader stop being two sentences. **Blocked on `xz-seek`**: `BlockTask::decode_into` takes only an output slice, so the decoder's own per-decode retention is not visible from here, and the phase waits for the crate to answer rather than shipping a constant standing in for it. |
| **19.15** | The budget rule run in containers at 256 MiB, 512 MiB and 1 GiB with nothing stated, reporting what each discovers and holds. A `runs/` probe, not a figure. |

**`19.14` and `19.15` were admitted after this spec was written**, and take the
next free numbers rather than being inserted. `19.14` reverses a repair this
spec's evidence section rejected: charging a compressed worker what it holds
was refused because "it would admit fewer readers for exactly the same resident
set", which rested on resident being a function of the stated budget rather
than of the worker count — falsified by `19.12`'s plain leg, which saturates at
twenty-four workers and goes flat while the budget doubles. `19.15` exists
because every reserve reading this phase took was made at `-m 3g` with the
budget *stated*, while discovery's whole purpose is the small allocation
nothing has ever run in. Reasoning:
[2026-09-09](../status/history/2026-09-09.md), "The reserve entries, reviewed:
the divisor is wrong, not the rule".

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

**A repair reaching `leader::scan_partition` is out of this phase** — it changes
what the leader does per piece for every source, which is the second branch's
shape, not a default.

**The plain source's own worker default is `Serial`, and 19.8 is not blocked on
anything further.** A plain `parse` is slower than serial at every worker count
in both builds the sitting timed, reaching 0.87× at its best point with the
pool-depth clamp lifted; the first branch's repair shrinks a cost rather than
making the shape scale. That is the defensible default this section demanded in
place of "serial, because the number is bad and we do not know why". Only a
reading showing a plain parallel `parse` beating serial reopens it.
([`../status/history/2026-09-09.md`](../status/history/2026-09-09.md), "The
plain-path fork routes to both branches".)
