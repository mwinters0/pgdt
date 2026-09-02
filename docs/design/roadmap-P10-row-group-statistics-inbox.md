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
Some rows of the register still order bytewise where the server does not — a
`text` or `character(n)` column whose collation the file does not state or
states as something other than `C`/`POSIX`, a `jsonb` string leaf, and `json` —
and each of those says so through `TableStream::comparison_notes`.

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

**Origin.** P11, 2026-09-01. See
[`architecture.md`](architecture.md), "Ordering operators compare typed", whose
table is the register. **Contingent on** the register's divergent rows, which
P11 went on to close one type at a time after this was filed — read the table
rather than trusting any list of them written here.

---

## A per-column census would decide `interval`'s Arrow type, and a statistics pass gets that answer for nothing

**Fact.** `interval` is a `Utf8View` because two values a dump can legally hold
have no `Interval(MonthDayNano)`: v17's `infinity`/`-infinity` (I34), and any
interval whose time part exceeds `2562047:47:16.854775807`, PostgreSQL storing
microseconds in the `int64` where Arrow stores nanoseconds (I40). Neither is a
format limit — `pg_dump` pins `INTERVALSTYLE = POSTGRES` (I4), so the text is
fully determined and parses. Whether a *given* column contains either is a
question about its values, not its declaration, and for most columns the answer
is no. The ADBC driver maps the type unconditionally and fails the batch with
`EINVAL` when it meets one, infinity included.

**Why P10 cares.** A pass that reads every value of a column to summarize it
already computes what this needs: "is every value finite and under the
ceiling". Taken there it is free; taken on its own it costs a scan that buys
one type. It also makes the Arrow type data-dependent, which this project
already accepts for the array-shape census — `List<T>` against `List<List<T>>`
is no smaller a difference than `Utf8View` against `Interval(MonthDayNano)` —
so the precedent for the mechanism and the precedent for the objection are the
same one. P10's grilling should decide whether column typing is in its scope at
all, or whether it only publishes the statistic and something else reads it.

**Origin.** The P11 text-held-type review, 2026-09-01. See
[`architecture.md`](architecture.md), "The bar: the dump alone determines the
value", whose rejected-alternative paragraphs carry the reasoning, and
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md).
**Contingent on** nothing changing about `Interval(MonthDayNano)`: if Arrow
gains a wider interval or an infinity encoding, the deficiency this works
around is gone and the census is unnecessary.
