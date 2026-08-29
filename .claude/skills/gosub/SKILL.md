---
name: gosub
description: Run rounds of unattended roadmap work, each in a fresh subagent, committing between rounds and stopping the moment something needs the maintainer. Use when the user invokes /gosub, or asks for several slices to be landed autonomously in sequence.
---

Drive `/go` in a loop. Each round is a **fresh subagent** that lands one
slice; this session orchestrates and never implements.

`/gosub [max-rounds]` — the cap defaults to **5**.

## Why a subagent per round

A slice is a cold-start task: read STATUS, read the spec, land the next
unticked box. Nothing from round 1 helps round 2, and carrying it forward
costs context for no benefit — by round 4 an inherited transcript is mostly
irrelevant work. So each round gets a genuinely new agent.

**Never** `subagent_type: "fork"` (it inherits this context, defeating the
point) and **never** `SendMessage` to a previous round's agent (it resumes a
spent one). Every round is a new `Agent` call with
`subagent_type: "general-purpose"`, no `model` override.

## The orchestrator does not do the work

Your job is dispatch, verification, commit, and the stop decision. You do not
read source files, do not fix the subagent's test failures, and do not finish
a slice it left half-done. If a round comes back wrong, that is a stop
condition, not a repair job.

Keep your own footprint small — a handful of tool calls per round. The context
you spend reading code is context the loop cannot spend on rounds.

## One round

**1. Baseline.** Before dispatching, record:

- `git rev-parse HEAD`, and that the tree is clean (`git status --short`).
  A dirty tree at the start of a round means the previous round did not
  finish — stop.
- The current text of STATUS's **"Decisions worth another look"** section.
- The unticked slice boxes in the active phase's checklist, in order.

**2. Dispatch a fresh subagent.** The prompt is short, because the `go` skill
carries the real instructions:

> Invoke the `go` skill (Skill tool, `skill: "go"`) and follow it exactly.
> Land exactly one slice — the next unticked box in the STATUS checklist.
>
> If the slice requires launching a job you expect to run more than 30
> minutes, read `.claude/skills/gosub/handoff.md` before you launch it and
> follow it instead of finishing the slice.
>
> When you are done, report: the slice number and title; whether you ticked
> its box, and if not, what remains; whether you added any entries to STATUS's
> "Decisions worth another look", quoted in full; whether you split the slice
> and earned a new `<N>.<M>.<K>`; the verbatim result lines from `cargo test
> --workspace`, `cargo clippy --workspace --all-targets`, and `cargo fmt
> --check`; and the path of any detached job you launched.

**3. Verify independently.** The report is a claim, not evidence. Run
`cargo test --workspace`, `cargo clippy --workspace --all-targets` and
`cargo fmt --check` yourself, and re-read STATUS's checklist and its
"Decisions worth another look" section. Where the report and the tree
disagree, the tree wins.

**4. Commit, if the round is clean.** A round is clean when the slice's box is
ticked, all three checks pass, and the tree actually changed. Commit it as one
slice:

