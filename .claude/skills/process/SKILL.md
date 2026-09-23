---
name: process
description: The phased development process this project runs on — the doc set, where each fact goes, and the obligations that come with landing work. Use whenever implementing a roadmap phase or slice, wrapping one up, repointing the record, writing or revising a phase spec or notes doc, or updating STATUS. Not needed for planning, grilling, or ad-hoc exploration.
---

**Read `docs/process.md` in full before doing anything else** — with `Read`, no
`offset`/`limit`, no grepping for the part that seems relevant. It is the source
of truth for everything below; this skill only names the obligations.

Then, for the work at hand: `docs/design/roadmap.md` (phase index, standing
rules); the phase spec `docs/design/roadmap-P<N>-<slug>.md`, binding what the
phase committed to; `docs/status/STATUS.md`; this phase's landed slices' notes;
the `docs/design/decisions.md` entries for every mechanism the slice touches; and
`docs/design/roadmap-P<N>-<slug>-inbox.md` if one exists — **specifying or
grilling a phase drains its inbox**, folding each entry into the spec or
discarding it as stale, then deleting the file.

## What landing a slice obliges

Every one of these is in the *same* change as the code, never a follow-up.

1. **A notes doc**, `roadmap-P<N>.<M>-<slug>-notes.md`, even for a slice landing
   no code: what the next slice inherits and the negative results — not a
   restatement of the spec, not a changelog.
2. **A `D<k>` entry** in `decisions.md` for each decision the code cannot
   explain, **cited from the code comment in one line** that never re-argues it
   or quotes a measured number. The register is capped, so an entry added may
   mean one pruned.
3. **The STATUS checklist ticked**, linking the notes doc, plus any change to
   "Not started" or the deficiency register the slice caused.
4. **The phase spec left untouched** — progress never goes there; the spec
   changes only when a *decision* does, with the reasoning in a history entry.
5. **A history entry** only for a pickup point or a discovery that changed the
   plan, never routine progress, and **short**: under its cap, of pointers
   and settled facts, reasoning filed beside its mechanism, evidence cited.
6. **An invariants-register entry** if the slice made a decision depend on
   external behaviour not already recorded there.
7. **An inbox entry** for each fact a phase with no spec yet will need — the
   fact, why *that* phase cares, where it came from — filed now, not at wrap.
8. **The deficiency register** (`docs/status/deficiencies.md`) **re-read** if
   the checklist line names a `KD<k>`: closing its last part strikes index line
   and code-marker detail together in this change, a part closing into a
   *property* migrating beside its mechanism; partial closure rewrites the
   entry to what is still true and never annotates it with what was fixed.

## Writing or re-slicing a slice list

**Writing the checklist sets the phase's roadmap index row to `Current`**, in the
same change. **Check the deficiency register for `(b)` entries naming this
phase** and make entry and slice name each other — the only moment anyone will
think to. **A slice that splits re-targets every entry pointing at it**;
`deficiencies.py` fails until it lands.

## Wrapping a phase

Three edits in one change, each of the first two lying without the third: the
checklist **deleted** and its slice notes consolidated; the index row set to
**`Complete`**; **every `(b)` entry the phase owned re-homed**, onto the phase
that absorbs it or down to `(c) unowned`.

## A slice that cannot be finished as specified

Do not tick it and describe the gap in prose. Leave the box unchecked, say in the
checklist what landed and what did not, and record the reasoning where
`docs/process.md`'s "Where does this fact go?" sends it. A slice that shipped the
wrong contract earns an `<N>.<M>.<K>` follow-up; one that shipped part of its
contract is still that slice, unfinished.

## Repointing

`docs/process.md`, "Repointing" is the third hygiene beside the wrap and the
keystone, and the `repoint` skill is its procedure. What it obliges of *every*
landing is its middle: **subtract as you add** — hunt every copy of a fact the
change moves, by handle or by wording — and land under the caps
`scripts/repoint.py` asserts, never raising one.

## Working unattended

Read `docs/process.md`, "Working unattended" — the section that most often
applies, and its sub-rules before writing or closing an entry under STATUS's
**"Decisions worth another look"**, which is capped at five.
