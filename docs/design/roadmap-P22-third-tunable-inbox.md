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

**Origin.** 2026-09-16, the P20 wrap; readings in the sitting named by
[`../status/history/2026-09-16.md`](../status/history/2026-09-16.md).
*Contingent on* the `introspect` build's overhead not being what killed the
control leg — an attribution build takes an atomic per allocation, so the kill
is a censored reading, while the 6.6% headroom on the surviving leg is not.

---

## Billing a query's loaded statistics is a live option, not a settled one

**Fact.** P20 registered a branch before its sitting: *a remainder that grows
with the statistics volume is billed, not reserved* — a query billing its
loaded cache statistics against its allowance before resolving its worker
count, so it resolves fewer workers rather than being killed. P20 wrapped
without taking that branch, because the constant it would have been priced
against is one this phase may replace. The instrument the branch needs exists
and is kept: `instrument::statistics_loaded`, the heap a cache load hands a
pass, which is the only statistics term a query has.

**Why P22 cares.** The branch is a fourth consumer resolving against the same
number, which is this phase's subject. Whether a query bills or reserves is
not separable from what the third tunable means.

**Origin.** 2026-09-16, the P20 wrap; [`roadmap.md`](roadmap.md), "P23 —
Statistics coverage and the resident reserve" holds the unspent intent and
`KD34` the measured gap.
