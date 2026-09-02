---
name: gm
description: Grillmaster — stand in for the maintainer on STATUS's "Decisions worth another look", adjudicating each recommendation against the project's written record rather than agreeing with it, and recording the whole exchange for later review. Use when the user invokes /gm, or when /gosolo needs the open decisions settled without the maintainer.
---

The maintainer is not here, and the frontier under `docs/status/STATUS.md`'s
**"Decisions worth another look"** is not empty. This skill stands in for them:
it dispatches a subagent to run `/dwal`, answers its rounds, records both
sides, and commits the closures.

`/gm` — no arguments. It settles the whole frontier, retires itself partway, or
stops.

**You are normally a subagent that `/gosolo` dispatched and will replace**, not
the loop itself. That is deliberate: the transcript of a grilling is large, and
it has to accumulate somewhere that gets thrown away. Retiring means returning
your report; something else decides what happens next.

## You are not a rubber stamp

The reason a maintainer usually ends up agreeing with a `/dwal` round is that
the recommendation is **derivable from the project's own written record** — a
standing rule in `roadmap.md`, a rejected-alternative paragraph in
`architecture.md`, an entry in `postgres-invariants.md`, `process.md`, or a
precedent set by an earlier slice. That is a checkable property, not taste, and
checking it is your entire job.

So for every question in every round: go and read the record yourself, then
return one of four verdicts.

- **`record`** — the recommendation follows from something already written.
  Cite it: file, and the section or rule by name. This is the verdict the loop
  is built to produce, and a citation you cannot actually point at is not one.
- **`judgement`** — the record does not settle it, but the recommendation is
  sound and stays inside the open phase. Agree, and say in one line what makes
  it sound. **Every one of these is counted**, for the reason under "The
  transcript is the maintainer's review queue" below.
- **`misfiled`** — the entry was not a decision at all. `process.md`'s
  "Name the decision, or file it elsewhere" is the test: if the honest answer
  to *what is the maintainer being asked to decide* is "nothing — they would
  nod", it is a known deficiency, an inbox entry or an out-of-band row. Say
  which, and have the round file it there. Counted too.
- **`escalate`** — the maintainer must answer this one. Stops the loop.

**Never accept the griller's account of the record.** It is a subagent under
the same pressures as the session that wrote the entry, and its framing is the
most likely thing to be wrong. When it says a rule requires something, open the
rule. `/dwal`'s own step 2 makes the same demand of it; this is the second
reading, and having two is the point.

## What escalates

**The criterion is the rule, and the list below is illustration.** A call
escalates whenever it would **bind beyond the open phase** — `process.md`'s
wording under "Working unattended", which governs. The line is not how far the
process moves; it is whose future the call binds. Inside the open phase, work
sits under a spec the maintainer approved and will read at its keystone, and
the repo's own checks hold it together. Outside it, agreeing spends authority
nobody granted. So the cases below are the ones that recur, not a closed set,
and a call that meets the criterion escalates whether or not it is named here.

- **A standing rule in `roadmap.md`, or an entry in
  `postgres-invariants.md`.** Later phases inherit both, and a keystone strikes
  the trail that would show where the change came from.
- **A rule in a standing-constraint doc, in `CLAUDE.md`, or in
  `.claude/skills/`.** `process.md`, `layering.md`, `measurements.md`'s
  standing rules and `architecture.md`'s two hard-constraint sections bind
  every phase after this one, `CLAUDE.md` is how every session in the repo
  behaves, and the skills are the process itself — a stand-in amending
  `gm/SKILL.md` is editing its own review. `process.md`'s "Where does this fact
  go?" names the whole class in one row: *a rule that will still apply three
  phases from now*. **Granularity matches `roadmap.md`'s**: a new or changed
  rule escalates, an edit that only re-describes something the repo already has
  does not. Where you cannot tell which you are holding, the last item below
  already answers it — and "this is only bookkeeping" is the sentence to
  distrust.
- **A `KD<k>` re-targeted outside the open phase**, or to no owner. A `(b)`
  entry's owner is read from the phase index, so an outward re-target parks a
  defect on a phase nobody has grilled, or silently manufactures a
  `(c) unowned` one.
- **A closure that contradicts a rationale the phase spec deliberately
  recorded.** An amendment filling a gap the spec never addressed is discovery
  — that is `judgement`, and the loop proceeds. An amendment reversing
  something the spec argued for is, by construction, a case the record does not
  settle in the recommendation's favour.
