# P33.7 notes — acceptance

**`long-line-resident` was re-taken at `7f593832`**, P33's last commit before
33.7, and folded into [`measurements.md`](measurements.md), "What a held line
holds resident", which reads it against `run_long_line_resident`'s falsifiers
and the spec's account; `runs/long-line-resident-20261116-1717/`, its log and
pickup, and `runs/measure-20261116T172728/`, the sitting's tables, readings and
instrument reports. No production code.

- **Every planned factor is confirmed as a bound; none is re-set.** A
  one-partition query holds its line and the terms of its factor its input
  writes, no doubling among them, and every factor over-charges only by terms
  its input writes nothing of (the figure's prose names each). The charge D154
  sets is a reservation's commit, which a resident set does not see past the
  pages written, so no reading can show a factor understated by less than
  those terms.
- **A parse is flat in its line length**, holding no line at any length to
  the bound, default or strict.
- **One falsifier fires**, the residual rule at four partitions over escaped
  text; read as no doubling, and the factors confirmed beside it — STATUS's
  "Decisions worth another look".

## The hand runs

- **The strict parse held no line.** Taken at `7f593832`
  (`runs/p33.7-strict-hand-run-20261116-1842/`, `run.sh`, `parse.log` and
  `HANDOFF.md`): a data-level, strict `pgdt parse` of the reference dump
  stating `--memory 452984832`, 432 MiB, under its longest line
  (`runs/datanommer-survey/summary.txt`), no line setting, under `peak-rss`
  in a 4g container, into a cache of its own so the kept one is not
  replaced. It exited 0 with no refusal, counted the 761 blocks and the rows
  the default parse counts (`runs/p38.7-datanommer-count-20261031-1224/`),
  and peaked at 219,152 KiB (`parse.log`'s `maxrss_kib` line), under the
  stated `--memory` and under half the longest line, so no copy of that line
  was resident whole. Nothing a parse prints says it drew, waited or parked;
  the peak is what shows it.
- **It read one reader and kept statistics for few blocks**: the allowance
  resolved one reader, lowered from the source's 24, and a statistics
  allowance far under what the dump's statistics held on the default parse,
  so 700 of 761 blocks declined statistics, the strict check going on past
  each: below the 441 MiB longest line the 384 MiB reserve leaves one reader,
  so a run under that line carves no more (the spec's "How it is shown"). Its wall clock is no
  figure.
- **The query run is not started.** Stating a `--memory`, it carves its
  longest line and never parks (D160), so `M281` is not its premise; its
  `--memory` carves two readers or more below what its clusters' lines charge
  at once, as the spec's "How it is shown" states, derived here from the
  survey and the arrangement before it runs.

## What the phase inherits

- **A four-partition query over escaped text or bytea holds less than four
  lines and four copies**, increasingly with the line, where over text it
  holds four lines exactly; unattributed. Whether each partition's copy is
  live at the peak is the question, for the `introspect` build.
- **A parse's slow growth with its line**, which the figure states, still
  has no name; the `introspect` build's live peak at 512 MiB and at the
  bound would say whether it is Rust's heap.
- **A query over escaped text took longer under the resident wrapper** than at
  `93428ef7`, where a long field's parse took far less: wall clocks the
  figure does not publish, priced to the 40 commits between and to no one of
  them. A count pair over a query row would place it.

## Negative results

- **No re-take for the fired falsifier**: the handoff's reading rule, a
  falsifier tripping being the finding; the four-partition column's rep
  spread at 512 MiB is of the order of the margin it fires by.
