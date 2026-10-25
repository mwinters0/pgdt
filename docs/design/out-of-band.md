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

**M1–M238 are struck**, and nothing at or below `M238` is reused. That is a
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
| M239 | 2026-10-20 | A filter literal's equality form moves from pgdt-query's `predicate/resolve.rs` to pgdt-codec's `key/equality.rs`, nested in `Comparison` as `Comparison::Equal`; priced by `--count-pair` | | [2026-10-20](../status/history/2026-10-20.md), "`M239` admitted: a filter literal's equality form, grilled with the maintainer" |
| M240 | 2026-10-20 | arm64's release archive is built on the maintainer's machine in the release image and attached to CI's draft by a new `release_ci.py attach`, which rewrites `SHA256SUMS` over both and reports whether CI's x86-64 archive matches the rehearsal's, attested by GitHub's release attestation alone; x86-64 stays CI-built with build provenance; the arm64 smoke moves to `release-publish`, on real arm64 against the draft's asset; `/release`, `installing.md`'s "Verify", a `D<k>` and a new `release-invariants.md` follow | | [2026-10-20](../status/history/2026-10-20.md), "`M240` admitted: arm64's archive built here, attested by its release alone, chosen with the maintainer" |
| M241 | 2026-10-21 | The release suite is the round's less the members a manifest marks `[package.metadata.pgdt] apparatus`, `count-instructions` and `peak-rss`; `release_ci.py perf-events` and its workflow steps go; `D110` | | [2026-10-21](../status/history/2026-10-21.md), "`count-instructions`' `perf_event_open` ENOENT on CI" |
| M242 | 2026-10-21 | A miss on a block another reader is decoding waits for that decode (`BlockCache::claim`, one lock with the retention list) instead of repeating it, on both `.xz` sources; `KD20` struck, `KD111`'s candidate gone from the code | | [2026-10-21](../status/history/2026-10-21.md), "`M242`: a block in flight is decoded once" |
| M243 | 2026-10-21 | `measure.mount_preflight` reads a nonce back from each staging directory inside a container and refuses a sitting whose container sees another copy; `CLAUDE.md`'s "Long-running processes" watches a launch to its first timed reading before reporting it | | [2026-10-21](../status/history/2026-10-21.md), "`M242`: a block in flight is decoded once" |
| M244 | 2026-10-22 | CMake is a `mise.toml` pin, so the release image installs it; `release.py build` refuses a build in which a package of `pgdt`'s normal-dependency closure left a `CMakeCache.txt`, the check replacing the absence `M218` relied on; the Dockerfile's and `pgdt-io`'s `object_store` notes follow, and `D111` | | [2026-10-22](../status/history/2026-10-22.md), "The zlib-ng leg, reviewed"; "`M244`: CMake pinned, the shipped build checked" |
| M245 | 2026-10-25 | `Extent::known()` struck, no production caller; its unit test's assertion goes with it | | [2026-10-22](../status/history/2026-10-22.md), "15.3's open extent, reviewed"; [2026-10-25](../status/history/2026-10-25.md), "`M245`: `Extent::known()` struck" |
| M246 | 2026-10-25 | Every workspace member's root declares itself on `unsafe` — `forbid(unsafe_code)` where it has none, `deny` with an `allow` at each necessary site, a crate whose only site is `introspect`'s forbidding it without the feature — and a check asserts each root does; `D27` cites the declarations; the check is `layering.py`'s | | [2026-10-23](../status/history/2026-10-23.md), "`gz-seek`'s backend and `unsafe`, reviewed"; [2026-10-25](../status/history/2026-10-25.md), "`M246`: every root declares itself on `unsafe`" |
| M247 | 2026-10-24 | Each workspace library is linted alone, under its own features, by the round's `alone` check (`check.py --alone`, `--affected` naming each library it reaches); what one feature reaches is gated on it — the gzip first pass's fetched arm and `walk_fetched` on `http`, `one_group_bounds` on `test-support` | | [2026-10-24](../status/history/2026-10-24.md), "`M247`: each library linted alone" |
| M248 | 2026-10-25 | The round lints every build the project makes or names to an embedder: the `alone` check gains named per-package feature sets beside `ALONE_BUILDS`, each at both builds and run by `--affected` where it reaches the package — the instrument (`pgdt/introspect` over the workspace), `pgdt`'s `jemalloc` and `system` legs, `gz-seek`'s `zlib-ng` and `zlib` legs, `datafusion-pgdump`'s `http` — and `test_check.py` asserts off `cargo metadata` that every feature a member declares is turned on by some linted build or exempted in the table with its reason | | [2026-10-24](../status/history/2026-10-24.md), "Lint coverage by feature, reviewed"; [2026-10-25](../status/history/2026-10-25.md), "`M248`: every build linted" |
| M249 | 2026-10-25 | A `gz-seek` unit test on a level-1 file moves a `Start::Symbol` checkpoint in an index handed to `Index::from_parts` and shows each move refused: its plaintext by ±1 by the unit ending there (`UnitCheck`), its bit by the unit it begins; D118's Evidence names it | | [2026-10-25](../status/history/2026-10-25.md), "15.10's static-block unit end, reviewed"; "`M249`: a symbol checkpoint moved, refused" |
| M250 | | The repoint's leftovers: the read-as-NULL note and `Error::Unrepresentable` say "this query cannot represent", the `.xz` non-seekable warning says a read behind another decodes it again from byte 0 unless its one block is held, their asserted texts and the manual's quoted line with them; the gzip serde refusal test builds its damage from the real persisted parts and asserts `from_parts` refused it; the two `.xz` file tests assert the block path and the gzip arms test each arm's through `partitions`; `gzip_levels`' comment says what is asserted | | [2026-10-25](../status/history/2026-10-25.md), "Repoint at `949111d6`: what it left uncorrected" |
| M251 | | `[workspace.lints.rust] unsafe_code = "deny"` is every target's floor, every member inheriting it and `layering.py` asserting each does; the four test and bench sites gain an `allow(unsafe_code)`; a shipped root declares only `forbid`, or `forbid` without `introspect`, its `deny` lines gone, and `layering.py` refuses one that does not `forbid` with no `allow` in its production tree; `D27`'s line follows | | [2026-10-25](../status/history/2026-10-25.md), "`M246`'s non-shipped roots, reviewed" |
