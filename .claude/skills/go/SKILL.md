---
name: go
description: Start a round of autonomous, unattended implementation work on the roadmap — pick up the next slice, or with no phase open the next out-of-band item, land it, update the docs, stop at the right boundary. Use when the user invokes /go, or asks for the next round of work to be done autonomously/unattended.
---

This session is **unattended**. Nobody is here to answer a question, review an
approach, or unblock you.

## Before touching anything

1. **Invoke the `process` skill** — first, not after you have decided what to
   build. It requires a full read of `docs/process.md` and names the obligations.
2. Read `docs/status/STATUS.md` — the phase checklist, "Not started" and
   "Decisions worth another look" — and the deficiency register beside it,
   `docs/status/deficiencies.md`.
3. Read the current phase spec and the notes docs of its landed slices, if a
   phase is open.
4. Read `docs/design/out-of-band.md` **whole**, for rows whose Date is empty;
   they are in allocation order, so a partial read is how one goes unpicked-up.
5. **If the slice takes, reads or reasons from a measurement, invoke the
   `evidence` skill before designing it** — including one whose deliverable is a
   reading, one fitting a model to numbers already taken, and one that sets a
   constant from a figure.

## Pick the work

**A blocking out-of-band row comes first; otherwise the next unticked slice.** A
row blocks when its Date is empty and its Blocks column names the open phase;
take the lowest such `M<k>`, one to a round. Otherwise take the next unticked
slice **in order** — not the most interesting one, not a refactor you noticed on
the way, not several at once; a partly-landed slice's checklist entry says what
remains, and that is what you finish.

**With no phase open — `STATUS.md` carries no checklist — the ledger is the
work**: take the lowest `M<k>` whose Date is empty, one to a round. Allocation
order is run order; the history entry a row points at says if it is not. An open
phase whose boxes are all ticked is a wrap waiting, not a licence to drain the
ledger: stop there.

**Re-test the premise before building, and stop if it fails.** For an out-of-band
row that is the admission rule — changes no decision any spec records, *and* fits
one session — which the admitting session applied to a description while you are
the first to see the work. For a slice row it is whatever the row assumes: check
the cheap ones, the arithmetic, the code the row names, the commit a figure was
taken at. Either way, if it fails, **leave the tree alone** and write it under
"Decisions worth another look" — what the row assumes, the artifact that
falsifies it (file and line, a figure's cell, a number in a log), and what the
row would have to become — then stop. Do not widen it into a slice on your own;
a stand-in can settle what is written down and nothing else.

If the slice looks mis-sized once you are inside it — a self-contained piece plus
a rework of something already tested — that is the seam: split it per
`docs/process.md`'s "Slice numbering", land the half you are confident in, and
leave the rest as an earned `<N>.<M>.<K>`.

An out-of-band round finishes differently — no notes doc and no spec row, the
ledger line pointing at a history entry that *is* the notes. Fill the row's Date
in, clear its Blocks column, and write that entry.

## Rules that bind an unattended session

- The three from `docs/process.md`, "Working unattended": **never mix
  high-confidence and low-confidence work in one review cycle**; **stop at the
  last clean boundary** rather than reworking a tested core path on a judgement
  call, finishing the unaffected work rather than treating that as blocked; and
  **put a judgement call you had to make anyway under "Decisions worth another
  look"** rather than into silence or a question nobody can answer.
- **Tick a box only when the whole spec row is delivered**; anything else leaves
  it empty with an honest entry beside it. An out-of-band round ticks nothing.
- **No `git commit`.** Leave the work in the tree for review.
- **Long-running jobs follow `CLAUDE.md`'s protocol**: detached, logging to
  `runs/`, never waited on, the log path recorded for a later session.

## Before you finish

Hunt every copy of any fact the change moved, by handle and by wording
(`docs/process.md`, "Repointing": subtract as you add). Run `mise run check
--affected` once, as the last thing that touches the tree, and report its
summary verbatim — it names every failing test, and the log it names answers
anything more, so it is never piped or re-run to read it. A cap `repoint.py` names fails it and is
this change's to fix; a red *meter* passes it and is not — a repoint is a round
of its own — so report it and stop there. Then
check the change carries everything the `process` skill lists: the notes doc, the `D<k>`
entries, the checklist ticked or honestly annotated, the spec untouched, and a
history entry only if a future session needs a pickup point or the plan changed.
