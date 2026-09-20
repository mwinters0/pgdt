# Handing off a long job

Read this before launching a job you expect to run **more than 30 minutes** — a
`measure.py --all` sweep, a koji scan, anything at that scale. `CLAUDE.md`'s
protocol still binds: detached, `setsid`, stdout and stderr to a file under
`runs/`, never waited on. Three obligations on top, because the agent that reads
the results is a fresh one, hours from now, with none of your context.

## 1. Start it, then watch for five minutes

Confirm the job is *making progress*, not merely alive: sample twice, a few
minutes apart, checking something that must move — the log's byte count, the
output file's size, a row or block counter. Five minutes is a ceiling, not a
target; stop early once progress is evident, and if it is not, kill the job by
process group, fix it, and start over. Never hand off a job you have not seen
working.

## 2. Write the handoff doc

`runs/<job-name>-<YYYYMMDD-HHMM>/HANDOFF.md`, beside the log — a gitignored
`runs/` artifact consumed by nothing in the repo. Everything load-bearing goes in
the **frontmatter**, the reader being a cron fire that will read only that; below
it, write whatever prose the follow-up agent needs — what the job is proving,
which figures or counts matter, what an unexpected result would mean.

```yaml
---
job: measure-all-sweep
slice: 8.3
started: 2026-08-29T14:07:00-05:00     # absolute, with offset
expect: ~60m
log: runs/measure-20260829-1407/log
check: sudo nerdctl inspect -f '{{.State.Status}}' pgdt-koji
progress: wc -c < runs/measure-20260829-1407/log
exit: sudo nerdctl inspect -f '{{.State.ExitCode}}' pgdt-koji
running: check says running
done: check says exited; then exit 0 = ok, 130 = SIGINT (cache saved, resumable), 143 = SIGTERM
failed: exit nonzero and not 130/143; or log tail has "panicked"/"No space left"
stuck: progress unchanged across two consecutive fires
stop: sudo nerdctl stop pgdt-koji     # safe: parse saves cache, same command resumes
volatile: /dev/shm/pgdt (warm staging, evicted per figure); container pgdt-koji (state lost on prune); /mnt/ssd/fedora/scratch/pgdump_query/measure (kept, but partial figures overwrite)
result: runs/measure-20260829-1407/tables.md — one table per figure
next: fold tables.md into docs/design/measurements.md per --check consumers, tick 8.3, notes doc
---
```

- `check`, `progress`, `exit` and `stop` are **single commands runnable verbatim
  from the repo root** — no placeholders, no variables the reader lacks, no
  "adjust the path"; they will be pasted, not interpreted. `progress` must print
  a number that only ever goes up.
- `running`, `done`, `failed` and `stuck` say how to read that output in as few
  words as carry the meaning, and **name every exit code the job can plausibly
  produce**; silence about one is the reader guessing.
- `stop` ends the job safely and says what that costs — for a bare process, a
  process-group kill (`pkill -g <pgid>`) with the pgid written out. `started` and
  `expect` let the reader compute elapsed time without asking.
- `volatile` names every piece of state **gone or overwritten** by the time
  someone investigates: tmpfs staging, container state a prune destroys, scratch
  the next run reuses, temp files. A diagnosis agent can look only where this
  line points.
- `next` is one line: what the follow-up agent does with the result. Not how.

## 3. Report the path and stop

Short: the handoff doc's path, the slice it belongs to, and one line on what you
saw in the five minutes. **Do not tick the slice's box** — annotate the checklist
entry with what remains and why it waits. The orchestrator may come back once to
have you fix a frontmatter line it could not run, and nothing else.
