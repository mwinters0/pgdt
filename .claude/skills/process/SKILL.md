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
- The current phase spec, `docs/design/roadmap-phase<N>-<slug>.md` — the
  binding statement of what this phase committed to.
- `docs/status/STATUS.md` — what exists right now, and the phase's slice
  checklist.
- The notes docs for slices of this phase that already landed — they carry
  what this slice inherits.

## What landing a slice obliges

Every one of these belongs in the *same* change as the code, not a follow-up:

1. **A notes doc**, `roadmap-phase<N>.<M>-<slug>-notes.md` — even for a slice
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
