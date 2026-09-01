---
name: dwal
description: Grill the open entries under STATUS's "Decisions worth another look", then close each one in the same session. Use when the user invokes /dwal, or asks to review, settle, or grill the open decisions worth another look.
---

The maintainer is here, and is answering the calls that unattended sessions
made without them — which is what `docs/status/STATUS.md`'s **"Decisions worth
another look"** exists to collect. This skill spends it.

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
  `architecture.md` section or spec that documents it. `CLAUDE.md`'s
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

An answer usually implies work. Route it by `docs/design/roadmap.md`'s
"Out-of-band work" admission rule, which is not a judgement call — the ledger
states both sides of it, and `CLAUDE.md` restates the test. Where it comes out
out-of-band, the number goes after the ledger's watermark, the Date column
stays empty until it lands, and the row points at today's history entry.

Do not implement it here. This skill settles decisions and files them; a fresh
session with a clean context builds. Say plainly what was admitted and what
should pick it up.

## What this skill is not

It is not a status review, and not a way to reopen settled design. The entries
are the agenda. If the maintainer wants to grill something else, that is the
`grilling` skill directly.
