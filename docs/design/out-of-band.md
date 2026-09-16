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
empty until it lands — so a queued item can be cited by number, and so this
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

**M1–M110 are struck**, and nothing at or below `M110` is reused. That is a
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
| M111 | 2026-09-16 | A plain `query`'s batch pin (`max_source_span`) derived from the read-buffer budget and the requested count instead of a constant, so `--jobs` buys readers at the cost of batch size |  | [2026-09-16](../status/history/2026-09-16.md) |
| M112 |  | `MIN_SOURCE_SPAN` read off the caller's announced `ScanOptions::chunk_size` rather than the shipped `DEFAULT_CHUNK_SIZE`, so the span floors at the unit the source actually retains at any `--chunk-size` |  | [2026-09-16](../status/history/2026-09-16.md) |
| M113 |  | The CLI decline test moved off the `1.25 × MEMORY_UNPOOLED_BOUND` crossover it currently sits exactly on, and a unit test pinning both of `statistics_allowance`'s bands — the zero line and the cap/ceiling swap at `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` |  | [P20.7 notes](roadmap-P20.7-decline-notes.md) |
| M114 |  | `BatchSpanNarrowed`'s remedy clause and the origin suffix on a budget-quoting note, split out of `M112` because both are text about what a memory knob buys and the knob's role is open |  | [2026-09-16](../status/history/2026-09-16.md) |

