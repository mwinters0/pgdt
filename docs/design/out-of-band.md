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

**M1–M258 are spent**, and nothing at or below `M258` is reused; those the
table below carries are outstanding, and the rest are struck. That is a
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
| M254 | 2026-10-29 | Every in-image build is held to one constant `SOURCE_DATE_EPOCH` set by `release.py`; accepted by a clean keyed build at v0.2.0's commit and one on target state kept from an earlier commit's build both reproducing CI's x86-64 `pgdt`; `attach` then refuses CI's x86-64 archive differing from the rehearsal's instead of reporting it, and D109 follows | | [2026-10-26](../status/history/2026-10-26.md), "The release build's clock, reviewed"; [2026-10-29](../status/history/2026-10-29.md), "`M254`: the release build's clock pinned" |
| M255 | | A heaptrack recording's container is its timed shape's own (`RunSpec.memory`) plus one 1 GiB heaptrack term, the same rule for both pairs; `heaptrack_held_bound` and `HEAPTRACK_FIRST_PASS_MEMORY` go; each recording prints its container's `memory.peak` beside its `.gz`, from inside its own `sh -c`; `test_measure.py` holds both, and `measurements.md`'s "The apparatus" heaptrack sentence follows; accepted by both pairs recorded under it, the step pair's `memory.peak` inside its container | | [2026-10-27](../status/history/2026-10-27.md), "The heaptrack recording's container, reviewed" |
| M256 | | `native/walk.rs`'s `Walk::run` made cheaper per symbol — a bit buffer held across symbols, a length and its distance decoded together, no bounds check a symbol — D118 unchanged; accepted by the level-1 file's serial first pass read before and after in one sitting | | [2026-10-28](../status/history/2026-10-28.md), "D118's walk, reviewed" |
| M258 | 2026-10-29 | An I/O failure on a file the user named — every open and `stat` of a local dump and plain read of one, every open, read and write of the cache and the file a save writes beside it — names its path (`Error::File`), and the OS's message is printed once, `Error::Io`'s no longer chained; `KD151` rewritten to the reads through a codec still unnamed | | [2026-10-29](../status/history/2026-10-29.md), "`KD151` filed and admitted as `M258`"; [2026-10-29](../status/history/2026-10-29.md), "`M258`: a failing file named" |
| M259 | 2026-10-29 | `gz-seek` renamed `pgdt-gzip-seek` while it lives in this tree — its directory, package and `pgdt_gzip_seek` paths, the features other crates name, the scripts and the standing docs; the figure ids `gzseek-*` and the harness's names for them unchanged; its split into a repo of its own not admitted | | [2026-10-29](../status/history/2026-10-29.md), "`gz-seek` renamed `pgdt-gzip-seek`: `M259`" |
| M260 | | A read failing through a local compressed dump's codec — an index's walk, a unit's decode, a first pass — or a gzip cache's window read names its file: the `Positioned` impl over the dump and `SectionWindows` hold the path and fail with an `io::Error` whose payload is `Error::File`, the traits staying `io::Result`; `From<io::Error>` lifts that payload to `Error::File` where no codec wrapped it; closes `KD151` | | [2026-10-29](../status/history/2026-10-29.md), "`KD151`'s remainder admitted as `M260`" |
| M261 | | Every `pgdt_core::Error` variant carrying a cause displays it and never chains it — `Join`, `Arrow`, `Codec` and `CacheEncode` brought under `Io`'s and `File`'s rule — so `pgdt` prints a cause's message once; accepted by a `pgdt` test showing a codec failure's message once and the suite's walks down to a `pgdt_core::Error` still finding it; the rule and its rejected alternatives into D74, or a `D<k>` of its own past D74's cap | | [2026-10-29](../status/history/2026-10-29.md), "One rule for a cause, admitted as `M261`" |
