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
   deficiencies", and "Decisions worth another look".
3. Read the current phase spec and the notes docs of the slices already
   landed in this phase.
4. Read `docs/design/out-of-band.md`, the **out-of-band ledger**, for the rows
   whose Date is empty. Read it whole — the rows are in allocation order, not
   landing order, so the outstanding ones are scattered through it and a
   partial read is how one goes unpicked-up.
5. **If the slice you pick takes, reads, or reasons from a measurement —
   invoke the `evidence` skill before designing it.** That includes a slice
   whose deliverable is a reading, one that fits a model to numbers already
   taken, and one that sets a constant from a figure. Its first rule is that an
   *account* is arithmetic from the source and a *fit* is not one, and applying
   it has already turned an hour-long sitting into a `grep`.

## Pick the work

**A blocking out-of-band row comes first; otherwise the next unticked slice.**

An out-of-band row blocks when its Date is empty and its Blocks column names
the open phase. Those exist because grilling a decision turns up a defect or a
clarification that changes no decision — so it lands out-of-band — and the
phase's remaining slices should not be built around it. Take the lowest such
`M<k>`. If several block, they are all in the way; take them in ledger order,
one to a round.

Otherwise: **the next unticked slice in the STATUS checklist, in order.** Not
the most interesting one, not a refactor you noticed on the way, not several at
once. If the next slice is already partly landed, its checklist entry says what
remains — finish that, not something else.

**Re-test an out-of-band row before building it, and stop if it fails.** The
ledger's admission rule is that an item changes no decision any spec records
*and* fits one session. The admitting session applied that test to a
description; you are the first to see the work itself. If it turns out to
change a decision, or not to fit, it was misfiled: leave the row alone, write
what you found under "Decisions worth another look", and stop. Do not widen it
into a slice on your own — that is the review the admission rule exists to
force.

**Re-test a *slice's premise* the same way, and this half is newer.** The rule
above has always covered ledger rows; a slice row is written at spec time and is
just as capable of resting on something that has since become false — a figure
re-taken, a term shown not to exist, a mechanism repaired. So before building,
ask what the row assumes and whether it is still true, and check the cheap ones:
the arithmetic, the code the row names, the commit a figure was taken at.

If the premise is false, **stop and do not build the row as written**. You are
unattended, so leave the tree alone and write it under "Decisions worth another
look" — what the row assumes, the artifact that falsifies it (file and line, a
figure's cell, a number in a log), and what the row would have to become. A
stand-in has a `refuted` verdict for exactly this and can settle it without the
maintainer; what it cannot do is settle one nobody wrote down.

> The instance: a slice was queued to choose a constant against a measured
> "fixed term", and the term was an artifact of the window two sittings had
> fitted over — falsified by a reading already in the same log. Building the
> row as written would have priced a quantity that does not exist.

An out-of-band round finishes differently from a slice: no notes doc and no
spec row, because the ledger line points at a history entry and that entry *is*
the notes. Fill the row's Date in, clear its Blocks column, and write the
history entry that says why.

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
  leaves the box empty with an honest entry beside it. An out-of-band round
  ticks nothing: its ledger row's Date is the equivalent.
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
