# P22 inbox — facts filed for its grilling

Evidence that P22 (the third tunable) will need. **This is a queue, not a
document**: when P22 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## The starve band is priced, and it kills

**Fact.** The 2026-09-16 attribution sitting ran the two starve-band legs this
phase's sketch asked 20.8 to price — an allowance at or above
`5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` at a `--jobs` high enough that
`Parallelism::fit` solves the count against the margin ceiling. On the
`introspect` build, `control-xz24-starve-parse` was OOM-killed in **all five
reps**, and `wide-xz24-starve-parse` survived with a remainder of 392 MiB worst
and **6.6% of the limit left**. The slack one worker's step leaves is not
routinely large; in this arrangement it is the whole problem.

**Why P22 cares.** The sketch says a reading showing the slack is routinely
large "shrinks this phase or ends it". The reading says the opposite, so the
phase is not shrunk by it — and the starving case is now a measured
arrangement rather than an argument, which is what the sketch wanted before
grilling.

**Origin.** 2026-09-16, the statistics-memory wrap; readings in the sitting named by
[`../status/history/2026-09-16.md`](../status/history/2026-09-16.md).
*Contingent on* the `introspect` build's overhead not being what killed the
control leg — an attribution build takes an atomic per allocation, so the kill
is a censored reading, while the 6.6% headroom on the surviving leg is not.

---

## A query bills its loaded statistics rather than reserving them

**Fact.** *A remainder that grows with the statistics volume is billed, not
reserved*: `pgdt query`'s mapping pass bills a loaded cache's statistics
against its allowance before resolving its worker count, so it resolves fewer
workers rather than being killed, and its replay, which keeps none, is billed
nothing ([`decisions.md`](decisions.md), "D85"). It was taken before the
constant it is priced against, which this phase may replace. Its instrument is
kept: `instrument::statistics_loaded`, the heap a cache load hands a pass,
which is the only statistics term a query has.

**Why P22 cares.** The billing is a fourth consumer resolving against the same
number, which is this phase's subject. Whether a query bills or reserves is
not separable from what the third tunable means.

**Origin.** 2026-09-16, the statistics-memory wrap, which registered the
branch; taken on 2026-09-23. [`roadmap.md`](roadmap.md), "P23 — Statistics
coverage and the resident reserve" holds the same fact and `KD34` the measured
gap.

---

## The plain-source inertness now has a user-facing consumer

**Fact.** `PlanNoteKind::ParallelismBudgetLimited` no longer offers "raise the
memory budget to get more" alone. Both arms name the **announced read chunk**
(`crate::scan::ScanOptions::chunk_size_bytes`) as the lever a plain source
actually has — a source cutting by it sizes both terms of the charge from it —
and both say in the same sentence that either lever buys seats rather than
speed, and that a plain source's sub-streams may not run concurrently at all.
That qualification is `KD17`: `measurements.md`'s `parallel-scan-throughput`
has the plain typed `query` column flat across the whole `--jobs` axis.
[`../manual/dump-inspection.md`](../manual/dump-inspection.md),
"`--chunk-size`: you almost certainly do not need it" says the same to a user.

**Why P22 cares.** The sketch's last open decision is whether the plain-source
inertness `KD32` names is fixed by the same change or stays, and that is no
longer an internal question. **If the phase makes a stated allowance reach a
plain source**, both surfaces above stop being true and move in that change —
the remedy becomes the budget again and the manual paragraph goes. **If it
leaves the inertness standing**, the phase is choosing to keep a diagnostic and
a manual section pointing a user at `--chunk-size`, whose gain on this path
nothing has measured; what such a reading would need is recorded at `KD17`'s
marker in `pgdump_query/src/stream.rs`. Either way the wording is part of the
decision's cost rather than a follow-up to it.

**Origin.** 2026-09-19, out-of-band `M119` and the review that closed the entry
it raised under STATUS's "Decisions worth another look";
[`../status/history/2026-09-19.md`](../status/history/2026-09-19.md).
*Contingent on* `KD17` standing: if what serializes a plain source's
sub-streams is identified and fixed, seats do become throughput, the
qualification comes off both surfaces, and the chunk is a remedy rather than a
trade.
