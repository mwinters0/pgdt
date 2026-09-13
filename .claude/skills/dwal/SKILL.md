---
name: dwal
description: Grill the open entries under STATUS's "Decisions worth another look", close each one in the same session, and commit once the frontier is empty. Use when the user invokes /dwal, or asks to review, settle, or grill the open decisions worth another look.
---

The maintainer is here, answering the calls unattended sessions made without
them — what `docs/status/STATUS.md`'s **"Decisions worth another look"**
collects. **Sometimes they are a stand-in**: under `/gm` an agent adjudicates
each recommendation against the written record and the exchange is recorded for
the maintainer to read afterwards. Nothing below changes except "Driven by `/gm`"
at the end; write every round as if a person will read it. **The rules for that
section are `docs/process.md`'s and this skill does not restate them** — step 4
puts them in front of you in full. What follows is only the procedure.

## 1. Read the section, and stop if it is empty

If nothing is open, say so in one line and stop; manufacturing a question defeats
the cap. Otherwise list the open entries first so the user can steer you, and
**absent a steer take them one at a time, oldest first** — three questions
spanning two mechanisms is a worse round than three on one. The exception is two
entries touching the same mechanism, whose answers constrain each other.

## 2. Establish the facts first — the entry may already be wrong

It was written by a session that has since ended, about code and docs that have
moved. Before framing a single question: read the mechanism it governs — the
source it names and the `decisions.md` entry or spec documenting it; check its
claims against what is actually there; and look for what it did *not* consider,
its author's framing being the likeliest thing to be too narrow — a binary where
a third option exists, a rule stated in one direction the code enforces in both.
Finding that is the highest value this review has. Finding facts is your job,
never the maintainer's: never open a round with a question the filesystem
answers.

## 3. Grill

Invoke the `grilling` skill and work the design tree over the entry; its rules
apply unchanged. The tree usually reaches past the entry — the call sits in a
mechanism, whose adjacent holes are in scope. **Do not invoke the `process` skill
to grill**; step 4 is its moment.

## 4. Close every entry you grilled, in this session

**Invoke the `process` skill before the first edit** and follow its rules as
written, including the closure shape that is invisible without them, where the
review affirmed the call and changed nothing. One thing they do not cover:
**do not write rules for a mechanism that does not exist yet** — where the answer
implies work still to be done, the reasoning goes in the history entry and the
work gets admitted, while the process doc, the registers and the code keep
describing what is actually built until that change lands.

## 5. Admit the work the answer unblocked

Route it by `docs/process.md`'s "Out-of-band work" admission rule, which is not a
judgement call. Out-of-band, the number goes after the ledger's watermark, the
Date stays empty until it lands, and the row points at today's history entry.
**Set the Blocks column as you file the row** — you have just grilled the thing
and are the only session that will know whether it stands in the way of the open
phase's remaining slices. Do not implement it here; say plainly what was admitted
and what should pick it up.

## 6. Commit, once the frontier is empty

**The frontier being empty is the commit condition, and nothing earlier is** —
committing mid-tree leaves a `STATUS.md` in history with an entry deleted and its
reasoning nowhere. Run the repo's consistency checks first (`CLAUDE.md` names
them); a docs-only closure needs no build or test run, so say so rather than
running one for form. **One commit for the whole round**, listing each entry and
what its review settled, with the admitted work left out; commit only the
closure's files and say what else you left. A round ending **without** an empty
frontier commits nothing: say which entries are open and what they wait on.

## Hand over rather than grill on a spent context

Past roughly a dozen questions — fewer where they took heavy reading — **say so
and hand over**; a fresh session needs nothing from this conversation, every
closure being filed as it is settled and the frontier being in `STATUS.md`.
**Never hand over mid-entry** — step 4 requires every entry you grilled to be
closed here. The tree you leave is dirty and uncommitted per step 6; read those
files before your first round if you are picking one up.

## Driven by `/gm`

- **End your turn after each round, reported verbatim** — the context you
  established, the numbered questions and your recommended answers, in the
  `grilling` skill's format; answers come back as a message. Do not compress a
  round because its reader is an agent.
- **Do not commit.** `/gm` verifies the tree independently and commits; step 6
  still governs *whether* there is anything to commit.
- **You may be retired before the frontier is empty**: close what you have
  grilled, report, and stop. **An answer may come back as an escalation** — stop
  there, leave the tree exactly as it is including closures already written, and
  report what is open.

## What this skill is not

Not a status review, and not a way to reopen settled design. The entries are the
agenda; grilling anything else is the `grilling` skill directly.
