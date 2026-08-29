# Handing off a long job

Read this when you are about to launch a job you expect to run **more than 30
minutes** — a `measure.py --all` sweep, a koji scan, anything at that scale.
`CLAUDE.md`'s protocol still binds you: detached, `setsid`, stdout and stderr
to a file under `runs/`, never waited on. This adds three obligations on top.

You are the agent that designed and started the job. You are **not** the agent
that will read its results — a fresh one does that, hours from now, with none
of your context. Everything it needs must be in the file you write.

## 1. Start it, then watch for five minutes

Launch the job, then confirm over about five minutes that it is *making
progress*, not merely alive. A process that started and immediately began
failing in a loop is the case this catches. Sample twice, a few minutes
apart, and check something that must move: the log's byte count, the output
file's size, a row or block counter. A process that exists but whose log has
not grown between samples has not started successfully.

Five minutes is the ceiling, not a target. Stop early once progress is
evident. If it is not evident, kill the job by process group, fix it, and
start over — do not hand off a job you have not seen working.

## 2. Write the handoff doc

`runs/<job-name>-<YYYYMMDD-HHMM>/HANDOFF.md`, beside the log. It is a `runs/`
artifact: gitignored, machine-specific, hardcoded paths, and nothing in the
repo consumes it.

Everything load-bearing goes in the **frontmatter**, because the reader is a
cron fire that will read only that. Below the frontmatter, write whatever prose
the follow-up agent needs — what the job is proving, which figures or counts
matter, what an unexpected result would mean. The frontmatter is terse; the
body does not have to be.

```yaml
---
job: measure-all-sweep
slice: 8.3
started: 2026-08-29T14:07:00-05:00     # absolute, with offset
expect: ~60m
log: runs/measure-20260829-1407/log
check: sudo nerdctl inspect -f '{{.State.Status}}' pgdq-koji
progress: wc -c < runs/measure-20260829-1407/log
exit: sudo nerdctl inspect -f '{{.State.ExitCode}}' pgdq-koji
running: check says running
done: check says exited; then exit 0 = ok, 130 = SIGINT (cache saved, resumable), 143 = SIGTERM
failed: exit nonzero and not 130/143; or log tail has "panicked"/"No space left"
stuck: progress unchanged across two consecutive fires
stop: sudo nerdctl stop pgdq-koji     # safe: parse saves cache, same command resumes
result: runs/measure-20260829-1407/tables.md — one table per figure
next: fold tables.md into docs/design/measurements.md per --check consumers, tick 8.3, notes doc
---
```

Rules for those keys:

- `check`, `progress`, `exit`, `stop` are **single commands, runnable verbatim
  from the repo root** — no placeholders, no shell variables the reader does
  not have, no "adjust the path". They will be pasted, not interpreted.
- `progress` must print a number that only ever goes up.
- `running`, `done`, `failed`, `stuck` say how to read that output, in as few
  words as carry the meaning. Every exit code the job can plausibly produce is
  named. Silence about a code is the reader guessing.
- `stop` is how to end the job safely, with what it costs. If the job is a
  bare process, that is a process-group kill (`pkill -g <pgid>`) with the pgid
  written out — killing the harness alone orphans its generator.
- `started` and `expect` let the reader compute elapsed time without asking
  anyone.
- `next` is one line: what the follow-up agent does with the result. Not how.

## 3. Report the path and stop

Your final report to the orchestrator is short: the handoff doc's path, the
slice it belongs to, and the one-line summary of what you saw in the five
minutes. Do not tick the slice's box — the job has not produced anything yet.
Annotate the checklist entry with what remains and why it is waiting.

Then you are done. The orchestrator may come back once to have you fix a
frontmatter line it could not run. It will not ask you for anything else.
