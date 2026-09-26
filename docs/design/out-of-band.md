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

**M1–M158 are struck**, and nothing at or below `M158` is reused. That is a
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
| `M159` | 2026-09-26 | A cache that cannot be used — another file's stored size, another build's format, or not a pgdt cache at all — refuses the run by default, as a user's accident; `--overwrite-unusable-cache`, and its library option, opts into starting cold and overwriting the first two, for a stable URL whose file is replaced between runs, and never the third, which `KD30` closing (magic and version read first) makes tellable; D20 and D22 rewritten in the same change | | [2026-09-26](../status/history/2026-09-26.md), "Grilling the open calls" |
| `M160` | 2026-09-26 | Withdrawn: an uncached `.xz` stays opened on the calling task, the property stated beside `open_local` — only a first open walks, before anything else is scheduled, and `seek table build started` says so | | [2026-09-26](../status/history/2026-09-26.md), "Grilling the open calls" |
| `M161` | | A discrete range literal whose bound's successor leaves its subtype — an `int4range` bound at `int4`'s maximum, a `daterange` one at `date`'s — is refused as the server's canonical function refuses it, not only an `int8range`'s | | [2026-09-26](../status/history/2026-09-26.md), "The repoint's open calls" |
| `M162` | | `check.py --verify` never reuses a passing run of a tree whose newest run, of any command list, failed | | [2026-09-26](../status/history/2026-09-26.md), "The repoint's open calls" |
| `M163` | | A block gathered under another sizing is re-read twice in a run under a stated `--row-group-max-rows` — at the default size, then the finer one its groups predict — and `STATUS.md`, `backfill`'s doc and `pgdump_query/tests/statistics.rs`'s doc say so where they say "once", a test counting the re-reads | | [2026-09-26](../status/history/2026-09-26.md), "Grilling the open calls" |
| `M164` | | `--where`'s tokenizer opens a quoted region only where the term grammar reads one — at the start of a part — so `--where "note=don't and x=1"` is a conjunction and `--filter` refuses the same string, closing `KD49` | | [2026-09-26](../status/history/2026-09-26.md), "Grilling the open calls" |
| `M165` | | A read or decode that fails mid-run re-checks the source's identity before it is reported, so a dump changed underneath the run stops as `SourceChangedWhileRead` rather than as a short read, an `.xz` error or invalid UTF-8; `gather_block_statistics`' unchecked `size - read_pos` no longer underflows at a truncation | | [2026-09-26](../status/history/2026-09-26.md), "A dump changed mid-run: what happens" |
| `M166` | | A remote source whose probe states a weak entity tag is not pinned by `If-Match`, which compares strongly and would refuse every read after the probe: reproduced first against a server stating one, then pinned by `If-Unmodified-Since` or read unpinned and said so | | [2026-09-26](../status/history/2026-09-26.md), "A dump changed mid-run: what happens" |
| `M167` | | The piecewise `.xz` arm compares the check of every block a run read from, the last included, before the run reports success, so a block failing its own check fails the run though its bytes were already handed on; a change `xz_seek` needs is requested of its own agent | | [2026-09-26](../status/history/2026-09-26.md), "A dump changed mid-run: what happens" |
