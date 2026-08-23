---
name: go
description: Start a round of autonomous, unattended implementation work on the roadmap — pick up the next slice, land it, update the docs, stop at the right boundary. Use when the user invokes /go, or asks for the next round of work to be done autonomously/unattended.
---

This session is **unattended**. The maintainer is not here to answer a
question, review an approach, or unblock you. Work accordingly.

## Before touching anything

1. **Invoke the `process` skill.** It requires a full read of
   `docs/process.md` and names what landing a slice obliges. Do this first,
   not after you have decided what to build.
2. Read `docs/status/STATUS.md` — the phase checklist, "Not started", "Known
   gaps", and "Decisions worth another look".
3. Read the current phase spec and the notes docs of the slices already
   landed in this phase.

## Pick the work

**The next unticked slice in the STATUS checklist, in order.** Not the most
interesting one, not a refactor you noticed on the way, not several at once.
If the next slice is already partly landed, its checklist entry says what
remains — finish that, not something else.

If the next slice looks mis-sized once you are inside it — a self-contained
piece plus a rework of something already tested — that is the seam. Split it
per the process doc's slice-numbering rule, land the half you are confident
in, and leave the rest as an earned `<N>.<M>.<K>`. Do not land both halves
because they were written on one line of a table.

## Rules that bind an unattended session

- **Never mix high-confidence and low-confidence work in one review cycle.**
  This is the one that matters most; the process doc's "Working unattended"
  section explains why.
- **Stop at the last clean boundary** rather than reworking a tested core path
  on a judgement call.
- **A judgement call you had to make anyway goes in STATUS's "Decisions worth
  another look"** — not into silence, and not into a question nobody is here
  to answer. Proceeding and flagging is the intended third option.
- **Tick a box only when the whole spec row is delivered.** Anything else
  leaves the box empty with an honest entry beside it.
- **No `git commit`.** Leave the work in the tree for review.
- **Long-running jobs follow `CLAUDE.md`'s protocol**: detached, logging to
  `runs/`, never waited on, with the log path recorded for a later session.

## Before you finish

Verify the build and tests actually pass (`cargo test --workspace`,
`cargo clippy --workspace`, `cargo fmt --check`) and report the real result —
a failing test named as failing, not smoothed over.

Then make sure the change carries everything the `process` skill lists: the
slice's notes doc, the STATUS checklist ticked or honestly annotated, the
phase spec untouched, and a history entry only if a future session needs a
pickup point or the plan actually changed.
