# P21 inbox — facts filed for its grilling

Evidence that P21 (statistics gathered by a query) will need. **This is a queue,
not a document**: when P21 is grilled, walk every entry, fold it into the spec
or discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## A query's replay is carved with no statistics held

**Fact.** `pgdt query` carves its two passes apart
(`pgdt/src/main.rs`, `Discovered::resolve_query`): the mapping pass is billed a
loaded cache's statistics as `held`, the replay nothing. That is true only
because the replay keeps none — `Segment::new` in `pgdump_query/src/stream.rs`
drops a block's statistics from each segment, and the matched blocks are
dropped before a row is read.

**Why P21 cares.** A gather during the replay that keeps a matched block's
statistics, or merges into them, breaks that premise silently: the replay
would hold what its carving billed as nothing, under-billing with nothing
failing. Such a gather has to bill what it keeps in the replay's own carving.
Keeping them is local — the `Arc` each `CopyBlock` already carries — so the
premise costs P21 nothing to change, only something to remember.

**Origin.** `M135`, 2026-09-23;
[`../status/history/2026-09-23.md`](../status/history/2026-09-23.md),
"`pgdt query`'s two passes are carved apart". *Contingent on* `Segment` still
dropping a block's statistics.
