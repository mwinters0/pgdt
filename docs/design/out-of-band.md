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

**M1–M274 are spent**, and nothing at or below `M274` is reused; those the
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
| M275 | 2026-10-31 | `pgdt query` warns on stderr, before any row, of each identity diagnostic its cache's load raised — a moved modification signal or another origin — which D21 calls warned and `parse`, `info` and `pgdt sql` already say; `Diagnostic::is_identity` names the three kinds the mapping pass also carries; accepted by `identity.rs`'s `query_warns_of_a_moved_signal_and_reads_on`, failing without the warning | | [2026-10-31](../status/history/2026-10-31.md), "`M275`: `query` warns of a moved identity" |
| M276 | 2026-11-01 | The codec's `date`, `time`, `timetz`, timestamp and `interval` readers refuse a text they read past the work buffer its input function parses it in (`within_workbuf`), marked `pg-refuses: I83` and `I84`, `CACHE_FORMAT_VERSION` bumped, so 33.3.3's census head for a counted scalar is exact; accepted by `value/datetime/tests.rs`'s `a_text_past_its_work_buffer_is_refused` | | [2026-11-01](../status/history/2026-11-01.md), "`M276`: the datetime readers end at the server's work buffer" |
| M277 | 2026-11-03 | A refused field's value is named as the restore's error names it, clipped to `MAX_COPY_DATA_DISPLAY`'s 100 bytes on a character boundary with `...` (`limit_printout_length`), read off the field's head by `printout_head`, in `Error::FieldRefused` and in the cache's `ColumnRefusals::first`; I94 filed for it, `CACHE_FORMAT_VERSION` bumped | | [2026-11-03](../status/history/2026-11-03.md), "`M277`: a refusal quotes its field as the restore does" |
| M278 | 2026-11-03 | `Error::FieldDecode` quotes its field as `Error::FieldRefused` does (I94), by `pgdt-core`'s `printout` over the unescaped text, at both constructors — the batch's append path (`batch.rs`, `append_typed`'s error now unit) and a filter term's `field_decode` (`predicate/eval.rs`) — so a long field's decode error neither prints the field nor copies it beside its held line; accepted by `batch/tests.rs`'s `a_long_field_s_decode_error_quotes_its_head` and `predicate/tests.rs`'s `a_long_undecodable_field_is_quoted_by_its_head`, each failing on its parent | | [2026-11-03](../status/history/2026-11-03.md), "`M278`: a query's decode error quotes its field's head" |
| M279 | 2026-11-11 | A figure's own container is held to `contained.MAX_FRACTION` of the machine the sweep runs on, refused at preflight as `contained.py`'s limit is; `LONG_LINE_MEMORY` `15g`; `measure.py`'s comments on `LONG_LINE_SERIAL_ONLY` and `LONG_LINE_TERM_KINDS` and `measurements.md`'s "The apparatus" stating the one ceiling | | [2026-11-11](../status/history/2026-11-11.md), "33.16.2's term legs: `int8[]` and `jsonb` read the bound" |
| M280 | | The census asks a field's tier only of a column its counter tests: `FieldCount` names its tested columns, and `BlockCensusExt::observe_row` and `CensusFold` both read that one list rather than `observe_row` calling `tier` on every field and `CensusFold` asking `keeps` of each; priced by its own `--count-pair` (D108), a fall expected on every data-level parse | | [2026-11-12](../status/history/2026-11-12.md), "33.25's rise stands; the census's held columns are `M280`'s" |
