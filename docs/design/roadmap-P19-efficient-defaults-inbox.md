# P19 inbox — efficient defaults for a parallel scan

Facts found before this phase was specified, filed by destination
(`../process.md`, "Inboxes: facts filed by destination"). **Read this before
grilling or specifying P19, and drain it as part of that grilling** — fold each
entry into the spec or discard it as stale, then delete this file.

## One span allowance is charged per sub-stream, and the default may be wrong

**Fact.** `QueryOptions::max_source_span` defaults to 64 MiB and is charged to
*each* sub-stream: every one holds its own batcher pinning its own chunks, so N
sub-streams pin N × 64 MiB. `plan_partitions` divides the stated budget by
`partition_bytes + max_source_span` accordingly, which is right as
accounting. The consequence is that the divisor is dominated by pinning rather
than by decoding — 64 MiB against a 24 MiB block — and at the shipped defaults
(`--parallel-memory` 64 MiB) the span term alone meets the budget, so
`pgdq query --jobs N` plans one sub-stream for every N.

**Why this phase cares.** It is the difference between a parallel query being
affordable at the defaults and being unreachable at them, which is this phase's
subject. The alternative the divisor's own review did not weigh: *divide* one
span allowance among sub-streams instead of charging it to each, so pinning
stays flat in N. That changes a shipped default, so it is a spec question
rather than a slice.

**Origin.** The stated-budget review, 2026-09-08 — the grilling of the
`PARALLEL_BUDGET` entry under STATUS's "Decisions worth another look". Detail
beside the mechanism: [`architecture.md`](architecture.md), "Execution model
and API surface".

## Half of resource discovery is `std`'s already

**Fact.** Rust's `available_parallelism` reads the cgroup CPU quota. The koji
verification's probe at `--cpus 4` seeded eight glibc arenas rather than the twenty-four a
24-CPU host would give, which is what demonstrated it. There is no `std`
equivalent for the memory limit.

**Why this phase cares.** It halves the work and it decides what the runtime
invariants register has to carry: the CPU side needs no entry, the memory side
needs all of it — cgroup v2 `memory.max` and `memory.high`, v1
`memory.limit_in_bytes`, the unlimited sentinel, and what a nested or hybrid
hierarchy reports.

**Origin.** The koji verification, 2026-09-08 ([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md),
"The `16.14` OOM is glibc's arenas, and a CPU limit is not the remedy").

## The arenas are seeded before a byte is read, and `--jobs` does not size them

**Fact.** `main.rs`'s bare `#[tokio::main]` builds one worker thread per
*visible* CPU whatever `--jobs` says, so a serial parse on a 24-CPU host seeds
24 glibc arenas at `t=0`. `MALLOC_ARENA_MAX=2` moved a koji leg from 476 MiB to
328 MiB anonymous resident; `--cpus 4` alone still reached eight arenas and
~476 MiB, so sizing the runtime from `--jobs` is not the same lever and does not
work on its own.

**Why this phase cares.** A budget discovered from the cgroup and then blown by
arenas the process seeded at startup is not a sound default. This is why the
phase's order is bound: cap the process's own overhead first, *then* discover,
*then* set the default from what was discovered. Note the price — a
`mallopt(M_ARENA_MAX, …)` re-bases the apparatus of every registered figure.

**Origin.** The koji verification, 2026-09-08; previously a roadmap `Future`
item, moved into this phase.

## Both budget announcements are already shipped, so this phase inherits them built

