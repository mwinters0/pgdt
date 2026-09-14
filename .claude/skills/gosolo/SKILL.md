---
name: gosolo
description: Carry a whole phase unattended — land slices with gosub, and when a round raises a decision, settle it with the grillmaster instead of stopping for the maintainer. Use when the user invokes /gosolo, or asks for a phase to be run start to finish without them.
---

`/gosub` stops the moment a round raises something the maintainer should weigh;
`/gosolo` answers it with `/gm` and keeps going, so a phase runs from its first
unticked box to its last. `/gosolo [max-rounds]` — cap **10**, counting
implementation rounds, not grilling or repoint rounds. Which loop to run is the maintainer's
call at invocation and the only difference between them, so **say which one you
are in at the start of the final report**.

## Run `gosub`, with two overrides

**Invoke the `gosub` skill (Skill tool, `skill: "gosub"`) and follow it as
written** — the round, the dispatch prompt, the verification, the commit shapes,
the long-job protocol and every stop condition. Do not restate it and do not
diverge. Four of its stop conditions become transitions; nothing else changes.

**Override 1 — a new entry under "Decisions worth another look" is a transition,
not a stop.** Commit the round if it is clean, then dispatch a grillmaster to
settle the frontier. `gosub`'s step 5 still runs first and is unchanged: an entry
that is purely in-phase scheduling or labelling is settled and deleted there and
never reaches this override. When a grillmaster returns having committed its
closures, resume at step 1 of "One round" — a fresh baseline, the ledger having
possibly grown a blocking row.

**Dispatch it, never invoke it.** A fresh `Agent`, `general-purpose`, no `model`
override, **never a fork**, with this prompt and nothing else: *Invoke the `gm`
skill (Skill tool, `skill: "gm"`) and follow it exactly.* Invoking `gm` here
would make *you* the grillmaster and land every grilling round in the context
that has to survive the whole phase.

**A grilling may take more than one grillmaster.** `/gm` retires itself at a
rotation boundary and returns saying the frontier is not empty and that it
committed nothing — neither a stop nor a failure. Dispatch another with the same
prompt until one returns an empty frontier and a commit; each is genuinely new,
so **never `SendMessage` to a spent one**, pass nothing forward, and do not
summarise the previous segment. The tree is dirty between segments, which is not
`gosub`'s dirty tree: **never dispatch an implementation round while a grilling
is unfinished.**

**Override 2 — a split is a transition too, and `/gm` grills it whether or not
the round filed an entry.** Commit the landed half if the checklist reflects the
split, its box is ticked and all four checks pass, then dispatch a grillmaster
with the split appended to its prompt as an agenda item, stating the original
slice, the earned `<N>.<M>.<K>` and the seam the subagent named. If no box was
ticked at all, that is not a split but `gosub`'s ordinary unticked-box stop.

**Override 3 — a red `repoint.py` meter is a transition.** Commit the round if
it is clean, then dispatch a fresh `general-purpose` agent, no `model`, never a
fork, with this prompt and nothing else: *Invoke the `repoint` skill (Skill
tool, `skill: "repoint"`) and follow it exactly, including "Driven
unattended".* When it returns having committed, resume at step 1.

**Override 4 — the amendment-chain stop is a transition to `/gm`**, with one
agenda item appended to the grillmaster's prompt: *enumerate the whole account
behind `<the row, constant or clause>` before it is amended again* — the
`evidence` skill's first rule applied to a gate. An agenda item under override
2 or 4 is the only thing a grillmaster's prompt ever carries beyond the skill
invocation.

Everything else in `gosub` stands unchanged, **including that a phase boundary is
a stop**: the next phase needs grilling and a spec, which are the maintainer's.
Finishing the phase is how `/gosolo` is meant to end.

**`/gm` escalating ends the loop** — the maintainer arriving in the only way this
loop admits. Take nothing further: `/gm` will have left its tree untouched, and
that state is what they need to see, so do not close the other entries yourself,
start another round, or re-dispatch `/gm` with different framing.

## Keep a resume log

You are the one thing here that lives for the whole phase, and everything needed
to resume is committed except the narrative. Write it as you go, in
`/mnt/ssd/fedora/scratch/pgdump_query/grilling/P<N>-loop.md` beside the
transcripts, **one line per round appended as that round's commit lands**:
`| Round | Work | Commit | Grilling | Verdicts |`. `Work` is the slice number or
the `M<k>`; `Grilling` names the transcript segments the round's review produced,
or is empty; `Verdicts` carries that grilling's `record`/`judgement`/`misfiled`
counts. A `/gosolo` started fresh mid-phase reads this file, the STATUS checklist
and the ledger, and can both continue and write a truthful final report.

## The final report

`gosub`'s, plus: that it was `/gosolo`, the round cap and the resume log's path;
each grilling's settled entries, verdict counts, commit and how many `/gm`
segments it took; the transcript paths, that reading being the review this loop
deferred rather than skipped; out-of-band rows admitted along the way with
numbers and Blocks, and which later rounds picked them up; and, on an escalation,
the question in full and what `/gm` read before deciding it could not answer.
State the verdict counts plainly: a phase answered almost entirely on `record`
ran on precedent, one with a lot of `judgement` improvised.
