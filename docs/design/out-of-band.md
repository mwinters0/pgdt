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
takes a blocking row ahead of the next unticked slice — and cleared when the
item lands, at the same time the Date is filled in.

**Admission rule.** An item is out-of-band only if it changes no decision any
spec records **and** fits one session. Anything that changes a decision goes
back through grilling → spec amendment → a numbered slice; that rule is what
keeps this ledger from becoming where design work goes to avoid review.

**Ledger lines stay one line each.** The file is read as an index, never as an
account — the detail lives in the dated history entry it points at. It grows
until a keystone, which strikes it along with the phase docs and leaves a
watermark saying which numbers are spent (`../process.md`, "The out-of-band
ledger is struck too").

**M1–M135 are struck**, and nothing at or below `M135` is reused. That is a
high-water mark rather than a claim that every one of them landed: some were
absorbed into a neighbour, folded into a phase slice or withdrawn, and their
numbers are spent all the same. What each struck item decided is in `decisions.md` —
[`decisions.md`](decisions.md) for a mechanism,
[`measurements.md`](measurements.md) for an apparatus change,
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
| M136 | 2026-09-23 | `--where`: a `(` before a keyword is structure, as D60 states | | [2026-09-23](../status/history/2026-09-23.md), "The repoint's findings, dispositioned" |
| M137 | 2026-09-23 | A declared type is read as PostgreSQL's grammar reads it — aliases, a mid-name typmod, `interval`'s fields; strikes `KD44` | | [2026-09-23](../status/history/2026-09-23.md), "The repoint's findings, dispositioned" |
| M138 | 2026-09-23 | Every stale figure re-taken by `measure.py --all`, launched detached once `M136` and `M137` land | | [2026-09-23](../status/history/2026-09-23.md), "The repoint's findings, dispositioned" |
| M139 | 2026-09-23 | Named what moved `peak-rss` and `rss-attribution` between `9b35bea` and `542fdfb`: `f672ad6`'s linked HTTP stack and `d767794`'s save | | [2026-09-23](../status/history/2026-09-23.md), "`M139`: what moved the resident figures" |
| M140 | 2026-09-23 | `--statistics-group-size` is `--row-group-size`, and `STATISTICS_GROUP_*` is `ROW_GROUP_*`: Parquet's name for the unit `RowGroup` already carried | | [2026-09-23](../status/history/2026-09-23.md), "`M140`: a statistics group is a row group" |
| M141 | 2026-09-23 | `--statistics-min-rows` and `--statistics-max-rows` are `--row-group-min-rows` and `--row-group-max-rows`; the manual says how a row group differs from Parquet's | | [2026-09-23](../status/history/2026-09-23.md), "`M140`: a statistics group is a row group" |
