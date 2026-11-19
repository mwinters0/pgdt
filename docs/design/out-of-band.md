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

**M1–M287 are spent**, and nothing at or below `M287` is reused; those the
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
| M280 | 2026-11-19 | The census asks a field's tier only of a column its counter tests: `FieldCount` names its tested columns, and `BlockCensusExt::observe_row` and `CensusFold` both read that one list rather than `observe_row` calling `tier` on every field and `CensusFold` asking `keeps` of each; priced by its own `--count-pair` (D108), a fall expected on every data-level parse | | [2026-11-12](../status/history/2026-11-12.md), "33.25's rise stands; the census's held columns are `M280`'s" |
| M281 | 2026-11-19 | A sub-stream asked to park emits nothing for it: a batch pinning no read chunk parks with its rows held, and one pinning re-owns its view columns first (`GenericByteViewArray::gc`, `append_array` into a fresh builder, `RetainedChunks`' block indices cleared), so batches cut at `max_rows` the same each run; D163 and `replay`'s and `TablePartitions`' rustdoc say so; accepted by `tests/ledger.rs` tests parking a sub-stream with and without views, asserting its batches equal an unlimited ledger's and that a parked sub-stream pins no chunk, failing on the parent | | [2026-11-15](../status/history/2026-11-15.md), "A parked sub-stream keeps its batch, its views re-owned: `M281`, `KD171`" |
| M282 | 2026-11-19 | `ReadOptions::line_bound` is a private field in every build, stated by a test outside `pgdt-dump` only through a `test-support` function in `pgdt_dump::internal`, so `layering.py`'s `internal` rule covers it and `line_bound()` has no `cfg` arm; every `ReadOptions { … }` outside the crate moves to `with_chunk_size`; D164's `Code:` names the function | | [2026-11-16](../status/history/2026-11-16.md), "A test's line bound crosses the crate boundary through `internal`: `M282`" |
| M283 | 2026-11-19 | `RowBatcher::new` installs the offset cut only where neither `max_bytes` nor `max_source_span` is at or below `OFFSET_CUT_BYTES`, a span cap bounding a batch's text already, so a default batch's per-row byte test is its parent's; D164's `Rejected` gains the ceiling under every cap; `push_row` debug-asserts `bytes_in_batch` within the span, which the rustdoc on `bytes_in_batch`, `count_row` and `new` states; `batch/tests.rs` asserts a default batcher keeps `max_bytes` `None`, one with both caps lifted takes the cut, and a stated `max_bytes` below it stands | | [2026-11-16](../status/history/2026-11-16.md), "The offset cut is installed only past the caps that already imply it: `M283`" |
| M284 | 2026-11-19 | `pgdt` names mimalloc's purge-delay option by name, not by `MI_OPTION_PURGE_DELAY`'s hand-counted 15: `build.rs`, with `mimalloc` or `introspect` on, compiles a C shim against `DEP_MIMALLOC_INCLUDE_DIR` setting the default and reading it back, which `alloc.rs`'s `purge` and the block tracker's test call; `tests/allocator.rs` stays as the test of the default and its override; `UF6`'s Workaround rewritten, and the P23 inbox's "mimalloc reads back every option by name" re-weighed against the shim | | [2026-11-17](../status/history/2026-11-17.md), "33.33's `--version` marker and hand-counted index reviewed: `M284`, `UF6`" |
| M285 | 2026-11-19 | `ledger/tests.rs` reaches parking past a statistics overrun under `parking`'s `Terms`: two lines holding and a statistics draw past the room asking the later line to give up and parking the standing reader after the earliest line, let go as the later line gives up, and a line alone holding with statistics past the room parking it with nothing admitted; D163's `Evidence` stands | | [2026-11-18](../status/history/2026-11-18.md), "The repoint's findings reviewed: D163 widened, `M285`, `M286`" |
| M286 | 2026-11-19 | `CompressedFileSource::open`'s gzip arm refuses a file opening with a BGZF member whose chain breaks by its own `why`, that chain not holding, and keeps "not BGZF" for a file whose first member is not BGZF's, as `is_bgzf(leading)` already tells them apart; `walk/tests.rs` asserts the cut-short file's refusal names the chain, and the plain gzip file's still says "not BGZF" | | [2026-11-18](../status/history/2026-11-18.md), "The repoint's findings reviewed: D163 widened, `M285`, `M286`" |
| M287 | 2026-11-19 | `ChunkCarry::charge_absorbing` charges nothing but the field a fold holds whole across the boundary where the carry knows, before absorbing `next`, that the row it carries streams — its block streams rows, `next` holds no newline, and the file goes on past it — so a row a pass streams is never drawn for; `KD170` struck with its marker; `dump-inspection.md`'s "`--jobs` and `--memory`: the workers and the allowance" narrowed to a row of at most two read chunks; a test reads a row a pass streams under a floor below two chunks, refused on the parent | | [2026-11-19](../status/history/2026-11-19.md), "33.7's strict run: a floor below its longest line, `KD170` promoted" |
| M288 | 2026-11-19 | `ArrayWalk::braces` (`output/walk.rs`) and `ArrayScan::braces` (`output.rs`) answer `None` past `MAXDIM` levels, the input side's constant (`input/array.rs`) shared rather than restated, so a brace run deeper than an array holds fails its field rather than overflowing the stack; `KD175` struck with its marker; tests decode and walk a run of 100,000 `{`, aborting on the parent, and refuse seven levels where six read | | [2026-11-19](../status/history/2026-11-19.md), "`M288`: a brace run past `MAXDIM` fails its field" |
| M289 | | `scan_preamble`'s spans tile `[0, preamble_end)`, its first opening at byte 0 as `forward::splice` floors a segment's, so a file opening on blank lines holds them in its first span under `parse --preamble-only` too; its rustdoc's "leading blank lines are in none" goes; `CACHE_FORMAT_VERSION` bumped; `KD169` struck with its marker; a test runs `parse --preamble-only` then `parse` over such a file and finds no `TilingBroken`, failing on the parent | | [2026-11-19](../status/history/2026-11-19.md), "Five deficiencies admitted as hygiene: `M288`–`M292`" |
| M290 | | `parse_alter_type_add_value_body` reads `BEFORE`/`AFTER '<neighbour>'` into `SpanBody::AlterTypeAddValue`, and `fold_alter_type_add_value` inserts the label beside its neighbour, a neighbour among no labels read leaving the enum inexact as `fold_enum_labels_unread` does; `info/labels.rs` prints the placement; the rustdoc's "`BEFORE`/`AFTER` is ignored" rewritten; `CACHE_FORMAT_VERSION` bumped; `KD89` struck with its marker; tests fold labels placed `BEFORE` the first and `AFTER` a middle one into `pg_enum`'s order, and one naming an undeclared neighbour inexact, failing on the parent | | [2026-11-19](../status/history/2026-11-19.md), "Five deficiencies admitted as hygiene: `M288`–`M292`" |
| M291 | | `render`'s `time`, `timetz`, `date`, `timestamp` and `timestamptz` arms refuse as `Error::FieldRender` a value outside the range its `*_in` reads — past `24:00:00` or below zero, outside `DATE_MIN_DAYS`..`DATE_END_DAYS` or `MIN_TIMESTAMP`..`END_TIMESTAMP` — against the codec's bounds (`value/datetime.rs`) exported rather than restated; `KD166` struck with its marker; `batch/builder/tests.rs` renders an array holding each, refused, and each bound, read back by `decode`, the refusals failing on the parent | | [2026-11-19](../status/history/2026-11-19.md), "Five deficiencies admitted as hygiene: `M288`–`M292`" |
| M292 | | `Scanner::next_event` ends a `COPY` block at a `\.` line only where a line ending follows it: the file's last line `\.`, or `\.` CR in a block whose rows end in LF or CR LF, with none after it is refused by an `Error` variant of its own naming the marker's offset and that a restore refuses it (I91), a bare-CR block staying `KD101`'s; `KD105` struck with its marker; `scan` tests refuse both at the file's end and still end a block at `\.` LF and `\.` CR LF there, the refusals failing on the parent | | [2026-11-19](../status/history/2026-11-19.md), "Five deficiencies admitted as hygiene: `M288`–`M292`" |
