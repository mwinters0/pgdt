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
[`postgres-invariants.md`](postgres-invariants.md), and `decisions.md`'s
"D33".

**Contingent on.** The `--disable-triggers` deficiency `KD1` staying open
(`../status/deficiencies.md`, "Known deficiencies"). If the unscheduled fix
for it (`roadmap.md`, "Future") lands first, both shapes collapse back to one
and only the `data_offset` question remains.

---

## The statement-end scan Track A needs already exists, and it is the scanner's lexer

**Fact.** `preamble::StatementScan` is an incremental scan of a statement's
lines — paren depth, where `lex::Lexer` leaves it (inside a literal, a quoted
identifier, a comment or none), whether its last line ends in a `--` comment,
and the last non-whitespace byte — fed by `feed_line(&[u8])` and queried by
`complete()`/`in_quote()`. The lexer is the one `scan::CopyScanner` lexes every
line outside a block with, psql's rules under the dump's own
`standard_conforming_strings` (I50), so the run's end and the scanner's
regions cannot disagree. It is line-at-a-time: no region boundary straddles a
newline, and every caller holds lines. `map::Builder`'s `INSERT` run drives it
with no `String` per line and no statement buffer at all, and
`statement_complete`/`in_open_quote` are wrappers over it, so there is one
implementation of the rule.

**Why P8 cares.** Track A's row reader has to find where each
`INSERT INTO … VALUES (…);` statement ends before it can split the value list,
and that is exactly this scan. It should be extended rather than re-written —
a second quote tracker beside this one is a second reading of psql's lexer.
What the reader will want that the map does not is a *position*: the scan
updates state and reports whether the run so far is complete, but does not
return the offset of the terminating `;`. Adding that is a method on an
existing type, and the lines it walks are the ones the scanner surfaces.
A `$$` inside an `INSERT` row is proved over a hand-built dump only
(`pgdump_query/tests/map.rs`, `an_insert_run_holding_a_dollar_pair_keeps_every_row`):
the objects fixture holds `$$` in a comment, a default and a quoted name, and
an `--inserts` row holding one waits for this reader.

**This phase also owns `KD9`'s three untaken cuts**, all in the scan above,
because slicing P8 is when they acquire a slice and `scripts/deficiencies.py`
holds the pairing. One is that `map::Builder::insert_run_line` feeds the
`INSERT INTO <table>` prefix it has just matched back into `feed_line`, so
those bytes are crossed twice; another is that `feed_line` counts parens in a
`memchr2` pass per run of code, which no `INSERT` statement's end needs; the
third is that the scanner has already lexed every line it surfaces, so the
line's region, comment and paren count could travel with its `Event::Line`
instead of being found again. The second is why the reader's requirements have
to come first: skipping it is a mode flag, and whether the depth count is dead
weight is a question only a caller splitting a `VALUES` tuple can answer. None
is measured — a profile sizes them, and `decisions.md`'s "D33" carries the
reading that made the first two worth keeping.

**Also worth knowing at spec time.** The trailing-`;` test uses ASCII
whitespace where `str::trim_end` used Unicode, and the map's own `feed_line`
joins lines with `\n` *between* them and never before the first — both are
documented on the type and both are load-bearing for a caller that reuses it.

**Origin.** The `INSERT` fast path, 2026-09-03
([`../status/history/2026-09-03.md`](../status/history/2026-09-03.md)), and
the lexer change of 2026-10-01
([`../status/history/2026-10-01.md`](../status/history/2026-10-01.md)),
which put it on the scanner's lexer. The mechanism is
[`decisions.md`](decisions.md), "D33".

---

## A refused field fails a parse at a `COPY` block's close

**Fact.** A field its type's `*_in` refuses at a `pg-refuses` check fails a
data-level parse keying it (31.12): the statistics observer answers
`BlockGathered::Refused`, and `map::Builder::on_copy_end` raises
`Error::FieldRefused`, as a restore under `ON_ERROR_STOP` fails the table's
`COPY`. Nothing raises it for an `INSERT` run, whose rows nothing reads yet.

**Why P8 cares.** Track A reads `INSERT` rows, and a restore of an
`--inserts` dump fails per statement, not per table: without `ON_ERROR_STOP`
it loses the one row and goes on. Whether the parse fails at a refused
`INSERT` value as at a `COPY` field, where the raise sits when a run has
no `on_copy_end`, and what it names the statement by — `FieldRefused`'s
`line` is `COPY`'s count of a block's rows (31.12.2), which a run of
statements has no counterpart of — is that track's to decide.

**Origin.** 31.12, 2026-10-03:
[`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md).
Contingent on the raise staying at the block's close.
