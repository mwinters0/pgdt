# P19.15 — the budget rule against real cgroups

The gate the phase held `19.13`'s box open for. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the rule being probed is
[`architecture.md`](architecture.md), "Execution model and API surface".

**It does not pass.** A flagless `.xz` `parse` inside a **512 MiB** allocation
holds 474 MiB median and 503.7 MiB at its worst of three — 1.6% of the limit —
against a per-rep spread of 10.2%, which is the criterion the reserve review set
for itself. Everything else about the resolution is right, and the predicted
thin band is comfortable. So the reserve reopens, and not to the term the spec
named: see "Where the reopening cannot go", below, and the entry under
`STATUS.md`'s "Decisions worth another look" that is holding the call.

## Apparatus

A `runs/` probe, **not a figure** — no document may quote a number below as a
measurement, and nothing in the repo consumes the output
([`measurements.md`](measurements.md)).

- `runs/19.15-budget-probe.py` — nine legs, three reps, interleaved and
  reversed on even reps. Log and readings beside it as
  `19.15-budget-probe.{log,json}`.
- `runs/19.15-arena-probe.py` — the two interesting limits again at
  `MALLOC_ARENA_MAX=2`, to test the standing account of the excess.
- `runs/19.15-edge-probe.py` — ten reps of the one leg whose headroom is inside
  the noise, because three cannot say whether the distribution crosses the
  limit.

Instrument, container parameters and command shape are the `reserve` figure's
(`scripts/measure.py`, `RESERVE_FAMILY`) **minus the two flags**: `getrusage`
through a forking `perl` wrapper, `-m X --memory-swap X` so there is no swap to
hide behind, `postgres:16` because the allocator is part of the apparatus, and
`parse --source /dump.sql --dqcache /tmp/x.dqcache`. The binary is
`runs/pgdq-19.15`, a release build of `9850bf8` — which *is* the scratch build
of the rule the spec asked for, `19.13`'s code being in the tree with its box
open. The inputs are the reserve figure's own, read off the SSD rather than
tmpfs: peak resident is the quantity and the staging area does not reach it.
The machine was not quiet — this host runs other containers — which is the
ordinary reason a probe is not a figure.

## The reading

Peak resident set, median of three, beside the limit it ran in and what the run
reported it had resolved. `head@worst` is the margin the *worst* of the three
reps left, which is the number a cgroup's killer actually reads.

| leg | limit | `jobs=` | `memory_bytes=` | med RSS | worst RSS | spread | head@med | head@worst |
|---|---|---|---|---|---|---|---|---|
| `.xz` | 256 MiB | 1 | 0 | 15.1 | 15.3 | 3.4% | 94.1% | 94.0% |
| `.xz` | 512 MiB | 3 | 195.1 MiB | 474.4 | **503.7** | 10.2% | 7.3% | **1.6%** |
| `.xz` | 1 GiB | 11 | 715.4 MiB | 781.5 | 930.0 | 26.0% | 23.7% | 9.2% |
| `.xz` | 1.5 GiB | 19 | 1235.6 MiB | 995.8 | 1022.6 | 9.6% | 35.2% | 33.4% |
| `.xz` | 8 GiB | 24 | 1560.8 MiB | 1136.9 | 1251.8 | 10.3% | 86.1% | 84.7% |
| plain | 256 MiB | 1 | 0 | 5.9 | 6.1 | 5.5% | 97.7% | 97.6% |
| plain | 512 MiB | 1 | 64 MiB | 6.0 | 6.1 | 2.4% | 98.8% | 98.8% |
| plain | 1 GiB | 1 | 64 MiB | 6.0 | 6.1 | 11.0% | 99.4% | 99.4% |
| plain, `--jobs 24` | 1 GiB | 24 (stated) | 64 MiB | 80.5 | 111.0 | 38.4% | 92.1% | 89.2% |

Ten further reps of the 512 MiB `.xz` leg: **421.8–509.1 MiB, median 446.4,
spread 19.5%, no run killed** (`runs/19.15-edge-probe.log`). The worst of those
ten leaves **2.9 MiB of 512** — 0.57%. So on this file the distribution's upper
tail sits just under the limit rather than across it, which is a bound and not a
reprieve: thirteen reps have not crossed it, the margin at the top of them is
three megabytes, and the shape being read is one 3 GiB dump's.

**Two limits beyond the three the spec names, and each earns itself.** 1.5 GiB
is where `19.12`'s fit put the thinnest headroom, so the row the gate was aimed
at had to be in the sitting; 8 GiB is the unlimited arm at scale — `8 GiB −
256 MiB` clears `24 × 65.03 MiB`, so the *source's* recommendation binds and
what the rule does on a roomy host is visible without an unbounded run.

## What the resolution got right, on every leg

The rule itself resolves as designed, and this is the half that is not in doubt.

- **The pair is consistent everywhere.** Each `.xz` leg's count is what its
  allowance affords and its budget is exactly what that many readers spend —
  195.1 MiB is `3 × 65.03`, 1560.8 is `24 × 65.03` — never the cap. At 8 GiB the
  count stops at twenty-four because that is the recommendation, not because
  the allowance ran out.
- **`jobs=` reads its provenance correctly in all three spellings** that the
  sitting can produce: `(recommended by the source; lowered from 24 by the
  allocation)` on four `.xz` legs, `(recommended by the source)` on the plain
  legs and on the 8 GiB one, `(stated)` on the `--jobs 24` leg.
