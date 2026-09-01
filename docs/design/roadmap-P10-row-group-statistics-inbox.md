# P10 inbox — facts filed for its grilling

Evidence found in earlier phases that P10 (row-group statistics) will need.
**This is a queue, not a document**: when P10 is grilled, walk every entry,
fold it into its spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## Per-block, per-column recording during the scan exists, and it avoided the L1/L2 injection it looked like it needed

**Fact.** pgdq carries the array-shape census: `CopyBlock::array_shapes`, a
`Vec<ArrayShape>` written per column during the scan by
`map::Builder::on_row`, finalized at `CopyEnd` and persisted in the cache.
Every mapping pass records one, so a block in the map always carries a total
census.

It is the first per-block per-column fact gathered during the scan, and the
`layering.md` problem it faced is the one `RowGroupStats` faces: L1 does the
scanning and cannot know a column's type, which is L2's. The layering doc
pre-answers this with rule 6 (inject the parse function downward). **The census
did not need to.** I25 makes an array's dimensionality readable off the raw,
still-COPY-escaped field — a leading brace run — so the recording stayed
type-blind and entirely inside L1, and the *interpretation* (which columns are
arrays, what a depth means) sits wholly in the consumer.

**Why P10 cares.** Per-row-group column statistics are the same shape of
problem and reach for the same rule 6 answer, which `layering.md` already
records as the intended one. Two things transfer. First, the cheaper option is
worth checking first: a statistic that can be computed from the literal's
lexical form alone needs no injection and no type knowledge, and stays in L1
where the scan already is. Second, where injection genuinely is needed, the
census is the worked example of what the *storage* side then looks like —
per-block, `Option` to distinguish "not gathered" from "gathered, saw nothing",
and a two-sided believability test, because a scan reads a block's bytes once
and a partial earlier pass leaves blocks that can never be back-filled.

**Origin.** 2026-08-26. See
[`architecture.md`](architecture.md), "The array shape census" and "What the
census decides, and who may believe it".

---

## A column's order and its Arrow type have come apart, and `ResolvedSchema` carries the order separately

**Fact.** `ResolvedSchema::comparisons` is a per-column `ComparisonPlan` whose
`CompareKind` says how two of that column's *field texts* order, and it no
longer follows the Arrow type. Two columns held as strings now have a real
order: a bare `numeric` (`Utf8View`) compares as an arbitrary-precision
decimal, and an enum (`Dictionary(Int32, Utf8)`) compares by the declaration
order the dump carries. Both are exactly PostgreSQL's own order (I33, I34).
Four rows of the register still order bytewise where the server does not — a
`text` column whose collation the file does not state or states as something
other than `C`/`POSIX`, `character(n)`, and the text-held types — and each of
those says so through `TableStream::ordering_notes`.

**Why P10 cares.** `roadmap.md`'s P12 section justifies its own position with
"a per-row-group minimum over a `Utf8View` column is a lexicographic bound
where a typed one is a real one", which reads as though the Arrow type decides
whether a statistic is meaningful. It does not any more. A min/max over a bare
`numeric` or an enum column can be a *real* bound taken with the column's own
comparison before that column ever gains a narrower Arrow type — and, in the
other direction, a `text` column's lexicographic bound is knowably wrong for
the server's order on exactly the four rows the register marks divergent, which
is the same question P10 has to answer for pruning soundness. `comparisons` is
the vector that answers "is a bound over this column sound", and it is filled
by `resolve_columns` at L2, one per column, cut by `stream::project` with the
rest.

**Origin.** 11.4, 2026-09-01. See
[`architecture.md`](architecture.md), "Ordering operators compare typed", whose
table is the register, and
[`roadmap-P11.4-enum-and-bare-numeric-notes.md`](roadmap-P11.4-enum-and-bare-numeric-notes.md).
**Contingent on** the register's remaining divergent rows: 11.5 and 11.6 close
more of them, so re-read the table rather than trusting this list of four.
