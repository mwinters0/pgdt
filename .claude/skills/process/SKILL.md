---
name: process
description: The phased development process this project runs on — the doc set, where each fact goes, and the obligations that come with landing work. Use whenever implementing a roadmap phase or slice, wrapping one up, writing or revising a phase spec or notes doc, or updating STATUS. Not needed for planning, grilling, or ad-hoc exploration.
---

**Read `docs/process.md` in full before doing anything else** — the whole file,
with `Read`, no `offset`/`limit`, no grepping for the part that seems
relevant. It is the source of truth for everything below; this skill only
names the obligations and points at it.

Then read, for the work at hand:

- `docs/design/roadmap.md` — the phase index and the standing policies.
- The current phase spec, `docs/design/roadmap-P<N>-<slug>.md` — the
  binding statement of what this phase committed to.
- `docs/status/STATUS.md` — what exists right now, and the phase's slice
  checklist.
- `docs/design/roadmap-P<N>-<slug>-inbox.md`, if one exists — facts an earlier
  phase filed for this one. **Specifying or grilling a phase means draining
  its inbox**: fold each entry into the spec or discard it as stale, then
  delete the file.
- The notes docs for slices of this phase that already landed — they carry
  what this slice inherits.

## What landing a slice obliges

Every one of these belongs in the *same* change as the code, not a follow-up:

1. **A notes doc**, `roadmap-P<N>.<M>-<slug>-notes.md` — even for a slice
   that lands no code. Written for the *next* slice: what it inherits, the
   non-obvious calls and why. Not a restatement of the spec, not a changelog.
2. **The STATUS checklist ticked** for that slice, linking its notes doc, plus
   any change to "Not started" / "Known gaps" the slice caused.
3. **The phase spec left untouched.** Progress never goes in the spec — no ✅,
   no "deferred" annotations, no rewritten slice rows. The spec changes only
   when a *decision* changes, and then the reasoning goes in a history entry.
4. **A history entry** (`docs/status/history/<today>.md`) only for the two
   things it is for: what a future session must pick up mid-work, and a
   discovery that changed the plan. Not routine progress.
5. **An invariants-register entry** if the slice made a decision depend on
   external behaviour that was not already recorded there.
6. **An inbox entry** in `roadmap-P<M>-<slug>-inbox.md` for each fact the slice
   turned up that a phase with no spec yet will need — filed now, not at wrap,
   because that is when you know it. The fact, why *that* phase cares, and
   where it came from; if you can't name why that phase cares, it isn't one.
   Most forward-looking remarks belong somewhere else — see `docs/process.md`,
   "Inboxes: facts filed by destination".
7. **The deficiency register re-read**, if the slice's checklist line names a
   `KD<k>`. Closing its last part means **striking the entry in this same
   change** — index line, detail paragraph, code marker — and a part that
   closed into a *property* migrates beside its mechanism rather than being
   deleted. Closing only some parts rewrites the entry to what is still true;
   it is never annotated with what was fixed.

## When you write or re-slice a phase's slice list

Two obligations that fire at spec time, not at landing, and both are about
`STATUS.md`'s deficiency register:

- **Check the register for `(b)` entries naming this phase**, and make the
  entry and the slice that will close it name each other. This is the only
  moment anyone will think to; the entry may have been written by a session
  months earlier.
- **A slice that splits re-targets every entry pointing at it.** The pointer
  going stale is the failure the pairing exists to catch, and
  `deficiencies.py` fails until the re-target lands — see `docs/process.md`,
  "Known deficiencies".

## When you wrap a phase

Three edits are one change, because each of the first two makes the register
lie without the third:

- **The phase's `STATUS.md` checklist is deleted**, its slice notes
  consolidated.
- **The roadmap index row goes to `Complete`.** That cell is the only thing
  left that can say the phase ran and finished, and `deficiencies.py` reads it.
- **Every `(b)` entry the phase owned is re-homed.** A finished phase is not a
  destination: the entry either names the phase that actually absorbs it, or it
  drops to `(c) unowned` — which is a legitimate resting state and the right
  one unless somebody genuinely holds the intent. `deficiencies.py` fails until
  it does.

## What to do when a slice cannot be finished as specified

Do not tick it and describe the gap in prose. Leave the box unchecked, say in
the checklist what landed and what did not, and record the reasoning where the
process doc's "Where does this fact go?" table sends it. A slice that shipped
the wrong contract earns an `<N>.<M>.<K>` follow-up; a slice that shipped only
part of its contract is still that slice, unfinished.

## Working unattended

Read the process doc's "Working unattended" section — it is the one that most
often applies. In short: never put high-confidence and low-confidence work in
one review cycle; stop at the last clean boundary rather than reworking a
tested core path on a judgement call; and put every call the maintainer should
weigh into STATUS's **"Decisions worth another look"**, which exists precisely
so that proceeding-and-flagging beats both stalling and staying silent.

That section is **capped at five entries**, and an entry is closed by the
session that hears the maintainer's answer — filed, then deleted, never edited
to record that it was reviewed. An affirmed call that changed nothing has its
reasoning written beside the mechanism it governs *before* the entry goes; that
is the only case whose content lives nowhere else. Read the process doc's
sub-rules under that heading before writing an entry or closing one.
