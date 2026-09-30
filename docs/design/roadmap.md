# Roadmap

Where this project is going. Each phase that has been specified gets its own
doc, `roadmap-P<N>-<slug>.md`; the sections below are the index. Phases
still sketched here are at a level sufficient to keep current work from
painting us into a corner — **each gets its own full grilling session when it
becomes current**, and the resulting spec becomes its own doc.

**`P<k>` is an identifier, not a position** (`../process.md`, "Phase identity
is `P<k>`"). Phases are discovered work: they are allocated numbers in the
order they are *found*, run in the order that suits, and sometimes run two at a
time. **The order is this table, top to bottom** — never the numbers, and never
the slug beside them, which is a caption rather than a name. Numbers are never
reused, including a struck phase's.

| Phase | State | Where it is |
|---|---|---|
| P1–P7, P9–P14, P16, P17, P19, P20, P25, P27 | **Struck** at a keystone review | [`decisions.md`](decisions.md); git holds the specs |
| P28 — unrepresentable values | Current | [spec](roadmap-P28-unrepresentable-values.md); checklist in [`../status/STATUS.md`](../status/STATUS.md) |
| P22 — the third tunable | Sketched; not grilled | this file, below |
| P21 — statistics gathered by a query | Sketched; not grilled | this file, below; [inbox](roadmap-P21-query-statistics-inbox.md) |
| P23 — statistics coverage and the resident reserve | Sketched; not grilled | this file, below |
| P26 — statistics refused on their cost, reconsidered | Sketched; not grilled | this file, below |
| P15 — gzip input | Sketched; not grilled | this file, below; [inbox](roadmap-P15-gzip-inbox.md) |
| P18 — zstd and lz4 input | Sketched; not grilled | this file, below; [inbox](roadmap-P18-zstd-inbox.md) — carved out of the gzip work |
| P8 — format coverage | Sketched; not grilled | this file, below; [inbox](roadmap-P8-format-coverage-inbox.md) |
| P24 — Python bindings | Sketched; not grilled | this file, below; [inbox](roadmap-P24-python-inbox.md) |

**A row's state is one of `Sketched`, `Specified`, `Current`, `Complete` or
`Struck`**, and the prose after it is a caption. Two of them are set in the same
change as a `STATUS.md` edit: **`Current` when the phase is sliced**, in the
change that writes its checklist, and **`Complete` at the wrap**, in the change
that deletes it. A phase carrying a checklist is `Current` and a `Current` phase
carries one — so a transition that half happened is visible from either side,
and this table can say a phase ran and finished long after the checklist is
gone, with the keystone that would strike it years later. That matters beyond
bookkeeping: `scripts/deficiencies.py` reads this column. It holds the
checklist pairing both ways — a checklist under a row that is not `Current`
fails, and so does a `Current` row with no checklist — and a known deficiency
owned by a phase that is `Complete`, `Struck`, or absent from this table has no
destination, so it drops to `(c) unowned` unless another phase absorbs it
(`../status/deficiencies.md`, "Known deficiencies").

The struck phases' decisions are in
[`decisions.md`](decisions.md), not by phase; their specs and notes went
at a keystone review (`../process.md`, "The keystone: striking the
centering"). **Phase numbering continues from `P28`** — nothing at or below it
is reused, whether it was struck, sketched, or never specified.

Two standing-constraint docs cut across everything below.
[`decisions.md`](decisions.md), "D68" and "D74", assign each module to one of
four layers, fix the direction dependencies may point, and pre-answer the
cross-layer phases below: what statistics may persist and how they parse a
value is settled there, and the remote source, as the ones P15 and P18 add, is L1. [`postgres-invariants.md`](postgres-invariants.md)
is the evidence layer: every `pg_dump` behaviour a decision treats as
guaranteed, with its proof and its re-verification command.

## Pre-1.0: no compatibility obligations

Everything in this roadmap happens before 1.0, and **nothing here carries a
backwards-compatibility or API-stability guarantee**. Public types, the CLI
surface, the cache format, and the Arrow schema a given table resolves to are
all free to change in any release until 1.0. We are iterating, not publishing
contracts.

This is a standing decision, not a question to reopen each phase: don't design
around hypothetical downstream breakage, don't add compatibility shims, and
don't caveat a proposal with migration concerns. The cache's `format_version`
envelope exists so that a stale cache is *detected* rather than misread — not
as a promise to keep reading old ones.

The one place this genuinely costs something is the Arrow schema drifting as
type coverage grows (a column that resolves to `Utf8View` today may
resolve to `Int32` after the next release, invalidating a downstream query
plan). The Future item "caller-supplied type mapping" below is the answer for
anyone who needs a schema pinned; until 1.0 it is the only one.

## Project goals

Two things distinguish this project from existing `pg_dump` tooling
(`pgdumplib` and friends), and both shape the phase ordering below:

- **Embeddable as a query data source**, not just a dump reader — Arrow-native
  output, and a DataFusion `TableProvider`.
- **High performance is a core goal, not a later optimization**, specifically
  for the local-file reader. Dumps are routinely hundreds of gigabytes; the
  difference between a saturated-device scan and a merely-correct one is the
  difference between a usable tool and an overnight job. Concretely: the
  local-file path should stay device-bound, not CPU-bound, on hardware from
  HDD through NVMe, at flat memory. Where it currently stands, and what a scan
  spends its time on, is [`decisions.md`](decisions.md), "D29"; the figures are [`measurements.md`](measurements.md).

  **Flat memory is a property of block-level structures**: resident that does
  not grow with the rows streamed is what shows a scan streams rather than
  retains. What is drawn from row values — statistics above all — varies with
  their widths, so no flat check holds for it except over rows of uniform width.

  This targets the `COPY`-block/bulk-row path specifically. Preamble and other
  non-data DDL scanning is bounded by schema size, not file
  size — a few thousand lines even for a multi-hundred-GB dump — and isn't
  held to the same device-bound target.

## Standing rules

Decisions that apply to all work below, not to any one phase. They are here
rather than in a phase doc because they outlived the phases that produced them.

### Four workers is the optimization baseline; twenty-four is the guard

**Tune the parallel paths against four workers, and check twenty-four has not
been made worse per worker.** Four is the count that survives moving between
machines — it is what `POOL_DEPTH` delivers on a plain source, what the koji
verification ran its parallel leg at, and a plausible allocation almost
anywhere — so it is the target a future optimization pass aims at. A dev
machine with twenty-four hardware threads is not something later work may
assume.

The guard is the other half and it is what stops the baseline becoming a
ceiling: an optimization that helps at four and makes twenty-four slower *per
worker* has over-fitted to the baseline. `parallel-scan-throughput`'s wide
`--jobs` axis is kept for exactly that reading — a compressed parse keeps
scaling well past four (`measurements.md`, `parallel-scan-throughput`) — so the wide numbers keep being
gathered wherever the machine can give them, and are read as a guard rather
than as the target.

### A default runs as fast as the allocation permits

**Where a caller states nothing, the defaults are read from what the
environment actually allows — the CPU quota, the memory limit — rather than
from a constant chosen to be portable.** The posture to match is `xz -T0`. The
deployment case is what settles it: under an orchestrator the process is
*given* an allocation, is the only party that knows it, and cannot be asked to
have it restated on a command line.

**The expected deployment is a container, so the discovered-limit path is the
normal one and the unlimited path is an anomaly.** pgdt is built assuming it
runs with an allocation somebody chose for it. That is why filling a discovered
limit is the default rather than an option — and why "no limit found" is not a
second supported mode to tune for, but a state in which nothing has said what
pgdt may take while it shares the host with whatever else runs there. The
status output reports which case applied, so an operator who expected a
container and reads `no memory limit found: nothing is enforcing one on this
process` has learned their allocation is not being enforced.

**An allocation stated is permission; a machine merely observed is not.** A
cgroup limit is somebody telling pgdt what it may have, so taking it is what
they asked for. A host with *no* limit has told us nothing — it is a shared
machine until proved otherwise, and pgdt is not the only process on it. So the
two cases are not symmetric and must not be written as one: **where a limit is
discovered the default fills it, and where none is, the default takes what the
source recommends, capped at half of what the machine reports available.**
Reading physical RAM to fill an unlimited host is refused — total memory is not
*our* memory.

Four bounds, and they are what keep this from being "take everything":

- **A stated flag wins outright**, in both directions. This governs the absence
  of a flag, never its presence.
- **The library's own default stays `Serial`.** What this rule sets is what the
  *CLI* resolves and what a *source* recommends for itself; an embedder is
  handed the mechanism and takes it deliberately.
- **We do not take an allocation we cannot show we use, and the bound has two
  halves.** Never allocate more memory than the workers can use, and never
  allocate workers there is no memory for. The pair has to settle consistently
  on machines of *different shape* — a wide host with little memory per core
  and a narrow one with a great deal — so neither number may be chosen without
  the other. `stream::worker_count` and `BufferPool::slots`' clamp are the
  second half in code: resident saturates at `WorkerMemory::at` of the
  announced count — its readers' own terms plus the block pool's retention
  list — and what no pool holds, so budget above that is taken by nothing.
  Below it the bound is not the budget: where a stated `--jobs` outruns what
  the budget affords, the block pool's slot ceiling follows the announced count
  and resident passes the stated number (`KD21`). The first half is
  what a source's own recommendation owes — a count the file cannot supply
  work for must not be multiplied into a budget request.
- **Unstated is not unlimited.** Absent a discovered limit the charge a source
  recommends is capped at half of what the machine reports available, because
  the alternative is sizing pgdt's appetite from hardware nobody said it could
  have. That cap
  costs no speed where there is room — resident saturates at the worker count
  regardless — and binds wherever the recommended count costs more than half of
  what the host has free, which on a large-block file is a large machine too.

**Each source answers for its own defaults, worker count and budget alike.**
That is why `ByteRangeSource::default_workers` is a trait method rather than a
test for `.xz`: the gzip, zstd/lz4 and format-coverage phases each add a source
whose right default is its own, and a conditional written against the one
compressed source in the tree would be reopened by every one of them. A source
that recommends a worker count also says what **one of those workers** needs,
since a count nothing can afford is not a recommendation.

**Per worker rather than as a total, because a total is a total for some
count.** The only count a source knows is its own, so a caller that replaced it
with a stated `--jobs` could not recover the per-worker cost without dividing by
a number that is no longer in play. Stated per worker the answer is independent
of every count, and multiplying is the composition's — which is also what lets
the composition hand back a *pair*, the count an allowance affords beside the
budget that many workers spend.

### Two tunables fit pgdt to hardware: memory and parallelism

**A person fits pgdt to their hardware by stating a memory allowance and a
worker count, and needs to state nothing else.** The implementation interprets
those two however it must — a pool's depth, a statistic's granularity, whether
a path is afforded at all — and no new flag is added whose purpose is fitting
the process to a machine. This is for the user's simplicity: an operator sizing
a container knows what memory and how many cores it has, and nothing about
chunk sizes or row groups.

**The memory number means resident**, the number a person gives the
container: a stated one is treated exactly as a discovered limit, and whatever
pgdt holds — pools, statistics, the reserve for everything unbilled — is carved
from it.

**It governs what a person needs to state, not what exists.** A flag stating
*intent* — `--statistics-level <levels>`, what to record of which tables and
columns — is not a
hardware knob, and neither is a threshold of the input contract such as
`--max-line-bytes`. An expert override already shipped, `--chunk-size`, may
stay; what the rule refuses is a default that is only right once a third knob
is turned. **P22 reopens exactly this clause** — not the rule's purpose, which
stands, but whether one number can be right for the four consumers it now fans
out into.

**Reasonable defaults are promised for reasonable data only.** A shape the
defaults cannot fit — a table of a thousand wide text columns under a small
allocation — is fitted by raising memory or narrowing intent, and the defaults
are not bent to reach it. A long dump of ordinary rows is not such a shape.

**The check.** A CLI test lists every flag taking a byte count or a count
against an allowlist classifying each as hardware, intent, input contract or
expert override, and fails on a flag nobody has classified — so a third
hardware knob is a decision somebody wrote down, not one that arrived quietly
(`pgdt/src/main.rs`, `every_numeric_flag_is_classified`).

### A parse does all the work a later query could use

**A parse carries the intent to do every piece of work that accelerates a later
query, so its default does all of it** — `pgdt parse` and the library's mapping
pass alike. A caller who wants less done issues cold queries instead; stating
less is an opt-out, never the default. **What it spends doing that is a separate
question**: workers and memory keep the defaults "A default runs as fast as the
allocation permits" sets, the library's conservative.

### Coverage increases monotonically

**A later change may subdivide a span or attach detail to it, never reduce
coverage.** Specificity increases; the tiling property does not degrade.
Splitting the one large-object span into one span per object is the intended
shape of "more specific"; introducing a span kind that leaves bytes
unaccounted for is not.

The rule is only real because something checks it. `every_fixture_tiles_exactly`
asserts tiling over every fixture, and the cases that matter most are the
degenerate ones: `--data-only` (no DDL), `--schema-only` (no data),
`--inserts` (no `COPY` blocks at all), and the concatenated multi-database
shape. Absent that test, "spans sum to file size" decays into an aspiration the
first time a span kind is added.

### The input contract is valid PostgreSQL, not `pg_dump`'s output

**pgdt reads plain SQL that a PostgreSQL server would accept, whoever wrote
it.** `pg_dump` is the producer we verify against because it is the one we can
run six versions of, not the boundary of what we accept. A file that is
hand-written, hand-edited, or emitted by another `pg_dump`-compatible tool is
in scope, and a wrong answer on one is a defect like any other.

Two consequences, and the second is what the rule is really for:

- **A declared type is read the way PostgreSQL's own type system reads it**,
  because that is the type the values in the `COPY` block were written by.
  `resolve_declared_type` is nothing but this — `int4` → `Int32`, a typmod
  wherever it falls, the domain walk — so a spelling read *more literally*
  than the server reads it is the anomaly, not the interpretation.
- **A resolution outcome states only what the DDL supports.** Refusing a
  column is always available; refusing it under a label that makes a false
  claim about the column is not.

When another producer is identified, its output is incorporated into the
fixture generators the same way `pg_dump`'s is — the rule below is about the
method, not about `pg_dump`.

### Expand the generated fixtures freely; verify objectively wherever possible

**When a decision depends on the exact bytes some producer writes for a shape,
put that shape in `scripts/fixture_schema_*.sql` and regenerate, rather than
reasoning out what the output must be.** This is closed, and the bias is
deliberately lopsided: a fixture column costs one generator run across six
majors and a few lines of schema, while a wrong inference costs a design built
on it. We have paid the second price more than once.

- 4.1 specified the array and composite fixture shapes by reasoning about
  `array_out`; two values it did not know it needed surfaced only afterwards
  and earned 4.1.1 — an array over a domain whose base is `box`, whose
  separator is `;` and which therefore walks straight through a refusal written
  against the declared spelling (I22), and a zero-field composite (I23).
- 4.4 typed `d[]` where `d` is a domain over an array as `List<List<T>>` by
  composing the mapping rules. No such column exists in the fixtures, so six
  majors of round-trip tests passed over a column no value can fill; the real
  literal is one brace deep (I26), and finding that cost a slice (4.4.2).
- The sweep that found it also found five shapes that worked and were pinned
  by nothing at all — domain over composite, array of that, composite inside
  composite, array of a user range, domain over a range. 4.4.2's
  `t_nested_array` holds all five.

The corollary is the part that is easy to skip: **a shape observed to work is
not covered until a fixture holds it.** "The recursion handles it" is the
reasoning that failed above, and it will keep failing, because the recursion is
right about the type and says nothing about the bytes.

**Half the rule is mechanical.**
`tests/pgtype.rs::every_resolution_outcome_is_produced_by_a_real_fixture_column`
requires each `ColumnResolution` variant to have a real fixture column behind
it, and the list it reads is also its exhaustive match: a new variant is a
compile error until it is listed, so a new refusal cannot land without the
`pg_dump` output that reaches it. **The one exemption is an outcome no scan can
produce**, a property of how much of the file was read rather than of a
declared type: `MetadataNotScanned`, listed apart in the same list, pinned by
`pgdt/tests/partial_reporting.rs` against a hand-built cache, and failing the
test should a fixture ever reach it. The other half, a shape that resolves to
an existing outcome and merely works, stays a judgement call; naming that limit
beats a check implying coverage it does not have.

The cost is real and bounded — regeneration needs Docker and the six
PostgreSQL images (`CLAUDE.md`), and added columns widen literals other tests
read. Pay it. The alternative is discovering the shape from a user's dump.

**Where a fixture is impossible, the invariant proving it impossible stands in
for one.** Some behaviour is reachable only from input a producer we can run
provably never emits — `integer[][]` survives no round trip through
`format_type` (I21), so no generator input makes `pg_dump` write it. That is
not an exemption from the rule but a case of satisfying it: the objective
evidence exists, it is in the invariants register, and it is stronger than a
fixture would be. Such behaviour is pinned by unit test over a hand-built
`TypeDef` list, and the entry it rests on is cited at the test. The rule's
target is *guessing*, and a verified invariant is the opposite of a guess.

**The citation is the check.** A test claiming this carve-out names its `I<n>`
at the test site, or it is not covered by it — which puts the behaviour inside
the register's own ritual, where every entry carries a re-verify command that
gets walked at each new PostgreSQL major. That is a stronger guarantee than a
second enforcement mechanism would be, and it is what stops the carve-out from
becoming a way to skip a fixture that was perfectly possible.

**The check.** Every standing rule here carries one, and this rule's is
partial by nature: a test walks every fixture, resolves every column of every
block, and asserts that **each `ColumnResolution` variant but the exemption
above is produced by at least one real fixture column**. Adding a resolution
outcome without a fixture that reaches it then fails a test instead of relying
on discipline — which is exactly what 4.4 would have hit. It lands with 4.4.2, reusing the fixture-walk
helper `tests/map.rs` already has.

What it does *not* check is the other half: a shape that resolves to an
existing outcome and merely works — the five the sweep found — is still a
judgement call. That gap is named rather than papered over, because a check
claiming more coverage than it has is worse than one that states its limit.

### Refactor when the shape stops fitting

**A refactor that would survive the roadmap — or at least its next several
phases — and that reduces complexity or increases flexibility is taken when it
is noticed, not deferred to a tidying pass that never comes.** The 1.0
architecture is meant to be as clean and elegant as we can make it, and rushing
to a done state without periodic refactoring is how a codebase turns to
spaghetti.

The rule is a tie-breaker, not a licence. It applies when the alternative is
choosing between two acceptable placements of the same logic, or when a slice
has just added the second special case to something that wanted one. It does
not license rewriting a mechanism whose shape is merely unfamiliar, and it
never widens a slice's behaviour: a refactor taken under this rule is
behaviour-preserving, and anything that changes what the code *does* is a
decision, so it needs the ordinary treatment — grill it, amend the spec, give
it a slice number.

**The check.** A refactor taken under this rule names, at the point it lands,
which later phases it expects to survive and what it made simpler. If neither
can be stated, the rule did not apply and the change is a preference.

### A slice row that commits to a measurement names its instrument

**A slice row promising a figure says what will take it.** Not the number, not
the threshold — the instrument: a microbenchmark, an end-to-end run under a
named command, a comparison against a named control. A row that names only the
deliverable leaves the instrument to whoever implements it, and the implementer
will pick the cheapest one that can be argued to satisfy the words.

This is narrow on purpose. It binds rows that commit to a *measurement*, where
the instrument is the part most likely to be under-determined and where two
instruments can differ by an order of magnitude in cost while both answering to
the same sentence. Everywhere else the ordinary spec discipline is enough.

**A named instrument is satisfied by whatever it resolves — including a
bound.** The row commits to running the instrument, not to the answer coming
back above its floor. A null or negative reading is a result, and the
instrument's own floor is part of the deliverable: state it beside the figure,
and name the sharper instrument without building one. A row is unsatisfied
only if the instrument it named was not built or not run. This is the second
failure of the same row-shape, and it points the other way from the first:
4.6.1 named its instrument, ran it, and got the composite column's share back
*below* what differencing two generated files can resolve. Rescuing that with
an unbudgeted second instrument is scope growth arriving through a measurement
row — the hazard an unattended session is most likely to walk into, since
building something looks more like finishing than reporting a bound does.
Reasoning: [`../status/history/2026-08-27.md`](../status/history/2026-08-27.md),
"The review of the six measurement decisions".

**Why it is a rule rather than a note.** 4.6's row asked for two figures. For
one it named both instruments — "a `decoders.rs` micro **and** `pgdt query
--schema-mode typed` against `strings`" — and got both. For the other it named
only "composite decode throughput", got a micro, and earned **4.6.1** at
review to supply the end-to-end half. Same row, same author, same slice: the
half that named its instrument was delivered whole. Every phase from here is
measurement-heavy — one of them was an entire performance campaign — so the
hazard is live for the unwritten specs. Reasoning:
[`../status/history/2026-08-27.md`](../status/history/2026-08-27.md), "4.6.1
is earned, and the spec row's ambiguity is why".

### Attribution is introspective; only the gate is blind

**A reading that decides whether the shipped thing works stays black-box; a
reading that says *why* asks the process itself.** Those are two deliverables
and they have been taking one instrument, which is how this project came to
plan sitting after sitting differencing whole runs to attribute a term the
process could have printed.

**What forces the split is what a high-water mark is.** `getrusage`'s
`ru_maxrss` is one scalar with no decomposition and no time axis, so the only
way to take it apart is to vary something and subtract — and every subtraction
is another sitting, carries both legs' spreads, and confounds whatever else
moved with it. Resident is at least three terms:

```
RSS = what the program asked for and still holds
    + what the allocator obtained and has not returned
    + what is neither heap nor ours — thread stacks, the binary, mapped files
```

A peak-RSS leg measures their sum and nothing else. Every question asked about
memory here is a question about *one* term — is a fixed term live structure or
arena retention, does a pool ceiling bind, what does an evicted-but-viewed
block cost — and no number of subtractions between sums answers one of them as
well as a process printing its own.

**The rule.** A slice row whose deliverable is an **attribution** names an
introspective instrument: something inside the process reporting the term
directly — a counter, the allocator's own statistics, a profile attributing to
call stacks. Differencing whole runs is the **fallback**, and a row that
chooses it says which introspective instrument was considered and why it cannot
answer. Where no such instrument exists yet, **building one is the slice**, and
it comes before the sitting rather than after the sitting fails.

That last clause is the whole of what this rule adds. The preference was
already written — `.claude/skills/evidence/SKILL.md`, "Before planning a slice
whose deliverable is a reading" — and it changed nothing, because a preference between instruments
is inert when only one of them exists. What a session actually chooses between
is a harness leg it can add in an afternoon and an instrument nobody has built,
and it will choose the leg every time and be able to defend it.

**The gate is the exception, and it is genuinely blind.** What kills a process
is resident bytes under a real limit, not live heap, so a pass/fail reading
against a cgroup is taken on the shipped build, under the shipped allocator, in
the container, exactly as [`measurements.md`](measurements.md)'s apparatus says.
The rule above does not weaken it — it protects it, by keeping diagnosis from
being smuggled into the one reading that has to be taken on the thing that
ships.

**The apparatus rules govern figures, not diagnosis, and reading them wider is
the specific mistake.** "The allocator is part of the apparatus" and "a figure
is taken with the default glibc build" bind what may be **published**. A
diagnostic sitting already does what a figure may not: `--alone` marks a whole
run NOT PUBLISHABLE, and the plain path's account ran against a scratch build
with a lifted `POOL_DEPTH`. So an instrumented build, a counting allocator or a
profiling one has been available for attribution all along, and a spec refusing
one *for a figure's reasons* has applied the wrong rule to it.

**It is not a rule about memory.** The same split holds for time — a sampling
profile attributes per function where a subtraction of medians attributes per
run, which is why the profile recipe sits beside the sweep — and for anything
else a run can be asked about itself: bytes read, blocks decoded, workers
admitted. A resolved count read off the run's own `scan started` line is this
rule already being followed. Where the process can count a thing, counting it
beats subtracting around it.

Reasoning: [`../status/history/2026-09-11.md`](../status/history/2026-09-11.md),
"Attribution was being done with the gate's instrument".

### A test may assume the tools `mise` pins

**A test that needs a pinned tool asserts its presence; it does not skip
itself.** `mise.toml` is the project's declaration of what a checkout has, and
`mise install` is one command — which is the point of using `mise` at all: a
CI run is one step away from every tool the suite needs. So a missing pinned
tool is a misconfigured environment, and the honest report is a failure naming
`mise install`, not a green run with a printed skip.

**Why it is a rule.** A conditional skip is invisible in a passing suite, and
the test most likely to carry one is a test nothing else covers — which is
exactly when its silent absence costs the most.
`pgdt/tests/perf_generator_fidelity.rs` is the whole example: one test, the
suite's only guard against `scripts/generate_perf_data.py` drifting away from
what `pg_dump` writes, and it skipped itself when `uv` was absent. Three
infidelities had already survived in that generator for want of any guard at
all.

**The check.** A test that reaches for an external tool either asserts it is
present, or is gated behind an env var that defaults to *enforcing* — never a
silent skip. A genuinely machine-local resource, which `mise` cannot pin, is
the exception and gates the other way: unset by default, absent in every other
checkout, per `CLAUDE.local.md`'s rule for the koji replica.

### Arrow follows DataFusion

**The workspace's `arrow` pin is the one the targeted DataFusion release
depends on, and the two move together.** A crate here that builds DataFusion
plans hands DataFusion `arrow` types, so a second `arrow` major anywhere in the
workspace is a build that does not link; the library does not keep a pin of
its own. So an `arrow` upgrade waits for a DataFusion release that takes it,
and a DataFusion upgrade takes its `arrow` in the same change.

### Four decisions that keep later phases additive

Plain-format-only and single-threaded is a deliberate scope, not a limitation
to design around. These four choices are what made the scan-performance work
additive and are what make P8 additive rather than a rewrite, and they are cheap to hold to — so hold to them, even
where the work in front of you would not require them.

- **The COPY TEXT decoder stays independent of where its bytes came from.**
  `copy.rs` operates on a caller-owned slice and never assumes "a file at
  offset N". Every dump format stores table data as this same COPY TEXT
  payload, so this decoder is the one component all of P8 reuses verbatim.
- **Structure discovery is a separate concern from row decoding.** `scan.rs`
  finds `COPY` boundaries in a plain file; an archive reads them from a TOC.
  Keeping "what entries exist and where are their bytes" apart from "decode
  these bytes into rows" is what lets a container layer slot in later.
- **The cache format is versioned and records what produced it.** The cache
  file carries a format-version field and a container-kind tag beside the
  serialized `DumpIndex`, so
  archive-derived indexes and entry-relative offsets are a later variant rather
  than a breaking change. A cache whose version or kind is not recognised is
  refused as another build's until `--overwrite-unusable-cache` or a deletion
  replaces it (`decisions.md`, "D20" and "D22").
- **`ResumeToken` exposes no fields, ever.** Its contents today include a
  file offset, and a raw file offset is meaningless inside a compressed
  archive entry. Opaque now means the representation can change
  without an API break.

A fifth already binds: the batch layer builds `Utf8View` arrays over the
scanner's existing chunk buffer instead of copying field bytes out of it, and
the read path pools that buffer so it outlives the pass that scans it
([`decisions.md`](decisions.md), "D46"
and "I/O, memory and parallelism").

### Permanent non-goals

- **Writing or modifying dump files.**
- **A full SQL/DDL parser** — only enough recognition to locate `COPY` blocks,
  classify statements into spans, and extract columns and types from
  `CREATE TABLE`.
Note that CSV-format `COPY` blocks are **not** on this list. They are a Future
item; see below.

## P28 — Unrepresentable values

**A query's outcome never depends on which rows it happened to read**, and
how a value its column's Arrow type cannot hold is handled (`KD8`) is the
user's choice among three modes — read as NULL, its column widened to text,
or a deterministic refusal; and `parse` gains a *metadata* level recording no
census. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md).

## P22 — The third tunable

**One number fans out into four consumers and has to be right for all of
them**: the worker count, the read-buffer budget, a query's batch span and now
the statistics allowance. Each of those terms has been re-fitted on its own —
`M111` the span, `M112` its floor, the allowance last — which is the tell that
the number, not any one derivation, is what is overloaded. Sketched to
corner-avoidance depth. The model as it stands was never measured or gated, so
this phase inherits an unpriced constant rather than a freshly fitted one, and
P23 is where that constant is read.

**It revises [this file](roadmap.md), "Two tunables fit pgdt to hardware:
memory and parallelism"** — that section's last clause, and nothing else about
its purpose. Until this phase is specified the rule stands as written and the
code keeps describing what is built.

What it inherits:

- **The two bands** (`pgdump_query/src/io.rs`, `statistics_allowance`): below
  `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` the cap binds and statistics
  have a floor; at and above it the ceiling binds and statistics get only the
  slack one worker's step leaves, so a **wide** host at a high `--jobs` is the
  starving case. **That band is priced** — the reading, and what it says about
  shrinking this phase, is
  [`roadmap-P22-third-tunable-inbox.md`](roadmap-P22-third-tunable-inbox.md).
- **`KD32` and `KD25`**, both `(c) unowned` today and both the same seam from
  the other end — a stated allowance is inert on a plain source. This phase is
  their destination, and re-stancing them to `(b)` is part of specifying it, a
  `(b)` owner having to exist before an entry may name it.
- **`decisions.md`, "D83"** (the typed number bounds the process, the budget
  the pools) and **"D3"** (the reserve is a subtraction, not a fraction).

What it must decide: whether the new number is statistics-only or a second
allowance the other consumers also read; what an unset one derives, given that
today's derivation is what every figure was taken against; and whether the
plain-source inertness `KD32` names is fixed by the same change or stays.

## P21 — Statistics gathered by a query

A query gathers statistics for what it already reads — the columns its filter
evaluates — where today only a parse gathers. Sketched to corner-avoidance
depth, and after the pruning consumer, which is what makes a partial gather
worth having. What it inherits:

- **Presence per group**: a pruned or stopped query reads part of a block, so a
  statistic must tell "not yet gathered" from "none available" — an all-NULL
  column's bounds — per group, where a parse gather needs it per block and column.
- **A term is not evaluated on every row** (`decisions.md`, "D54"), so a filter
  column is observed completely only where every row's field is read anyway.
- **Block sortedness and back-fill are whole-block facts**; a partly
  gathered block settles neither.
- **The replay never saves the cache**; the mapping pass does, and a
  partitioned replay saving meets "D20" and the save gate ("D62").

## P23 — Statistics coverage and the resident reserve

**Statistics stop where the account fills, and the reserve does not cover what
a run holds.** What statistics may hold resident is bounded
([`decisions.md`](decisions.md), "D85"); what is not delivered is usable
statistics for a long dump, or a price for the constant the bound is carved
from. Both gaps are measured rather than supposed — `KD33` and `KD34` — and
both were left rather than fixed because the architecture they answer to is
still moving: P21 changes who gathers, P22 changes what the number they answer
to means. Sketched to corner-avoidance depth, and after both.

What it inherits:

- **The coverage flaw is cumulative, not per-block** (`KD33`). Nothing releases
  `Term::Retained` while the pass gathers forward, so a long dump fills its
  allowance partway through and every block after it declines — statistics become a prefix, and a
  query prunes nothing over the tail. How much of a dump is covered depends on
  the box, which is the part a user cannot predict.
- **Granularity derived from the dump's length is the remedy that survives
  `D85`.** That entry rejects *coarsening to fit* because a cache would then
  depend on its container; a group size chosen from the file's size, known
  before a byte is gathered, is deterministic — the same dump yields the same
  cache on every machine — so the refusal does not reach it. The decline stays
  as the backstop for shapes the estimate gets wrong.
- **The reserve is under-covering by a measured margin** (`KD34`). The
  2026-09-16 attribution sitting read a worst remainder of 544 MiB above charge
  plus account, against `MEMORY_RESERVE`'s 384 MiB, and its readings are the
  input this phase would otherwise have to re-take. No sitting re-takes them:
  they are `runs/20.8-reserve-attribution-20260916-1857/readings.json`, named
  again by [`../status/history/2026-09-16.md`](../status/history/2026-09-16.md).
- **A branch already taken**: a remainder growing with the statistics volume
  is billed to the query rather than reserved — `pgdt query`'s mapping pass
  carves its workers around the statistics a loaded cache holds, and its
  replay, which keeps none, around nothing (D85). Its instrument is kept —
  `instrument::statistics_loaded`, the heap a cache load hands a pass, which is
  the only statistics term a query has. It is filed for P22 as well, being a
  fourth consumer of the one number, and whichever phase runs first settles
  whether it stays billed.
- **The figures owed.** `reserve`, `rss-attribution`, `statistics-gathering`
  and `statistics-pruning` were re-taken at `da05a72`, against the reserve as
  it stands, so a settled constant re-takes them. `reserve`'s stated axis — a
  typed `--jobs 24` under `--memory` — is where the margin lowers only the
  budget, and its worst rep has held about `MEMORY_RESERVE` above the
  resolved budget, over it at one sitting and under it at the next
  (`measurements.md`, `reserve`; `KD34`). Both statistics
  figures' inputs are owed a change as well: bare `text` is bounded bytewise
  now, so `statistics-gathering`'s control (`v_text`, `v_long_text`,
  `v_escaped`) should grow, every retained column carrying one more
  `Option<ColumnBounds>` the account charges; and `statistics-pruning`'s
  `v_category` carries bounds its fidelity guard no longer asserts absent,
  `pgdt query` still reading only its dictionary. Since `542fdfb` a
  summed column keeps an `i128` a group and every tracked column a `u64` of
  text bytes (D91), and `CACHE_FORMAT_VERSION` moved three times;
  `statistics-gathering` at `da05a72` carries the heavier cache, and what it
  adds over `542fdfb`'s is unattributed.
- **A default `parse` on NVMe may be CPU-bound, which is D10's reopen
  condition.** Every `scan-throughput-*` and `chunk-size` run gathers
  nothing, where the shipped `parse` gathers, and
  `statistics-gathering`'s warm gathering leg takes several times what the
  NVMe needs to deliver the same bytes (`measurements.md`,
  `statistics-gathering`, `scan-throughput-nvme`); no cold figure measures the
  default. A cold-NVMe figure of the default `parse` is this phase's first
  evidence, before D10 is re-read. The `INSERT` path meets the same condition
  at any statistics setting (`KD9`).

## P26 — Statistics refused on their cost, reconsidered

**Statistics a gather could keep in the pass it already runs, refused because
of what keeping them costs** — CPU, memory under the statistics account
([`decisions.md`](decisions.md), "D85", "D86") or cache volume — reconsidered
together, against what each would let a query skip or answer. Sketched to
corner-avoidance depth. What it starts from:

- **A per-group bloom filter**, for equality and `IN` on an unsorted column of
  many distinct values — the one shape measured to prune nothing today
  ([`measurements.md`](measurements.md), `statistics-pruning`, the `v_smallint`
  row), and the membership a hash join's dynamic filter would test: past the
  `IN` list's limit (`hash_lookup`) or on several keys (`struct(…) IN`) a join
  prunes by its bounds alone, and an `IN` over a column with no dictionary by
  nothing.
- **A per-block distinct-count sketch**, refused as a gathered count by "D79";
  "D89" derives an exact count from complete dictionaries instead, so what is
  left is the estimate for columns whose dictionaries overflow.
- **Every other refusal the register grounds in gathering cost** — "D76"'s
  clipped head in place of a key for a bytewise value is one — swept from
  `decisions.md` when this phase is grilled.

## P15 — gzip input

**Inbox:** [`roadmap-P15-gzip-inbox.md`](roadmap-P15-gzip-inbox.md) — facts earlier phases filed for this one. Drain it when grilling this phase.

`.gz` input, in both shapes it arrives in: the single-member stream a writer
produces streaming, and the multi-member one — of which `bgzip`'s BGZF is the
disciplined case, every member carrying its compressed length in a header extra
field. **One phase, not two**, because BGZF is a strict subset of gzip rather
than a sibling format: the magic is the same, the decoder is the same, and a
gzip source that could not read a BGZF file would be wrong. What differs
between them is one thing, the payload of the index that makes a backward read
affordable — a set of decoder *checkpoints* for the single-member shape, a list
of member offsets for the multi-member one — and everything around that index
is shared. Splitting them would build the recognition path, the decoder and the
cache envelope's second variant twice, or once speculatively.

Two things separate gzip from xz, and they are why this is a phase rather than
one more arm of that source's:

- **It does not answer `size()` from its own footer.** `ISIZE` is the
  uncompressed length mod 2³², useless above 4 GiB. So this phase either relaxes
  what `ByteRangeSource::size` promises or computes the size in a first pass —
  a decision xz never forced, and one that reaches every caller that
  clamps a read against `size()`. The two shapes do not answer it alike: a
  member-walk sums exact per-member `ISIZE`s, while the single-member shape has
  nothing short of a full decode.
- **This is `pg_dump`'s own plain-format output.** For plain text a nonzero
  `--compress` level compresses the entire output file, and gzip long predates
  the method selector, so `pg_dump -Fp -Z9` has produced a file this build
  cannot read for as long as there has been a build. That closes a
  **compatibility gap**, not a convenience — and the file it has to read is the
  non-seekable single-member shape, since `pg_dump` writes it streaming.

**Scheduled ahead of P8**, whose Track B needs per-entry gzip streaming decode
inside the archive container. Landing the decoder here means that track reuses
it rather than acquiring it alongside a TOC parser.

## P18 — zstd and lz4 input

**Inbox:** [`roadmap-P18-zstd-inbox.md`](roadmap-P18-zstd-inbox.md) — facts earlier phases filed for this one. Drain it when grilling this phase.

The codecs `pg_dump` gained with PG 16, in the single-frame shape and in the
seekable one (`t2sz`'s zstd seekable format, a skippable frame carrying the
frame index). Same argument as P15 above — a `--compress=zstd` plain dump is
ordinary `pg_dump` output this build cannot read — and the same `size()`
problem in its own form: zstd's frame content size is *optional* and a
streaming writer omits it, so the number is unavailable for exactly the files
this phase exists to read.

**Carved out of P15 rather than shipped with it**, because nothing the gzip
work does is on this phase's critical path: a different decoder dependency, a
different seekable container, a different index payload, and a codec that
arrives with a PG version rather than predating everything. What the two share
is the `size()` contract decision, and that is settled once — by whichever runs
first — rather than being a reason to run them together.

**lz4 rides here**, for the reason it never belonged with gzip: it is the third
arm of the same PG 16 method selector, with the same shape of problem and no
gzip-specific anything. Whether it is worth reading at all is this phase's to
decide, and the honest answer may be no — it earns its place from a real dump
somebody has, not from the flag existing.

**Scheduled ahead of P8** for P15's reason, and after it for no reason beyond
gzip being the older and more likely input.

## P8 — Format coverage beyond plain COPY TEXT

**Inbox:** [`roadmap-P8-format-coverage-inbox.md`](roadmap-P8-format-coverage-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

Everything that widens the set of `pg_dump` outputs we can read. Two
independent tracks; A is listed first because it is cheap, not because it
matters more.

**Track A — variants within plain format.** `--inserts` /
`--column-inserts` output, and any other gaps the compatibility matrix in
`docs/design/pg-dump-compatibility.md` turns up as it fills in via the fixture
tooling. Small: a second row-source implementation feeding the same
batch layer, and its target is **already located and attributed** —
`InsertRun::table` and `row_count` come out of the map, so this track adds
parsing, not scanning.

**Not CSV-format `COPY` blocks** — see "Permanent non-goals" above. Track A is
`--inserts`, and nothing else that once sat beside it.

**Track B — archive container formats.** `--format=custom`,
`--format=directory`, and `--format=tar`. Formerly a permanent non-goal;
now planned, because the project's ambition (an embeddable, Arrow-native,
engine-integrated source) puts it in a different class from tools that only
need to read a dump once. Reading a table out of a custom-format archive into
a DataFusion query is squarely in scope for what this library is for.

The work is a **container layer**, not a new parser. All four formats store
table data as the same COPY TEXT payload the MVP already decodes; what
differs is how you find an entry and how you get its bytes:

| Format | How structure is found | Entry bytes |
|---|---|---|
| plain | scan for `COPY ... FROM stdin;` headers | raw, inline |
| custom | parse the `PGDMP` header + TOC | per-entry compressed block |
| directory | read `toc.dat`, one file per entry | per-file compressed |
| tar | read the tar member index + `toc.dat` | uncompressed tar members |

So the layer sits between `ByteRangeSource` (below) and the batch/stream API
(above), and reuses `copy.rs` verbatim. Two things
in it are genuinely new and should be scoped as such when this phase becomes
current:

- **Per-entry streaming decompression** (gzip, and lz4/zstd for PG 16+
  archives) — the decoders P15 and P18 land for whole-file input, applied per
  entry.
  What is new here is the *placement*: an archive compresses **internally**, so
  the container layer decompresses an entry rather than a file, and there is no
  whole-file byte stream for a source below it to present.
- **Non-seekable positions within an entry.** A raw file offset is not a
  resumable position inside a compressed stream, so `ResumeToken` stays
  opaque and the cache gains an entry-relative addressing mode.

The decisions that keep all of this additive rather than a rewrite are listed
under "Four decisions that keep later phases additive" above.

## P24 — Python bindings

Python bindings over the library, likely through `pyo3`, as a new workspace
member, sketched to corner-avoidance depth: the surface the library presents
an embedder, which `datafusion-pgdump` is the first to use, is what this phase
presents, and what concerns Python alone is in [its
inbox](roadmap-P24-python-inbox.md).

## Out-of-band work

Not every change is a phase. Work that belongs to none — a CLI ergonomics
change, a defect fix that changes no decision — takes an `M<k>` and a row in
the standing ledger, **[`out-of-band.md`](out-of-band.md)**, which holds the
admission rule, the allocation rules, the watermark saying which numbers are
spent, and the rows still outstanding. `M<k>` numbers work like `P<k>`:
allocated on discovery, never reused, and struck at each keystone.

**It is its own file rather than a section here.** The ledger grows
monotonically between keystones while this index does not, and a register read
only as far as somebody's window reaches is one that reissues a number already
spoken for — which is the one failure the allocation rules cannot absorb. The
rows are also a work queue, read by the unattended loop
([`.claude/skills/go/SKILL.md`](../../.claude/skills/go/SKILL.md)), so an
unread tail is unstarted work as well as a spent number.

## Future — wanted, unscheduled

Work we intend to do without committing it to a phase. An item moves out of
this section when it acquires a phase number, not when it acquires a design.

**This section is iterated through once the product is feature-complete**, so
it is where an option surfaced mid-phase and deliberately not taken belongs —
including a *cheaper alternative* to an item already here, which is recorded
inside that item rather than as a second one. It survives a keystone untouched:
no phase doc holds it, and nothing but acquiring a phase number takes an item
out. An option left only in a phase spec or a slice notes doc does not survive,
which is what makes the difference worth minding at the moment one is found.

- **A block cache whose retention floor tracks the reader count.**
  `BufferPool::slots` clamps the block pool at `POOL_DEPTH.max(jobs)`, so the
  cache holds up to four *units* however few readers there are — at 128 MiB
  blocks that is roughly 512 MiB a compressed scan must be charged before it
  may decode a block at all, which is most of a small container and is what
  sends such a scan through the streaming decoder there (`decisions.md`, "I/O, memory and parallelism"). A floor that followed the count instead would charge far less at
  one and two readers. Reworking a pool's sizing rule needs evidence the
  charge's account does not produce. **The charge bills the pool honestly
  already** — a reader holds one unit and the pool retains
  `(POOL_DEPTH.max(jobs) − 1) × unit` beside it, which no per-reader term
  carries ([2026-09-12](../status/history/2026-09-12.md), "The charge
  over-bills the pool floor at every count"), and `io::WorkerMemory` is the
  shape that bills it ([`decisions.md`](decisions.md), "I/O, memory and parallelism").
  That charge is not this item: the item is whether the floor should follow the
  count at all — and billing it is what makes the question answerable, the
  decline it widens now being the honest one. The cheap half of it — whether the
  floor buys anything at all below `POOL_DEPTH` readers — is a reading, not a
  design, and one probe bears on it without answering it. At a 512 MiB limit on
  koji's block size, a `runs/` probe swept the reader count by varying the
  reserve alone: four readers was 1.66× the streaming fallback and three 1.31×,
  but two readers was 6% slower than declining and one reader 3% slower
  ([2026-09-11](../status/history/2026-09-11.md), "`19.16` lands: 384 MiB").
  **It timed an arrangement that no longer ships**: there a one-reader block path
  held about 63 MiB, its budget leaving the block pool two slots, where the
  shipped charge counts all four ([`measurements.md`](measurements.md),
  "What a scan holds above the budget it was given"), and no reading times the
  shipped arrangement. One file, one limit and a probe rather than a figure; what
  is left is whether a count-tracking floor would put a third reader inside
  allocations that today get none.

- **A "safe mode" that deliberately under-fills a stated allocation.** The
  defaults fill a discovered cgroup limit, on the reasoning that a limit is
  somebody saying what pgdt may have. That is wrong for an operator who knows
  they *share* the cgroup — a sidecar in the same pod, two pgdt invocations in
  one container — and who would rather take a fraction of the allocation than
  all of it. The obvious shape is a flag taking the same half-of-available
  fraction the no-limit path already caps at and applying it to the discovered
  limit instead. What needs grilling before it is written is what it composes
  with rather than the number: `--memory` stated explicitly, the
  reserve, and the below-floor path, which is where a fraction of a small
  allocation lands immediately. Reasoning:
  [2026-09-09](../status/history/2026-09-09.md), "Fast by default, and the
  no-limit cap".

- **The `POOL_MAX_BYTES` cap on the plain partition product, lifted, so a
  raised `--chunk-size` keeps its multiple.** `leader::scan_partition` reads a
  plain piece a chunk at a time (`io::PartitionRead`), so the plain source is
  single-unit, every chunk-sized buffer a worker takes is pooled, and no partition-length
  buffer is allocated for an arena to retain — a probe on the build that
  introduced the chunked read put that at 9.4 MiB at `--jobs 24` against 209.2
  ([`decisions.md`](decisions.md), "D52"). What is left is
  the cap. Its reason was to bound the allocation a raised `--chunk-size` would
  make, and no such allocation exists;
  with the cap still in place a partition is one chunk at `--chunk-size 8m` and
  above, which hands back the 100% tail re-read `io::PLAIN_PARTITION_CHUNKS`
  exists to cap. Lifting it is one expression, and it is here rather than taken
  along with the read shape because it widens the *cut* at large stated chunks
  and nothing has measured that — a wider cut was measured to cost throughput
  at the compressed default, which is a different mechanism but the same
  question asked of the same number ([`decisions.md`](decisions.md),
  "D8"). It wants a reading at `--chunk-size 2m`/`4m`/`8m`
  before it lands, not a spec. **It also raises what the charge bills**, the
  same product being `Partitioning::partition_bytes` — which over-bills the
  plain path already
  (`KD25`, [`../status/deficiencies.md`](../status/deficiencies.md),
  "Known deficiencies"),
  so whichever of the two lands
  first decides whether the other is arithmetic or a second decision.

- **TOC attribution across an intervening statement, so `--disable-triggers`
  dumps stay attributed.** I31 puts `ALTER TABLE … DISABLE TRIGGER ALL;` — and
  a `SET SESSION AUTHORIZATION DEFAULT;` ahead of the first entry — between a
  `-- Data for Name:` block and its data, and no data span in such a file
  carries a TOC entry as a result (deficiency `KD1`). Not a
  correctness hazard and opt-in, which is why it is here rather than scheduled.
  Three coordinated changes: the `Data for` comment must absorb into
  `Mode::Statement`, `push_statement_span`'s `Framing` veto must not eat an
  owned `toc`, and `on_copy_start`'s `Mode::Idle` arm must inherit
  `governing_toc`. The third is why this is a graded slice rather than
  out-of-band work — `decisions.md` states `governing_toc`'s inheritance as
  a decision. The fixture half is a seventh `edge_cases` flag set (`--data-only
  --disable-triggers`), which by I31's scope limit is the only combination that
  emits anything.

- **Collation-aware comparison: the `S`-irrelevant set widened, then the
  remainder deferred to an environment.** Call the server that wrote the dump
  `S` and the environment pgdt runs in `E`. A column that states a collation
  other than `C`/`POSIX` is ordered bytewise today, so `<`/`>` return a row set
  the server would not (deficiency `KD7`). The partition that shapes the fix is
  exhaustive, which is why it is two increments and not a queue of collations:

  **The `S`-irrelevant half is exact and needs no environment.** A collation
  whose ordering reduces to `memcmp` is right on every server, whatever its
  libc, ICU, platform or major (I43). Today the register resolves two stated
  names, `C` and `POSIX` — plus `name`, which reaches the same verdict through
  its type's default rather than through a clause — where I43's set also holds
  `ucs_basic` and anything the file declares under the **builtin** provider,
  whose provider `pg_dump` writes verbatim (I42). Widening it is a parser over `CREATE COLLATION` and a
  register arm — no FFI, no environment, no conditional — and each column it
  reaches moves from an advisory divergence to an exact answer. This is where
  the increment starts, and it is worth landing even if the second half never
  is.

  **The `S`-relevant remainder cannot be closed from the file, at any price.**
  A plain dump carries a collation's name and never its version (I32, I42), and
  the ordering *is* a function of that version — measured, not supposed: ICU 70
  → 76 moves 3.0% of a broad Unicode corpus, and the same locale name under ICU
  versus glibc moves 5,796 of 5,998 strings. So bundling rules for `en_US.utf8`
  is not a cheaper approximation of this work, it is a different and wrong
  answer: it would be a snapshot of one glibc's tables presented with no sign
  that it is one. There is no third source. Either pgdt answers bytewise and
  registers the divergence, as it does now, or it **delegates to a provider in
  `E`** — the same libc and ICU the server would call — and states which one it
  used.

  **Deferral makes the conditional the user's to discharge, which is the whole
  move.** pgdt cannot know `S`'s provider version; a user who owns `S` usually
  can, and can run pgdt in an environment that matches it. That is a recourse
  they can execute — matching an *image* is cheap where restoring the dump into
  a real server is exactly the cost pgdt exists to avoid — and it is the same
  shape as `--database` resolving an ambiguity this build refuses to guess.
  It transitively reaches every collation, ICU included, without pgdt carrying
  a single collation rule. What it does not do is close `KD7` absolutely: the
  verdict such a comparison earns is *agrees with a server*, conditional on the
  provider matching, never the unqualified *agrees, on every server* that the
  `S`-irrelevant set earns. Two things make that honest rather than hopeful —
  `ucol_getVersion()` reproduces `pg_collation.collversion` exactly, so pgdt can
  report the version it computed under, and pgdt can name what a user must
  match.

  **The feasibility is demonstrated, not assumed**, by a spike outside this
  repo (`pgcollate`; see `CLAUDE.local.md` for the path). A zero-dependency Rust
  binary, built once, reproduced real `ORDER BY` byte for byte across seven
  container environments — PG 13–18, glibc and musl, ICU 70/76/78 — over a
  5,998-string corpus for six collations each, with zero mismatches. Its
  constraints, which any spec here inherits: ICU must be `dlopen`'d and its
  symbols are version-suffixed, so the `postgres` binary's own ELF `DT_NEEDED`
  is what names the right soname; a fully static build is foreclosed, which
  costs this project nothing since it already ships glibc-only; the `varstr_cmp`
  tie-break is load-bearing; and `=` is not `cmp() == 0`, since `texteq` never
  consults the collation for a deterministic collation.

  **Unscheduled, and additive to what exists.** Nothing in the current register
  is shaped against this — new arms split
  `ComparisonDivergence::NonBytewiseCollation` additively, and a collator would
  ride in `ComparisonPlan`, which is already per-column and already carries two
  per-column facts. Its **prerequisite is already met**: nothing can defer to a
  collation it has not read out of the preamble, and the declared collation is
  read ([`decisions.md`](decisions.md), "D55"). It acquires a phase number when it acquires a design, and that
  grilling is unblocked. Reasoning and evidence:
  [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md),
  "Collation splits by whether the answer depends on the source server".

- **CSV-format `COPY` blocks, as part of alternate-format support, post-1.0.**
  `pg_dump` has no CSV mode at all (I13), but `psql` writes `COPY ... WITH
  (FORMAT csv)` and that is valid PostgreSQL, so it is inside the input
  contract — just not built. What it costs is a second field decoder with its
  own quoting rules, plus the fixtures to pin it, and `copy.rs`'s independence
  from its byte source ("Four decisions that keep later phases additive") is
  what makes it a variant rather than a rework. Not to be confused with
  `--format`, which names the archive container and is P8 Track B.

- **Caller-supplied type mapping.** Let a caller override the
  PostgreSQL-type→Arrow-type resolution: per column, per declared type, or
  wholesale. Two uses, and the second is the important one. It lets a caller who
  distrusts our mapping substitute their own; and it lets a caller **pin a
  schema** against the drift that progressive type coverage otherwise causes, by mapping everything to a string type and getting the unparsed
  values, CSV-style. That makes it the mitigation named under "Pre-1.0" above.
  `SchemaMode::Strings` is the crude version of this that ships today.

- **The shape-general array representation, as a selectable alternative.**
  `Struct{dims: List<Int32>, lbounds: List<Int32>, elements: List<T>}` is
  lossless for every array PostgreSQL can produce — any dimensionality, any
  lower bound, varying freely from row to row — where the `List<T>` that ships
  today covers only the uniform 1-D case and degrades the rest to `Utf8View`
  ([`decisions.md`](decisions.md), "D34"). `List<T>` was chosen on koji-shaped evidence: short,
  uniform, one-dimensional arrays, where the struct costs +16 bytes per row and
  the loss of `List` as the signal every generic Arrow consumer reads as "this
  is an array". **A schema of matrices or scientific data inverts that
  arithmetic**, and those users should get the struct rather than a string. So
  this is a *knob*, not a replacement: the caller selects which representation
  an array column resolves to. It belongs beside **caller-supplied type
  mapping** above, which is the same knob at a different granularity, and
  adding it breaks nothing — it only ever changes columns that are `Utf8View`
  today, or a shape the caller has told us to represent differently.

- **A diagnostic for a brace run past `MAXDIM`.** `array_out` cannot emit more
  than 6 leading braces (I25), so a longer run means the file is not `pg_dump`
  output — hand-edited, concatenated, or damaged. The census records it and
  resolution deliberately declines to treat it as evidence, so the column keeps
  its optimistic type and the row surfaces as an ordinary `Error::FieldDecode`
  when something reads it. Nothing reports it at *scan* time, so a file nobody
  queries that column of stays silently damaged. The file-level `Diagnostic`
  channel is where this belongs; it is unscheduled because no fixture produces
  the shape.

- **A per-path shape census, so nested arrays get the same treatment as
  top-level ones.** The census records a shape per *column*, which fixes
  a top-level array column's dimensionality exactly and leaves an array
  *inside* a composite (or inside another array's element type) on the
  optimistic path permanently: its shape has nowhere to be recorded, so a
  multi-dimensional or `[lb:ub]`-decorated value there stays a hard
  `FieldDecode` even after `pgdt parse`
  ([`decisions.md`](decisions.md), "D34"). Keying the census by a *path* within the column
  rather than by the column closes that, at the cost of a bigger cache record
  and a per-path walk. Deferred on frequency — a composite with a
  multi-dimensional array field is rare even by that phase's standards — and it
  is purely additive whenever it lands: it only ever converts a hard error into
  a resolved type, so nothing that works before it works differently after.

- **Let a live scan emit rows again, by carrying the map in the resume token.**
  Map-building is separate from row emission
  ([`decisions.md`](decisions.md), "D48"), which costs a second read of the queried block: once to find its extent, once to emit its rows. The
  interleaved form can be recovered without reintroducing the unmapped hole
  that motivated the split, because `ResumeToken` is opaque and valid only
  within the producing process — so it can carry the live segment's in-flight
  spans and its open `CopyStart`, and a resumed stream can splice them in
  rather than re-deriving them. Deliberately deferred: it is an optimization
  over a query path still being iterated on, and it should be revisited once
  the feature set is settled rather than designed around now.

- **A public-surface sweep at feature-completeness.** The library's public
  surface is tightened once, when the feature set is complete, rather than piecemeal as each
  phase finds something it does not use. Known candidates: `batch::read_table`
  (push mode, whose only callers are tests and a bench, and which the DataFusion
  provider does not use); `TableStream::batch_source_offset`, which exists for
  `pgdt query`'s merge; and whether `pgdump_query` re-exports `bytes::Bytes`,
  which `ByteRangeSource::read_range` names in its signature so that an
  embedder implementing a source must depend on `bytes` in step with us.

- **Attach-time parse in `datafusion-cli-pgdump`.** Building a missing cache
  from inside the DataFusion binary instead of refusing and naming `pgdt parse`.
  It wants `pgdt`'s parse configuration — parallelism and memory discovery, the
  interrupt guard, the status output — moved into the library first, so that
  the two binaries parse alike (`decisions.md`, "D90").

- **An allocator-contention figure for a capped arena count.** Parallel `.xz` throughput under `MALLOC_ARENA_MAX=2` against uncapped — the price no figure takes, and what would reopen the in-binary cap `decisions.md`, "D13" refuses.

- **Group sizing from the header for a table of fixed-width columns.** Where
  every column's text form has a bounded width, the `COPY` header bounds a
  row's width before the first row, so a block could be sized without waiting
  for its end — its in-flight statistics at the final size from the start. The
  general rule sizes by rows per group at a block's end, because `varchar` and
  its kin leave a header silent on width ([`decisions.md`](decisions.md),
  "D82"); this is the cheaper case beside it, taken once that rule has shipped.

- **A query's line limit taken from the map.** A parse that needed
  `--max-line-bytes` has seen the dump's longest line, so the cache could
  record it and a query read with the larger of that and the default, leaving
  `pgdt query --max-line-bytes` and the provider's `pgdump.max_line_bytes` for
  a dump whose cache predates it. The limit is also what one row may cost
  resident ([`decisions.md`](decisions.md), "D23"), and the parse already paid
  it. It moves the cache format.

- **`RESET` for a provider's session settings, upstream.** It waits on
  DataFusion ([`../status/upstream.md`](../status/upstream.md), "UF3"), and is
  taken at the pin that carries the fix, beside `SET pgdump.memory = 0` rather
  than in place of it.
