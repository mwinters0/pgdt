---
name: gogm
description: Carry a whole phase unattended — land slices with gosub, and when a round raises a decision, settle it with the grillmaster instead of stopping for the maintainer. Use when the user invokes /gogm, or asks for a phase to be run start to finish without them.
---

`/gosub` stops the moment a round raises something the maintainer should weigh.
`/gogm` answers it instead, with `/gm`, and keeps going — so a phase runs from
its first unticked box to its last without them.

`/gogm [max-rounds]` — the cap defaults to **10**, and counts implementation
rounds, not grilling rounds.

## Which loop to run

**`/gosub` when you want to stay close to the work**; `/gogm` when the phase's
implementation looks straightforward and the value of the loop is that it keeps
moving. That is the maintainer's call at invocation and it is the only
difference between them: both run the same rounds, the same independent
verification, the same commits, and stop on everything except the two
conditions below.

Say which one you are in at the start of the final report, since the two
produce the same-looking history.

## Run `gosub`, with two overrides

**Invoke the `gosub` skill (Skill tool, `skill: "gosub"`) and follow it as
written.** It holds the round, the dispatch prompt, the independent
verification, the commit shapes, the long-job protocol, and every stop
condition. Do not restate it and do not diverge from it. Two of its stop
conditions become transitions, and nothing else changes.

**Override 1 — a new entry under "Decisions worth another look" is a
transition, not a stop.** That entry is the round asking for review, and
review is available: commit the round if it is clean, then invoke the `gm`
skill (Skill tool, `skill: "gm"`) and let it settle the frontier. When it
returns having committed its closures, resume at step 1 of `gosub`'s "One
round" — a fresh baseline, because the frontier is now empty and the ledger may
have grown a blocking row that the next round will pick up ahead of the next
slice.

**Override 2 — a split is a transition too, and `/gm` grills it whether or not
the round filed an entry.** `/gosub` stops on an unticked box after a split
because a split is a re-plan and the next slice may no longer be the right one.
That reasoning is sound and it is exactly what a grilling round is for, so here
the re-plan gets reviewed rather than parked. Commit the landed half if the
checklist reflects the split, the landed half's box is ticked and all three
checks pass — then hand `/gm` the split as an agenda item alongside whatever is
under "Decisions worth another look", stating the original slice, the earned
`<N>.<M>.<K>`, and the seam the subagent named. If the tree has no ticked box
at all, that is not a split; it is `gosub`'s ordinary unticked-box stop.

Everything else in `gosub` stands unchanged — a failing check, a tree that did
not change, the round cap, a subagent that says it needs the maintainer, and
the phase boundary. **A phase boundary is still a stop**, including for this
loop: the next phase needs grilling and a spec, and those are the maintainer's.
Finishing the phase is how `/gogm` is meant to end.

## `/gm` escalating ends the loop

`/gm` escalates when a call binds beyond the open phase — a standing rule, an
invariant, a `KD<k>` re-targeted outside the phase, a spec rationale reversed —
or when it simply cannot settle the question. That is the maintainer arriving
in the only way this loop admits, so stop there.

Take nothing further: `/dwal`'s rule is that a round with a non-empty frontier
commits nothing, so `/gm` will have left its tree untouched, and that state is
what the maintainer needs to see. Do not close the other entries yourself, do
not start another round, and do not re-dispatch `/gm` with different framing.

## The final report

`gosub`'s final report, plus what this loop adds:

- That it was `/gogm` rather than `/gosub`, and the round cap.
- Each grilling: which entries were settled, the verdict counts
  (`record` / `judgement` / `misfiled`), and the commit.
- The transcript paths, so the maintainer can read the whole exchange
  asynchronously — that reading is the review this loop deferred rather than
  skipped, and the report is where they find out it is waiting.
- Out-of-band rows admitted along the way, with numbers and Blocks columns, and
  which of them later rounds picked up.
- If it stopped on an escalation: the question in full, and what `/gm` read
  before deciding it could not answer.

The verdict counts are the part to state plainly rather than bury. A phase
answered almost entirely on `record` ran on precedent; a phase with a lot of
`judgement` in it improvised, and the maintainer should read that transcript
before the next `/gogm`.
