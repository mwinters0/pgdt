# P33.32.1 notes — the purge delay read between 10 and 100 ms

**Under the spec's rule as amended ("What a held line costs"), no delay from
10 to 100 ms holds 33.7's query run, and 0 ms does.** That is the grid's next
leg, not the rule's answer, so the choice waits on 33.32.2, which reads
candidates between 0 and 10 ms. No production code.

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

## Where the choice stands

**The grid's rule, mirrored a second time, wants a value between 0 and
10 ms** (`roadmap-P33.32-purge-notes.md`, "The leg grid"). Its condition is
that 0 ms resolves a cost on a `purge-delay` row where 10 ms does not, and the
`query --schema-mode strings` row shows exactly that (`measurements.md`,
"What mimalloc's purge delay costs, and what it leaves resident"). Falling to
0 ms now would repeat what that section refuses for 10 ms: taking the grid's
next leg in place of the rule's answer. So the slice was split
(`../status/history/2026-11-18.md`, "33.32.1 split: the grid is read below
10 ms"). 33.32.2 reads candidates between 0 and 10 ms the way this slice read
its own, then makes the choice, and 33.33.1 sets whatever it chooses.

- **If no candidate holds, the choice is 0 ms**, the cheapest delay that
  holds, and its cost is `purge-delay`'s 0 ms column: five of eight rows
  resolved slower, all of them system time, and no `parse` row among them.
- **None of 20 to 75 ms joins `PURGE_DELAY_LEGS`, and neither figure is
  re-taken**, because none of them holds.
- **`long-line-resident` has three reps of 0 ms**, all from
  `purge-delay-resident`'s one sitting, where the rule asks for six from two.
  On every leg the headroom under the allowance is gigabytes and the spread a
  few MiB, and only the query run can reject a delay
  (`roadmap-P33.32-purge-notes.md`, "The leg grid"). Whether three reps are
  enough there is for 33.32.2 to settle when it chooses.

## Negative results

- **A rep's wall clock is no figure**: going by the gaps between `*.exit`
  mtimes, which include the container's start, the 0 ms reps take slightly
  longer than the others. The figure's serial `pgdt sql -c` row prices the
  same thing.
