---
name: gosub
description: Run rounds of unattended roadmap work, each in a fresh subagent, committing between rounds and stopping the moment something needs the maintainer. Use when the user invokes /gosub, or asks for several slices or out-of-band items to be landed autonomously in sequence.
---

Drive `/go` in a loop. Each round is a **fresh subagent** that lands one slice,
or with no phase open one out-of-band item;
this session orchestrates and never implements. `/gosub [max-rounds]`, cap **5**.

Every round is a new `Agent` call, `subagent_type: "general-purpose"`, no `model`
override — **never** a fork, **never** `SendMessage` to a previous round's agent.
**The orchestrator does not do the work**: dispatch, verification, commit and the
stop decision only. Do not read source files, fix the subagent's test failures,
or finish a slice it left half-done — a round that comes back wrong is a stop
condition, not a repair job.

## One round

**1. Baseline.** Record `git rev-parse HEAD` and that the tree is clean (`git
status --short`) — a dirty tree means the previous round did not finish, so stop;
the current text of STATUS's **"Decisions worth another look"**; the unticked
slice boxes in order; and the rows in `docs/design/out-of-band.md` whose Date is
empty, noting which name the open phase in `Blocks`. A blocking row is what the
round picks up ahead of the next slice; with no phase open, the lowest row whose
Date is empty is what it picks up (`go`, "Pick the work").

**2. Dispatch a fresh subagent**, with this prompt and nothing else:

> Invoke the `go` skill (Skill tool, `skill: "go"`) and follow it exactly. Land
> exactly one piece of work — the one its "Pick the work" selects — reporting
> the number of the task you've selected before you implement. If it requires a
> job you expect to run over 30 minutes, read `.claude/skills/gosub/handoff.md`
> and follow that instead of finishing the slice.
>
> Then report: the slice number and title, or the `M<k>` if you took an
> out-of-band row; whether you ticked its box and what remains if not; any
> entries you added to STATUS's "Decisions worth another look", quoted in full;
> any split and the `<N>.<M>.<K>` it earned; the verbatim result lines from
> `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo
> fmt --check`, `cd scripts && uv run python -m unittest` and `uv run
> repoint.py`; and the path of any detached job.

**3. Verify independently.** The report is a claim, not evidence: run the five
checks yourself and re-read the checklist and "Decisions worth another look".
Where the report and the tree disagree, the tree wins.

**4. Commit, if the round is clean** — box ticked, the cargo checks and the
scripts' tests passing, `repoint.py` green on its caps (its meter is a stop condition, not a defect of
the round), tree actually changed. Subject `<N>.<M> <slice title>`, body two or three sentences on
what landed and any call the notes doc flags, then `Co-Authored-By: Claude Opus 5
<noreply@anthropic.com>`. **An out-of-band round is clean on a different signal**:
it ticks no box, its ledger row's Date filling in instead, so read the row, check
the Blocks column was cleared and the history entry it points at exists, and
subject the commit `<M<k>> <what changed>` with that entry named in the body.
Commit even when a stop condition fired for some *other* reason; the one
exception is that you **never commit a round that failed verification or left its
box unticked**.

**5. Triage a new "Decisions worth another look" entry** (below). **6. Decide.**
Stop, or start the next round from step 1.

## Scheduling and labelling are yours

An entry is yours when it is **purely about where work goes and in what order,
inside the open phase**; the test is whether any answer changes what the phase
delivers. Typically a slice to be split, a blocking out-of-band row that should
come first, a mis-set `Blocks` column, or work filed as a slice that is
out-of-band by the admission rule and the reverse. Settle it the way `/dwal`
would and under the same obligations — read the mechanism, check the entry's
claims, route the work by `docs/process.md`'s "Out-of-band work" admission rule,
file the reasoning where its "Where does this fact go?" sends it, **delete** the
entry rather than annotating it, commit that as its own change, restart at step 1
— and report each one, quoted as it stood, with what you decided.

**Hand it over whenever the order changes the outcome** — a slice taken on
evidence a later one is meant to produce, an ordering foreclosing an option the
phase was holding open. The tell is that you can name something the phase would
deliver differently. **When in doubt, stop.** Anything binding **beyond** the
open phase is never yours (`docs/process.md`, "Working unattended"); amending a
spec to record a reordering is fine, amending one to change why a decision was
made is not.

## Stop conditions

Any one ends the loop. Report it plainly; do not work around it.

- **A new "Decisions worth another look" entry that step 5 did not settle.**
- **The slice's box is still unticked**, including after a split that earned an
  `<N>.<M>.<K>`, or an out-of-band row whose Date is still empty — except a round
  that handed off a long job, unticked *by design* and ticked by step 5 below.
- **`cargo test`, `clippy`, `fmt --check` or the scripts' `unittest` fails**,
  whatever the report said.
- **Nothing is left to take**: an open phase with no unticked slice — a phase
  boundary is always a stop — or, with none open, no row whose Date is empty.
