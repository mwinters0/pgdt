# P8 inbox — facts filed for its grilling

Evidence found in earlier phases that P8 (format coverage beyond plain
COPY TEXT) will need. **This is a queue, not a document**: when P8 is
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

**Why P8 cares.** Track B reads the archive container (custom, directory,
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

**Origin.** 2026-08-24. See I2 in
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

**Why P8 cares.** This is a property of the *archive's entry ordering*,
not of plain-text rendering, so it carries over to every container format
Track B adds. Any Track B design that enumerates a table's data by walking
entries until the name changes is wrong for the same reason the plain-format
version was — and a container's TOC makes "just index every entry up front"
cheap, which is the better answer there and worth reaching for deliberately
rather than rediscovering the trap.

**Origin.** 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md).

---

## An `INSERT` run's span has two shapes, and neither records where the rows start

**Fact.** `map::Builder`'s `Mode::Comment` close arm opens
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

**Why P8 cares.** Track A reads `--inserts` rows, and it faces **two**
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

**Contingent on.** The `--disable-triggers` deficiency `KD1` staying open
(`../status/STATUS.md`, "Known deficiencies"). If the unscheduled fix for it
(`roadmap.md`, "Future") lands first, both shapes collapse back to one and only
the `data_offset` question remains.

---

## The statement-end scan Track A needs already exists, and it is chunk-safe

**Fact.** `preamble::StatementScan` is an incremental, byte-level, quote-aware
scan of SQL statement text — paren depth, in-string (`''` doubling),
in-quoted-identifier (`""`), in-`--`-comment, and the last non-whitespace byte
— fed by `feed(&[u8])` and queried by `complete()`/`in_quote()`. It carries a
`Pending` across calls, so a `''`, `""` or `--` pair split between two `feed`s
is read correctly: a caller may hand it a file's chunks rather than its lines
and get the same answer. `map::Builder`'s `INSERT` run drives it with no
`String` per line and no statement buffer at all, and
`statement_complete`/`in_open_quote` are wrappers over it, so there is one
implementation of the rule.

**Why P8 cares.** Track A's row reader has to find where each
`INSERT INTO … VALUES (…);` statement ends before it can split the value list,
and that is exactly this scan. It should be extended rather than re-written —
a second quote tracker beside this one is two places for
`standard_conforming_strings` to be assumed. What the reader will want that the
map does not is a *position*: the scan updates state and reports whether the
run so far is complete, but does not return the offset of the terminating `;`.
Adding that is a method on an existing type, and shaping it is a spec-time
question because it decides whether a reader walks lines (as the map does) or
chunks.

**This phase also owns `KD9`'s two untaken cuts**, both inside the scan above,
because slicing P8 is when they acquire a slice and `scripts/deficiencies.py`
holds the pairing. One is that `map::Builder::insert_run_line` feeds the
`INSERT INTO <table>` prefix it has just matched back into `feed_line`, so
those bytes are crossed twice; the other is that `feed` counts parens in a
second `memchr2` pass per plain run, which no `INSERT` statement's end needs.
The second is why the reader's requirements have to come first: skipping it is
a mode flag, and whether the depth count is dead weight is a question only a
caller splitting a `VALUES` tuple can answer. Neither is measured — a profile
sizes both, and `architecture.md`'s "Bulk regions" carries the reading that
made them worth keeping.

**Also worth knowing at spec time.** The trailing-`;` test uses ASCII
whitespace where `str::trim_end` used Unicode, and the map's own `feed_line`
joins lines with `\n` *between* them and never before the first — both are
documented on the type and both are load-bearing for a caller that reuses it.

**Origin.** P7.5, 2026-09-03. See
[`roadmap-P7.5-insert-fast-path-notes.md`](roadmap-P7.5-insert-fast-path-notes.md)
and `architecture.md`'s "Bulk regions: one span kind, three payloads".
