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
- **`docs/design/architecture.md`, occurrence by occurrence.** It is the one
  file that spells *both* flags, so the sweep was a read-and-judge pass rather
  than a substitution: each of its 25 uses of the word was read in enough
  surrounding prose to say which program it names. **17 name `info`'s report
  and are now `--detail`**; **8 name `pg_dump`'s own `--verbose` and are
  untouched** — the `-- TOC entry`/`-- Dependencies:` lines and the comment
  block's variable height under "TOC enrichment", and the fixture section's
  `-- Started on` header lines, `objects/verbose.sql`, the regeneration-noise
  paragraph, the flag-set table's `verbose` flavor name and the sentence saying
  that flag is the `objects` set's whole reason. The separating criterion is
  what the sentence is *about*: a use describing what the dump file contains,
  or which fixture produced it, is `pg_dump`'s, and a use describing what pgdq
  prints is ours. Where the rename left a sentence that no longer said which
  program was meant, the referent was made explicit rather than left bare —
  `info --detail` in full at the start of the "CLI surface" and
  "Machine-readable resolution" claims, at the `Range<T>` substitution, at the
  enum-label error clause, and in the `--preamble-only` rejection, which now
  says `info`'s own `--detail`/`--map`.

**What was deliberately left alone, and why.** Two categories spell
`--verbose` and were left as found; two more spell it and were retargeted
anyway, each for its own reason:

- `pgdump_query/src/map.rs`, `pgdump_query/tests/map.rs`,
  `scripts/generate_fixtures.py` and `docs/design/pg-dump-compatibility.md`
  all spell **`pg_dump`'s own** `--verbose` flag — the one that widens a TOC
  comment block, source of the `objects` fixture's `verbose` flavor — which is
  a different flag on a different program and shares only the word.
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

One judgement call did rise to the maintainer, and was reversed:
`architecture.md` was first held back on the grounds that a 7,700-line document
mixing two flags of the same name has no mechanical way to tell them apart. The
answer is that "hard to tell apart" is a reason to read each occurrence, not a
reason to leave a stale spelling in the project's primary design document, so
the file was swept by hand and is listed above with the swept files. Every
other call — the two exclusions and the two retargets — follows from something
already written (the rule that a dated/historical record is not rewritten to
match later terminology, or — for `M67` and the notes docs — the distinction
between a document that states what was true on a closed day and one that is
still read for what is true now), so nothing else went to "Decisions worth
another look".

## What the next slice inherits

**`16.19` needs the word `verbose` free**, which this slice bought back: no
CLI flag, struct field, or function parameter in the tree spells it as
anything but a historical reference now. `16.19`'s `-vvv`/`--quiet` level
flags can use it without colliding with `info`'s meaning.

**`architecture.md` still spells `--verbose` in eight places, and every one of
them is `pg_dump`'s flag.** An editor of "TOC enrichment" or the `objects`
fixture rationale meets the word there and should leave it: it names the
external program's flag, and pgdq's report is `--detail` throughout the rest of
the file. A search for the bare word therefore no longer finds a stale
spelling of ours — which is what makes the next such rename a search rather
than a re-read.

## Figures

**Two declared paths moved, both already red.** `pgdump_query-cli/src/main.rs`
is a path every figure declaring it already carries red (`M69`'s worker-count
rationale, the CLI merge, the mapping leader); `scripts/measure.py` is a path
`session-drift` already holds red. No figure's *reachability* status changes:
this slice adds a renamed argument and renamed prose, no new executable
behaviour, so it is red on the same terms those two files already were and
adds no new reason of its own.
