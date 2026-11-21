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

**M1–M298 are spent**, and nothing at or below `M298` is reused; those the
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
| M299 | 2026-11-20 | `CompareKind::datafusion_order` and `datafusion_divergence` (`crates/pgdt-codec/src/compare.rs`) name every kind, with no catch-all arm, so a kind added fails to compile at both. Today such a kind would keep its own order under DataFusion's semantics, with no divergence note | | [2026-11-20](../status/history/2026-11-20.md), "M299: the DataFusion order names every kind" |
| M300 | 2026-11-20 | The replay generator (`crates/pgdt-query/src/stream/replay.rs`): its `Active` tuple becomes a named struct; its four batch emits become one step that the four sites call, each difference between them either removed or stated (`KD173`, at one, struck); a stream's schema and notes are settled at construction, as `in_file_order`'s were, and their mutexes go. Priced by `--count-pair` (D108) | | [2026-11-20](../status/history/2026-11-20.md), "M300: the replay's batch emits are one step" |
| M301 | 2026-11-20 | `ChunkCarry` (`crates/pgdt-dump/src/scan/chunked.rs`) holds its state between chunks as one `Leftover`, which `holding()` and `absorb()` each match whole, so a draw cannot price one case while the absorb takes another (D158); `absorb`'s split, unfinished and ended flags become its locals, and no decision in D23, D138, D139 or D158 moves. Priced by `--count-pair`, its two rises accepted (D108) | | [2026-11-20](../status/history/2026-11-20.md), "M301: the chunk carry's state is one `Leftover`" |
| M302 | withdrawn 2026-11-20 | A pass context for `pgdt-mapping`'s long parameter lists, its two interrupted exits written once, and `map_forward`'s `CopyStart` arm split out; withdrawn unlanded into P43, whose one block open is that arm and takes the context (`roadmap-P43-block-model.md`) | | [2026-11-20](../status/history/2026-11-20.md), "P43 grilled" |
| M303 | withdrawn 2026-11-20 | The statistics `Gatherer` split into a block's gatherer, a piece's and the strict checker; withdrawn unlanded into P43, which reshapes the observer contract those three implement (`roadmap-P43-block-model.md`) | | [2026-11-20](../status/history/2026-11-20.md), "P43 grilled" |
| M304 | 2026-11-21 | `scripts/file_limits.py` meters `scripts/*.py` as it meters `*.rs`, so `measure.py` and `test_measure.py`, the harness's two largest files, meet the seams discipline; splitting them is the seams round that follows | | [2026-11-21](../status/history/2026-11-21.md), "M304: the scripts are metered" |
| M305 | 2026-11-20 | `MIN_TIMESTAMP` and `END_TIMESTAMP` are stated once, in `value/datetime/grammar.rs`, which `value/datetime.rs` takes them from; `pgdt-io`'s checkpoint lookup in `first_pass/pass.rs` calls `pgdt-gzip-seek`'s `Index::unit_at`, which had no other caller; `unrepresentable.rs`'s inline `mod tests` moves to a sibling `tests.rs`, as the rest of the codec's are | | [2026-11-20](../status/history/2026-11-20.md), "M305: one statement of the timestamp range" |
| M306 | 2026-11-21 | `Recorder::finish` (`crates/pgdt-model/src/recorder.rs`) `debug_assert!`s that a gathered block's row groups hold its `row_count` rows, so every host's tests catch a row the scanner counted and the recorder was never handed (D169) | | [2026-11-21](../status/history/2026-11-21.md), "M306: a gathered block holds its counted rows" |
| M307 | 2026-11-21 | `scripts/measure.py` becomes the command line over a `measurement` package beside it: 60 modules drawn by concept (configuration, apparatus, parsing, command shapes, the memory charge, builds, the session, the register, a module per figure family, staleness, the recipes), each edge one way; its tests move beside their modules; `file_limits.py` meters every `.py` under `scripts/`; a figure's run binds to its id in `figures/runners.py` rather than on `Figure` | | [2026-11-21](../status/history/2026-11-21.md), "M307: the harness is a package" |
| M308 | | `docs/design/concepts.md` claims every production `.py` under `scripts/`, the harness's concepts drawn there, and `scripts/concepts.py` holds them to it as it holds the crates' `.rs`; a **Not in one place** line files `Session.time_run`'s family branches; `docs/process.md`'s "Seams" and the seams skill name the scripts | | [2026-11-21](../status/history/2026-11-21.md), "M307's tree, reviewed" |
| M309 | | `scripts/module_graph.py` reads the import edges of every `.py` under `scripts/` and fails on a cycle there as on a module cycle, `--design` projecting a scripts design's moves; `measurement/test_layout.py`'s edges and cycle finder move into it | | [2026-11-21](../status/history/2026-11-21.md), "M307's tree, reviewed" |
| M310 | | `Session.time_run`'s dry-run stand-ins and `Session.prebuild`'s parallel-scan cache move onto their families' declarations, so `scripts/measurement/timing.py` names no figure family; strikes M308's **Not in one place** line | | [2026-11-21](../status/history/2026-11-21.md), "M307's tree, reviewed" |
