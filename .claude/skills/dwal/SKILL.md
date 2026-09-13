---
name: dwal
description: Grill the open entries under STATUS's "Decisions worth another look", close each one in the same session, and commit once the frontier is empty. Use when the user invokes /dwal, or asks to review, settle, or grill the open decisions worth another look.
---

The maintainer is here, and is answering the calls that unattended sessions
made without them — which is what `docs/status/STATUS.md`'s **"Decisions worth
another look"** exists to collect. This skill spends it.

**Sometimes the maintainer is a stand-in.** Under `/gm` the answers come from
an agent adjudicating each recommendation against the project's written record,
and the whole exchange is recorded for the maintainer to read afterwards.
Nothing below changes for that — the procedure, the depth and the format are
the same either way, and the one paragraph that differs is "Driven by `/gm`" at
the end. Write every round as if a person will read it, because one will.

**The rules for that section are `docs/process.md`'s, and this skill does not
restate them.** They govern what an entry is, what closing one means, and the
cap — and step 4 puts them in context in full, before the first edit. What
follows is only the procedure for working through them, which lives nowhere
else.

## 1. Read the section, and stop if it is empty

Read STATUS's "Decisions worth another look". If nothing is open, say so in one
line and stop — there is nothing here to grill, and manufacturing a question
defeats the cap.

Otherwise list the open entries to the user before doing anything else, so they
can redirect you to one in particular. **Absent a steer, take them one at a
time, oldest first.** Entries are usually unrelated, and three questions
spanning two mechanisms is a worse round than three on one. The exception is
two entries touching the same mechanism: grill those together, since the
answers will constrain each other.

## 2. Establish the facts first — the entry may already be wrong

An entry was written by a session that has since ended, describing code and
docs that have moved. Before framing a single question:

- Read the mechanism the entry governs — the source it names, and the
  `decisions.md` section or spec that documents it. `CLAUDE.md`'s
  read-triggers apply here as anywhere.
- Check the entry's claims against what is actually there. An entry asserting
  that a check reads unticked lines only is a claim about code you can go read.
- Look for what the entry did *not* consider. The session that wrote it was
  unattended and under time pressure, so its framing is the most likely thing
  to be too narrow — a binary where a third option exists, or a rule stated in
  one direction that the code enforces in both. Finding that is the highest
  value this review has.

Finding facts is your job, never the maintainer's. Do not open a round with a
question the filesystem answers.

## 3. Grill

Invoke the `grilling` skill and work the design tree over the entry. Its rules
apply unchanged.

**Do not invoke the `process` skill to grill.** `CLAUDE.md` exempts planning
and grilling from it deliberately, and reading it now spends context on
obligations that do not apply until you write. Step 4 is its moment.

The tree usually reaches past the entry itself. An entry names one call, but
the call sits in a mechanism, and the mechanism has adjacent holes the entry's
author never looked at — those are in scope, and finding them is why this is a
grilling rather than a yes/no.

## 4. Close every entry you grilled, in this session

**Invoke the `process` skill before the first edit.** Closing edits
`STATUS.md`, which is one of its triggers, and its full read of
`docs/process.md` is what puts that section's rules in front of you — including
the closure shape that is invisible without them, where the review affirmed the
call and changed nothing. Follow them as written there.

One thing to hold that those rules do not cover, because it is this skill's
situation rather than theirs: **do not write rules for a mechanism that does
not exist yet.** Where the answer implies work still to be done, the reasoning
goes in the history entry and the work gets admitted — but `process.md`, the
register preambles and the code keep describing what is actually built until
that change lands. Three docs describing a check nobody wrote is worse than the
entry you just closed.

## 5. Admit the work the answer unblocked

An answer usually implies work. Route it by `docs/design/out-of-band.md`'s
admission rule, which is not a judgement call — the ledger
states both sides of it, and `CLAUDE.md` restates the test. Where it comes out
out-of-band, the number goes after the ledger's watermark, the Date column
stays empty until it lands, and the row points at today's history entry.

