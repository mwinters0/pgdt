---
name: dwal
description: Grill the open entries under STATUS's "Decisions worth another look", then close each one in the same session. Use when the user invokes /dwal, or asks to review, settle, or grill the open decisions worth another look.
---

The maintainer is here, and is answering the calls that unattended sessions
made without them. That is the whole purpose of
`docs/status/STATUS.md`'s **"Decisions worth another look"**: a pressure valve
so unattended work can be honest rather than silent. This skill spends it.

**An entry is closed by being deleted.** Deletion is the only record that the
review happened — never edit an entry to say it was reviewed, and never leave
one open because it was "discussed". The section is capped at five, so an entry
that survives a review it was the subject of costs a slot that the next
unattended session needs.

## 1. Read the section, and stop if it is empty

Read STATUS's "Decisions worth another look". If it says nothing is open, say
so in one line and stop — there is nothing here to grill, and manufacturing a
question defeats the cap.

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
- Check the entry's claims against what is actually there. An entry that
  asserts a check reads unticked lines only is a claim about code you can go
  read.
- Look for what the entry did *not* consider. The session that wrote it was
  unattended and under time pressure, so its framing is the most likely thing
  to be too narrow — a binary where a third option exists, or a rule stated in
  one direction that the code enforces in both. Finding that is the highest
  value this review has.

Finding facts is your job, never the maintainer's. Do not open a round with a
question the filesystem answers.

## 3. Grill

Invoke the `grilling` skill and work the design tree over the entry. Its
rules apply unchanged: at most three questions a round, recommend an answer to
each, and write the settled decisions down before asking the next round.

**Do not invoke the `process` skill to grill.** `CLAUDE.md` exempts planning
and grilling from it deliberately, and reading it now spends context on
obligations that do not apply until you write. Invoke it in step 4, before the
first edit.

The tree usually reaches past the entry itself. An entry names one call, but
the call sits in a mechanism, and the mechanism has adjacent holes the entry's
author never looked at — those are in scope, and finding them is why this is a
grilling rather than a yes/no.

## 4. Close every entry you grilled, in this session

Invoke the `process` skill now: closing edits `STATUS.md`, which is one of its
triggers, and `docs/process.md`'s "Decisions worth another look" rules are what
govern the closure.

Closing has two shapes, and the second is the one that gets skipped:

- **The answer changed something.** File the fold-in where each fact belongs —
  the decision in the doc that holds the decision, the reasoning and the
  rejected alternatives in a `docs/status/history/<today>.md` entry — then
  delete the entry.
- **The answer affirmed the call and changed nothing.** The reasoning still has
  to land somewhere: write it beside the mechanism it governs as a
  *rejected-alternative* paragraph **first**, then delete the entry. An
  affirmed call with no record is how the same question gets re-asked by the
  next unattended session, which is exactly what the register is against.

Either way the entry is gone when you finish.

**Do not write rules for a mechanism that does not exist yet.** Where the
answer implies work still to be done, the reasoning goes in the history entry
and the work gets admitted — but `process.md`, the register preambles and the
code keep describing what is actually built until the change lands. Three docs
describing a check nobody wrote is worse than the entry you just closed.

## 5. Admit the work the answer unblocked

An answer usually implies work. Route it by `docs/design/roadmap.md`'s
admission rule, which is not a judgement call:

- **Changes no decision any spec records, and fits one session** — an
  out-of-band row. Take the number after the ledger's watermark, leave the Date
  column empty until it lands, and point the row at today's history entry.
- **Changes a decision a spec records** — it is not out-of-band. It goes
  through grilling → spec amendment → a numbered slice, and saying so is the
  right outcome of this session.

Do not implement it here. This skill settles decisions and files them; a fresh
session with a clean context builds. Say plainly what was admitted and what
should pick it up.

## What this skill is not

It is not a status review, and not a way to reopen settled design. The entries
are the agenda. If the maintainer wants to grill something else, that is the
`grilling` skill directly.
