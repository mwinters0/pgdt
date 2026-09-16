# P20 — statistics memory: what the phase left behind

The phase's decisions are in [`decisions.md`](decisions.md), D75–D85, and the
mechanisms are the code. What is here is what neither carries: the results that
were negative, and the facts aimed at the phases that follow.

## What it delivered, and what it did not

**Delivered: statistics are bounded.** They shipped with nothing bounding what
they held resident (`KD28`, struck by 20.7). The bound is the account plus the
decline — every statistic alive is counted against one allowance, and a block
whose gathering passes it frees what it held and records the number. The bound
holds over the dump's *length* as well as a row's width, the account being
cumulative.

**Not delivered: coverage, or the constant.** The bound costs the tail of a
long dump (`KD33`), and `MEMORY_RESERVE` was never read against a run with
statistics in it (`KD34`). Both are P23's. The phase stopped there rather than
fitting a constant that P21 and P22 are about to move.

## Negative results

**Accounting.** An interning map rehashes with both tables live, so a map about
to grow is charged its larger table *before* the insert; a vector is charged
before the push, not after; a fold moves the piece's charge onto the block in
one update, or the moved groups read twice at the peak; a piece's remnant goes
back to the piece's own charge rather than being dropped while the block still
carries it. Across threads an announcement is a term of its own — a shortfall
is judged against the whole account and an excess against the account less what
is announced (D81) — and the account is one lock in every build, an update, its
peak and the check read whole. `HashMap::retain` leaves the table its size
while reporting a smaller capacity, so a pruned map is rebuilt at its kept size.

**A block's peak is not its retained heap.** The interning maps are a term the
retained heap does not carry, and on a small block most of the growth arrives
in `finish`. An allowance read off `heap_bytes` does not decline the block; read
it off an unbounded control's own peak.

**Merging.** Clipped bytewise bounds do not order as their values, so a merge
picking the larger *stored* text can keep the smaller value's inexact bound — a
closed group keeps its extremes' heads until `finish` clips them. Merging only
at an even group count leaves a block stuck past its cap, a window's fold being
able to leave it odd; the last closed group is taken back into the open one. A
merge with a piece alive breaks the join.

**Sizing.** The minimum cannot be read at `2^20` once the cap has merged —
summing the finer rows per group grows with the block — so it reads the size the
cap left, and the predicate must therefore be **monotone in size** (20.4's
nearest-rank median was not; the upper middle group is). The cap cannot yield to
a stated maximum block by block: that reads a partial distribution and depends
on where the leader split, so serial and parallel would disagree; a stated
maximum turns the cap off for every block instead. A block records the maximum
it was sized under whether or not it reaches it, so what stops a third read is
the *size*.

**Declining.** Not inside a merge: `charge_held` runs where the groups and each
column's vectors are half rebuilt, so `over` is recorded there and acted on at
the next whole boundary. A declined piece declines its block rather than being
skipped — a block that lost a piece's rows cannot state anything over them, and
dense group indices cannot express a gap. **The decline is deliberately not
deterministic between a serial and a parallel pass**: the account sees a
window's pieces as well as its blocks, so the same file at a higher `--jobs` can
decline where a serial pass did not.

**A stated allowance cannot raise a plain dump's read buffers.** `fit` with no
`WorkerMemory` returns `DEFAULT_MEMORY_BUDGET.min(cap)` and `LocalFileSource`
recommends none (`KD25`), so `--memory 8g` leaves a plain dump's pools on the
64 MiB constant where `--parallel-memory 8g` gave them 8 GiB. No flag reaches a
plain source's pool budget; `--chunk-size` is what is left. What a plain `query`
gets from `--jobs` is bought with batch size instead (D84).

**The attribution sitting's three.** The wide-text input built to fill an
allowance could not: dictionary *text* is interned once per block and column,
not per group, so its whole-file account was 51.86 MiB against a smallest
allowance of 89.60 MiB, and it was deleted rather than corrected. Widening rows
*lowers* statistics volume — final groups are about
`min(block_bytes / 1 MiB, STATISTICS_GROUP_CAP, rows / min_rows)`, so above a
row width of one group size per minimum row count the density merge binds and
doubling the width halves the volume; many columns of short values raise it, as
does a bytewise-comparable column earning per-group bounds. And the statistics
allowance is **non-monotone in the container limit**, being
`margin_allowance(allowance) − budget`: across the `xz24` legs it ran
47.6 → 39.1 → 6.9 → 0.04 → 10.2 → 13.6 MiB as the limit grew.

## What the next phases inherit

**P23** — `KD33`, `KD34`, and the attribution readings, which no sitting
re-takes. Its remedy for the first is a granularity derived from the dump's
length; D85 refuses coarsening *to fit* because a cache would depend on its
container, and a length-derived size is the same on every machine, so the
refusal does not reach it. One instrument is kept for it:
`instrument::statistics_loaded`, the heap a cache load hands a pass, the only
statistics term a query has.

**P22** — [its inbox](roadmap-P22-third-tunable-inbox.md): the starve band is
priced and it kills, and the registered billing branch is unspent.

**The margin binds a typed number, and that moved figures.** A stated budget
carried no ceiling; an allowance does, crossing over at
`4 × MEMORY_RESERVE − 5 × MEMORY_UNPOOLED_BOUND`. `measure.py`'s
`stated_allowance(budget)` states `budget + MEMORY_RESERVE` so every figure
still names the buffer budget it was registered for, but legs above the
crossover resolve fewer readers than their published cells were taken at.
`reserve`, `rss-attribution`, `statistics-gathering` and `statistics-pruning`
are stale for this reason and are not re-taken until the constant settles.

**What is unpriced.** What the decline check costs — one `bool` off an update
already taken under the account's lock, and a branch a row — and what a decline
*saves*, a pass that declines every block doing less work than one that gathers.
No figure says either. Koji's row density was read (20.2) and its median does
not meet the registered criterion, so the median stands.
