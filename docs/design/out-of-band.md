# Out-of-band work

The ledger of work that belongs to no phase. It sits beside
[`roadmap.md`](roadmap.md)'s phase index rather than inside it because it grows
monotonically between keystones: a register read only as far as somebody's
window reaches is one that reissues a number already spoken for, and the rows
here are a work queue as well as a record. The rule it implements is
[`../process.md`](../process.md), "Out-of-band work".

An item is the kind of change that fits one session and answers to no phase's
intent: a CLI ergonomics change, a defect fix that changes no decision. It gets
a number `M<k>` and **one terse ledger line** — date, what changed, whether it
blocks the open phase, and the history entry that says why — in the table
below the watermark, started again by the first item admitted after a
keystone. Nothing else: no spec (there was no intent doc to write), and no notes doc,
because the history entry *is* the notes. If out-of-band work turns up a fact
an unspecified phase needs, that fact goes in that phase's inbox, as always.

**A number is allocated on admission, not on landing.** An item queued for
later takes its `M<k>` and its row when it is admitted, with the Date column
empty until it lands — or naming the day it was withdrawn, where it never
will — so a queued item can be cited by number, and so this
table stays the authority on which numbers are spent. Row order is allocation
order, which is why a queued row may sit above one that landed before it.

**The admitting session sets the Blocks column, because only it knows.** An
item admitted while a phase is open is either in the way of that phase's
remaining slices or it is not, and the session that just finished grilling the
decision can say which; a session picking the row up weeks later cannot, and
guesses. `Blocks` names the open phase when that phase's remaining slices
should not be landed around it, and is empty otherwise. It is read by the
unattended loop — [`.claude/skills/go/SKILL.md`](../../.claude/skills/go/SKILL.md)
takes a blocking row ahead of the next unticked slice, and with no phase open
takes every row in allocation order — and cleared when the
item lands, at the same time the Date is filled in.

**Admission rule.** An item is out-of-band only if it changes no decision any
spec records **and** fits one session. Anything that changes a decision goes
back through grilling → spec amendment → a numbered slice; that rule is what
keeps this ledger from becoming where design work goes to avoid review. **A
row is a commitment to build**, the unattended loop taking the lowest undated
one: a gap nobody has decided to close is a `KD<k>` **(c) unowned**, and one
that is no gap is rescinded, its number spent.

