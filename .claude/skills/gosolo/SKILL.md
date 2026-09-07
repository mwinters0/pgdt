---
name: gosolo
description: Carry a whole phase unattended — land slices with gosub, and when a round raises a decision, settle it with the grillmaster instead of stopping for the maintainer. Use when the user invokes /gosolo, or asks for a phase to be run start to finish without them.
---

`/gosub` stops the moment a round raises something the maintainer should weigh.
`/gosolo` answers it instead, with `/gm`, and keeps going — so a phase runs from
its first unticked box to its last without them.

`/gosolo [max-rounds]` — the cap defaults to **10**, and counts implementation
rounds, not grilling rounds.

## Which loop to run

**`/gosub` when you want to stay close to the work**; `/gosolo` when the phase's
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
transition, not a stop.** That entry is the round asking for review, and review
is available: commit the round if it is clean, then dispatch a grillmaster and
let it settle the frontier.

`gosub`'s step 5 still runs first and is unchanged here: an entry that is purely
in-phase scheduling or labelling is settled and deleted there, so it never
reaches this override and costs no grillmaster. What this override picks up is
what step 5 left standing — the entries that are actually design calls. When one returns having committed its closures,
resume at step 1 of `gosub`'s "One round" — a fresh baseline, because the
frontier is now empty and the ledger may have grown a blocking row that the
next round will pick up ahead of the next slice.

**Dispatch it, never invoke it.** A fresh `Agent`, `general-purpose`, no
`model` override, **never a fork**, with this prompt and nothing else:

> Invoke the `gm` skill (Skill tool, `skill: "gm"`) and follow it exactly.

Invoking `gm` as a skill here would make *you* the grillmaster, and every round
of every grilling would land in the context that has to survive the whole
phase — which is the accumulation this design exists to bound. The grillmaster
is an agent you spend and replace, exactly as `gosub` spends one per
implementation round.

**A grilling may take more than one grillmaster.** `/gm` retires itself at a
rotation boundary rather than adjudicating on a degraded context, and returns
saying the frontier is not empty and that it committed nothing. That is not a
stop and not a failure: dispatch another with the same prompt, and keep doing so
until one returns with an empty frontier and a commit. Each is genuinely new —
never `SendMessage` to a spent one, which resumes the context you just retired.
Pass nothing forward and do not summarise the previous segment: the fresh one
picks the work up from the repo, where the frontier is shorter and the closures
so far are already written. Feeding context forward is the rotation undone.

**The tree is dirty between segments, and that is not `gosub`'s dirty tree.**
`/dwal` commits nothing while the frontier is non-empty, so a rotation leaves
written, uncommitted closures behind. `gosub`'s baseline check — a dirty tree
means the previous round did not finish — is about an **implementation** round
and stands exactly as written. Keeping the two apart is your job and it is
simple: **never dispatch an implementation round while a grilling is
unfinished.** The grilling is a loop, and it exits only when a `/gm` returns
with an empty frontier and a commit hash.

**Override 2 — a split is a transition too, and `/gm` grills it whether or not
the round filed an entry.** `/gosub` stops on an unticked box after a split
because a split is a re-plan and the next slice may no longer be the right one.
That reasoning is sound and it is exactly what a grilling round is for, so here
the re-plan gets reviewed rather than parked. Commit the landed half if the
checklist reflects the split, the landed half's box is ticked and all three
checks pass — then dispatch a grillmaster with the split appended to its prompt
as an agenda item alongside whatever is under "Decisions worth another look",
stating the original slice, the earned `<N>.<M>.<K>`, and the seam the subagent
named. That is the one thing a grillmaster's prompt ever carries beyond the
skill invocation, and it is a pointer at work in the tree rather than context
being fed forward. If the tree has no ticked box
at all, that is not a split; it is `gosub`'s ordinary unticked-box stop.

Everything else in `gosub` stands unchanged — a failing check, a tree that did
not change, the round cap, a subagent that says it needs the maintainer, and
the phase boundary. **A phase boundary is still a stop**, including for this
loop: the next phase needs grilling and a spec, and those are the maintainer's.
Finishing the phase is how `/gosolo` is meant to end.

## `/gm` escalating ends the loop

`/gm` escalates when a call binds beyond the open phase — a standing rule, an
invariant, a `KD<k>` re-targeted outside the phase, a spec rationale reversed —
or when it simply cannot settle the question. That is the maintainer arriving
in the only way this loop admits, so stop there.

Take nothing further: `/dwal`'s rule is that a round with a non-empty frontier
commits nothing, so `/gm` will have left its tree untouched, and that state is
what the maintainer needs to see. Do not close the other entries yourself, do
not start another round, and do not re-dispatch `/gm` with different framing.

## Keep a resume log

You are the one thing in this loop that lives for the whole phase, and a phase
is many rounds. Everything you would need to resume is already committed except
the narrative, so write that down as you go rather than carrying it:
`/mnt/ssd/fedora/scratch/pgdump_query/grilling/P<N>-loop.md`, beside the
transcripts, **one line per round**:

```
| Round | Work | Commit | Grilling | Verdicts |
```

`Work` is the slice number or the `M<k>`; `Grilling` names the transcript
segments a round's review produced, or is empty; `Verdicts` carries that
grilling's `record`/`judgement`/`misfiled` counts. Append the line as each
round's commit lands, never at the end.

That makes you disposable on the same terms as everything else here. A `/gosolo`
started fresh mid-phase reads this file, the STATUS checklist and the ledger,
and can both continue and write a truthful final report — which is the whole
reason the log exists rather than a supervising agent above you.

## The final report

`gosub`'s final report, plus what this loop adds:

- That it was `/gosolo` rather than `/gosub`, the round cap, and the resume
  log's path.
- Each grilling: which entries were settled, the verdict counts
  (`record` / `judgement` / `misfiled`), the commit, and how many `/gm`
  segments it took — a grilling that rotated three times is a long frontier or
  a heavily-researched one, and worth seeing.
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
before the next `/gosolo`.
