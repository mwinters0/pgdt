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

**M1–M111 are struck**, and nothing at or below `M111` is reused. That is a
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
| M112 | 2026-09-18 | The derived batch span floors at the caller's announced `ScanOptions::chunk_size_bytes` rather than the shipped `SCAN_CHUNK_DEFAULT_SIZE_BYTES`, so it stops at the unit the source actually retains at any `--chunk-size`; `MIN_SOURCE_SPAN` struck |  | [2026-09-18](../status/history/2026-09-18.md) |
| M113 | 2026-09-18 | The CLI decline test moved off the `1.25 × MEMORY_UNPOOLED_BOUND` zero line it sat exactly on, and a unit test pinning both of `statistics_allowance`'s bands — that line and the cap/ceiling swap at `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` |  | [2026-09-18](../status/history/2026-09-18.md) |
| M114 |  | `BatchSpanNarrowed`'s remedy clause and the origin suffix on a budget-quoting note, split out of `M112` because both are text about what a memory knob buys and the knob's role is open |  | [2026-09-16](../status/history/2026-09-16.md) |
| M115 | 2026-09-18 | `cargo fmt` run over the tree, red since `2e8bd7b` pushed renamed identifiers past the column limit, so the standing check a session runs is green again rather than read against a baseline |  | [2026-09-18](../status/history/2026-09-18.md) |
| M116 |  | `--strict-identity` on `pgdq info`, refusing a bound-but-moved signal as `parse` and `query` do, with `requires = "source"` making the flag a usage error in cache-only mode, whose identity is null |  | [2026-09-18](../status/history/2026-09-18.md) |
| M117 |  | `StrictIdentityUnmet` names the expected and actual modification times rather than a fixed clause: `weak_against` compares both values and keeps neither, so the refusal states its reason but not what it saw |  | [2026-09-18](../status/history/2026-09-18.md) |
| M118 |  | An opt-in conformance test reading a fixture end to end from a real static origin, gated on an env var unset by default so CI and every other checkout pass without it — the one question the bespoke oracle cannot answer, whether we misread HTTP the same way twice |  | [2026-09-18](../status/history/2026-09-18.md) |

