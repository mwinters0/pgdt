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

**M1–M150 are struck**, and nothing at or below `M150` is reused. That is a
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
| `M151` | 2026-09-25 | `oracle_register.py` reads the comparison walk where it now lives, `comparison_walk` behind `comparison_for`'s visit bound, so `test_oracle_register`'s committed-tree cases pass again | | [2026-09-25](../status/history/2026-09-25.md), "M151: the oracle register lost its anchor" |
| `M152` | 2026-09-25 | The `dev` profile optimizes, the workspace at `opt-level = 1` and its dependencies at `2`, so the fixture sweeps stop running unoptimized; `release` and every figure untouched (D92) | | [2026-09-25](../status/history/2026-09-25.md), "M152: the test build optimizes" |
| `M153` | 2026-09-25 | The fixture sweeps' several-fold slowdown between 2026-09-22 and 2026-09-25, bisected under `M152`'s profile, is a test cost: `25.1`'s statistics fixtures; the code's own share is a few percent, at plan time, and no product regression | | [2026-09-25](../status/history/2026-09-25.md), "M153: the sweeps' slowdown is the fixtures" |
| `M154` | | `cargo-nextest`, pinned in `mise.toml`, runs the suite: test binaries in parallel under a capped thread count, doctests by `cargo test --doc` | | [2026-09-25](../status/history/2026-09-25.md), "The test suite's cost" |
| `M155` | | `scripts/check.py`, with a `mise run check` task, runs each per-round check once, logs it whole under `runs/`, prints a fixed short summary and stamps the tree it tested; `--verify` reuses a passing run on an unchanged tree; `CLAUDE.md` and the skills name only it | | [2026-09-25](../status/history/2026-09-25.md), "The test suite's cost" |
| `M156` | | `check.py --affected` runs no cargo on a change touching only paths no test reads, and otherwise the changed crates, their dependents and the tests reading a changed path; a phase wrap runs the whole suite | | [2026-09-25](../status/history/2026-09-25.md), "The test suite's cost" |
| `M157` | 2026-09-25 | `statistics.rs`'s `agrees` compares two refusals by table, column and kind, not row offset or value, so `statistics_never_change_an_answer` holds under load; `per_major`'s doc comment claims no read order. Runs before `M154` | | [2026-09-25](../status/history/2026-09-25.md), "M157: a refusal compared by what refused" |
