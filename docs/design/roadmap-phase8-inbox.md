# Phase 8 inbox — facts filed for its grilling

Evidence found in earlier phases that Phase 8 (format coverage beyond plain
COPY TEXT) will need. **This is a queue, not a document**: when Phase 8 is
grilled, walk every entry, fold it into the spec or discard it as stale, and
delete this file. See `docs/process.md`, "Inboxes: facts filed by
destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## The partition-root marker lives in the TOC entry, not in the SQL

**Fact.** I2: when load-via-partition-root applies, a `TABLE DATA` entry's
`COPY` header names the partition's **root** table, so one header name owns
several blocks in a single dump — and `pg_dump` forces the mode by itself for
a table hash-partitioned on an enum column, so a flagless dump produces it.
Plain-format output is disambiguated by a `-- load via partition root <root>`
line, which `dumpTableData()` writes into the entry's **`defn`** field;
`_printTocEntry()` is what renders `defn` as a comment in plain format.

**Why Phase 8 cares.** Track B reads the archive container (custom, directory,
tar) rather than plain text. The marker is not part of the SQL — it is
archive metadata that plain format happens to render as a comment — so a
container reader has to get it from the TOC entry directly, and `crate::map`'s
lexical `partition_root_marker()` will find nothing there. Whether the entry's
`defn` is even preserved in the archive TOC the same way is **unverified**;
that is a source-reading job for Track B, not an assumption to inherit. Track
A (`--inserts`) is unaffected in shape — the marker survives `--inserts` in
plain format (observed) — but the same multi-block-under-one-name problem
applies to `INSERT` runs, and `stream::target_settled`'s stop rule reads
`CopyBlock::partition_root`, which nothing populates for a non-`COPY` region
yet.

**Origin.** Slice 3.2.1.2.1, 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md) — which carries the
reproduction and the exact source functions — and the
`--load-via-partition-root` row in
[`pg-dump-compatibility.md`](pg-dump-compatibility.md).

**Contingent on.** Nothing in `pgdq`; this is upstream behaviour with a
re-verification step recorded on I2 itself.

---

## Entry order is the archive's order, and it is not grouped by table

**Fact.** `DOTypeNameCompare()` in `pg_dump_sort.c` orders entries by (type
priority, namespace name, **object name**), and a `TABLE DATA` entry's object
name is the *partition's* own name, not the root's. So several blocks sharing
one `COPY` header name are **not** adjacent — an unrelated table whose name
sorts between two partition names is emitted between their blocks. Observed,
flagless, 16.15.

**Why Phase 8 cares.** This is a property of the *archive's entry ordering*,
not of plain-text rendering, so it carries over to every container format
Track B adds. Any Track B design that enumerates a table's data by walking
entries until the name changes is wrong for the same reason the plain-format
version was — and a container's TOC makes "just index every entry up front"
cheap, which is the better answer there and worth reaching for deliberately
rather than rediscovering the trap.

**Origin.** Slice 3.2.1.2.1, 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md).

---

## An `INSERT` run's span has two shapes, and neither records where the rows start

**Fact.** As of M7, `map::Builder`'s `Mode::Comment` close arm opens
`Mode::InsertRun` at the *comment's* offset when a `-- Data for Name: …; Type:
TABLE DATA` block heads the run, so a TOC-commented run is one `Data` span
covering comment and rows alike — the same absorption `on_copy_start` has
always done for a `COPY` block. **But that absorption needs the comment to be
adjacent to the data, and `--disable-triggers` breaks the adjacency** (I31): it
writes `ALTER TABLE … DISABLE TRIGGER ALL;`, and before the first entry a `SET
SESSION AUTHORIZATION DEFAULT;`, in between. There the run instead starts
exactly at its first `INSERT INTO` and carries `toc: None`. Either way
`DataBlock::InsertRun` carries no inner offsets — no counterpart to
`CopyBlock`'s `header_offset`/`data_offset` — because nothing reads its rows
yet.

**Why Phase 8 cares.** Track A reads `--inserts` rows, and it faces **two**
span shapes for one construct: `span.start` is at the first statement, or an
arbitrary number of comment lines before it (more under `--verbose`). The only
invariant that holds across both is `span.start <= ` the first `INSERT INTO`,
which is not enough to seek on. So the choice is: scan forward from
`span.start` for the first statement, or give `InsertRun` a `data_offset` the
mapping pass records the way `CopyBlock` does. The second shape is what
settles it — a scan-forward reader has to tolerate an intervening `ALTER TABLE`
statement, not just comment lines, which is re-deriving the mapping pass's own
work at read time. `data_offset` is a cache format change, so it wants deciding
at spec time rather than mid-slice. Note also that a zero-row table contributes
**no** span at all: its comment block runs into the next entry's and only the
later entry survives (`public.empty_table` in `edge_cases/inserts.sql`).

**Origin.** M7, 2026-08-27; the second shape found grilling M7 the same day.
See [`../status/history/2026-08-27.md`](../status/history/2026-08-27.md), "M7
attributes an `--inserts` dump's rows" and "Grilling M7", I31 in
[`postgres-invariants.md`](postgres-invariants.md), and `architecture.md`'s
"Bulk regions: one span kind, three payloads".

**Contingent on.** The `--disable-triggers` known gap staying open
(`../status/STATUS.md`). If the unscheduled fix for it (`roadmap.md`, "Future")
lands first, both shapes collapse back to one and only the `data_offset`
question remains.
