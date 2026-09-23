---
name: repoint
description: Repoint the record — read the standing docs and source comments blind against the code, correct or strike what the code no longer bears out, trim to the caps, move the stamp. Use when the user invokes /repoint, when `scripts/repoint.py` is red, at a phase wrap, or when /gosolo needs the record re-read without the maintainer.
---

The rule is `docs/process.md`, "Repointing"; **invoke the `process` skill
first**, and the `evidence` skill before judging any claim about a measured
quantity. This is the procedure. Nothing here changes what the code does.

## 1. Scope

Run `cd scripts && uv run repoint.py`. Keep the meter's two numbers and every
red line. Read the stamp in `docs/status/STATUS.md`; the scope is what the
record touched since it: `git diff --stat <stamp> -- '*.rs' docs/`, grouped by
module. A wrap's repoint takes the phase's modules whether or not they are in
that diff. A red on a *cap* is fixed in step 4; the blind read is steps 2–3.

## 2. Read blind, one reader per module group

Dispatch fresh `general-purpose` subagents, in parallel, **never forks** — a
fork already knows what the record says and will read it as true. Give each
one exactly this and nothing else:

- the module's source files and their tests, by path;
- the record lines that name the module, collected by
  `rg -n -w '<module>|<its public types and functions>'` over
  `docs/design/decisions.md`, `docs/design/roadmap.md`,
  `docs/design/*-invariants.md`, `docs/design/measurements.md`,
  `docs/status/STATUS.md`, `docs/status/deficiencies.md` and `docs/manual/`,
  plus the register section headed with that module's name.

> Read the source in full. Then read only the record lines given. For every
> sentence in those lines, and for every comment in the source that asserts a
> property, decide whether the code bears it out. Report each one it does not
> as: the record's file and line, the claim quoted, the code's file and line
> that contradicts it, and what is true. Mark a claim the code alone cannot
> decide as *untestable*. Report nothing else — no praise, no proposals, no
> edits — and read nothing else: not `docs/status/history/`, not `git log`,
> not the rest of the docs.

## 3. Verify, then correct or strike

Every finding is a claim until you have opened both lines yourself. Then, per
`docs/process.md`, "Repointing", step 2 and "Where does this fact go?":

- the record is wrong and the code right — correct the sentence, and hunt every
  copy by handle and by wording (**subtract as you add**);
- the record states the intent and the code falls short — a `KD<k>`, with its
  marker, or a note under "Decisions worth another look" if the fix is a call;
- an entry that no longer binds — strike it; a rejected alternative that nobody
  would propose again goes with it, and a citation to a struck entry is
  retargeted or its sentence inlined;
- *untestable* — a claim no test asserts is either given a test, rewritten as
  what the code does, or deleted (`evidence` skill, the comment rule).

A dated entry is never rewritten; a keystone's deletions are not undone.

## 4. Trim, check, stamp, commit

Trim whatever `repoint.py` names to its cap — striking, never raising the
cap. Run `uv run repoint.py`, `uv run citations.py`, `uv run deficiencies.py`,
and `uv run measure.py --check` if a figure's consumer moved; a comment-only
source change owes a `--stale` acknowledgement like any other. Move the stamp
to `HEAD`'s short sha — the commit you are about to make counts from it. One
commit, subject `repoint: <n> claims corrected, <m> entries struck`, body
naming each correction by file and what was true.

## Driven unattended

- **Correct only what is phase-local** under `docs/process.md`, "Working
  unattended"'s refuted-record conditions: a claim in `STATUS.md`, a `KD<k>`, a
  comment, the manual, the open phase's docs. The register, `roadmap.md`'s
  standing rules, the invariants, `process.md` and the skills are beyond the
  phase: **list, do not edit**, and never strike a `D<k>` unattended.
- Listed findings go in today's dated entry under a heading beginning
  *Repoint findings for the maintainer*, each with both lines quoted.
- The commit and the stamp move as above, counting listed findings in the
  body. Report the counts, the commit and the entry's path.