**Set the Blocks column as you file the row.** A row admitted while a phase is
open either stands in the way of that phase's remaining slices or it does not,
and you have just grilled the thing — you are the only session that will know.
Naming the phase there is what puts the item ahead of the next unticked slice
when an unattended session picks the work up; leaving it empty says the phase
can be built around it.

Do not implement it here. This skill settles decisions and files them; a fresh
session with a clean context builds. Say plainly what was admitted and what
should pick it up.

## Hand over rather than grill on a spent context

A long grilling degrades as it lengthens, and the first thing to go is the
judgement the whole exercise is for: questions get shallower and the record
gets read less carefully, neither of which is visible from inside. Past roughly
a dozen questions — fewer where they took heavy reading — **say so and hand
over** rather than pushing to the end of a long frontier.

Handing over is cheap by construction. Every closure is filed as it is settled,
a closed entry is a *deleted* one, and the frontier is in `STATUS.md` — so a
fresh session reads a shorter agenda and docs that are already current, and
needs nothing from this conversation. What it re-derives is the design tree,
which is the price, and step 2's cold re-reading of the mechanism is the part
that makes paying it worthwhile.

**Never hand over mid-entry.** Step 4 requires every entry you grilled to be
closed in this session, so the boundary lands after a closure or not at all.

The tree you leave behind is dirty and uncommitted, per step 6. That is the
hand-over, not a mess: read those files before your first round if you are the
session picking one up, so a closure you file does not contradict one already
there.

## 6. Commit, once the frontier is empty

**The frontier being empty is the commit condition, and nothing earlier is.**
A closure is one reviewed thought: the entry deleted, the reasoning filed
beside its mechanism, and the work admitted. Committing mid-tree splits that
across revisions and leaves a `STATUS.md` in history with an entry deleted and
its reasoning nowhere — which is exactly the state the closure rules exist to
prevent. So grill every entry to the end, close them all, *then* commit.

Run the repo's own consistency checks first — `CLAUDE.md` names them, and a
closure that edits the deficiency register or a phase index is precisely what
they are for. A docs-only closure needs no build or test run; say so rather
than running one for form.

**One commit for the whole round**, listing each entry and what its review
settled. The admitted work is *not* in it: step 5 files a ledger row with an
empty Date, and the implementation belongs to the session that picks it up.

Two things this step does not license. It is not permission to commit anything
else that happens to be in the tree — if the working tree carries unrelated
changes, commit only the closure's files and say what you left. And a round
that ends **without** an empty frontier commits nothing: leave it in the tree,
say which entries are still open and what they are waiting on, so the next
session sees what you saw.

## Driven by `/gm`

When `/gm` dispatched you, three things differ and nothing else does.

**End your turn after each round, and report the round verbatim** — the
context you established, the numbered questions, and your recommended answers,
in the format the `grilling` skill sets out. The answers come back as a
message. Do not compress a round because its reader is an agent: the context
before the questions is the most-read part of the whole exchange, and it is
being recorded.

**Do not commit.** `/gm` verifies the tree independently and commits the round,
for the reason `gosub` gives — a report is a claim, not evidence. Step 6's
frontier condition still governs *whether* there is anything to commit; you
simply are not the one who does it.

**You may be retired before the frontier is empty.** `/gm` rotates at a
boundary rather than adjudicating on a degraded context, and retires you with
it. When it says so, do what "Hand over rather than grill on a spent context"
says — close what you have grilled, report, and stop. A fresh pair picks up
from the repo, and it is not given this conversation.

**An answer may come back as an escalation**, meaning the stand-in will not
settle that question and the maintainer must. Stop there. Leave the tree
exactly as it is, including the closures you have already written, and report
what is open — step 6's rule that a round with a non-empty frontier commits
nothing is what makes that state readable rather than half-filed.

## What this skill is not

It is not a status review, and not a way to reopen settled design. The entries
are the agenda. If the maintainer wants to grill something else, that is the
`grilling` skill directly.