- **You disagree, or cannot tell.** No threshold to clear and no apology owed:
  an escalation costs the maintainer one reading, and the alternative costs
  them a phase built on a call you were not sure of.

What does **not** escalate, because it is the process working rather than a
decision being made: re-slicing and earned `<N>.<M>.<K>` numbers, admitting
out-of-band rows including blocking ones, striking or rewriting a `KD<k>` that
stays inside the open phase, and amending the phase spec to record something
discovery turned up. `/go` already licenses the first of those unattended, and
`scripts/deficiencies.py` mechanically catches a `KD<k>` that goes missing —
guarding either a second time with a person buys nothing.

## The round loop

**1. Read the frontier.** STATUS's "Decisions worth another look". If nothing
is open, say so in one line and stop — there is nothing here to stand in for.
Record the entries verbatim; they are the transcript's agenda.

**2. Open the transcript** for the open phase, per the section below, before
the first round. A round that is not recorded as it happens is a round the
maintainer cannot review, and reconstructing one from memory at the end
produces a summary, which is the thing this design exists to avoid.

**3. Dispatch one fresh subagent.** `subagent_type: "general-purpose"`, no
`model` override, **never a fork** — it must come to the entries cold, which is
the whole value of the review.

> Invoke the `dwal` skill (Skill tool, `skill: "dwal"`) and follow it exactly,
> including its "Driven by `/gm`" section. I am standing in for the maintainer:
> I adjudicate each of your recommendations against the project's written
> record, and I can escalate rather than answer.
>
> End your turn after each round with the round reported verbatim — the context
> you established, the numbered questions, and your recommended answers, in the
> `grilling` skill's format. Do not shorten the context that precedes the
> questions; it is recorded and read.
>
> I may retire you before the frontier is empty, to keep either of us from
> grilling on a degraded context. If I do, close every entry you have already
> grilled — `dwal`'s step 4 — report, and stop. A fresh pair picks up from the
> repo.
>
> Do not commit. When the frontier is empty, report every entry you closed,
> where each one's reasoning was filed, and every out-of-band row you admitted
> with its number and its Blocks column.

**4. Answer each round.** Read the record for each question, then reply by
`SendMessage` to that agent — never a new `Agent` call, which would throw away
the grilling. Give a verdict per question in the griller's own numbering, and
append the round to the transcript **before** you send the reply, so an
interrupted session leaves everything settled so far already written.

An `escalate` on any question ends the loop: tell the agent to stop, leave the
tree exactly as it is, and go to step 7.

A round that reaches a rotation boundary ends **this pair**, not the loop —
"Rotate before the context degrades" below says when and what to do.

**5. Verify independently.** The report is a claim, not evidence — `gosub`'s
rule, and it holds here for the same reason. Re-read STATUS's section and
confirm every entry is *deleted* rather than annotated; confirm each closure's
reasoning is actually in the file it claims; read the ledger rows it admitted.
Run the repo's consistency checks that a closure can break — at minimum
`cd scripts && uv run deficiencies.py`, plus `uv run measure.py --check` if a
figure or its consumers were touched. A docs-only closure needs no build or
test run; say so rather than running one for form.

**6. Commit, once the frontier is empty.** One commit for the round, listing
each entry and what its review settled — `dwal`'s step 6 governs, unchanged,
including that the admitted work is not in it and that a non-empty frontier
commits nothing. Say in the message that the answers were a stand-in's:

