# P20 — Statistics memory and its measured expectations

**Current.** Grilled with the maintainer 2026-09-15; the slices are
[`../status/STATUS.md`](../status/STATUS.md), "P20 progress". This document
says what the phase does and why, never how it lands in code.

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

### Length is bounded by a constant, per block

**A block's statistics hold at most a fixed number of groups under an unstated
group size**; past it, adjacent groups merge pairwise, so resident grows with
blocks × columns rather than bytes × columns, boundaries stay byte-aligned and
parallel gathering stays identical to serial. **A stated
`--statistics-group-size` is honoured exactly**, a precision a person asked
for.

**The cap is a judgement, confirmed or moved by the koji gathering run**
(P20.6), argued from one criterion: koji's flagless gather fits 512m with the
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
groups, held-open head, piece-local dictionary — until the window folds, a
block's dictionary held twice while it is gathered, and the retained
statistics' peak at a save, whose encode buffer holds them again. A term the
account cannot see breaks "nothing is OOM-killed" on exactly the wide,
many-worker shape where the term is large. **The margin is left against
everything held, the account included, which closes `KD28`.** Removing terms —
folding partitions as they return, interning once — is left an optimization
the gate may motivate.

*Rejected:* a fixed statistics share ahead of the workers, which starves
parallelism on dumps that gather little; a split estimated from the preamble's
column counts, which know nothing of widths; the reserve covering the window's
terms, safe only on data that never needed it.

### The reserve rises uniformly, and is measured

**One `MEMORY_RESERVE` for every operation.** Gathering raises resident most,
but a query decodes the cache whole and consults statistics too, and a person
does not write one container definition for `parse` and another for `query`;
one reserve across every operation is the one they can understand.
*Rejected:* a reserve raised only when the request gathers.

**Its size is measured, in 20.7, not judged.** Slicing first set it to
512 MiB as a judgement "confirmed" by the re-taken `reserve` figure, and that
confirmation was vacuous: the figure's criterion faults only a reserve too
small, 384 MiB already met it, and its legs state `--statistics none` and
never query, so they never exercise what the bump is for. It stays 384 MiB
until 20.7 reads it, so no figure's arrangement moves twice and the sitting is
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
`reserve` figure's limits. **The criterion, registered here before the
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
20.3, moving the arrangements twice; a reserve sized to the worst case at the
reference allocation, a flat number already known wrong at other sizes;
choosing the branch after the reading.

### 512m stays the reference allocation

**Whatever 20.7 measures, 512m stays the reference**: if the reserve reaches
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
covering the decode, or the query billing it where 20.7 finds it grows with
the statistics. **That is a property, written beside the cache load**: the remedy a
person already has for a smaller allocation is more memory or `--dqcache
none`.

*Rejected:* a cache layout whose statistics a query can skip under a small
allowance, a change for a case outside the same-container premise; a `(c)`
deficiency, the remedy existing today.

### The two-tunable rule's check

**A CLI test lists every flag taking a byte count or a count against an
allowlist** classifying each as hardware (`--jobs`, `--memory`), intent
(`--statistics`, `--statistics-group-size`), input contract
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

- **The reserve's remainder, attributed, then gated** (P20.7) — the
  `introspect` build over gathering `parse` and `query` legs, and one blind
  sitting at the value it gives; instrument, legs and criterion are "The
  reserve rises uniformly, and is measured".
- **The account reconciled against live heap** (P20.1) — the process prints its
  statistics account, and the `introspect` build's counting allocator gives
  live bytes over generated reasonable-width and wide-text shapes; the account
  never falls short of live statistics bytes by more than a tolerance the slice
  states. It is what shows the account right before a gate would find it wrong
  by being killed.
- **A generated wide-text input that must decline** (P20.5) — flagless `parse`
  in 512m on the shipped build: exits clean, reports the decline, stays under
  the limit; a reasonable-width companion declines nothing.
- **Koji, flagless gathering `parse` in 512m, shipped build, detached** (P20.6)
  — the length axis on real data: exits clean, neither `task` nor
  `buildroot_listing` declines, peak resident leaves the margin. The existing
  recipe stays `--statistics none`, a scan regression check whose timings stay
  comparable, and the gathering run sits beside it.
- **What the koji runs give for free** (P20.6) — the gathering run's wall clock
  against the `none` run's is gathering's device-bound cost, and one `query`
  over its cache is the decode at a real group count.

**Existing figures keep `--statistics none`**, still measuring scanning.
`statistics-gathering` moves from its 2g stopgap into the 512m reference and is
re-taken; every figure whose arrangement moves with the measured reserve is
re-taken (P20.5).

*Rejected:* the gates alone, where an under-counting account is found only by a
kill; koji alone, narrow and never declining; figures gathering by default,
which re-times every scan figure under a cost that has its own figure.

## The slices, and why this order

The checklist is `STATUS.md`'s. Each slice makes the next one's mistakes
visible, the instrument first:

1. **The account** is built and reconciled before anything declines against
   it.
2. **The length cap** lands before koji, which cannot fit without it.
3. **`--memory`** changes resolution before the decline carves from it; the
   reserve's value does not move here.
4. **The decline** reworks the gatherer's core path; kept apart from 3, which
   reworks the resolver's, so neither review accepts the other's confidence.
7. **The reserve, measured** — only once statistics are billed and declined
   is the remainder the reserve covers the one it will cover. Numbered 20.7
   because it was admitted after slicing; it runs before 20.5.
5. **The generated gates and re-taken figures**, at the measured reserve.
6. **Koji**, launched detached and ticked when a later session reads it; a
   cap it refutes earns a `P20.6.1`.

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
