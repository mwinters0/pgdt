# P33.7 notes — acceptance

**`long-line-resident` was re-taken at `543e5877`**, 33.35's code, and
folded into [`measurements.md`](measurements.md), "What a held line holds
resident", which reads it against `run_long_line_resident`'s falsifiers and
the spec's account; `runs/long-line-resident-20261118-2206/`, its log and
pickup, and `runs/measure-20261118T220656/`, the sitting's tables, readings
and instrument reports. It replaces the take at `4cb97fd4`
(`runs/measure-20261117T020756/`) whole: every leg states the same allowance,
and the build differs by the 100 ms purge delay and the bill (D13, D165). No
production code.

- **Every planned factor is confirmed as a bound; none is re-set**, `text[]`'s
  at the bound too, each read off its plan as at `4cb97fd4`. A one-partition
  query holds its line and the terms of its factor its input writes, no
  doubling among them, and every factor over-charges only by terms its input
  writes nothing of (the figure's prose names each). The charge D154 sets is
  a reservation's commit, which a resident set does not see past the pages
  written, so no reading can show a factor understated by less than those
  terms.
- **`text[]` at the bound holds the base and nine lines**, where its secant
  put it, inside the 15g container and the stated allowance.
- **A parse is flat in its line length**, holding no line at any length to
  the bound, default or strict.
- **No falsifier fires**, and **no leg waits or is refused under the bill**:
  `long_line_allowance` adds nothing for it, and the room it leaves still
  holds every four-partition leg's lines at once, which each such column
  still reads; the one charge past the room, `text[]` at the bound, is alone
  and admitted under the floor.
- **The parse columns read a few hundred KiB lower than at `4cb97fd4`**, the
  100 ms delay's, as `purge-delay-resident` reads it within one build; no
  query cell moves with the bill.

## The hand runs

- **The strict parse held no line.** Taken at `7f593832`, the same build
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
  so a run under that line carves no more (the spec's "How it is shown"). Its
  wall clock is no figure.
- **Under 33.35's bill the same run is refused.** Re-taken at `543e5877` in
  `run.sh`'s shape, its own cache, the build `long-line-resident` timed
  (`runs/p33.7-strict-hand-run-20261118-2344/`, `parse.log`, `HANDOFF.md`):
  one reader, a read budget and a statistics allowance of zero, then exit 1
  a minute into the scan, `LineOutgrowsMemory` for a draw of 2,007,136 bytes
  at offset 35,247,420,889, one reader left 0 once the bill's 335,544,320
  bytes come off; peak 88,372 KiB. The offset is in `_hyper_1_128_chunk`,
  the dump's first block holding a line over 1 MiB, rows whose `msg`, `text`,
  runs to about 2.8 MB (`runs/datanommer-survey/copy-index.tsv`,
  `long-lines.tsv`): a row a strict parse streams, drawn for under two
  chunks before its carry knows it streams, `KD170`'s refusal. The bill comes
  off the floor (D165), and at 432 MiB the larger of the cap and the margin's
  ceiling is under it, so the floor is zero, as the handoff predicted before
  the run; it turns positive only above at least 750 MiB.
- **So the strict run is read at 813 MiB**, the smallest whole MiB whose
  floor holds two read chunks, as the spec's "How it is shown" now states.
  Above 640 MiB the cap passes the margin's ceiling, so the floor is the
  allowance less `MEMORY_RESERVE`, one reader's budget, `RETAINED_BOUND` and
  the statistics held. One reader's budget on this `.xz` is read off a
  refusal rather than derived: a copy of the refused run's cache, holding
  96 bytes of statistics, resumed at 811 MiB refused the same line's draw with
  997,504 bytes left, which puts `WorkerMemory::at(1)` at 111,200,032 bytes.
  On a fresh cache that leaves 2,046,176 bytes at 812 MiB and 3,094,752 at
  813 MiB.
- **At 813 MiB the strict parse held no line, at either delay.** Taken from
  `543e5877`, the 100 ms delay and zero at once, each into its own cache
  (`runs/p33.7-strict-floor-20261119-0050/`, `run.sh`, `HANDOFF.md`, each
  leg's `.log`, `readings.txt`): one reader, lowered from the source's 24, a
  statistics allowance of zero, so 760 of 761 blocks declined statistics;
  exit 0, no refusal, the 761 blocks and 1,025,133,886 rows, each table's
  count the 2026-11-16 strict run's. Peaks 173,472 KiB at 100 ms and
  163,716 KiB at zero, under the 832,512 KiB stated and under half the
  longest line, so no line was resident whole. The legs shared the machine,
  which a peak resident does not see; their wall clock is no figure.
- **The peaks differ by 9,756 KiB, so D165's `Reopens` fired**: under the
  196,608 KiB two 64 MiB steps under `RETAINED_BOUND` leave. One rep a leg,
  a trigger's reading, not a fit; and a floor carved to one reader, not
  parked, so it does not read what a parked floor keeps of what its readers
  freed (D165's `Rejected`). Filed for the maintainer (STATUS's "Decisions
  worth another look"), not acted on in 33.7.
- **The query run read its rows and passed its `--memory`.** Taken at
  `4cb97fd4` (`runs/p33.7-query-hand-run-20261117-0320/`, `run.sh`, each
  run's `.csv` and `.err`): `pgdt sql`'s `SELECT count(*) AS n,
  sum(octet_length(msg)) AS bytes` over `_hyper_1_509_chunk`, the block
  holding the dump's longest line, through a copy of the strict run's cache
  (`/mnt/ssd/fedora/scratch/pgdump_query/datanommer/p33.7-query/`), under
  `SET pgdump.memory = 2147483648` in a 4g container, then with no allowance
  stated in a 12g one. Both exited 0 with no refusal and answered 1,402,674
  rows and 4,786,198,833 bytes. The stated run peaked at 2,569,128,
  2,432,292 and 2,608,992 KiB over three runs, past the 2,097,152 KiB it
  stated — the spec's "its peak resident within that `--memory`" does not
  hold — and at 1,904,416, 1,908,752 and 1,936,140 KiB under
  `MIMALLOC_PURGE_DELAY=0`, within it; 33.31 names what is retained, 33.32
  prices the purge delay and 33.33 sets 100 ms.
- **At 33.33's default the query run still goes past its `--memory` on one
  rep.** Taken at `373e1320`, three reps fixed in `run.sh` before it ran
  (`runs/p33.7-query-default-20261118/`, `readings.txt`, each rep's `.csv`
  and `.err`, `version.txt` naming the 100 ms delay): each exit 0, no
  refusal, the unlimited run's rows, peaking at 2,081,420, 2,104,304 and
  2,090,480 KiB against the 2,097,152 stated. 100 ms stays, chosen on
  throughput, and what it keeps is billed (the spec's "What a held line
  costs"), so the query run, the figure and the strict run wait on 33.35.
- **Under 33.35's bill 2 GiB carves two readers, and nothing waits by the
  cut.** Taken at `43e77f42`, six reps fixed in `run.sh`
  (`runs/p33.7-query-billed-20261118/`, `readings.txt`): `input_partitions=2`,
  each exit 0, no refusal, the unlimited run's rows, every peak under the
  2,097,152 KiB stated. But the cut falls about 2.61 GB into the block, before
  the whole cluster, so its lines are one reader's, read one after another.
  The bill costs 400 MiB of allowance, so 2 GiB carves as an unbilled
  1648 MiB would, 6 MiB short of three readers' edge (33.34's 1654 MiB);
  billed at its unrounded 273 MiB it would have carved four.
- **So the run is read at eight readers**, the smallest whole MiB whose
  `EXPLAIN` carves eight, about 2387 MiB by 33.34's edge plus the bill's
  400 MiB. Placed from the survey (`runs/datanommer-survey/long-lines.tsv`),
  the block holds ten lines over 16 MiB: a 110 and a 55 MiB line near its
  start, and the cluster of eight. The cut puts the two 440 MiB lines in
  different pieces only at four, seven and eight readers. At four, by byte
  progress, the later piece gives up its 440 MiB line before the earlier
  reaches the other, so a 440 waits on a 220. At eight, three pieces reach a
  220 and both 440s within a third of a gigabyte, together charging about
  2203 MiB against a room of one line's charge and less than a term. Those
  charges sit under the allowance, which is why the spec compares them with
  the room, not with `--memory` (its "How it is shown").
- **At eight readers the query run holds.** Taken at `543e5877`, the release
  image's build byte for byte `43e77f42`'s, six reps fixed in `run.sh`
  (`runs/p33.7-query-eight-20261118/`, `readings.txt`, `carve.txt`):
  2387 MiB is the smallest whole MiB whose `EXPLAIN` carves eight, 2386 MiB
  carving seven. Each rep exit 0, no refusal, the unlimited run's rows, every
  peak under the 2,444,288 KiB stated, the worst 337,640 KiB under it. The
  resident at eight readers is 33.34's at 1987 and 2048 MiB to within its
  spread, so the bill's 400 MiB of allowance is what brings it under.
- **Why readers wait there**: the read's line charge is its longest line,
  462,157,089 bytes, at the plan's factor of one, 924,314,178 bytes (D160),
  which `Arrangement::resolve` takes off the margin's ceiling before solving
  the count, so the room holds that charge and the cache's statistics, and
  less than one reader's term past them. Eight cut the block's 5.22 GB into
  pieces of about 650 MB (`cut_blocks`), placing a 440 MiB line in each of
  the sixth and seventh, each reached within a third of a gigabyte of its
  piece's start, the sixth's after a 220 MiB line: the two 440 MiB lines
  alone charge about twice what the room holds, so readers wait by the cut
  rather than by timing. The lines are placed from the survey's line numbers
  (`runs/datanommer-survey/long-lines.tsv`) and the longest line's start
  (`pgdt info --detail`) rather than read, and nothing records a wait (the
  spec's `Rejected`).
- **What the excess is**: the instrument build (`introspect-limited.txt`)
  reads a live Rust heap peaking at 1,427,385,872 bytes, under the
  allowance, beside the held line's own mapping (D139) and liblzma's decoders,
  which it does not count; resident at a zero purge delay falls by 480 to
  690 MiB, so what passes the allowance is pages mimalloc keeps of freed
  buffers for its purge delay. 33.31's block tracker names them: at the peak,
  mostly the view builder's blocks and the compressed source's decoded-block
  buffers rather than a long line's decoded copy
  (`roadmap-P33.31-blocks-notes.md`, "The readings"). A decoded copy past its
  draw, held by `RepartitionExec`'s coalescer, is live at the peak beside the
  longest line, and at a zero delay the margin absorbs it
  (`../status/upstream.md`, "UF5").

## What the phase inherits

- **A four-partition query over escaped text or bytea holds less than four
  lines and four copies**, increasingly with the line, where over text it
  holds four lines exactly; unattributed. Whether each partition's
  copy is live at the peak is the question, for the `introspect` build.
- **A parse's slow growth with its line**, which the figure states, still
  has no name; the `introspect` build's live peak at 512 MiB and at the
  bound would say whether it is Rust's heap.
- **Wall clocks the figure does not publish moved, unattributed**: a query
  over escaped text still takes longer under the resident wrapper than at
  `93428ef7` at 512 MiB, and about half as much longer at the bound, priced
  to no one commit; a strict parse over bytea takes longer at `543e5877` than
  at `4cb97fd4` at every length. A count pair over each row would place it.

## Negative results

- **The query run's shape**, one block through `pgdt sql` rather than a
  whole-table read or `pgdt query`, is the spec's, "How it is shown".