- **A 256 MiB allocation resolves to a budget of zero and runs**, on both
  sources — 15.1 MiB of resident on the `.xz`, which is the streaming fallback,
  and 5.9 MiB on the plain file. The arrangement `19.9` pinned in a fixture root
  is the arrangement a real 256 MiB cgroup produces.
- **The plain path is untouched by any of this**: 6 MiB at every limit, which is
  `peak-rss`'s published 5.85 and the cheapest check that the instrument is the
  one that took it.

**The per-worker charge a flagless run asks for is 65.03 MiB, not 58.03.**
`default_memory_per_worker` is read before any read loop has hinted a chunk
size, so `pool.slot_bytes()` is `POOL_MAX_BYTES` (8 MiB) rather than the 1 MiB a
scan settles at — `19.13`'s notes predicted "about a tenth high" and the
readings put it at 12%. It costs nothing here: the budget asked for is inert
above `jobs × what a reader holds`, and the count is what the allowance
actually affords.

## `19.12`'s fit is refuted in both terms

Least squares over the four block-decoding legs, against the reader count each
resolved:

| | fixed | a reader | predicts n=3 | n=24 |
|---|---|---|---|---|
| `19.12` (stated budget, `--jobs 24`) | 180 MiB | 59.4 MiB | 358 | 1606 |
| this sitting (flagless, discovered) | **403 MiB** | **31.2 MiB** | 497 | 1151 |

Residuals reach ±36 MiB. The intercept more than doubled and the slope halved,
and the two errors cancel at a high reader count and compound at a low one —
which is exactly why the thin point moved. `19.12` read resident across a
*stated* budget with the count pinned at twenty-four; a flagless run brings the
count down with the budget, so the two axes are not the same axis and the
earlier fit was extrapolated onto an arrangement it never measured. That
extrapolation is what the reserve review's headroom table was built on.

**The fixed term is the problem, and it is bigger than the reserve.** ~403 MiB
of resident is there before the first reader is counted, against a
`MEMORY_RESERVE` of 256 MiB. Every allocation whose budget affords few readers
therefore sits close to its limit, and the smallest such allocation that reaches
the block path at all — 512 MiB — sits closest.

## It is not arena retention

The standing account of resident above the pools is glibc arena retention
(`roadmap-P19.12-reserve-retake-notes.md`, "`19.7`'s prediction is refuted").
`MALLOC_ARENA_MAX=2` on the same two legs:

| leg | uncapped | capped | the cap buys |
|---|---|---|---|
| `.xz`, 512 MiB | 474.4 | 412.8 | 61.6 MiB (13%) |
| `.xz`, 1 GiB | 781.5 | 636.1 | 145.4 MiB (19%) |

So arenas are **a sixth** of the 512 MiB leg's fixed term, not the bulk of it,
and the cap lifts that leg's median headroom from 7.3% to 19.4% — better, and
still a setting the operator has to make themselves. The remaining ~340 MiB is
unattributed: what the pools are charged for accounts for about 250 MiB of the
512 MiB leg (eight coupled block slots at 24 MiB, three decoders at 9.5, three
chunk buffers, and the ~6 MiB a plain scan holds), and the rest is not yet
named. **Nothing here should be read as an attribution**; what the sitting
establishes is that capping arenas does not rescue the margin.

## Where the reopening cannot go

The spec named the reopening in advance: "a proportional term is what it
reopens to". **The reading rules that out.** A proportional reserve is smaller
than a constant one at a small limit and larger at a large one, and the failure
is at the *small* limit — `limit × (1 − f)` with any `f` that leaves a 1.8 GiB
allocation 20% would hand a 512 MiB allocation *more* budget than 256 MiB does,
not less. The three shapes that remain are a larger constant (which the review
already priced: ~400 MiB puts a 600 MiB cgroup below the floor, and the floor is
then the thing to decide), a *floor on the limit* below which the block path is
declined outright, and attacking the 403 MiB fixed term itself. Choosing among
them is a decision the phase's spec records, so it is not this session's:
`STATUS.md`'s "Decisions worth another look" carries it, and the evidence is
[2026-09-11](../status/history/2026-09-11.md), "The budget rule's headroom fails
at 512 MiB, and not where it was predicted to".

## `KD18` inside an allocation costs a quarter of what it costs outside one

The `--jobs 24` plain leg is the first reading of that shape against a real
limit, and the entry's 209 MiB does not appear: it holds **80.5 MiB**, because
the discovered budget is 64 MiB and `stream::worker_count` admits eight workers
of it, not twenty-four. `16 + 8.03 × 8` is 80.2. So the deficiency's cost is
bounded by the budget as well as by the flag — inside a container a user who
states `--jobs 24` on a plain file gets eight workers' worth of arena retention,
and reaching 209 MiB means stating `--parallel-memory` too. The entry is left as
it is: nothing about it closed, and its detail paragraph is about the shape, not
about a container.

## What the next slices inherit

- **`19.13`'s box stays open**, and now for a different reason than the one its
  checklist entry gave: the gate has run, and it found the constant wanting
  rather than confirming it. Its code is unchanged and still in the tree.
- **The unit-test half of this row was already delivered** by `19.13` —
  `no_limit_found_takes_the_sources_own_answer_under_the_memavailable_cap`, over
  `19.9`'s `no-limit` and `cramped` roots — so what this slice owed was the
  at-scale reading, which the 8 GiB leg is.
- **`19.11` inherits nothing publishable.** No figure moved, no harness path
  changed, and the closing sweep's `reserve` table is still what publishes a
  number about the reserve.
