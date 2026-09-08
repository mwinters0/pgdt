# P16.18 — `info --verbose` becomes `--detail`

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md).

## What exists now

**The flag is `--detail` everywhere it names `info`'s per-block, per-column and
per-type report.** `pgdump_query-cli/src/main.rs`'s `Info` variant field is
`detail: bool` (clap's derive turns that into `--detail` on its own, the same
way `verbose: bool` produced `--verbose`), the `--json`/`--detail`/`--map`
conflict message names the new spelling, and every function that threads the
flag through (`info_offline`, `report`, `print_index`, `print_metadata`) took
the parameter's new name along with it. No behaviour changed: it is the same
`bool` reaching the same call sites.

**Nothing is aliased.** Pre-1.0, so `--verbose` is gone rather than kept as a
second spelling of `--detail` (`roadmap.md`, "Pre-1.0"); a script or habit
still passing `--verbose` gets clap's ordinary unknown-flag refusal.

## What was actually swept, and the boundary drawn around it

The spec's `16.18` row names four anchors — `main.rs`'s argument and the
`--json` conflict message, both manual files, and `measure.py`'s koji recipe
with the `test_measure.py` assertion over it — but "renamed wherever it is
spelled" is the operative instruction, and satisfying it for the *actual flag*
reached further than that enumeration by itself:

- **The CLI's own integration tests** (`partial_reporting.rs`, `xz_source.rs`,
  `perf_generator_fidelity.rs`) invoke the real binary with `--verbose` on the
  command line — left unrenamed, the flag's removal (no alias) would fail
  every one of them at the process boundary, not at compile time. Function and
  test names built from the old word moved with it:
  `verbose_column_lines` → `detail_column_lines`,
  `the_json_export_and_the_verbose_listing_agree_column_for_column` →
  `..._detail_listing_...`,
  `the_verbose_listing_names_every_user_defined_type` →
  `the_detail_listing_names_every_user_defined_type`.
- **Two library-side spellings of the flag in prose the binary itself emits**:
  `predicate.rs`'s enum-overflow message (`"...see `info --verbose`"`, and its
  literal-string test) and a doc comment in `pgtype.rs`. Both name the CLI flag
  by its old spelling and would otherwise tell a user to pass a flag that no
  longer exists.
- **Two live, present-tense assertions of what the shipped CLI accepts today**:
  `README.md`'s worked example and `STATUS.md`'s CLI capability-table row. Both
  state a real invocation; leaving either at `--verbose` would print a
  now-refused command from the project's own front door.
- **`CLAUDE.md`'s command reference and its koji wrap-verification
  paragraph** — the same reasoning as the manual: both are literal commands a
  session runs from this file, not commentary about one.

**What was deliberately left alone, and why.** Three categories spell
`--verbose` and were left as found; two more spell it and were retargeted
anyway, each for its own reason:

- `pgdump_query/src/map.rs`, `pgdump_query/tests/map.rs`,
  `scripts/generate_fixtures.py` and `docs/design/pg-dump-compatibility.md`
  all spell **`pg_dump`'s own** `--verbose` flag — the one that widens a TOC
  comment block, source of the `objects` fixture's `verbose` flavor — which is
  a different flag on a different program and shares only the word.
- `docs/design/architecture.md` mixes both referents throughout — `pg_dump`'s
  own `--verbose` (its TOC-comment behaviour, `objects` fixture rationale) and
  ours (`info --verbose`'s per-column and per-type report) — sometimes within
  the same paragraph. The spec's own enumeration of this slice's scope does not
  name it, and a blind sweep risks silently renaming the wrong referent in a
  7,700-line document with no mechanical way to tell the two apart. It is left
  spelling `--verbose` for our flag in roughly twenty places; a session that
  next edits one of those sections should read it against this note before
  trusting the prose.
- `docs/design/measurements.md`'s koji narrative and any closed-day history
  entry describe what was literally run *at the time*, under the flag name
  current then. `CLAUDE.md`'s writing-style rule is what draws this line: a
  dated entry states what was true on its date and is explicitly not
  maintained, so these are left spelling `--verbose` deliberately, not by
  oversight.

Two more spellings were retargeted despite looking, at first glance, like the
same kind of historical record — what decides retargeting is not whether the
sentence describes a past run, but whether the document itself is maintained:

- The out-of-band ledger's `M67` row is not a record of anything at all: it
  describes a diagnostic still to be built, so it is a forward-looking
  description of what `M67` will do, and that description should name the
  flag as it will exist when someone builds it, not the spelling that happened
  to be current when the row was written. It was retargeted to `info
  --detail` for that reason.
- `roadmap-P16.7.1-stated-budget-notes.md`'s line 83 and
  `roadmap-P16.14-koji-verification-notes.md`'s lines 35 and 171 read like the
  `measurements.md` koji narrative above — they narrate a specific past run —
  but a slice notes doc is not a dated entry: it is a **live** phase document,
  `citations.py` resolves citations inside it, and a session reads it for what
  the mechanism does *now*, not for what was true on the day it landed. A
  reader who copies `P16.14`'s recovery command should get one that runs, so
  all three lines were retargeted to `info --detail` too.

No judgement call here rose to a maintainer decision: the three exclusions and
the two retargets above all follow from something already written (the spec's
own scope, the rule that a dated/historical record is not rewritten to match
later terminology, or — for `M67` and the notes docs — the distinction between
a document that states what was true on a closed day and one that is still
read for what is true now), so nothing went to "Decisions worth another
look".

## What the next slice inherits

**`16.19` needs the word `verbose` free**, which this slice bought back: no
CLI flag, struct field, or function parameter in the tree spells it as
anything but a historical reference now. `16.19`'s `-vvv`/`--quiet` level
flags can use it without colliding with `info`'s meaning.

**`architecture.md`'s mixed `--verbose` mentions are unresolved**, listed above
by file. Nothing obliges sweeping them before `16.19` lands, since `16.19`
introduces new prose of its own rather than editing the existing sections that
mix the two referents — but a future editor of "CLI surface" or the `objects`
fixture rationale should expect to find both spellings there and disambiguate
by content, not by search-and-replace.

## Figures

**Two declared paths moved, both already red.** `pgdump_query-cli/src/main.rs`
is a path every figure declaring it already carries red (`M69`'s worker-count
rationale, the CLI merge, the mapping leader); `scripts/measure.py` is a path
`session-drift` already holds red. No figure's *reachability* status changes:
this slice adds a renamed argument and renamed prose, no new executable
behaviour, so it is red on the same terms those two files already were and
adds no new reason of its own.
