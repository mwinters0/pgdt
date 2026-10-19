# P21 inbox — facts filed for its grilling

Evidence that P21 (statistics gathered by a query) will need. **This is a queue,
not a document**: when P21 is grilled, walk every entry, fold it into the spec
or discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## A query's replay is carved around the statistics it already holds

**Fact.** `pgdt query` carves each of its passes around what it holds
(`crates/pgdt/src/parallel.rs`, `Discovered::resolve_pass`): the mapping pass
around a loaded cache's statistics, and the replay, carved once that pass has
settled the table, around the settled table's — those its `Table` and its plan
keep by `Arc` (`TablePartitions::under`). Nothing the replay gathers is billed:
`Segment::new` in `crates/pgdt-query/src/stream/replay/segment.rs` drops a
block's statistics from each segment, and the replay gathers none.

**Why P21 cares.** A gather during the replay adds to what its carving billed,
which is fixed before a row is read: what it keeps or merges into a matched
block's statistics past that bill is held with nothing failing. Such a gather
has to bill what it adds in the replay's own carving.

**Origin.** 2026-09-23 ([`decisions.md`](decisions.md), "D85");
[`../status/history/2026-10-19.md`](../status/history/2026-10-19.md),
"`pgdt query` bills the statistics its replay holds". *Contingent on* the
replay gathering nothing.

## A query already maps a metadata-level table, and gathers nothing

**Fact.** A `parse` may leave a table at the *metadata* level —
location and row count, no census, no statistics — and a query over it maps
that table's blocks at the data level for itself, census and count only,
holding them for the query and writing nothing to the cache
([`decisions.md`](decisions.md), "D35").

**Why P21 cares.** That pass reads every row of the table already, the case
where a query-time gather observes a column completely; whether it gathers,
and whether it may save what it gathered past "the library never replaces
cache data", is P21's call.

**Origin.** A grilling, 2026-09-29. *Contingent on* that cold-semantics
rule standing ([`decisions.md`](decisions.md), "D35").
