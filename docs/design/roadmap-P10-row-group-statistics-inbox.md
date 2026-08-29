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
