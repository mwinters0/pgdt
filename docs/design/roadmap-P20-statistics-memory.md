# P20 — Statistics memory and its measured expectations

**Current.** Grilled with the maintainer 2026-09-15, and reopened the same day
for granularity ("Granularity follows row density"; the reason is [`../status/history/2026-09-15.md`](../status/history/2026-09-15.md));
the slices are [`../status/STATUS.md`](../status/STATUS.md), "P20 progress".
This document says what the phase does and why, never how it lands in code.

## What this phase is for

Row-group statistics shipped with nothing bounding what they hold resident
(`KD28`). Earlier resident-memory work bounded block-level structures, which
are flat in the dump; statistics are **facts about the data itself**, and no
default accommodates every real shape — a table of a thousand `varchar`
columns will never gather under a 512 MiB ceiling however it is tuned. So this
phase gives **reasonable defaults for reasonable data**, and fits the rest with
the two knobs a person already has, under the standing rule it adds:
[`roadmap.md`](roadmap.md), "Two tunables fit pgdq to hardware: memory and
parallelism".

**Reasonable data includes a long dump.** Statistics grow two ways: with a
row's *width* (columns, value sizes), which is the shape hand-tuning is for,
and with the dump's *length*, which is the case this project exists for. The
second is not a challenging shape, and under today's defaults it is unbounded:
koji, a narrow dump, would hold on the order of a gibibyte and a half of
statistics at the shipped group size (facts below) — no constant reserve
reaches a term growing with bytes.

## The decisions

### Granularity follows row density

**A group's size is a power of two, `2^20` at least.** Under an unstated group
size a block's groups are coarsened past that floor where its rows are sparse;
a stated `--statistics-group-size` is still honoured exactly, and a caller can
turn the coarsening off, leaving the floor as today.

**Granularity is a trade a person may state a side of.** Fewer rows per group
prunes more, so a query reads less; more groups cost the RAM that holds their
statistics and the CPU that gathers and decodes them. A person sensitive to
I/O wants more groups, one sensitive to RAM or CPU fewer, and the default is a
point between.

**The unit is rows per group.** A group costs an entry per column whether it
holds one row or thousands, so what grows with a row's width is statistics per
row, not per byte. Column count says nothing of a row's width unless every
column is fixed-width; that shape is worth its own sizing, after the general
rule that has to account for `varchar` and its kin.

**It is decided from the whole block, within the mapping pass, and the pass
stays streaming**: no row is held for it, and no read is added for it except
where a stated maximum asks for one (below). A block
gathers at its base size and, once its last row is seen, merges pairwise to the
size the rule chooses — exact, so the result is what gathering at that size
would have given, and decided after every parallel piece has joined, so serial
and parallel agree. **The saving is claimed for retained statistics**, the
finished blocks that accumulate until exit; a block still being gathered holds
statistics, never rows, at no more groups than the length cap allows.

**The decision reads a distribution of row density, never a mean**, because
row width varies within a table, and a block written in heap order is no sample
of itself. The distribution is **rows per group**, already recorded per group,
whose shape at every candidate size follows from the base size's by summing
adjacent pairs. **A minimum** chooses the smallest size at which a low quantile
group holds at least the minimum rows; **a maximum** the largest at which a high
quantile group holds at most the maximum. Where the length cap asks for a coarser size
than the minimum, the cap's size stands — **a guarantee about the block's size,
not an order of precedence**: it is never coarser than the larger of the cap's
size and the size the minimum picks from its own base distribution.

**That guarantee is what fixes the minimum's quantile at the *upper* middle
group**, the `⌊G/2⌋+1`-th smallest — equivalently, at most half the groups fall
short. The cap merges mid-scan, summing the finer distribution away, so the
minimum can only be read from the size the cap left; the guarantee therefore
holds only where the predicate is **monotone in size**, since a monotone one
cannot be satisfied below a size at which it fails. It nearly is already: a
merged group holds the sum of its parts, so it falls short only where both
parts did, and the count of short groups at most halves as the total halves.
The exception is an **odd tail** — a lone short last group with no pair — which
the nearest-rank `⌈G/2⌉`-th smallest lets push the fraction past half, and the
upper middle group does not. Under it the guarantee costs nothing to keep.

