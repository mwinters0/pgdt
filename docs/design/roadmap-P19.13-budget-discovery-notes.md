# P19.13 — the budget discovers the allocation

What the remaining slices inherit from `--parallel-memory`'s default becoming
the environment's answer instead of a constant. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; how the mechanism works now is
[`architecture.md`](architecture.md), "Execution model and API surface".

**The rule landed before its constant.** The spec's ordering is that `19.15`
runs against this build before `19.13` lands, so the mechanism below was written
and tested against a 256 MiB scratch reserve, `19.16` read the number off five
builds of it, and the box was ticked only when **384 MiB** was swapped in.

## What landed

- `io::discover_memory_limit()` — the `RT1`–`RT6` walk, with
  `discover_memory_limit_in(root)` beneath it so the v1 arm can be executed on
  a v2 machine.
- `io::available_memory()` — `RT8`'s `MemAvailable`, on the same root seam.
- `io::MEMORY_RESERVE` — **384 MiB**, `19.16`'s constant: the smallest of five
  candidates whose worst rep leaves at least 20% of the limit at every leg of
  the flagless family
  ([`roadmap-P19.16-reserve-constant-notes.md`](roadmap-P19.16-reserve-constant-notes.md)).
  Its doc comment states what the number does **not** cover — a host with more
  than 24 cores, and the block pool's floor below four readers — because both
  were found after the constant was chosen and neither is a reserve's to fix.
- `Parallelism::discover()` and `Parallelism::discover_for(jobs, per_worker)`,
  over `discover_in(root, …)` and `Parallelism::fit`.
- `ByteRangeSource::default_memory_per_worker()` — what **one** worker of this
  source holds; a defaulted `None`, overridden by `XzSource` with
  `block_reader_bytes()`.
- `ParallelArgs::resolve` asks for it whenever `--parallel-memory` is absent,
  exactly as it already asks for a count when `--jobs` is.
- `XzSource::default_workers` caps the core count at `SeekTable::block_count()`,
  and `Parallelism::discover_in` reduces the *count* alongside the budget where
  the environment's allowance affords fewer workers — the two halves of the
  consistent pair, below.

`MEMORY_RESERVE`, `discover_memory_limit` and `available_memory` are exported
from the crate root; `discover_for` is the entry point the CLI uses and
`discover()` is the no-source convenience the spec named.

## Three shapes to keep straight

**`discover()` is not what the CLI calls, and that is not an oversight.** The
spec names two primitives and one convenience; `discover_for` is that
convenience's general form, given recommendations a caller with an open source
already has, and `discover()` is `discover_for(available_parallelism(), None)`.
An embedder gets the sentence the spec promised — "the whole answer" — and the
CLI does not have to reimplement the reserve arithmetic to narrow it.

**The `Option` in the composition is the whole design, and it is easy to
flatten by accident.** The discovered branch's cap is `limit − MEMORY_RESERVE`;
the other branch's is half of `MemAvailable`, and where a source recommends
nothing there is **no cap at all** — `discover_in` returns before computing one,
which is what stops an unlimited host from falling back to 64 MiB and making a
compressed scan serial. A refactor that unifies the three into a single `min`
over an `Option<u64>` cap reintroduces exactly the defect the spec rejected.

**`Parallelism::fit` is where the pair settles, and it returns two numbers.**
Given `jobs`, `per_worker` and a cap it answers the largest `(count, budget)`
that fits: `count = clamp(cap / per_worker, 1, jobs)` and
`budget = min(cap, count × per_worker)`. Handing back the cap instead is the
over-ask — a budget the count cannot spend — and handing back `per_worker` at
the floor is the opposite one, a budget the allowance never granted. Both `min`s
are load-bearing.

**A budget of zero is a legitimate resolved value.** At or below a
reserve-sized limit `limit − MEMORY_RESERVE` is nothing, and the arrangement that produces is
one reader's worth on the streaming path, out of three floors that already
exist (`worker_count`'s `.max(1)`, `BufferPool::slots`' clamp to one,
`affordable` refusing block decode). `parse_parallel_memory` still refuses a
*stated* zero — a person who typed it meant something — and that asymmetry is
deliberate: this zero is arithmetic pgdq performed on a number the deployment
gave it.

## What `19.9` inherits

`19.9` is the test slice for this mechanism, and it is unblocked by this
landing rather than by anything else — its five deliverables all name something
that did not exist until now. Two of them changed shape here:

- **The status line's provenance is now the visible defect it was predicted to
  be.** A discovered budget prints bare, indistinguishable from a stated one,
  and `(default)` survives on exactly one arrangement: no flag, no limit found,
  and a source recommending nothing — a flagless plain scan on an unlimited
  host. `architecture.md`, "Status output", now says so, and names the
  three-way distinction the line is to gain.
