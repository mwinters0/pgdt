# P20 inbox — facts filed for its grilling

Evidence that P20 (statistics memory) will need. **This is a queue, not a
document**: when P20 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## A parallel gathering scan holds statistics per partition as well as per block

**Fact.** Each partition of a leader window gathers into its own observer until
the window is folded (`leader::run_region`): its closed groups with a
piece-local dictionary, its first group held open with every distinct text
un-interned, and each column's first value. So the statistics resident during a
gathering `parse` grow with the worker count and the partition size, not only
with the dump — at worst a window's worth of groups over again. A window's
partitions past the block's terminator gather too before they are discarded,
the over-read `KD22` names.

**Why P20 cares.** Bounding what statistics hold resident has to bound this
term, and a charge for it would be per worker, where the block's own
statistics are per dump; the reserve P20 re-derives is read at whatever count
its readings state.

**Origin.** P10.6, 2026-09-14
([`roadmap-P10.6-parallel-gathering-notes.md`](roadmap-P10.6-parallel-gathering-notes.md)).
Contingent on the leader folding a window's partitions only once every
partition of it has returned.
