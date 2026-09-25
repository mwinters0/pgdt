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
keeps this ledger from becoming where design work goes to avoid review.

**Ledger lines stay one line each.** The file is read as an index, never as an
account — the detail lives in the dated history entry it points at. It grows
until a keystone, which strikes it along with the phase docs and leaves a
watermark saying which numbers are spent (`../process.md`, "The out-of-band
ledger is struck too").

**M1–M141 are struck**, and nothing at or below `M141` is reused. That is a
high-water mark rather than a claim that every one of them landed: some were
absorbed into a neighbour, folded into a phase slice or withdrawn, and their
numbers are spent all the same. What each struck item decided is filed by kind —
[`decisions.md`](decisions.md) for a mechanism,
[`measurements.md`](measurements.md) for an apparatus change,
[`postgres-invariants.md`](postgres-invariants.md) for an external fact,
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
| `M142` | 2026-09-24 | Type and comparison defects: a checked `jsonb` exponent; a quoted SQL-only spelling not taken for the built-in; an unqualified collation a declared one could shadow read as not bytewise, `SET search_path` unread filed as `KD48`; a depth bound on the type walks | | [2026-09-24](../status/history/2026-09-24.md), "M142: type and comparison defects" |
| `M143` | 2026-09-24 | A quoted and an unquoted type name normalized alike on both sides of a lookup, closing `KD4`; a collation's name kept the same way | | [2026-09-24](../status/history/2026-09-24.md), "M143: a quoted name finds its definition" |
| `M144` | 2026-09-24 | Tests asserting what they claim: `EVERY_OUTCOME` tied to `ColumnResolution` by a test macro; the stated-maximum test exempting an unpaired last group, `KD43`'s example pinned | | [2026-09-24](../status/history/2026-09-24.md), "M144: tests asserting what they claim" |
| `M145` | 2026-09-24 | The provider's catalog built at construction, a refused table reported and not listed, closing the lazy build's race; a column's stored groups checked in `table_summary`; `SHOW ALL`'s `pgdump.memory` text | | [2026-09-24](../status/history/2026-09-24.md), "M145: the provider's catalog is built whole" |
| `M146` | 2026-09-24 | A query's mapping pass saying what lowered its count; a signal after the mapping pass or during the listing exiting 128+signal (D26) | | [2026-09-24](../status/history/2026-09-24.md), "M146: the mapping pass's count, and a late signal" |
| `M147` | 2026-09-25 | A query's mapping pass carrying `held_bytes`, and `replay_jobs` where its count was cut, on a `mapping` span the CLI opens around the mapping await, in place of `mapping arrangement` | | [2026-09-25](../status/history/2026-09-25.md), "M147: the mapping pass's span" |
| `M148` | 2026-09-25 | An interrupted `parse` re-raising its signal after the save, `128+n` as a namespace's init (RT19); the default action armed, in the handler, by a first signal or by the scan's return, so a second signal or one during the listing ends the process at once | | [2026-09-25](../status/history/2026-09-25.md), "M148: an interrupted parse dies by its signal" |
| `M149` | 2026-09-25 | As its PID namespace's init, `pgdt` and `datafusion-cli-pgdump` ending on every signal whose default action ends them elsewhere, exiting `128+n` (RT19), `parse`'s guard keeping `SIGINT`/`SIGTERM` for its scan; pinned per binary under `unshare -Urpf` | | [2026-09-25](../status/history/2026-09-25.md), "M149: every signal ends an init" |
