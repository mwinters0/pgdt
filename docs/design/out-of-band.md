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

**M1–M265 are spent**, and nothing at or below `M265` is reused; those the
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
| M266 | 2026-10-31 | `SET pgdump.<key>` refuses a `STORED AS PGDUMP` option as that statement's, naming its `OPTIONS` and the four scan settings, where it listed only those; the factory's and the settings' keys one list each in `settings.rs`; accepted by a `settings.rs` test over every option and a `SET pgdump.inheritance` through SQL | | [2026-10-31](../status/history/2026-10-31.md), "`pgdump.inheritance` stays a dump's option"; [2026-10-31](../status/history/2026-10-31.md), "`M266`: a `SET` of a dump's option names it" |
| M267 | 2026-10-31 | `generate_fixtures.py --flag-set <name>` regenerates the named flag sets of the schemas `--schema` names, or with `--answers-only` answers them again, leaving its siblings' dumps and answers untouched; accepted by `test_generate_fixtures.py`'s `OneFlagSet` and a one-set run at 13 | | [2026-10-31](../status/history/2026-10-31.md), "The 38.4–38.5 round's calls, reviewed"; [2026-10-31](../status/history/2026-10-31.md), "`M267`: one flag set regenerated alone" |
| M268 | 2026-10-31 | `pgdt info --json`'s `timescaledb` walks the databases the text listing does — the DDL's, then any only catalog blocks name — so a database the listing shows is never missing from the export; accepted by `descendants.rs`'s `the_json_export_states_a_catalog_only_its_blocks_name`, a plain TimescaleDB dump ahead of a `--create` one; closes `KD162` | | [2026-10-31](../status/history/2026-10-31.md), "`KD162` filed and admitted as `M268`"; [2026-10-31](../status/history/2026-10-31.md), "`M268`: the export walks the listing's databases" |
| M269 | 2026-10-31 | The statistics back-fill banks a cancelled read the leader dispatched, as the mapping pass does, so Ctrl-C there ends a `parse` interrupted rather than as `Error::ScanCancelled`; accepted by a `backfill/tests.rs` test over a source failing its read once the token fires; closes `KD37` | | [2026-10-31](../status/history/2026-10-31.md), "Seven `KD`s admitted as `M269`–`M274`"; [2026-10-31](../status/history/2026-10-31.md), "`M269`: a dropped leader read interrupts the back-fill" |
| M270 | 2026-10-31 | `attach_text` ends a run of text-storing spans at one longer than `SPAN_STORED_TEXT_MAX_BYTES`, read alone and capped, so a short span after a long one is stored whole; accepted by such a pair beside `pgdt-mapping/tests/map.rs`'s long-span test; closes `KD31` | | [2026-10-31](../status/history/2026-10-31.md), "Seven `KD`s admitted as `M269`–`M274`"; [2026-10-31](../status/history/2026-10-31.md), "`M270`: an over-cap span is read alone" |
| M271 | 2026-10-31 | `pgdt sql` writes an error ending its run to stderr, a `pgdump:` departure from upstream's `main`; accepted by a refused `--source` and a failed `-c` each leaving stdout without the error; closes `KD122` | | [2026-10-31](../status/history/2026-10-31.md), "Seven `KD`s admitted as `M269`–`M274`"; [2026-10-31](../status/history/2026-10-31.md), "`M271`: an error ending `pgdt sql` is on stderr" |
| M272 | 2026-10-31 | `--source`'s value name becomes `[NAME=]SOURCE[:SUFFIX]...`, its help keeping the grammar, and `help_text.rs`'s spec-line skip and `next_line_help` on `pgdt`'s `Sql` go, as does that candidate from `roadmap.md`'s "One pass over CLI output"; accepted by the library's `Args` printing every flag's help unwrapped without `next_line_help`; closes `KD110` | | [2026-10-31](../status/history/2026-10-31.md), "Seven `KD`s admitted as `M269`–`M274`"; [2026-10-31](../status/history/2026-10-31.md), "`M272`: `--source`'s value name is a placeholder" |
| M273 | | With `MemAvailable` unreadable a source's recommendation caps at `DEFAULT_MEMORY_BUDGET`, the provider's scans drawing it as a shared estimate, D11 rewritten; every budget and "lowered by" `pgdt` prints names the origin that bound it; accepted by an arrangement with no `MemAvailable`, a provider pair sharing it, and a `pgdt` line per origin; closes `KD113` and `KD50` | | [2026-10-31](../status/history/2026-10-31.md), "Seven `KD`s admitted as `M269`–`M274`" |
| M274 | | A stated density maximum reads the `⌈9(G−1)/10⌉`-th smallest group, monotone at every count, D82 amended and `CACHE_FORMAT_VERSION` bumped; accepted by `density/tests.rs`'s maximum sweep holding at every level and its unpaired-group test merging nothing; closes `KD43` | | [2026-10-31](../status/history/2026-10-31.md), "Seven `KD`s admitted as `M269`–`M274`" |