- **The below-reserve arrangement is reachable and observed.** A container at
  or under the reserve prints `memory_bytes=0` today, with no note saying why. That is
  `19.9`'s `PlanNote`, and the number to key it on is `budget <
  what one slot needs` rather than anything about the limit.

**The fixture tree `19.9` owns is a committed one; this slice built temporary
roots.** `io.rs`'s tests construct a `FakeRoot` per case — `proc/self/cgroup`,
`proc/self/mountinfo`, `proc/meminfo` and a cgroup mount under one `tempdir` —
because landing the reader untested was the worse option. What that leaves
`19.9` is the *resolution* layer (`ParallelArgs::resolve` over a real source),
the status line, the `PlanNote`, and whichever of the committed-tree cases it
wants under `fixtures/` rather than inline; the unit cases here are the
reader's own and should stay wherever the reader is.

## What `19.15` runs against

`19.15`'s probe is a scratch build of this rule, carrying the 256 MiB reserve
this row later replaced. Four
readings taken here as a smoke check against that build, not as the probe — `fixtures/16/types/default.sql`
at 24,621 bytes and an `xz --block-size=4096` copy of it with **seven** blocks,
one rep each, `postgres:16`, no quiet machine — say the rule fires as designed
and nothing more:

| container | plain | `.xz` (4 KiB blocks, 7 of them) |
|---|---|---|
| `-m 256m` | `jobs=1 memory_bytes=0` | `jobs=1 memory_bytes=0` |
| `-m 512m` | `jobs=1 memory_bytes=67108864` | `jobs=7 memory_bytes=117749016` |
| `-m 3g` | `jobs=1 memory_bytes=67108864` | `jobs=7 memory_bytes=117749016` |
| no limit | `jobs=1 memory_bytes=67108864 (default)` | `jobs=7 memory_bytes=117749016` |

**Both halves of the pair are visible in that column.** The `.xz` count is seven
rather than this machine's twenty-four because the *file* offers seven blocks to
cut at, and it falls to one at 256 MiB because the *allowance* affords no reader
at all — where before the repair the same three rows all read `jobs=24`, the
last of them beside a budget of zero. The recommendation is `7 × 16.03 MiB`,
which the 512 MiB, 3 GiB and no-limit legs all clear — so above 512 MiB it is
the *source* that binds here, which is the arrangement `19.15`'s unlimited arm
is built to exercise without an unbounded run.

## The decline's report, and where the clause went

The spec owes the implicit block-path floor a **report** naming the limit that
caused it ([`architecture.md`](architecture.md), "Execution model and API
surface"). What landed is a clause, not a line: `Resolved::plan_note_origin`
returns ` — the budget in force is <Resolved::budget_display>` and
`announce_plan_notes` appends it to **every** plan note it prints.

Three calls inside that, none of which the spec made:

- **The clause is `budget_display` itself rather than a second spelling.** The
  mode report and a warning cannot then disagree about where a budget came
  from — the same reason the decline's *recourse* is read off the source
  instead of re-derived.
- **All three notes carry it, not the decline alone.** Every `PlanNote` names a
  memory budget as the thing that bound the plan, so "which number do I change"
  is the same question on each, and a rule with one exception is a rule someone
  will get wrong when a fourth note is added.
- **A stated `--parallel-memory` gets no clause.** The note already names the
  number that person typed; appending `(stated)` to it is noise. This is what
  keeps `parallelism.rs`'s existing decline assertions — all of which state a
  budget — reading exactly as they did.

**`parse` gains nothing here, and that is the judgement to check.** Plan notes
belong to a query's replay, so a `parse` whose block path is declined prints no
warning. What it does print is the mode report, and under a flagless run the
decline and a lowered count are the *same* arithmetic — `cap < per_reader` is
what makes `fit` return one — so the line already reads `jobs=1 (recommended by
the source; lowered from N by the allocation)` beside the file that stated the
limit. A `parse` under a **stated** small budget is the case that stays silent,
and there the number is the user's own.

## Calls the spec did not decide

- **`default_memory_per_worker` is charged at the pool's current slot size**,
  which before any read loop has announced one is `POOL_MAX_BYTES` (8 MiB)
  rather than the 1 MiB a scan settles at — so the recommendation runs about a
  tenth high. It is the same number `block_decode_bytes` answers, deliberately:
  one statement of what a reader holds. Erring high asks for budget the worker
  count will not spend, and every byte above `jobs × per-reader` is
  structurally inert.
- **The recommendation is per worker, which is what makes it independent of any
  count.** It was a total for the source's own count until the pair had to
  settle consistently: a total is a total *for some count*, so recovering the
  per-worker cost from it means dividing by a number `--jobs` may have replaced,
  and a stated `--jobs 4` against a source recommending twenty-four would have
  been reduced to one worker on a cap that comfortably held four. Stated per
  worker there is nothing to recover and the multiplication is the composition's.
- **A stated `--jobs` is not reduced; a recommended one is.** `discover_for`
  lowers the count it was given, because that is what "never allocate workers
  there is no memory for" means — but `ParallelArgs::resolve` keeps a count that
  came from the flag and takes only the budget, since the standing rule governs
  the absence of a flag and never its presence
  ([`roadmap.md`](roadmap.md), "A default runs as fast as the allocation
  permits"). What a stated count actually delivers stays `stream::worker_count`'s
  to decide from the budget, exactly as before.
- **v1's mount is located through `/proc/self/mountinfo`; v2's is the
  `/sys/fs/cgroup` convention.** That is what `std` does for the CPU quota, for
  the same reason — v1 mount points genuinely vary and v2's does not — and the
  v1 arm falls back to `/sys/fs/cgroup/memory` where the file lists no such
  mount, which is also what a fixture root stating only a cgroup path gets.
  `\040`-escaped mount points are not unescaped, which is `std`'s stated limit
  too.
- **`NO_LIMIT_AT_OR_ABOVE` is 4 TiB.** `RT4` says to read v1's "unlimited" as a
  threshold rather than an equality, because the value is a function of the
  page size and the word width; the *smallest* of the four shapes it takes is
  8796093018112 (32-bit), so the threshold has to sit under that. It is applied
  to the v2 files too, where only a stated limit above 4 TiB could reach it —
  and reading such a limit as unlimited falls back to the `MemAvailable` cap,
  which on such a machine is the smaller number anyway.
- **The manual moved here rather than at `19.10`.** Eight sentences went false
  the moment the default stopped being 64 MiB, and a falsified claim is
  corrected by the change that falsifies it
  ([`../process.md`](../process.md), "Where does this fact go?"). What moved:
  the flag's opening sentence, the "two ordinary ways of compressing produce
  blocks too large for the default budget" paragraph, four "at the 64 MiB
  default" phrasings that are now "under a 64 MiB budget", the koji
  transcript's `memory_bytes` and the paragraph reading it back, and the
  cgroup-sizing advice, which now says the reserve is already left for a
  flagless run. `19.10` still owns the `MALLOC_ARENA_MAX` recommendation as
  `M76` leaves it, both flags' help text as a whole, and the moved
  whole-block-decode threshold.

## The harness is unaffected, and here is why to check that again

Two registered command shapes state no `--parallel-memory`: `peak-rss`'s
`control` row and the `reserve` figure's `control` leg, both `--jobs 1` on a
plain file, both there precisely to read *the shipped default*. Under
discovery that value is now `min(64 MiB, limit − MEMORY_RESERVE)`, and both run
in a container of 3 GiB or more — so both still resolve to 64 MiB and the
harness's emitted sentence ("with no budget stated runs at the library's 64 MiB
default") stays true. It stops being true below a limit of about 448 MiB, which
moved with the reserve and is the number to re-check if either container
shrinks. A future
change that shrinks either container is what to watch: the apparatus rule pins
a worker count on every shape, and after this slice the *budget* on those two
shapes is a property of the container as well.

## What a large-block file asks for, and what caps it

A flagless `.xz` run on a host with no memory limit asks for
`jobs × per-reader`, and *per-reader scales with the file's block size*. On a
24 MiB-block dump that is ~1.4 GiB, which is the number the spec worked
through. On a file written by `xz -9 -T0`, whose blocks are ~192 MiB, one
reader holds `2 × 192 + 8 + decode_footprint` — the footprint carrying that
level's 64 MiB dictionary — so ~456 MiB, and twenty-four of them ~10.7 GiB.
**That is the shape of a file with at least twenty-four such blocks**, which is
about 4.6 GiB uncompressed; below that the block cap on `default_workers` binds
first and the ask is `blocks × 456 MiB`.

**The `MemAvailable` half-cap binds there**, at ~9.5 GiB against this
machine's ~19 GiB reading, and the block path is then afforded at a reduced
count — twenty-one readers rather than twenty-four, the count now coming down
with the budget — where the 64 MiB constant declined it outright. That is the
intended posture and the intended reversal. Reviewed 2026-09-10 and affirmed:
the no-limit arm owes "approximately play nice, and never OOM" rather than an
exact share, so the fraction is not tuned — the reasoning is beside the
mechanism ([`architecture.md`](architecture.md), "Execution model and API
surface"). No reading covers this band; `19.15`'s containers are all far below
it.

## The consistent pair

The recommended worker count and the recommended budget settle as a pair, on any
machine's cpu-to-memory ratio: never more memory than the workers can use, never
workers there is no memory for ([`roadmap.md`](roadmap.md), "A default runs as
fast as the allocation permits"). Two places enforce it, one per half.

- `XzSource::default_workers` caps the core count at
  `xz_seek::SeekTable::block_count()`, the table being already on the source.
  `stream::cut` caps pieces at the block boundaries the file offers, so without
  it a file with fewer blocks than cores is multiplied into a budget request
  nothing can spend.
- `Parallelism::fit` reduces the *count* alongside the budget wherever the
  environment's allowance affords fewer workers, so a reduced allowance can no
  longer yield a count the budget does not cover.

The first supersedes a sentence of `19.8`'s rationale, which held that the
block count binds downstream and the recommendation need not know it. That was
written before a run reported its mode: once it does, a recommendation of
twenty-four workers printed beside a budget affording six is self-contradictory
in one line. Reasoning: [2026-09-10](../status/history/2026-09-10.md), "The
recommended pair has to be consistent".