```
<N>.<M> <slice title>

<two or three sentences: what landed, and any call the notes doc flags>

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Commit even when a stop condition has fired for some *other* reason — verified
work belongs in history, and one commit per slice is what makes it reviewable.
The one exception: **never commit a round that failed verification or left its
box unticked.** Leave that in the tree exactly as the subagent left it, so the
maintainer sees what the subagent saw.

**5. Decide.** Stop, or start the next round from step 1.

## Stop conditions

Any one of these ends the loop. Report it plainly; do not work around it.

- **A new entry under "Decisions worth another look."** This is the primary
  one. That section exists for calls the maintainer should weigh, so a new
  entry is the subagent asking for review — continuing past it would stack
  more work on an unreviewed judgement.
- **The slice's box is still unticked**, including when the subagent split it
  and left an earned `<N>.<M>.<K>` behind. A split is a re-plan, and the next
  slice may no longer be the right one. The one exception is a round that
  handed off a long job: its box is unticked *by design*, and the follow-up
  subagent in step 5 is what ticks it.
- **`cargo test`, `clippy`, or `fmt --check` fails**, whatever the report said.
- **No unticked slices remain in the phase.** Do not roll into the next phase:
  a phase needs grilling and a spec before it has slices, and grilling needs
  the maintainer. A phase boundary is always a stop.
- **The tree did not change**, or the round ticked nothing. Two rounds cannot
  disagree about what is next, so this means the loop is spinning.
- **The round cap is reached.**
- **The subagent reports it stopped at a boundary** or says it needs the
  maintainer, however it phrases it.

## A round that launches a long job

A job expected to run **over 30 minutes** does not fit inside a round. The
subagent that designed and started it has spent its context doing so, and by
the time the job lands, that context is a liability — an agent holding half a
day of stale reasoning reading a number it could read cold. So the job outlives
its subagent, and a fresh one reads the result.

The subagent's side is `.claude/skills/gosub/handoff.md`, named in the dispatch
prompt: it starts the job, watches five minutes for real progress, writes
`runs/<job>-<stamp>/HANDOFF.md`, and reports that path. Your side is below.

**This overrides `CLAUDE.md`'s "never monitor a long job" for the orchestrator
only, and only in this shape.** That rule exists because waiting past an hour
expires the prompt cache and reloads the whole conversation. A 30-minute poll
never crosses that hour, and each fire is a handful of commands against a
context you have deliberately kept small. Do not widen the interval to "save"
fires — an hourly poll is the expensive one. The subagents are still bound by
the rule as written: they never wait, and none of them is alive while the job
runs.

**1. Validate the handoff doc.** Read only the frontmatter. Run `check`,
`progress`, and `exit` verbatim, right now. Each must execute and produce
output you can read against the `running`/`done`/`failed` lines without
guessing. A command that needs a path fixed, a variable filled in, or a code
the frontmatter never names is not validated — `SendMessage` that subagent
once to fix the line, then re-run it. If it still does not hold up, stop the
job with `stop`, and stop the loop.

**2. Release the subagent.** Once the frontmatter validates, that agent is
finished. Never message it again — not for a status opinion, not for the
analysis. It is the agent this whole protocol exists to retire.

**3. Arm the poll.** `CronCreate`, `*/30 * * * *`, recurring, with a prompt
that stands on its own:

> Long-job check for `<handoff path>`. Read its frontmatter. Run `check` and
> `progress`. Follow `.claude/skills/gosub/SKILL.md`, "A round that launches a
> long job", step 4.

Cron jobs are session-only and fire only while this session is idle, so end
your turn after arming it and stay idle. Do not start another round: `CLAUDE.md`
forbids a `cargo` build or test while a measurement runs, and your own
verification step is exactly that. The loop is paused, not continuing.

**4. Each fire.** Run `check` and `progress`; keep the last `progress` value so
the next fire can compare.

- *Still running, progress moving* — say so in one line and end the turn.
- *Stuck* by the frontmatter's own `stuck` rule — stop the job with `stop`,
  disarm the cron, and stop the loop.
- *Failed* — disarm the cron, and stop the loop. Leave the log and the tree
  untouched; a fresh subagent is for results, not for a post-mortem the
  maintainer should see first.
- *Done* — disarm the cron (`CronDelete`), then step 5.
- *Over eight hours since `started`* — disarm the cron and stop the loop,
  whatever the job is doing. Leave the job running; write up status per
  "The eight-hour cutoff" below.

**5. Dispatch a fresh subagent for the result.** New `Agent` call,
`general-purpose`, no `model`, never a fork. Its prompt:

> The long job described in `<handoff path>` has finished. Read that file in
> full, including the body below the frontmatter. Confirm its `done` condition
> holds, then carry out its `next` line and whatever else slice `<N.M>`'s spec
> row requires to be complete.
>
> Invoke the `process` skill first and follow it — this slice's notes doc,
> STATUS, and any figure's consumers are part of finishing it. Do not commit.
>
> Report: whether you ticked the slice's box and what remains if not; any
> entries you added to STATUS's "Decisions worth another look", quoted in full;
> and the verbatim result lines from `cargo test --workspace`, `cargo clippy
> --workspace --all-targets`, and `cargo fmt --check`.

**6. Resume the loop at step 3 of "One round"** — verify independently, commit,
decide. From here the round is an ordinary one, and every stop condition
applies to it unchanged.

### The eight-hour cutoff

Eight hours after `started`, the session ends rather than the job. Disarm the
cron, leave the job running, and write up status as the last thing you do:

- A `docs/status/history/<today>.md` entry naming what was launched, the
  handoff doc's path, and that a later session reads it.
- STATUS's checklist entry for the slice, annotated with what remains.
- The final report below, saying plainly that the loop stopped on the cutoff
  with the job still running.

The handoff doc is what a later session picks up — which is why the frontmatter
has to hold without you.

## The final report

One message when the loop ends:

- Each round: slice, commit hash, one line on what landed.
- Why the loop stopped, quoting the trigger — the new "Decisions worth another
  look" entry in full, or the failing test's output, or "no slices remain in
  Phase N".
- Any long job still running: its handoff doc's path, what `check` last said,
  and how to stop it. The maintainer inherits it.
- What is left in the tree uncommitted, if anything, and why.
- What the maintainer needs to look at first.

Never soften a failure into progress, and never report a round you did not
verify yourself.