- **The tree did not change**, or the round ticked nothing and filled no Date.
- **The round cap is reached.**
- **The subagent reports it stopped at a boundary** or needs the maintainer.
- **`repoint.py`'s meter is red.** The live record has outgrown its last blind
  read; a repoint is a round of its own (`.claude/skills/repoint/SKILL.md`),
  never something a slice round does on the way.
- **A third consecutive round amends the same spec row, constant, rule or
  acceptance clause.** The loop is fitting one term a round against an account
  nobody has finished (`evidence` skill, rule 1); the next thing written is the
  whole account, and that is a grilling's job, not the next round's.

`/gosolo` runs this skill unchanged and overrides exactly four of these; the
overrides are in `.claude/skills/gosolo/SKILL.md` and nowhere else.

## A round that launches a long job

A job expected to run **over 30 minutes** outlives its subagent, whose side is
`.claude/skills/gosub/handoff.md`; a fresh one reads the result. **This overrides
`CLAUDE.md`'s "never monitor a long job" for the orchestrator only and only in
this shape**; do not widen the interval to "save" fires, and the subagents stay
bound by the rule as written.

**1. Validate the handoff doc**: read only the frontmatter and run `check`,
`progress` and `exit` verbatim now. Each must give output readable against the
`running`/`done`/`failed` lines without guessing; one needing a path fixed, a
variable filled in, or a code the frontmatter never names is not validated —
`SendMessage` that subagent once to fix the line, re-run, and if it still does
not hold, stop the job with `stop` and stop the loop. **2. Release the subagent**
— never message it again, for anything. **3. Arm the poll**: `CronCreate`, `*/30
* * * *`, recurring, prompt *Long-job check for `<handoff path>`. Read its
frontmatter. Run `check` and `progress`. Follow `.claude/skills/gosub/SKILL.md`,
"A round that launches a long job", step 4.* Cron fires only while this session
is idle, so end your turn, stay idle, and start no other round — `CLAUDE.md`
forbids a `cargo` build or test while a measurement runs.

**4. Each fire**, run `check` and `progress`, keeping the last `progress` value
to compare. *Running, progress moving* — one line, end the turn. *Stuck* by the
frontmatter's own rule — disarm the cron and dispatch the capture below **before**
stopping the job, then stop it with `stop` and stop the loop. *Failed* — disarm,
capture, stop the loop. *Done* — disarm (`CronDelete`), then step 5. *Over eight
hours since `started`* — disarm and stop the loop, leaving the job running, per
the cutoff below.

**5. Dispatch a fresh subagent for the result** — `general-purpose`, no `model`,
never a fork:

> The long job described in `<handoff path>` has finished. Read that file in
> full, including the body below the frontmatter. Confirm its `done` condition
> holds, then carry out its `next` line and whatever else slice `<N.M>`'s spec
> row requires. Invoke the `process` skill first and follow it — the notes doc,
> STATUS and any figure's consumers are part of finishing it. Do not commit.
> Report: whether you ticked the box and what remains if not; any new "Decisions
> worth another look" entries, quoted in full; and the five checks' verbatim
> result lines.

**6. Resume at step 3 of "One round"** — verify, commit, decide, every stop
condition applying unchanged.

**When the job fails or wedges**, one fresh `Agent` (`general-purpose`, no
`model`, never a fork) captures the volatile evidence and *only* captures it:
read the handoff doc in full, leave a still-running job running, **change nothing
else** — no retry, no fix, no cleanup, no source tree — and write
`runs/<job>-<stamp>/FAILURE.md` beside it with what `check` and `exit` say now,
enough log to show the failure, the state of everything the frontmatter's
`volatile` line names, whatever else the machine will soon stop being able to
tell you, a diagnosis with its confidence, and the list of what is left to clean
up. Then a `docs/status/history/<today>.md` entry naming the job, the failure and
that path, with slice `<N.M>`'s checklist entry annotated as blocked on it —
`runs/` being gitignored, verify both before you stop. **At the eight-hour
cutoff** the session ends rather than the job: disarm the cron, leave it running,
and write the same two pointers plus a final report saying the loop stopped on
the cutoff.

## The final report

One message when the loop ends: each round's slice, commit hash and one line on
what landed; why it stopped, quoting the trigger; **every live entry under
"Decisions worth another look", quoted in full, not only the ones this loop
added**, saying so if the section is empty; any long job still running, with its
handoff path, what `check` last said and how to stop it, or a failed one's
`FAILURE.md` path, diagnosis and leftovers; what is left uncommitted and why; and
what the maintainer should look at first. Never soften a failure into progress,
or report a round you did not verify yourself.

**A review does not re-arm the loop.** When it stopped on a "Decisions worth
another look" entry and the maintainer then reviews it — with `/dwal` or
otherwise — the loop is over: a review is a re-plan, so the "next unticked box"
is no longer the next piece of work. Do not start another round or offer to pick
up where it left off; `/gosub` restarts only when the maintainer invokes it
again. **`/gosolo` is the exception**, its grilling happening *inside* the loop,
which resumes at a fresh baseline.