*Rejected:* the nearest-rank median, whose odd tail cascades — `[m, m, m, m, 0,
0, 0]` meets a minimum `m`, its pairs `[2m, 2m, 0, 0]` do not, and the block
coarsens on to one group, so one past the cap loses all pruning on a parity
accident; the base rows per group kept beside a capped block until it finishes,
8 bytes a MiB and a term growing with length, which is what `KD28` and this
phase exist to remove; per-size counters of short and total groups, bounded but
needing their own row tally across pieces and window folds, no base-sized group
existing after the first merge.

**The default minimum is a width judgement, not a koji reading**: `2^20` stays
right for rows up to a reasonable width, taken as 1 KiB, so the default
minimum is `2^20 / 2^10` = 1,024 rows and changes nothing for a table of
reasonable width. Koji cannot choose it — its statistics fit under either
candidate, the cap governing its large blocks. **The minimum's quantile is
registered at the median**, the upper middle group above, which on any block
keeps retained groups within twice rows over the minimum whatever the
distribution — a bound `G ≤ 2R/m` that holds under either middle group, at
least half the groups reaching the minimum either way; **the criterion,
registered before the reading**: a lower quantile is chosen only if a koji
block of reasonable width comes close to that bound.

**A maximum is honoured only where stated, and then over the length cap**, as a
stated group size is: it is the side of the trade a person sensitive to I/O
states, and 20.7's decline, not the cap, keeps it from costing the process.
Its quantile is **the 90th percentile**, a judgement no reading prices, koji
stating no maximum: near enough a bound for a person who asked for one, without
one dense stretch multiplying a whole block's groups. **Every block is gathered
at `2^20` first; one whose groups break a stated maximum lacks what was asked
and is re-read by the back-fill at the size its recorded rows per group
predict**, and where rows cluster so that the size still misses, it keeps what
the re-read gave and the run says so — never a third read. So only a block too
dense for a stated maximum gathers below `2^20` or is read twice, and nobody
else pays either. **The default states a minimum and no
maximum**, so `2^20` is the default's finest size. **Where a block meets
neither, a stated maximum wins** over the minimum: only a person stating one
reaches the conflict, and a default does not quietly overrule what they asked.

**Three switches, each intent**: `--statistics-group-size`,
`--statistics-min-rows` and `--statistics-max-rows`, mirrored in
`StatisticsRequest`. `--statistics-min-rows 0` turns coarsening off. A stated
group size is exact, so it is refused beside either row flag, as beside
`--statistics none`; **it must be a power of two**, the sizes having to nest;
and a maximum below the minimum is refused.

**A block records the bounds it was sized under**, beside its size, since a
size no longer says which request chose it; D34's rule extends to each bound —
a stated one differing from the record re-reads the block, an unstated one
takes what the block holds. `FORMAT_VERSION` is bumped (D22).

*Rejected:* a separate phase ahead of this one — coarsening is the same
pairwise, byte-aligned merge the length cap uses, and the account 20.1 builds
is what prices it; sizing by the first `2^n` bytes, an unrepresentative head;
coarsening as the rows arrive, which cannot undo a merge that later, narrower
rows refute, and which a partition amid a block cannot reproduce; sizing from
the `COPY` header's column count as the general rule; a default maximum, which
every dense block would pay for in flight and which fights the cap on koji's
`buildroot_listing`; a stated maximum's base bounded from the header — a row
being at least as many bytes as its columns — which reads once but holds
groups so fine that a large table declines; a strict maximum.

### Length is bounded by a constant, per block

**A block's statistics hold at most a fixed number of groups under an unstated
group size**; past it, adjacent groups merge pairwise, so resident grows with
blocks × columns rather than bytes × columns, boundaries stay byte-aligned and
parallel gathering stays identical to serial. **A stated
`--statistics-group-size` is honoured exactly**, a precision a person asked
for.

**The cap is a judgement, confirmed or moved by the koji gathering run**
(P20.10), argued from one criterion: koji's flagless gather fits 512m with the
margin left. Arithmetic puts it on the order of 4,096 groups a block — the
largest that plausibly fits; at 1,024 koji's statistics would be about 10 MiB
and `task`'s groups about 350 MiB, at 16,384 about 150 MiB and 22 MiB.

*Rejected:* length left to the user, which makes the common case the tuned
one; finished statistics leaving memory after a save, which reopens D78;
deriving the cap from a pruning-precision target, which no reading prices.

### Granularity never depends on memory; whether a block gathers does