**Ledger lines stay one line each.** The file is read as an index, never as an
account — the detail lives in the dated history entry it points at. It grows
until a keystone, which strikes it along with the phase docs and leaves a
watermark saying which numbers are spent (`../process.md`, "The out-of-band
ledger is struck too").

**M1–M197 are struck**, and nothing at or below `M197` is reused. That is a
high-water mark rather than a claim that every one of them landed: some were
absorbed into a neighbour, folded into a phase slice or withdrawn, and their
numbers are spent all the same. What each struck item decided is filed by kind —
[`decisions.md`](decisions.md) for a mechanism, or the code's own comment where
the call is local to one item,
[`measurements.md`](measurements.md) for an apparatus change,
[`postgres-invariants.md`](postgres-invariants.md) and
[`runtime-invariants.md`](runtime-invariants.md) for an external fact,
[`../status/deficiencies.md`](../status/deficiencies.md) for a limit left open,
[`../manual/`](../manual/) for a flag,
[`decisions.md`](decisions.md), [`../process.md`](../process.md) and
[`.claude/skills/`](../../.claude/skills/) for a rule — and why it was done is
in the dated history entry it was filed under.

## The ledger

**The table below carries what is still outstanding**, and is written on by
the next item admitted. A landed row is provenance and went with the rest of
the centering; a row whose Date is still empty is a live obligation and stays,
since a number is allocated on admission and the unattended loop reads this
table as a work queue.

| Item | Date | What changed | Blocks | Why |
|---|---|---|---|---|
| M198 | 2026-10-01 | A zero in a negative-scale `numeric(p,s)` column, written `0`, decodes as zero and renders back as `0` (`decimal_unscaled_digits`, `render_decimal`), tested at every negative scale and through the null and refuse modes, closing `KD60` | | [2026-10-01](../status/history/2026-10-01.md) |
| M199 | 2026-10-02 | The preamble's `CREATE` keywords end at a word boundary, so `CREATE TABLESPACE` classifies as no table, tested over a `pg_dumpall` globals section holding one, closing `KD61` | | [2026-10-02](../status/history/2026-10-02.md) |
| M200 | 2026-10-02 | Every database of a `pg_dumpall` dump keeps its own version headers whatever the segment before it holds, the `dumpall` fixture's databases each asserted at every major, closing `KD62` | | [2026-10-02](../status/history/2026-10-02.md) |
| M201 | 2026-10-02 | A `numeric(p,s)` whose scale exceeds its precision maps to a decimal of precision `max(p,s)` at scale `s`, and a scale no Arrow decimal carries (past 76, or below `-128`) to text compared as a number, as the `p > 76` arm does; typed queries over both tested, closing `KD63` | | [2026-10-02](../status/history/2026-10-02.md) |
| M202 | 2026-10-02 | `CACHE_FORMAT_VERSION` bumped for what `M199`, `M200` and 31.5 changed in the persisted spans and metadata, `GOLDEN_ORDER` re-pinned beside it, and a digest of every fixture's persisted `DumpIndex` pinned beside the version | | [2026-10-02](../status/history/2026-10-02.md) |
| M203 | 2026-10-02 | A `real` or `double precision` literal is read by its own reader: any spelling `float8in` reads, compared by value, every out-of-range one `float8in`/`float4in` refuses refused, I57's rounded largest finite value included, `accepted_form` listing the float widening, and the manual saying a literal PostgreSQL refuses is refused, closing `KD74`, run after `M204` | | [2026-10-02](../status/history/2026-10-02.md) |
| M204 | 2026-10-02 | The `pg-refuses: I<n>` marker and `scripts/pg_refuses.py` resolving each against the invariant whose "Relied on by" lists its function, the refusals on PostgreSQL's terms already in `nested.rs` and `predicate.rs` marked, and I40 recording `cidr_in`'s | | [2026-10-02](../status/history/2026-10-02.md) |
| M205 | 2026-10-03 | An integer literal is read at its column's width, refused past it as `int2in`/`int4in` refuse it, closing `KD76` | | [2026-10-03](../status/history/2026-10-03.md) |
| M206 | 2026-10-03 | A date, time and timestamp literal and field are bounded part by part as `datetime.c` bounds them, to `IS_VALID_DATE`'s and `IS_VALID_TIMESTAMP`'s ranges, a signed part such as `12:-5:00`'s refused, closing `KD77` | | [2026-10-03](../status/history/2026-10-03.md) |
| M207 | 2026-10-03 | An interval literal and field are bounded as `interval_in` bounds them: each count and the month total to `int32`, the time to `int64` microseconds, a minute past 59 or a second past 60 refused, and a unit given twice, closing `KD78` | | [2026-10-03](../status/history/2026-10-03.md) |
| M208 | | A `numeric`, `numeric(p,s)` and `jsonb` number, literal and field, capped at `numeric_in`'s display scale and weight, closing `KD79` | P31 | [2026-10-02](../status/history/2026-10-02.md) |
| M209 | | A `uuid` literal and field take a hyphen only after a group of four digits, closing `KD80` | P31 | [2026-10-02](../status/history/2026-10-02.md) |
| M210 | | `nested_refusal` worded by the position's divergence: an `AsText` one (`json`) saying PostgreSQL refuses the same comparison, an unmodelled one (`money`, `bit`, `tsvector`, a user base type) that this build models no order for it, with no claim about the server; each tested | | [2026-10-02](../status/history/2026-10-02.md) |
