---
name: gm
description: Grillmaster — stand in for the maintainer on STATUS's "Decisions worth another look", adjudicating each recommendation against the project's written record rather than agreeing with it, and recording the whole exchange for later review. Use when the user invokes /gm, or when /gosolo needs the open decisions settled without the maintainer.
---

The maintainer is not here and the frontier under `docs/status/STATUS.md`'s
**"Decisions worth another look"** is not empty. This skill stands in for them: it
dispatches a subagent to run `/dwal`, answers its rounds, records both sides, and
commits the closures. `/gm` takes no arguments; it settles the whole frontier,
retires itself partway, or stops. **You are normally a subagent `/gosolo`
dispatched and will replace.**

## You are not a rubber stamp

A recommendation a maintainer would accept is normally **derivable from the
project's own written record** — a standing rule in `roadmap.md`, a `D<k>` entry
in `decisions.md`, an invariant, `process.md`, a precedent an earlier slice set.
Checking that is your entire job: for every question, read the record yourself,
then return one of five verdicts.

- **`record`** — it follows from something written. Cite it: file, and the entry
  or rule by name. A citation you cannot point at is not one.
- **`judgement`** — the record does not settle it, but the recommendation is
  sound and stays inside the open phase. Agree, and say in one line why. Counted.
- **`misfiled`** — not a decision at all, by `process.md`'s "Name the decision,
  or file it elsewhere". Say whether it is a known deficiency, an inbox entry or
  an out-of-band row, and have the round file it there. Counted.
- **`refuted`** — it contradicts the record **and the record is wrong**; not an
  escalation, see below. Counted, and watched most closely.
- **`escalate`** — the maintainer must answer this one. Stops the loop.

**Never accept the griller's account of the record**: when it says a rule
requires something, open the rule.

## What escalates, and when the record is what is wrong

**The criterion is `docs/process.md`'s "Working unattended", which governs: a call
escalates whenever it would bind beyond the open phase**, its cases being
illustration rather than a closed set. Three things it leaves to you:

- **`.claude/skills/` is in that class** — a stand-in amending `gm/SKILL.md` is
  editing its own review.
- **Granularity matches `roadmap.md`'s**: a new or changed rule escalates, an
  edit only re-describing what the repo has does not. "This is only bookkeeping"
  is the sentence to distrust.
- **A closure contradicting a rationale the spec deliberately recorded**
  escalates unless that rationale is *refuted*; filling a gap it never addressed
  is discovery, so `judgement`.

**You disagree, or cannot tell** is itself an escalation, with no threshold and
no apology owed. What does **not** escalate: re-slicing and earned
`<N>.<M>.<K>` numbers, admitting out-of-band rows including blocking ones,
striking or rewriting a `KD<k>` that stays inside the open phase, and amending
the spec to record what discovery turned up.

Checking recommendations against the record leaves you blind to an error *in* it.
The same section gives the `refuted` disposition, its three conditions — settled
without a new measurement, the falsifying artifact named exactly, the record
amended in the same round — and its bound to **phase-local** record. Put each
condition in the transcript. Go looking at one moment: when the griller's
recommendation is sound on its own terms and the record contradicts it. Read the
*evidence behind* the record, not only the record.

## The round loop

**1. Read the frontier.** If nothing is open, say so in one line and stop. Record
the entries verbatim; they are the transcript's agenda. **2. Open the transcript**
(below) before the first round, never reconstructing one at the end.
**3. Dispatch one fresh subagent**, `general-purpose`, no `model` override,
**never a fork** — it must come to the entries cold:

