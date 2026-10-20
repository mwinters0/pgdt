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

**M1–M223 are struck**, and nothing at or below `M223` is reused. That is a
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
| M224 | 2026-10-17 | pgdt-codec: PostgreSQL's cross-order answers closed — a range's, a multirange's and a nested composite's type named in the order, leaves of one type related as scalars are, `text` and `varchar` two orders; closes `KD120` | | [2026-10-17](../status/history/2026-10-17.md), "A nested key names its types at every level" |
| M225 | 2026-10-17 | pgdt-dump: `Event::CopyStart` boxed, `Event`'s size asserted no larger than `Row`'s; counted by its own `--count-pair`, a fall on every row | | [2026-10-17](../status/history/2026-10-17.md), "Boxing `Event::CopyStart` admitted as `M225`" |
| M226 | 2026-10-17 | pgdt-dump: `Opened` made `#[non_exhaustive]`, so only the open builds one and its parts come from one open | | [2026-10-17](../status/history/2026-10-17.md), "`M226` lands: only an open builds an `Opened`" |
| M227 | 2026-10-19 | pgdt: `query` carves its replay after the mapping pass, around the settled table's statistics, which its `Table` and plan hold; closes `KD123` | | [2026-10-19](../status/history/2026-10-19.md), "`pgdt query` bills the statistics its replay holds" |
| M228 | 2026-10-19 | pgdt: `the_aggregate_dynamic_filter_is_off_unless_stated` asserts the aggregate's filter after `SET` and `RESET` of `enable_dynamic_filter_pushdown` and `RESET` of its own option, as the manual states them | | [2026-10-19](../status/history/2026-10-19.md), "The aggregate's filter is off whatever the config says" |
| M229 | 2026-10-19 | datafusion-pgdump: `Error` a struct of the refusal and its spelling, the enum `Refusal` and `Error::Spelled`, `unspelled` gone; `Spelling` asked of a `&Refusal`, `Error::source()` the refusal's own; D107's `Rejected` takes the variant | | [2026-10-19](../status/history/2026-10-19.md), "A spelled refusal is the error's field, not a variant" |
| M230 | 2026-10-20 | pgdt: `--filter`'s and `--where`'s worded operators matched as words, case-insensitively, any whitespace between them and a boundary before `is` under D60's rule, in place of `strip_ci_suffix`'s byte cut; closes `KD52` and `KD124` | | [2026-10-20](../status/history/2026-10-20.md), "`M230` lands: the `IS` tests are words" |
| M231 | 2026-10-20 | pgdt-io: the cold remote `.xz` walk's warning says several round trips per stream, as the manual does, and `KD36`'s marker names both copies; closes `KD125` | | [2026-10-20](../status/history/2026-10-20.md), "`M231` lands: the walk's warning says several round trips" |
| M232 | 2026-10-20 | pgdt-model: `TilingBroken`'s message tallies its issues by kind — a `Discontinuity` a gap or an overlap by its ends, kinds with none left out; closes `KD126` | | [2026-10-20](../status/history/2026-10-20.md), "`M232` lands: a tiling failure is tallied by kind" |
| M233 | 2026-10-20 | pgdt-dump: one function computes an index's derived diagnostics, the non-seekable warning among them, for `build_index`, the cache load and `map_file`; closes `KD58` | | [2026-10-20](../status/history/2026-10-20.md), "`KD58`, `KD114`, `KD47` and `KD46` admitted as `M233`–`M236`" |
| M234 | 2026-10-20 | pgdt-dump: the TOC header parsed from its line untrimmed at the end, so an empty `Owner: ` is no owner and a name ending in whitespace is kept; closes `KD114` | | [2026-10-20](../status/history/2026-10-20.md), "`M234` lands: a TOC header is read to its line's end" |
| M235 | 2026-10-20 | pgdt-query: a column's extreme prefers the exact bound where a truncated one ties it; closes `KD47` | | [2026-10-20](../status/history/2026-10-20.md), "`M235` lands: an exact bound wins a tie" |
| M236 | 2026-10-20 | pgdt-model: `Sortedness::Constant` for a block of one distinct value, setting no direction in a table's proved order; closes `KD46` | | [2026-10-20](../status/history/2026-10-20.md), "`M236` lands: a constant block sets no direction" |
| M237 | | pgdt: `parse_filter` drops a term's leading ASCII whitespace before it splits, so `--filter` splits where `--where`'s tokenizer looked, and a seeded sweep holds every term `--filter` accepts to the `Expr` `--where` parses; closes `KD127` | | [2026-10-20](../status/history/2026-10-20.md), "`KD127` and `KD128` admitted as `M237` and `M238`" |
| M238 | 2026-10-20 | pgdt-model: `TilingBroken`'s message says a map short of the file's end is not reaching it; closes `KD128` | | [2026-10-20](../status/history/2026-10-20.md), "`M238` lands: a short map names the file's end" |
