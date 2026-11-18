# P33.32.1 notes — the purge delay read between 10 and 100 ms

**Under a test asking the worst rep's headroom to cover the reps' spread, no
delay from 10 to 100 ms holds 33.7's query run, and 0 ms does.** Fitting the
delay under that run was then given up: 100 ms stays, chosen on throughput,
and what it keeps is billed (the spec's "What a held line costs"), so these
readings price what each delay keeps on that run. No production code.

## The readings

Taken by hand from `7860c5fb`'s register build, the build behind every
earlier 0, 10 and 100 ms rep of 33.32, read in place
(`runs/p33.32.1-query-grid-20261118-0341/`: `run.sh`, `readings.txt`, and
each rep's `.csv`, `.err` and `.exit`, each leg's `asked-*.txt`). The run is
33.7's stated run. Two sittings, each three reps interleaved with the middle
one reversed. Sitting 1 ran 0 and 10 ms (their second sitting) beside the
candidates 20, 30, 50 and 75 ms. Sitting 2 ran the candidates again. The
candidates and the count were fixed in `run.sh` before either sitting ran.
Every run exited 0 with no refusal and the unlimited run's rows.

- **Pooled, only 0 ms holds**: six reps, its worst headroom three times its
  spread. **10 ms no longer holds.** Its first three reps held, but its
  second sitting's middle rep widened the spread past the headroom. Each
  candidate between 10 and 100 ms fails the same way, and 50 ms fails by the
  widest gap. 100 ms's nine reps put one past the line.
- **A positive delay's spread is about 57 to 87 MiB at every value read**, so
  holding needs its worst rep at least that far under the line, and every
  positive delay's worst rep came closer. 0 ms purges inside the free, and its
  spread is the smallest of all.
- **The sittings ran back to back** (`readings.txt` has the times), not hours
  apart as 33.32's were. That cannot change the verdict. A rep added to a
  pool can only raise its largest or lower its smallest, so a further sitting
  can never turn a failing delay into a holding one.

## What 33.34 inherits

- **What a delay keeps is mostly the step off zero.** Over 0 ms's median rep,
  10 ms keeps most of what 100 ms does, and the delays between add little
  more (`readings.txt`), as mimalloc's purge inside the free at zero and on a
  timer above it predicts (`roadmap-P33.32-purge-notes.md`, "The leg grid").
- **Every reading is one shape**, eight readers under 2 GiB, so nothing here
  says whether what a delay keeps grows with the readers or is the process's;
  that is 33.34's question.

## Negative results

- **A rep's wall clock is no figure**: going by the gaps between `*.exit`
  mtimes, which include the container's start, the 0 ms reps take slightly
  longer than the others. The figure's serial `pgdt sql -c` row prices the
  same thing.
