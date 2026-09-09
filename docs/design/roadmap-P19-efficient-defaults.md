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

So the rule is `budget = limit − reserve`, floored at today's 64 MiB constant,
with a **fraction as a ceiling only** — `min(fraction × limit, limit −
reserve)`. The ceiling exists so that a very large allocation is not handed to
the pools whole; it bounds nothing anyone has measured, and it is defensible as
"we do not take an allocation we cannot show we use" rather than as a number.

**What is read is the minimum over every limit that binds**: cgroup v2
`memory.max` and `memory.high`, v1 `memory.limit_in_bytes`, and every ancestor
cgroup rather than the nearest. `memory.high` throttles where `memory.max`
kills, and sustained reclaim ends a scan's throughput as surely as an OOM ends
the run; an ancestor's limit binds whichever level states it.

**An unlimited environment falls back to today's constant**, as settled going
in.

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
`MALLOC_ARENA_MAX` at the recommended value, one without — differing by roughly
the 200 MiB the arenas hold. The default has to survive the case where the
recommendation was not followed, because that is the case that kills the
process.

**The price is real and is written here so that it is met as a cost rather than
rediscovered as a defect**: the operator who *did* cap arenas gets a smaller
budget than their machine could support, because the shipped reserve is sized
for the operator who did not. That is what the cap being a recommendation rather
than a mechanism buys and costs, and it is the strongest argument for reversing
the decision at the top of this spec. It is not reopened here.

## Slice order: allocation, not schedule

**This phase's slice numbers after its evidence slices are allocation order
rather than schedule** (`../process.md`, "Slice numbering"). Its first slices
exist to produce the evidence that decides what the later ones are worth — the
plain-path account decides whether the pool-depth repair belongs to this phase
at all, and the reserve measurement decides the budget rule's one number — so a
list ordered in advance would order the work against the guesses the phase was
convened to replace.

Four orderings bind, and nothing else does:

- **the runtime-invariants register before anything reads `/sys/fs/cgroup`** —
  it is the condition the original rejection named, and it survived the reversal
  as work;
- **the `current_thread` runtime before the reserve is measured**, since it
  changes the thread count the reserve is a reserve for;
- **the plain-path account before any plain default is set**;
- **the closing sweep last, and after `KD16`**, since closing `KD16` re-bases a
  row that sweep publishes.

## The closing sweep

The phase ends in a sweep, and that sweep is where `M74` closes: it takes
`rss-attribution` alongside `peak-rss`, declares the `Shared` edge that
collapses their disagreeing readings of one command shape, pastes the emitted
section, deletes the `outside-register` marker and the `measure.NOT_OURS` row,
re-generates the stamp's accounting sentence and fills the ledger row's Date.

**Why a sweep is owed is now a different reason than when the phase was
allocated.** It was owed because a `mallopt` in the binary re-bases every
figure's apparatus; that is declined above, so what owes it is the apparatus
gaining `MALLOC_ARENA_MAX` and whatever the defaults themselves move.

## Slices

**These numbers after the evidence slices are allocation order, not schedule**
— see "Slice order: allocation, not schedule" above for the four orderings that
bind. A slice admitted after this spec takes the next free number rather than
being inserted.

| Slice | What it does |
|---|---|
| **19.1** | `docs/design/runtime-invariants.md` — the register, and `CLAUDE.md`'s read-trigger beside the Postgres one. No code. |
| **19.2** | The plain-path account: a profile of `--jobs 1` against `--jobs 2`, and a `--alone` sitting against a raised-`POOL_DEPTH` scratch build. No shipped code. |
| **19.3** | The CLI runs a `current_thread` runtime; `rt-multi-thread` leaves its manifest. |
| **19.4** | `Serial` carries an optional budget — closes **`KD16`**. |
| **19.5** | `Partitioning` states its retained unit; `plan_partitions` adds `max_source_span` only for a chunk-shaped source. |
| **19.6** | The reserve figure is registered in `scripts/measure.py` and taken diagnostically to choose the constant. No published table. |
| **19.7** | `discover_memory_limit`, `Parallelism::discover`, and the budget rule. |
| **19.8** | The source's own worker default: the trait method, `XzSource`'s override, `ParallelArgs::resolve`, and `DEFAULT_JOBS` removed. |
| **19.9** | Resolution tests, the status line's provenance, the below-floor `PlanNote`, and the v1 fixture tree that tests `RT4`'s shape against the reader. |
| **19.10** | The manual: the `MALLOC_ARENA_MAX` recommendation, the new defaults, and both flags' help text. |
| **19.11** | The closing sweep — publishes the reserve figure and `rss-attribution`, closing `M74`. |

**If 19.2's account lands on the pool depth**, the repair is admitted as the
next free number and run before 19.8, per the binding above. If it lands on
coordination the leader pays per worker, it takes a `KD<k>` instead and 19.8
ships the plain default with the reason written down.