**The cache is a function of the file and the request alone.** Where the
memory allowance cannot hold a block's statistics, gathering **declines for
that block** and the run says so — the shape of a compressed source declining
block decode (D19) — and the scan continues, the block dropping what it
gathered and what is in flight. Raising memory is therefore what fits a wide
table, and **nothing is OOM-killed for want of statistics**.

**A decline is recorded in the cache with the allowance it declined under**,
and back-fill retries it only under a larger one; at the same or a smaller
allowance the block is left and the run says so. Without the record,
`StatisticsRequest::backfill` reads a block holding none as lacking them, and
every `parse` at the same allocation would re-read and re-decline it — `task`'s
354 GiB each run. The record is an absence and a number; wherever a cache holds
statistics it is still a function of the file and the request.

*Rejected:* a constant bound with no memory reading, which leaves a wide table
an OOM kill; granularity coarsened to fit the allowance, which makes a cache
depend on the container that wrote it and breaks serial/parallel identity;
retrying a decline every run; retrying only when a selection names the table,
which leaves a person remembering what declined.

### Memory means resident, and is one flag

**`--parallel-memory` becomes `--memory`, the resident allowance** — the number
a person gives their container. A stated one is treated exactly as a
discovered limit: the reserve, the margin, the pools' charge and the
statistics' allowance are all carved from it. Today's flag is a read-buffer
budget whose process holds more than it states, so an operator sizing a
container from it is OOM-killed. No shim (pre-1.0).

**The library means what the CLI means.** A stated `Parallelism` memory is a
resident allowance carved the same way — what its rustdoc already calls it.
Stating none declines nothing: statistics are then bounded by the length cap
alone, the embedder having chosen no bound (D1).

**With no limit found and none stated, the CLI's statistics allowance is half
of `MemAvailable`** — the number "A default runs as fast as the allocation
permits" already uses for a host that has said nothing. A machine observed is
not permission, and the one estimate in the path (RT8) fails safe: a decline
costs a re-read later, not a kill.

*Rejected:* keeping the buffer budget and giving statistics what it leaves, a
knob an operator cannot act on; both flags, a third knob; a library default
allowance, a decline against a constant nobody chose; discovery in the
library, which D1 refuses; no allowance on a bare host, where pgdq shares
memory with everything; a fixed constant there.

### Workers are resolved first; statistics take what is left

**The worker count and the pools' charge resolve as today, and the statistics
allowance is what that arrangement leaves under the margin.** The count is
fixed before a byte is read, while statistics are known only as they
accumulate, so this is the one order that decides nothing before the data
exists; on a plain dump nearly the whole allowance is statistics'.

**The account a decline reads sees every statistic alive**: retained blocks,
blocks loaded from the cache, a parallel window's partition observers — closed
groups, held-open head, piece-local dictionary — until the window folds, and a
block's dictionary held twice while it is gathered. A term the
account cannot see breaks "nothing is OOM-killed" on exactly the wide,
many-worker shape where the term is large. **The margin is left against
everything held, the account included, which closes `KD28`.** Removing terms —
folding partitions as they return, interning once — is left an optimization
the gate may motivate.

**Neither end of the cache file holds it whole.** A save encodes into the file
through a buffered writer and a load decodes through a buffered reader, so no
copy of the serialized cache sits beside the statistics it carries: a save's
buffer would meet every retained statistic and every observer still gathering,
at the end of a gathering `parse` a second copy of all of them, and a load's
file bytes would set a `query`'s peak. **A save writes beside the cache and
renames over it**, so a process killed mid-save — the likelier the nearer it
runs to its limit — leaves the previous cache whole rather than a truncated one
that starts the next `parse` cold. The reason is
[`../status/history/2026-09-15.md`](../status/history/2026-09-15.md), "Neither
end of the cache file holds it whole".

*Rejected:* a fixed statistics share ahead of the workers, which starves
parallelism on dumps that gather little; a split estimated from the preamble's
column counts, which know nothing of widths; the reserve covering the window's
terms, safe only on data that never needed it; a decline predicting the save's
buffer from the retained statistics, no bounded ratio relating a statistic's
heap to its encoding; the buffer charged once allocated, too late for a
decline to act on; the reserve covering a load's file bytes, a term growing
with statistics; a streamed save straight onto the cache path, widening the
window in which a kill costs the resume point from the write to the encode.