**Fact.** Both announcements this phase would have specified exist. One names a
`--jobs` the stated budget refuses; the other names a budget-declined `.xz`
block path, carrying the file's largest block beside the budget, and adds the
container's shape to `info --detail`/`--json` off the persisted seek table.
Both are `PlanNote`s on `TableStream::plan_notes` rather than the
`DiagnosticKind` the second was first expected to be — the channel went the
other way because every `DiagnosticKind` is a property of the file and a budget
decline is not ([`architecture.md`](architecture.md), "Execution model and API
surface").

**Why this phase cares.** Its spec must not re-specify either: they exist, and
a spec row committing to an announcement that already ships would make the
phase unmeasurable against what it delivered. What is left for this phase is
the *defaults* — whether the shipped budget and worker count are the right
numbers — and both announcements now argue for it with a message a user can act
on rather than with silence. `KD16` is the one live remainder in this area:
`--parallel-memory` is dropped at the default `--jobs 1`, so the recourse both
messages name needs `--jobs 2` stated beside it
([`architecture.md`](architecture.md), "Execution model and API surface").

The harness-hygiene work around them stays out-of-band and is unaffected, with
one exception: `M74` carries an ordering this phase owns, below.

**Origin.** 2026-09-08, this phase's grilling; rewritten the same day as each
piece landed.

## This phase's closing sweep publishes `rss-attribution`, and closes `M74`

**Fact.** The instrument has landed: `scripts/rss_attribution.py` is folded
into `scripts/measure.py`, and `rss-attribution` is a registered figure
in `measure.UNTAKEN` with four command shapes of its own. What has **not**
landed is its table, and that is `M74` — the nine medians `measurements.md`
prints under "What the per-block resident growth is made of" were printed by
the standalone script at `41c96bb`, so the section still carries an
`outside-register` marker. Its `parse` reference row runs `peak-rss`'s
`blocks500` and `blocks4000` shapes, and the two must share one reading rather
than take one each: today they take one each and disagree, 9.58/44.26 MiB here
against 9.73/43.78 under `peak-rss`. Only a stamped sweep may publish it. The
`Shared` edge that collapses the two is declared in the change that takes the
sitting, because declaring it earlier closes no part of `M74` — the marker
still could not go on — and makes `sitting_problems` refuse `peak-rss`'s own
`41c96bb` marker with no sweep in between to clear it;
`test_a_taken_attribution_declares_its_borrow` holds the obligation. `M71` was
separate and has landed: a regime is a declared `measure.REGIMES` row, and one
the harness does not carry is refused rather than resolved to the warm path.

**Why this phase cares.** This phase ends in a sweep, because a
`mallopt(M_ARENA_MAX, …)` re-bases every registered figure's apparatus — and
that sweep is where `M74` closes. In one change it takes `rss-attribution`
alongside `peak-rss`, pastes the emitted section, deletes the
`outside-register` marker and the `measure.NOT_OURS` row, re-generates the
stamp's accounting sentence and fills the ledger row's Date. `M71` landed
ahead of it for the reason it was queued: this is the first phase with a reason
to add a regime, and adding one now costs a `REGIMES` row and a
`CONTENTION_LIMITS` row rather than publishing warm readings under a cold
heading with no error.

**Origin.** 2026-09-08, the review of the ledger's blocking column; restated
the same day the instrument landed and `M74` was admitted for the
sitting ([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md),
"`M65` lands its instrument; `M74` owns the sitting"). `M74` carries no
`Blocks`: that column names the open phase, and there is none.

## The plain path is slower parallel, and that is what makes the default source-dependent

**Fact.** `parallel-scan-throughput`: a plain `parse` reads **0.81×** its own
serial row at twenty-four workers and is already negative at two — 0.471 s
serial against 0.512 s — while the same scan on `.xz` reads **5.82×**. A plain
typed `query` is flat at 0.98× across the whole range, and a compressed one
reaches only 1.60×.

**Why this phase cares.** It is the evidence that one `--jobs` default cannot
be right, and it is the reason the default resolves after source recognition.
The regression at *two* workers, below `POOL_DEPTH`'s four-slot clamp, also
says the plain-path cost is not only the pool — something this phase should
account for before choosing the plain default.

The contingency this was filed under is **discharged**: the figure has since
been re-taken under the two-term divisor, which moved the two typed-`query`
columns and left the `parse` ones alone, `pgdq parse` never reaching
`plan_partitions`. The numbers above are the published ones.

**Origin.** The parallel figures' sitting, 2026-09-08.

## Discovery is a library mechanism whose default the library does not take

**Fact.** The library defaults to serial — "an embeddable component does not
spawn threads by surprise" — with embeddability a project goal
([`architecture.md`](architecture.md), "Execution model and API surface"). That
decision is **not** reversed, and it does not have to be for the CLI to
discover: the library *offers* the mechanism, and the library's own default
stays serial. The CLI is what calls it.

**Why this phase cares.** It decides where the code goes and prevents the
obvious two mistakes. Putting discovery only in the CLI duplicates it away from
the type it configures and leaves an embedder who *wants* it with nothing to
call; putting it in the library's default path breaks a decision the project
made for a stated goal. The shape that satisfies both: a constructor or free
function beside `Parallelism` that an embedder may call and nothing calls for
them, with `ParallelArgs::resolve` the one caller in this repo.

**Origin.** 2026-09-08, this phase's grilling.

## Changing the `--jobs` default is free of the figure register

**Fact.** Every registered figure's command shape, the profile recipe and the
koji recipe state `--jobs` explicitly, and `--check` refuses a shape that pins
no count ([`measurements.md`](measurements.md), "The apparatus"). So no figure
inherits the CLI default any more.

**Why this phase cares.** `DEFAULT_JOBS` is 1, and the reason given for it was
that a person stating no flag gets "the arrangement every published figure was
taken under" — a measurement rationale standing in for a user-facing default.
That rationale is spent: this phase may set the default from what the evidence
says a user should get, and no figure moves underneath it. What *would* move a
figure is the arena cap, which is a separate obligation and is why this phase
ends in a sweep.

**Origin.** The worker-count apparatus rule, 2026-09-07; restated here
2026-09-08.
