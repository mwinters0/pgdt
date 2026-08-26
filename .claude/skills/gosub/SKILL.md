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
  slice may no longer be the right one.
- **`cargo test`, `clippy`, or `fmt --check` fails**, whatever the report said.
- **No unticked slices remain in the phase.** Do not roll into the next phase:
  a phase needs grilling and a spec before it has slices, and grilling needs
  the maintainer. A phase boundary is always a stop.
- **The tree did not change**, or the round ticked nothing. Two rounds cannot
  disagree about what is next, so this means the loop is spinning.
- **The round cap is reached.**
- **The subagent reports it stopped at a boundary** or says it needs the
  maintainer, however it phrases it.

## Long-running jobs

`CLAUDE.md`'s protocol binds the orchestrator too. If a round launches a
detached job — a koji scan, a measurement — **do not wait on it and do not arm
a monitor.** Record the log path, note that a later session reads it, and treat
the round as clean if its slice's box is ticked; a slice whose box is unticked
*because* it is waiting on that job is a stop.

## The final report

One message when the loop ends:

- Each round: slice, commit hash, one line on what landed.
- Why the loop stopped, quoting the trigger — the new "Decisions worth another
  look" entry in full, or the failing test's output, or "no slices remain in
  Phase N".
- What is left in the tree uncommitted, if anything, and why.
- What the maintainer needs to look at first.

Never soften a failure into progress, and never report a round you did not
verify yourself.
