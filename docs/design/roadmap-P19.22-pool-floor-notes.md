# `P19.22` — the charge bills the pool floor

What `19.23`, `19.25` and the closing sweep inherit. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the mechanism is [`architecture.md`](architecture.md), "Execution
model and API surface", under *A source's cost is a shape rather than a
scalar*.

## What landed

`io::WorkerMemory` — a `Copy` value carrying a **per-worker term** and a
**shared floor** of `floor_unit` bytes for every worker short of `floor_below`
— replaces the `Option<u64>` per-worker scalar everywhere a budget used to be
divided by one:

- `ByteRangeSource::default_memory_per_worker` → `default_worker_memory`,
  answering `Option<WorkerMemory>`;
- `Partitioning` carries one (`worker_memory()`, stated by `.flooring(unit,
  below)`); `partition_bytes()` is now its per-worker term, unchanged in value
  and in every other consumer;
- `Parallelism::{fit, discover_for, discover_in, recommended_within}` take one
  and **solve** for the largest affordable count (`WorkerMemory::affords`)
  rather than dividing;
- `stream::worker_count` does the same for the **delivered** count;
- `BlockCache::worker_memory` is the single composition site, and
  `BlockCache::affordable` compares the budget against `at(1)`.

`XzSource::block_reader_bytes` is gone; `BlockCache::reader_bytes` is the name
every doc that meant the per-reader term now uses, and
`XzSource::block_worker_memory` is what composes it with the floor.

## The floor is an account, not a fit

`(POOL_DEPTH − jobs)⁺ × unit`, derived from the pool's own arithmetic rather
than measured: `BufferPool::slots` clamps the block pool at
`POOL_DEPTH.max(jobs)`, the free list and the retention list share those slots
(`BufferPool::reserve`), and each reader is decoding into a buffer besides — so
the pool holds `slots + jobs` units against a bill of `2 × jobs`, and the
difference is `slots − jobs`. Under discovery the budget the rule now names is
`at(n)`, which leaves `slots = POOL_DEPTH.max(n)` exactly at every `n`, so the
model is self-consistent: it is not a shape that holds only where it was
checked. Five cells of `19.16`'s readings confirm it to 1.4 MiB
([2026-09-12](../status/history/2026-09-12.md), "The charge under-bills the
pool floor, and the record says it does not").

## The three calls made inside the row

**`stream::worker_count` was repaired too, though the row names only
`Parallelism::fit`.** The charge has two consumers — the *recommended* count
and the *delivered* one — and repairing one of two is the failure mode that
produced this row in the first place (`.claude/skills/evidence/SKILL.md`, rule
5). Under discovery it changes nothing, the budget `fit` names already
affording the count it named; it binds where a budget was **stated**, which is
every `control_*` leg of the `reserve` figure.

**The gate is `at(1)`, so the decline widens — and that is the finding.** A
24 MiB-block file now wants 130 MiB where it wanted 58, and a 128 MiB-block one
650 where it wanted 266, so a **512 MiB allocation reads koji's shape through
the streaming decoder**. The spec accepted this in advance, and the probe taken
at exactly that limit says the block path buys nothing there anyway — two
readers 6% slower than declining, one reader 3%, at 250 MiB and 63 MiB resident
against the fallback's 15 ([`roadmap.md`](roadmap.md), "A block cache whose
retention floor tracks the reader count").

**`affords` enumerates below the floor and divides above it.** The cost is not
monotone below `floor_below` — a *larger* count can cost *less*, the floor
decaying faster than the per-worker term grows — so the two regimes are
separated rather than searched over: above the floor the largest affordable
count is one division, below it there are at most `POOL_DEPTH` candidates. A
plain descending scan over `1..=jobs` would also be correct and is O(`--jobs`),
which a user states.

## What the harness had to follow

`charge_bytes(unit, jobs)` is `WorkerMemory::at` mirrored, and `charge_model`'s
`billed` is now that rather than `jobs × reader_bytes`. **No number in the check
moved** — `billed_new = billed_old + floor` and `unnamed = held − billed_new` is
the same arithmetic — so the five seeded cells still reproduce, and what changed
is the claim: the floor is a named part of the bill rather than a term the bill
misses. The seeded test now asserts `billed − floor == budget`, the seed's
budget column being what the *pre-repair* rule resolved, which is what holds
both shapes to one set of readings as `19.24` intended.

The decline filter moved with it: a leg is excluded when its reported budget is
below `charge_bytes(unit, 1)`, which is the gate the library now applies.

## What the row found and did not fix — `KD21`

The model is exact under discovery because the budget `fit` names leaves the
block pool `POOL_DEPTH.max(n)` slots at every `n`, which is checked arithmetic
rather than a coincidence of the cells measured. It is **not** exact when a
caller states both `--jobs` and a `--parallel-memory` too small for that many:
`XzSource::apportion` sizes the pool from the *announced* count while
`stream::worker_count` delivers a smaller one, so the slots are bounded by the
byte budget instead and the pool overfills — 1,280 MiB against a stated 1,024 at
`--jobs 24 --parallel-memory 1g` on a 128 MiB-block file. That predates
`WorkerMemory` and is untouched by it; closing it reworks a pool's sizing rule,
so it is filed as `KD21` with its paragraph beside the mechanism rather than
fixed here.

## What `19.23` inherits

- **The model exists, so a predicted resident is now computable in the
  library.** `WorkerMemory::at(n)` is the whole of what a count costs, and
  `19.23` is the row that makes `fit` refuse a count whose predicted resident
  breaches the margin. What it still lacks is the term the reserve covers —
  `at(n)` bills the pools and nothing else, so the margin check needs
  `MEMORY_RESERVE` beside it rather than inside it.
- **`Parallelism::fit` is the one place a count is chosen against a budget on
  the recommendation side**, and `stream::worker_count` is its twin on the
  delivered side. A criterion added to one and not the other reproduces exactly
  the split this row closed.

## What the sweep inherits

Every compressed figure was already stale on `pgdump_query/src/io.rs`, and this
change genuinely moves the `reserve` figure's flagless axis: at `512m` the
24 MiB input now declines the block path, so that cell becomes a *declined* leg
rather than a two-reader one and the model is evaluated at one cell fewer.
`19.11` publishes whatever the sitting then reads; nothing here predicts it.