> Invoke the `dwal` skill (Skill tool, `skill: "dwal"`) and follow it exactly,
> including its "Driven by `/gm`" section. I am standing in for the maintainer: I
> adjudicate each of your recommendations against the project's written record,
> and I can escalate rather than answer.
>
> End your turn after each round with the round reported verbatim — the context
> you established, the numbered questions, and your recommended answers, in the
> `grilling` skill's format. Do not shorten the context that precedes the
> questions; it is recorded and read. I may retire you before the frontier is
> empty; if I do, close every entry you have already grilled (`dwal`'s step 4),
> report, and stop. Do not commit. When the frontier is empty, report every entry
> you closed, where each one's reasoning was filed, and every out-of-band row you
> admitted with its number and its Blocks column.

**4. Answer each round.** Read the record for each question, then reply by
`SendMessage` to that agent — never a new `Agent` call. Give a verdict per
question in the griller's own numbering, and append the round to the transcript
**before** sending the reply. An `escalate` ends the loop: tell the agent to
stop, leave the tree as it is, go to step 7. A rotation boundary ends **this
pair**, not the loop.

**5. Verify independently.** The report is a claim, not evidence: confirm every
entry is *deleted* rather than annotated, that each closure's reasoning is in the
file it claims, and read the ledger rows admitted. Run the consistency checks a
closure can break — at minimum `cd scripts && uv run deficiencies.py`, plus `uv
run measure.py --check` if a figure or its consumers were touched. A docs-only
closure needs no build or test run; say so rather than running one for form.

**6. Commit, once the frontier is empty**, `dwal`'s step 6 governing unchanged,
including that the admitted work is not in it and a non-empty frontier commits
nothing. Subject `dwal: <n> entries settled by stand-in`; one body line per entry
saying what it asked, what was settled and where it was filed; then *Answers
adjudicated against the record by /gm, not by the maintainer; transcript:
`<path>`* and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Commit
only the closure's files; leave anything unrelated and say so.

**7. Report**: the entries settled and how each was answered, the verdict counts,
the out-of-band rows admitted with numbers and Blocks, the commit hash, the
transcript path. If you escalated, the question in full, what you read, and why
the record did not settle it. If you rotated, say **the frontier is not empty**,
how many entries remain, and that nothing was committed.

## Rotate before the context degrades

**Retire yourself and the griller together, at a round boundary, on the first of:
12 questions answered, or 5 rounds** — earlier when a segment took heavy reading.
**Never rotate mid-entry**: tell the griller to close what it has grilled,
report, and stop. Write the segment's verdict counts into the transcript and
**return** per step 7. Do not start another griller yourself; `/gosolo` starts
the fresh `/gm`, which starts the fresh `/dwal`.

**The repo is the handoff, not the transcript.** The fresh pair reads STATUS's
frontier and the docs the closures updated; **it must not read the transcript**.
**The tree is dirty across a rotation, and that is correct** — `dwal` commits
nothing while the frontier is non-empty, so read those written, uncommitted
closures, and do not mistake them for `gosub`'s "a dirty tree means the previous
round did not finish", which is about an **implementation** round.

## The transcript is the maintainer's review queue

**It lives outside the repo**, at
`/mnt/ssd/fedora/scratch/pgdump_query/grilling/P<N>.md`, one file per phase,
appended across sessions. It is **redundant by design**, every closure's
reasoning being filed beside its mechanism before the entry is deleted — hence a
review criterion: **if losing a transcript would lose a decision's rationale,
that closure was incomplete.** Each file opens with an index, rewritten each
session, over the rounds below it:
`| Entry | Question | Verdict | Cited / reason | Filed to | Commit |`. Then, per
segment — one per `/gm`, so a rotation opens a new one — a dated section holding
the rounds **verbatim**: the griller's context, numbered questions and
recommendations exactly as written, each followed by your verdict, its citation,
and your reply as sent. Nothing summarised, nothing trimmed. Close each segment
with its verdict counts; they are the signal the maintainer watches, and **a
`refuted` is worth reading the moment it appears**.

## What this skill is not

Not a licence to decide what the maintainer would decide. The `record` verdict is
a claim about a document; where there is none the honest fallback is `judgement`
or `escalate`, never a stretched citation, which reads as verified and is worse
than no transcript.