### The reserve rises uniformly, and is measured

**One `MEMORY_RESERVE` for every operation.** Gathering raises resident most,
but a query decodes the cache whole and consults statistics too, and a person
does not write one container definition for `parse` and another for `query`;
one reserve across every operation is the one they can understand.
*Rejected:* a reserve raised only when the request gathers.

**Its size is measured, in 20.8, not judged.** Slicing first set it to
512 MiB as a judgement "confirmed" by the re-taken `reserve` figure, and that
confirmation was vacuous: the figure's criterion faults only a reserve too
small, 384 MiB already met it, and its legs state `--statistics none` and
never query, so they never exercise what the bump is for. It stays 384 MiB
until 20.8 reads it, so no figure's arrangement moves twice and the sitting is
not biased toward a number already shipped.

**Attribution first, one blind gate after** ([`roadmap.md`](roadmap.md),
"Attribution is introspective; only the gate is blind"). The `introspect`
build of the shipped source reads, per run, the remainder *peak resident −
(the charge + the statistics account)*, split into live heap, allocator
retention and the rest. The legs, each pair in one container:

- a flagless gathering `parse`, and
- a flagless `query` over the cache that `parse` just wrote,

over the reasonable-width input and the wide-text input that fills its
allowance, on plain, 24 MiB-block and 128 MiB-block `.xz`, across the
`reserve` figure's limits.

**And one leg for the band where statistics starve**: an allowance at or above
`5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` at a `--jobs` high enough that
`Parallelism::fit` solves the count against the margin ceiling rather than the
cap, so `statistics_allowance` is left only the slack one worker's step gives.
None of the legs above reaches it, and it is the arrangement any remedy would
be priced against — the reading is what turns that remedy from an argument
into a priced choice.

**The criterion, registered here before the
sitting**: the reserve is the smallest step covering the worst remainder such
that the worst rep leaves `MEMORY_MARGIN_PERCENT`; then one blind sitting on
the shipped build at that value passes or fails it.

**The branch, registered before the reading too: a remainder that grows with
the statistics volume is billed, not reserved.** A query's decode of the whole
cache is the likely one, a parse keeping up to its whole allowance. If it
grows, a query bills the loaded cache's statistics against its allowance
before resolving its worker count — fewer workers rather than a kill, which is
what lets it keep "A query fits the allocation its cache was written under" —
and the reserve covers only what stays flat. `MEMORY_UNPOOLED_BOUND` is
re-derived the same way if the remainder passes it.

*Rejected:* a blind grid of candidate builds as `19.16` took, which answers
which number passes and not what the remainder is; a provisional 512 MiB in
20.6, moving the arrangements twice; a reserve sized to the worst case at the
reference allocation, a flat number already known wrong at other sizes;
choosing the branch after the reading.

### 512m stays the reference allocation

**Whatever 20.8 measures, 512m stays the reference**: if the reserve reaches
512 MiB it is the smallest allocation that works, leaving no budget, so a
compressed run there streams rather than block-decodes while a plain `parse`
— serial anyway — loses nothing. The koji
runs, `PGDQ_MEASURE_MEMORY` and the resident expectations are read there, and
every figure whose resolved arrangement moves with the reserve is re-taken.

*Rejected:* moving the reference to 640m or 1g, which keeps existing figures'
arrangements by hiding what a bump costs.

### A query fits the allocation its cache was written under

A query decodes the cache whole (D78) and a `parse` bounds its statistics by
its own allowance, so a query in the same container fits — the reserve
covering the decode, or the query billing it where 20.8 finds it grows with
the statistics. **That is a property, written beside the cache load**: the remedy a
person already has for a smaller allocation is more memory or `--dqcache
none`.

*Rejected:* a cache layout whose statistics a query can skip under a small
allowance, a change for a case outside the same-container premise; a `(c)`
deficiency, the remedy existing today.

### The two-tunable rule's check

**A CLI test lists every flag taking a byte count or a count against an
allowlist** classifying each as hardware (`--jobs`, `--memory`), intent
(`--statistics`, `--statistics-group-size`, `--statistics-min-rows`,
`--statistics-max-rows`), input contract
(`--max-line-bytes`) or expert override (`--chunk-size`), and fails on a flag
nobody classified. `--chunk-size` stays; retiring it would be out-of-band work.