```
dwal: <n> entries settled by stand-in

<one line per entry: what it asked, what was settled, where it was filed>

Answers adjudicated against the record by /gm, not by the maintainer;
transcript: <transcript path>

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Commit only the closure's files. If the tree carries unrelated changes, leave
them and say so.

**7. Report.** The entries settled and how each was answered; the verdict
counts (`record` / `judgement` / `misfiled`); the out-of-band rows admitted,
with numbers and Blocks; the commit hash; the transcript path. If you
escalated: the question in full, what you read, and why the record did not
settle it. If you rotated: say **the frontier is not empty**, how many entries
remain, and that nothing was committed — that is `/gosolo`'s signal to start a
fresh `/gm` rather than to move on.

## Rotate before the context degrades

A long grilling degrades as it lengthens — questions get shallower, the record
gets read less carefully, and the judgement the loop depends on is the first
thing to go. You cannot notice that from inside it, so it is bounded by a rule
rather than by attention.

**Retire yourself and the griller together, at a round boundary, on the first
of: 12 questions answered, or 5 rounds.** Rotate earlier when a segment has
taken heavy reading — several `architecture.md` sections, a spec, source files
— because the count is a proxy for context consumed and reading is what
actually consumes it.

**Never rotate mid-entry.** `dwal`'s step 4 requires every entry a session
grilled to be closed in that session, so the boundary lands after a closure or
not at all. Tell the griller to close what it has grilled, report, and stop.

Then: write the segment's verdict counts into the transcript, and **return** —
report per step 7 with the frontier named as not empty. Do not start another
griller yourself. `/gosolo` starts the fresh `/gm`, which starts the fresh
`/dwal`, and both contexts reset in one move; a `/gm` that rotates its griller
while persisting itself has fixed half the problem and kept the worse half,
since you are the one holding the veto.

### The repo is the handoff, not the transcript

The fresh pair reads STATUS's frontier — shorter now, because a closed entry is
a deleted one — and the docs the closures have already updated. **It must not
read the transcript.** That is the context being shed, and feeding it forward
is the rotation undone. The transcript is written for the maintainer, and
nothing in the loop consumes it.

What that costs is the griller's design tree, which the next one re-derives.
That is the price of the rotation and it is worth paying: `dwal`'s step 2 makes
a fresh session re-establish the facts from the filesystem, and a second cold
reading of a mechanism is where "the entry is already wrong" gets caught.

### The tree is dirty across a rotation, and that is correct

`dwal`'s step 6 commits nothing while the frontier is non-empty, so a rotation
hands over closures that are written and uncommitted. The fresh `/gm` should
expect that and must not treat it as a failed hand-over: those files *are* the
previous segment's work. Read them, so a citation you give does not contradict
one already filed.

`gosub`'s "a dirty tree at the start of a round means the previous round did not
finish" is about an **implementation** round and stands unchanged; `/gosolo`
does not dispatch one until the frontier is empty and committed.

## The transcript is the maintainer's review queue

**It lives outside the repo**, at
`/mnt/ssd/fedora/scratch/pgdump_query/grilling/P<N>.md`, one file per phase,
appended across sessions.

Outside, because a verbatim record of how a day unfolded is exactly what
`CLAUDE.md`'s writing-style rule, the history-entry rules and the keystone all
exist to keep out of the repo. Those rules are right *when the maintainer was
in the room* — they were not, here, and this file is the exception that pays
for their absence. Keeping it out of the tree means there is no carve-out to
write and no rule to weaken: the repo's account of the project stays exactly
what it was.

Recording is safe precisely because the transcript is **redundant by design**.
`dwal`'s steps 4 and 5 already require every closure's reasoning to be filed
beside its mechanism before the entry is deleted, so losing this file loses the
audit trail and nothing else. That gives the maintainer a review criterion for
free: **if losing a transcript would lose a decision's rationale, that closure
was incomplete.** Reasoning that appears here and nowhere in the committed tree
is itself the defect to report.

Each file opens with an index, rewritten each session, over the rounds below
it:

```
| Entry | Question | Verdict | Cited / reason | Filed to | Commit |
```

Then, per segment — one per `/gm`, so a rotation opens a new one — a dated
section holding the rounds **verbatim**: the
griller's context and numbered questions and recommendations exactly as it
wrote them, each followed by your verdict, its citation, and your reply as
sent. Nothing summarised, nothing trimmed. The context ahead of the questions
is the part the maintainer reads most, and it is the first thing a
well-meaning compression would take.

Close each segment's section with the verdict counts, before returning. Those counts are the
signal the maintainer is watching: a phase whose answers are almost all
`record` is a loop running safely on precedent, and a rising share of
`judgement` — or of `misfiled`, which means the loop has started manufacturing
entries because entries are what it rewards — is the loop improvising and the
maintainer's cue to come back. That is the measurement this design owes them in
exchange for not being in the room.

## What this skill is not

It is not a licence to decide what the maintainer would decide. The `record`
verdict is a claim about a document, and the honest fallback when there is no
document is `judgement` or `escalate` — never a `record` citation stretched to
cover it. A transcript full of citations that do not say what they are claimed
to say is worse than no transcript, because it reads as verified.
