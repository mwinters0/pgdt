# P41 inbox — facts filed for its grilling

Evidence found in earlier phases that P41 (archive formats: the shared model,
and `--format=custom`) will need. **This is a queue, not a document**: when
P41 is grilled, walk every entry, fold it into the spec or discard it as stale,
and delete this file. See `docs/process.md`, "Inboxes: facts filed by
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

**Why P41 cares.** It reads the archive container (custom, and the model
directory and tar share) rather than plain text. The marker is not part of the
SQL — it is archive metadata that plain format happens to render as a comment —
so a container reader has to get it from the TOC entry directly, and
`crate::map`'s lexical `partition_root_marker()` will find nothing there.
Whether the entry's `defn` is even preserved in the archive TOC the same way is
**unverified**; that is a source-reading job for this phase, not an assumption
to inherit.

**Origin.** 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md) — which carries the
reproduction and the exact source functions — and the
`--load-via-partition-root` row in
[`pg-dump-compatibility.md`](pg-dump-compatibility.md).

**Contingent on.** Nothing in `pgdt`; this is upstream behaviour with a
re-verification step recorded on I2 itself.

---

## Entry order is the archive's order, and it is not grouped by table

**Fact.** `DOTypeNameCompare()` in `pg_dump_sort.c` orders entries by (type
priority, namespace name, **object name**), and a `TABLE DATA` entry's object
name is the *partition's* own name, not the root's. So several blocks sharing
one `COPY` header name are **not** adjacent — an unrelated table whose name
sorts between two partition names is emitted between their blocks. Observed,
flagless, 16.15.

**Why P41 cares.** This is a property of the *archive's entry ordering*,
not of plain-text rendering, so it carries over to every container format
the shared model reads. Any design that enumerates a table's data by walking
entries until the name changes is wrong for the same reason the plain-format
version was — and a container's TOC makes "just index every entry up front"
cheap, which is the better answer there and worth reaching for deliberately
rather than rediscovering the trap.

**Origin.** 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md).
