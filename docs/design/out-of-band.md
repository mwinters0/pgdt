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

**M1–M179 are struck**, and nothing at or below `M179` is reused. That is a
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
| M180 | 2026-09-29 | `measure.py` refuses `census-brace-free`, `census-arrays`, `statistics-gathering` and `statistics-pruning`, naming 28.9, until 28.9 re-points their builders and lifts it | | [2026-09-29](../status/history/2026-09-29.md) |
| M181 | 2026-09-30 | `datafusion-cli-pgdump` starts with DataFusion's aggregate dynamic filter off (`KD56`); the upstream register `docs/status/upstream.md`, `scripts/upstream.py`, and the `upstream-issue` and `upgrade-deps` skills | | [2026-09-30](../status/history/2026-09-30.md) |
| M182 | 2026-09-30 | `ComparisonSemantics::Arrow` renamed `ComparisonSemantics::DataFusion`, the front end its contract already names, with every citation of it | | [2026-09-30](../status/history/2026-09-30.md) |
| M183 | 2026-09-30 | `arrow_order`, `arrow_divergence`, `arrow_position_divergences`, both `arrow_divergences`, `BoundsSet::Arrow`, `ColumnStatistics::arrow_bounds` (and `pgdt info --json`'s key) named for DataFusion semantics, `NestedArrowOrder` renamed `NestedOrder`, and the P28 spec's "in Arrow's (the DataFusion translators)" with them | | [2026-09-30](../status/history/2026-09-30.md) |
| M184 | 2026-10-01 | The generated pruning check's error leg and its floors restored (`pgdump_query/tests/pruning.rs`, `check`), populated by `public.t_composite_matrix`, a types-schema table whose composite field holds a 2-D array (`KD2`) at every major | | [2026-10-01](../status/history/2026-10-01.md) |
| M185 | 2026-10-01 | The warm-set bound: `measure.py`'s tmpfs budget two full-size inputs plus 64 MiB, every figure asserted to fit it, not the largest figure's need plus 10%; `statistics-gathering` and `scan-throughput-warm` one sweep per input, every `scan-throughput-*` table a `dd` floor per input, and `nested-end-to-end` control with composite and arrays alone; preflight reserving the budget by `fallocate`; a staging failure aborting the sweep, and `Stager.warm_path` marking an input staged only once its copy completes | | [2026-10-01](../status/history/2026-10-01.md) |
| M186 | 2026-10-01 | `nested-end-to-end` grouped (control, composite) then (control, arrays), each nested file read against its own sweep's control, the headline a per-rep paired arrays−control difference, a control row per sweep; `measurements.md`'s "Re-take a comparison table whole" reworded so inputs share a sweep wherever anything read off their reps subtracts one from another, a reading normalised within its sweep set beside another's but never subtracted; each figure declaring the cross-input subtractions it reads, `test_measure.py` asserting each lies in one warm group and `_per_row_diffs` refusing an undeclared one | | [2026-10-01](../status/history/2026-10-01.md) |
| M187 | 2026-10-01 | `parallel-scan-throughput`'s two typed-query legs, plain and `.xz`, timed through `datafusion-cli-pgdump` as a full-table query no statistic answers, `target_partitions` the axis over the same counts, in place of `pgdt query`, whose in-order merge they measured (`KD57`) | | [2026-10-01](../status/history/2026-10-01.md) |
| M188 | 2026-10-01 | `PlanNoteKind::ParallelismBudgetLimited`'s two arms without "both buy seats rather than speed, and on a plain source the sub-streams seated may not run concurrently at all", what seats buy being the front end's to say; `the_count_note_names_a_lever_a_plain_source_has` and the variant's and `BatchSpanNarrowed`'s rustdoc without that premise or their `KD17` citations (`pgdump_query/src/stream.rs`) | | [2026-10-01](../status/history/2026-10-01.md) |
| M189 | 2026-10-01 | A hand-written test of D54's decode-failure clause in `pgdump_query/tests/pruning.rs`: a hand-built dump whose `integer` column holds text no decoder reads, in rows statistics on a sorted column rule out, under a filter reading that column first; unpruned raises, pruned answers in a skipped group serially and split and past a sorted block's stop serially, and split raises at a later piece's first row past the stop; D54 citing it as evidence | | [2026-10-01](../status/history/2026-10-01.md) |
| M190 | | `measure.PROFILE_AXIS` retired: the pair, its staging and printed block in `profile_recipe`, `test_the_axis_pair_differs_only_in_its_worker_count` and `test_the_axis_pair_is_taken_on_one_input_it_stages`, the two `ProfileRecipe` tests sharing it reduced to `PROFILE_SHAPES`, and `DFCLI_ACCOUNT`'s comment stating its own reason for being a pair | | [2026-10-01](../status/history/2026-10-01.md) |
