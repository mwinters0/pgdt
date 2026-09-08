# P19 inbox — efficient defaults for a parallel scan

Facts found before this phase was specified, filed by destination
(`../process.md`, "Inboxes: facts filed by destination"). **Read this before
grilling or specifying P19, and drain it as part of that grilling** — fold each
entry into the spec or discard it as stale, then delete this file.

## One span allowance is charged per sub-stream, and the default may be wrong

**Fact.** `QueryOptions::max_source_span` defaults to 64 MiB and is charged to
*each* sub-stream: every one holds its own batcher pinning its own chunks, so N
sub-streams pin N × 64 MiB. `16.15` made `plan_partitions` divide the stated
budget by `partition_bytes + max_source_span` accordingly, which is right as
accounting. The consequence is that the divisor is dominated by pinning rather
than by decoding — 64 MiB against a 24 MiB block — and at the shipped defaults
(`--parallel-memory` 64 MiB) the span term alone meets the budget, so
`pgdq query --jobs N` plans one sub-stream for every N.

**Why this phase cares.** It is the difference between a parallel query being
affordable at the defaults and being unreachable at them, which is this phase's
subject. The alternative not weighed by `16.15` or its amendment: *divide* one
span allowance among sub-streams instead of charging it to each, so pinning
stays flat in N. That changes a default `P16` committed to, so it is a spec
question rather than a slice.

**Origin.** `16.15`'s review, 2026-09-08 — the grilling of the
`PARALLEL_BUDGET` entry under STATUS's "Decisions worth another look". Detail
beside the mechanism: [`architecture.md`](architecture.md), "Execution model
and API surface".

## Half of resource discovery is `std`'s already

**Fact.** Rust's `available_parallelism` reads the cgroup CPU quota. `16.14`'s
probe at `--cpus 4` seeded eight glibc arenas rather than the twenty-four a
24-CPU host would give, which is what demonstrated it. There is no `std`
equivalent for the memory limit.

**Why this phase cares.** It halves the work and it decides what the runtime
invariants register has to carry: the CPU side needs no entry, the memory side
needs all of it — cgroup v2 `memory.max` and `memory.high`, v1
`memory.limit_in_bytes`, the unlimited sentinel, and what a nested or hybrid
hierarchy reports.

**Origin.** `16.14`, 2026-09-08 ([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md),
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

**Origin.** `16.14`, 2026-09-08; previously a roadmap `Future` item, moved into
this phase.

## `M67` and `M72` are this phase's subject, not out-of-band work

**Fact.** Two ledger rows are queued and unlanded, and both are "the shipped
value is wrong and the tool is silent about it": `M67`, the budget-declined
`.xz` fallback announcing itself with the block size beside the budget that
declined it; and `M72`, a `--jobs` the stated budget refuses saying so.

**Why this phase cares.** Once this phase's spec records the announcements as
decisions, they stop meeting the ledger's admission rule — work that changes a
decision a spec records is not out-of-band. **Strike both rows as part of
specifying this phase**, with their numbers spent, and cite them from the
slices that absorb them. `M65`, `M66` and `M71` are measurement-harness
hygiene, stay out-of-band, and are unaffected.

**Origin.** 2026-09-08, this phase's grilling.

## The plain path is slower parallel, and that is what makes the default source-dependent

**Fact.** `parallel-scan-throughput` at `e29939c`: a plain `parse` reads
**0.76×** its own serial row at twenty-four workers and is already negative at
two — 0.426 s serial against 0.561 s — while the same scan on `.xz` reads
**5.93×**. A plain typed `query` is flat at 0.98–0.99×.

**Why this phase cares.** It is the evidence that one `--jobs` default cannot
be right, and it is the reason the default resolves after source recognition.
The regression at *two* workers, below `POOL_DEPTH`'s four-slot clamp, also
says the plain-path cost is not only the pool — something this phase should
account for before choosing the plain default.

**Contingent on** the `16.15.1` re-take: that sitting re-measures this figure
with the two-term divisor, and the two typed-`query` columns will move. The
`parse` columns will not — `pgdq parse` never reaches `plan_partitions`.

**Origin.** `16.13.1`'s sitting, 2026-09-08.

## Discovery is a library mechanism whose default the library does not take

**Fact.** `P16` settled that the library defaults to serial — "an embeddable
component does not spawn threads by surprise" — with embeddability a project
goal. That decision is **not** reversed, and it does not have to be for the CLI
to discover: the library *offers* the mechanism, and the library's own default
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

**Fact.** `M69` made every registered figure's command shape, the profile
recipe and the koji recipe state `--jobs` explicitly, and `--check` refuses a
shape that pins no count. So no figure inherits the CLI default any more.

**Why this phase cares.** `16.16` set `DEFAULT_JOBS = 1` and gave as its reason
that a person stating no flag gets "the arrangement every published figure was
taken under" — a measurement rationale standing in for a user-facing default.
That rationale is spent: this phase may set the default from what the evidence
says a user should get, and no figure moves underneath it. What *would* move a
figure is the arena cap, which is a separate obligation and is why this phase
ends in a sweep.

**Origin.** `M69`, 2026-09-07; restated here 2026-09-08.
