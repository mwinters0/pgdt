# P39 inbox — facts filed for its grilling

Evidence found in earlier phases that P39 (PostgreSQL as a front end) will
need. **This is a queue, not a document**: when P39 is grilled, walk every
entry, fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## The query surface yields only Arrow batches

**Fact.** A table's stream, `TableStream`, yields `RecordBatch`es and nothing
below them; arrays are the query layer's, which ties their building to the
read buffers ([`decisions.md`](decisions.md), "D46", "D74").

**Why P39 cares.** A field's text handed to the type's `*_in` is exact for
every type, including domains, enums, extension types and the built-ins with
no reader here; a Datum built from a batch is a copy of every value not
fixed-width. Which path the extension takes, or which mix of the two, decides
whether the library grows a surface of unescaped fields beside its batches,
and where that surface lives.

**Origin.** Filed 2026-11-03, sketching P39; contingent on
`crates/pgdt-query/src/stream/table.rs`.

---

## A column under a collation this build does not implement is compared bytewise

**Fact.** `KD7`: under `ComparisonSemantics::Postgres`, a column stating a
collation other than those whose order is `memcmp`'s is compared bytewise, so
`<`/`>` select a row set the server would not, and `=`/`!=` do too under a
nondeterministic collation (I42, I43). `pgdt query` and the DataFusion
provider report it as a divergence ([`../status/deficiencies.md`](../status/deficiencies.md),
"Known deficiencies"; `ComparisonDivergence::NonBytewiseCollation`).

**Why P39 cares.** A foreign scan's server re-checks the quals it keeps local
on every row returned, but never sees a row in a block the library pruned.
Under the server's own comparison a divergence is a wrong answer, not an
advisory, so the extension decides which quals it hands the library to prune
on — and a collation the library cannot match is one it does not.

**Origin.** Filed 2026-11-03, sketching P39; contingent on `KD7` and on
`crates/pgdt-codec/src/compare.rs`.

---

## A PostgreSQL 18 dump can carry the source server's own planner statistics

**Fact.** `pg_dump --statistics` (PG 18+) writes one `STATISTICS DATA` entry
per relation, `pg_restore_relation_stats(...)` and
`pg_restore_attribute_stats(...)` calls, which the map attributes as one span
each ([`pg-dump-compatibility.md`](pg-dump-compatibility.md); I18 in
[`postgres-invariants.md`](postgres-invariants.md)). Nothing reads their
arguments.

**Why P39 cares.** The server's planner reads column statistics from
`pg_statistic`, never from a foreign scan, and the same two functions write
them: null fraction, width and distinct count, which the map has, and the most
common values and histogram bounds, which it does not. Where the dump carries
the source's own calls, replaying them onto the foreign tables hands the
planner the source's statistics; elsewhere the extension decides what of the
map's it writes there, and through which.

**Origin.** Filed 2026-11-03, sketching P39; contingent on the
`STATISTICS DATA` fixture `fixtures/18/objects/stats.sql`.

---

## The library discovers its memory from the process's cgroup

**Fact.** `pgdt_io::Arrangement::discover` and
`pgdt_io::discover_memory_limit` read the calling process's cgroup limit, and
an embedder may state its allowance instead, as `datafusion-pgdump`'s
`ScanBudget` holds one per session ([`roadmap.md`](roadmap.md), "Two tunables
fit pgdt to hardware: memory and parallelism").

**Why P39 cares.** In a backend that limit is the whole server's, shared by
every connection, so discovery there answers a question nobody asked. The
extension states an allowance, from a setting of its own or the server's
`work_mem`, and decides whether one bounds a backend or a query.

**Origin.** Filed 2026-11-03, sketching P39; contingent on
`crates/pgdt-io/src/memory.rs` and `crates/pgdt-io/src/arrangement.rs`.