*Rejected:* a review-time rule only; a test that every command runs at the
reference allocation, which is this phase's gate rather than the rule's check;
retiring every other hardware-fit flag here, which takes I/O flags out in a
statistics phase.

## The readings

Each names its instrument ([`roadmap.md`](roadmap.md), "A slice row that
commits to a measurement names its instrument"); the attribution is
introspective and only the gates are blind ("Attribution is introspective;
only the gate is blind").

- **The account reconciled against live heap** (P20.1) — the process prints its
  statistics account, and the `introspect` build's counting allocator gives
  live bytes over generated reasonable-width and wide-text shapes; the account
  never falls short of live statistics bytes by more than a tolerance the slice
  states. It is what shows the account right before a gate would find it wrong
  by being killed.
- **Koji's row density** (P20.2) — a gathering `parse` of koji on the shipped
  build tracking one narrow column per table, so every block's groups are
  counted at `2^20` for a fraction of full gathering's memory, launched
  detached; a script in `scripts/`, with its tests, reads each block's rows per
  group out of `info --json` and derives its distribution at every `2^n`,
  written to a `runs/` artifact the slice notes cite — a fact about the input,
  not a `measurements.md` figure. The fixtures and generated inputs are run
  through the same script as a check of the rule on known shapes. It applies
  "Granularity follows row density"'s registered criterion. Koji is the only
  real dump this rests on.
- **The reserve's remainder, attributed, then gated** (P20.8) — the
  `introspect` build over gathering `parse` and `query` legs, and one blind
  sitting at the value it gives; instrument, legs and criterion are "The
  reserve rises uniformly, and is measured".
- **A generated wide-text input that must decline** (P20.9) — flagless `parse`
  in 512m on the shipped build: exits clean, reports the decline, stays under
  the limit; a reasonable-width companion declines nothing.
- **Koji, flagless gathering `parse` in 512m, shipped build, detached** (P20.10)
  — the length axis on real data: exits clean, neither `task` nor
  `buildroot_listing` declines, peak resident leaves the margin. The existing
  recipe stays `--statistics none`, a scan regression check whose timings stay
  comparable, and the gathering run sits beside it.
- **What the koji runs give for free** (P20.10) — the gathering run's wall clock
  against the `none` run's is gathering's device-bound cost, and one `query`
  over its cache is the decode at a real group count.

**Existing figures keep `--statistics none`**, still measuring scanning.
`statistics-gathering` moves from its 2g stopgap into the 512m reference and is
re-taken; every figure whose arrangement moves with the measured reserve is
re-taken (P20.9).

*Rejected:* the gates alone, where an under-counting account is found only by a
kill; koji alone, narrow and never declining; figures gathering by default,
which re-times every scan figure under a cost that has its own figure.

## The slices, and why this order

The checklist is `STATUS.md`'s. Each slice makes the next one's mistakes
visible, the instrument first:

1. **The account** is built and reconciled before anything declines against
   it.
2. **Koji's row density**, read before any granularity code, so the quantile's
   criterion meets real data before the rule is written; no build may run
   beside it.
3. **The length cap**, the exact pairwise merge the density rule reuses, lands
   before koji, which cannot fit without it.
4. **The density minimum** on top of the merge — the decision at a block's
   end, the switches and the record of bounds — kept apart from 3, which
   reworks the gatherer's core.
5. **The stated maximum**, the one path that gathers below `2^20`, passes the
   cap and re-reads a block, through the back-fill that already re-reads.
6. **`--memory`** changes resolution before the decline carves from it; the
   reserve's value does not move here.
7. **The decline** reworks the gatherer's core path; kept apart from 6, which
   reworks the resolver's, so neither review accepts the other's confidence.
8. **The reserve, measured** — only once statistics are billed and declined
   is the remainder the reserve covers the one it will cover.
9. **The generated gates and re-taken figures**, at the measured reserve and
   the granularity that ships, so their inputs are chosen against the default a
   person gets.
10. **Koji**, launched detached and ticked when a later session reads it; a
   cap it refutes earns a `P20.10.1`.

**20.11, the streamed save and load**, was admitted after 20.1 landed and runs
next: it removes two terms rather than adding a charge, and the decline (7) and
the reserve's attribution (8) must meet an account with nothing left outside
it.

## Facts it rests on

**What statistics hold resident** (code read 2026-09-15):

- **Closed groups stay resident until exit**: each block's statistics are an
  `Arc<BlockStatistics>` on its `CopyBlock`, kept in the index the mapping pass
  returns; nothing is spilled, dropped after a save, or streamed.
- **Per group, per column, closed**, roughly 128–152 B for a short-bounded
  column with no dictionary; a dictionary adds an index per group and each
  distinct text once per block. The struct-cap worst case is about 20 KiB, but
  stored text is copied from the group's own values, so the variable part
  cannot exceed about three times the group's bytes. `statistics-gathering`'s
  resident growth divides to about 210 B a group a column on its control.
- **Every save encodes the whole cache into one buffer** and a load decodes it
  whole (D78).
- **Gathering vectors are not shrunk** at a block's finish, so a block gathered
  in this run can carry up to twice its capacity.
- **Open state per column per observer** is bounded by `CLIP_BYTES` and
  `DICTIONARY_CAP`; a parallel partition also holds its first group open.
- **A block's dictionary is held twice while it is gathered**, as entries and
  as the interning map's keys.
- **A parallel window's partitions each gather into their own observer until
  the whole window folds**, partitions past the block's terminator included
  (`KD22`).
- **`STORED_VALUE_CAP`, `DICTIONARY_CAP` and `CLIP_BYTES` are constants**;
  nothing capped statistics' total.
- **`MEMORY_RESERVE` is subtracted from every discovered limit**, gathering or
  not, and raising it is what declines the block path.
- **Koji had never been parsed with statistics**: its recipe states
  `--statistics none`.
- **Groups are dense**: a block lists every group from the first to the one its
  last row starts in, so it holds `⌈bytes / N⌉` of them whatever its rows'
  width. A row longer than `N` leaves groups no row starts in, each still
  carrying a null count, a bounds slot and a dictionary slot per tracked
  column, and pruning nothing (`statistics.rs`'s module rustdoc,
  `Gatherer::close_through`).
- **The group size is already per block in the cache** (`BlockStatistics`), and
  pruning and `info` read it per block.
- **Each closed group already records its rows** (`RowGroup::rows`), so the
  rows per group at `2N` are the sums of adjacent pairs at `N`.

**Koji's row density** (the same log's row counts over its block sizes):
`public.task` averages about 2.6 KB a row, some 400 rows per MiB, and
`public.buildroot_listing` about 20 B, some 54,000 per MiB. At a cap of 4,096
groups a block, `task`'s groups are already 128 MiB, so on the two blocks that
are 96% of koji's pairs the cap outweighs any plausible density rule.

**What a default gathering `parse` of koji would hold today** (estimated
2026-09-15 from `runs/pgdq-koji-20260905-0054/final-info.log`'s 74 `COPY`
blocks, column counts checked against the replica's `pg_attribute`; every koji
cache in `runs/` is unreadable by the current build, `KD30`):

- **747,738 groups and 8,220,944 group × column pairs** at the 1 MiB default,
  a byte-weighted mean of 10.99 columns — about 1.0 GiB at 128 B a pair,
  1.6 GiB at 210 B, 3.8 GiB at 500 B, statistics alone.
- **Two blocks are 96% of it**: `public.task`, 354 GiB of 19 columns (84% of
  pairs, 362,924 groups in one block), and `public.buildroot_listing`,
  331 GiB of 3 columns (12%).
- **Koji is narrow**: no table has more than 26 columns. Its volume is the
  length axis.

## Inbox, drained

- *"A parallel gathering scan holds statistics per partition as well as per
  block"* — folded: the account sees the window's terms ("Workers are resolved
  first").
- *"Three statistics costs no figure takes"* — discarded: the `--json` export
  and the eager plan grow with groups or blocks, which the length cap bounds,
  and the back-fill's second read is time informing no decision here.
- *"What the statistics figures leave unpriced"* — folded in part: the koji
  runs price gathering device-bound and the decode at a real group count; the
  rest informs no decision this phase makes.

## Not this phase

- Removing statistics' transient terms (folding partitions as they return,
  interning a dictionary once) — an optimization the gate may motivate.
- Retiring `--chunk-size` or any other expert override.
- A query that skips a cache's statistics under a smaller allocation than
  wrote it.
- Statistics gathered by a query (P21).
- Sizing a table of fixed-width columns from its header alone — [`roadmap.md`](roadmap.md),
  "Future — wanted, unscheduled".
