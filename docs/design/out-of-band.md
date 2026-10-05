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

**M1–M212 are struck**, and nothing at or below `M212` is reused. That is a
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
| M213 | | `--heaptrack-recipe` records a `system` `profiling` build in a `--target-dir` of its own under `PGDT_MEASURE_ALLOC_BUILD_ROOT`, its text, `HEAPTRACK_AXIS`'s and `measurements.md`'s heaptrack paragraph saying it sees C and Rust alike and citing `KD109` for the shipped heap, with `test_measure.py` asserting the build | | [2026-10-05](../status/history/2026-10-05.md) |
| M214 | | `help_text.rs`'s width assertion skips only `sql`'s `--dump` spec line, citing `D67`, every other spec-only line on every page held to the width | | [2026-10-05](../status/history/2026-10-05.md) |
| M215 | 2026-10-05 | Every figure runs in `archlinux:base`: `measure.Config.image` and `dfcli_image` one pinned image, the `"dfcli"` RunSpec's image switch gone, and "The apparatus"'s glibc paragraphs, `pgdt sql`'s departure and its per-table image notes rewritten to one image of the build host's distribution, and `preflight` refusing a sweep whose `pgdt --version` does not start in it, naming the pin; `rss_wrapper`'s `perl` replaced by `peak-rss`, a workspace bin crate built static for `<machine>-unknown-linux-musl` by `preflight` and mounted read-only at `/peak-rss`, exiting `128 + signo` on a signal death with a `signal=<n>` line, its crate tests and a preflight probe around `/bin/true` in the pinned image as its checks, `GETRUSAGE_SYSCALL` gone, and "The instrument", `OOM_ORACLE`'s account of the exit code and CONTRIBUTING's measuring prerequisites rewritten to it | | [2026-10-05](../status/history/2026-10-05.md) |
| M216 | | A `cold-nvme-parallel` regime — a `measure.REGIMES` row on the NVMe staging `scan-throughput-nvme` reads, a `CONTENTION_LIMITS` row — and `parallel-scan-throughput` legs of the provider's typed plain scan at 1, 2 and 4 partitions in it, taken in the sweep; the figure re-taken, and `measurements.md`'s "Neither cold device is a parallel regime" left rejecting the SATA one alone | | [2026-10-05](../status/history/2026-10-05.md) |
