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

**M1–M172 are struck**, and nothing at or below `M172` is reused. That is a
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
| M173 | 2026-09-27 | An enum's `MIN`/`MAX` handed over as `Dictionary` scalars off the label-order bounds, closing `KD45` | | [2026-09-26](../status/history/2026-09-26.md) |
| M174 | 2026-09-27 | `measure.py`'s two images pinned by digest, and the glibc each figure ran under named in the stamp or its own marker | | [2026-09-27](../status/history/2026-09-27.md) |
| M175 | 2026-09-27 | A scan's `EXPLAIN` line prints its static filter and its dynamic ones as one `predicate=`, as Parquet does | | [2026-09-26](../status/history/2026-09-26.md) |
| M176 | 2026-09-27 | A test-only node running a scan's partitions in a stated order: `statistics_never_change_an_answer` in file order, `dynamic_filters.rs` gaining the zeros' extremes in both, both targets re-run under load | | [2026-09-27](../status/history/2026-09-27.md) |
| M177 | 2026-09-28 | `render_allocations`' counting allocator counts only its own thread, so a stray allocation elsewhere in the process cannot fail the per-row budget | | [2026-09-28](../status/history/2026-09-28.md) |
| M178 | 2026-09-28 | `measure.py --pin-cpus`, off by default, placing a leg on one or two L3 groups by its thread count; binaries staged on tmpfs; `time` read to the microsecond with user and sys; a startup leg; and three sittings deciding whether pinning halves drift and pricing what staging moves | | [2026-09-28](../status/history/2026-09-28.md) |
| M179 | | Binaries staged on tmpfs in every regime as the apparatus, `measurements.md`'s rule saying so, then a full sweep and a second on its commit re-taking `session-drift` | | [2026-09-28](../status/history/2026-09-28.md) |
