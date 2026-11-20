# P8 inbox — facts filed for its grilling

Evidence found in earlier phases that P8 (`--inserts` and `--column-inserts`
rows) will need. **This is a queue, not a document**: when P8 is
grilled, walk every entry, fold it into the spec or discard it as stale, and
delete this file. See `docs/process.md`, "Inboxes: facts filed by
destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## Under load-via-partition-root, an `INSERT` run's name is its root's

**Fact.** I2: when load-via-partition-root applies, a `TABLE DATA` entry's
data names the partition's **root** table, so one name owns several blocks in a
single dump, and `pg_dump` forces the mode by itself for a table
hash-partitioned on an enum column. The `-- load via partition root <root>`
line that disambiguates it survives `--inserts` in plain format (observed).

**Why P8 cares.** The same multi-block-under-one-name problem applies to
`INSERT` runs, and `pgdt_mapping`'s `target_settled` stop rule reads
`CopyBlock::partition_root`, which nothing populates for a non-`COPY` region
yet.

**Origin.** 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md); P41's inbox holds the
archive half, the marker's place in a TOC entry.

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

**Why P8 cares.** P8 reads `--inserts` rows, and it faces **two**
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

## The statement-end scan P8 needs already exists, and it is the scanner's lexer

**Fact.** `map::statement::StatementScan` is an incremental scan of a statement's
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

**Why P8 cares.** P8's row reader has to find where each
`INSERT INTO … VALUES (…);` statement ends before it can split the value list,
and that is exactly this scan. It should be extended rather than re-written —
a second quote tracker beside this one is a second reading of psql's lexer.
What the reader will want that the map does not is a *position*: the scan
updates state and reports whether the run so far is complete, but does not
return the offset of the terminating `;`. Adding that is a method on an
existing type, and the lines it walks are the ones the scanner surfaces.
A `$$` inside an `INSERT` row is proved over a hand-built dump only
(`crates/pgdt-mapping/tests/map.rs`, `an_insert_run_holding_a_dollar_pair_keeps_every_row`):
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
data-level parse keying it: the statistics observer answers
`BlockGathered::Refused`, and `map::Builder::on_copy_end` raises
`Error::FieldRefused`, as a restore under `ON_ERROR_STOP` fails the table's
`COPY`. Nothing raises it for an `INSERT` run, whose rows nothing reads yet.

**Why P8 cares.** P8 reads `INSERT` rows, and a restore of an
`--inserts` dump fails per statement, not per table: without `ON_ERROR_STOP`
it loses the one row and goes on. Whether the parse fails at a refused
`INSERT` value as at a `COPY` field, where the raise sits when a run has
no `on_copy_end`, and what it names the statement by — `FieldRefused`'s
`line` is `COPY`'s count of a block's rows, which a run of
statements has no counterpart of — is this phase's to decide.

**Origin.** The field refusal's landing, 2026-10-03. Contingent on the raise staying at the block's close.

---

## The row contracts are over `COPY`-escaped bytes, and a line is a row

**Fact.** `BlockObserver::fold` takes a field "still escaped, a delimiter never
among them" (`pgdt-model/src/statistics.rs`), and so does `FieldCount::tier`
(`block.rs`). The unescape happens at the leaves, in at least five places:
`ColumnGatherer::observe`, `FieldCheck::refused`, `Counter::tier`,
`RecordedRows`, and the batcher through `RawRow`/`RowSplit`. In the codec,
`FieldWalk` owns a `pgdt_core::Unescape` (`grammar/walk.rs`). `LongestLine`,
`FieldRefusal::line` and the row groups' byte offsets all assume `line == row`.
Both `nested_key` and `checked_key` take their input as `COPY` text.

**Why P8 cares.** "A second row-source implementation feeding the same batch
layer" forks. One branch transcodes each `INSERT` tuple to `COPY` text, which
reuses everything downstream unchanged at the cost of a copy per row. The other
gives the fold and `RawRow` an escape dialect, which needs no copy but teaches
each leaf and each tab-splitting site a second dialect. D28 bears on the
second branch: a second `push_field` site was refused because it defeats
inlining. The grilling settles the fork, and `line == row` with it.

**Origin.** An architecture review at `ec20c8fd`, 2026-11-20. *Contingent on*
the leaves still unescaping.
