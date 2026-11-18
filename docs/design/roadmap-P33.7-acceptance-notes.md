# P33.7 notes — acceptance

**`long-line-resident` was re-taken at `4cb97fd4`**, the commit reading
`text[]` at the bound, and folded into [`measurements.md`](measurements.md),
"What a held line holds resident", which reads it against
`run_long_line_resident`'s falsifiers and the spec's account;
`runs/long-line-resident-20261117-0207/`, its log and pickup, and
`runs/measure-20261117T020756/`, the sitting's tables, readings and
instrument reports. It replaces the take at `7f593832`
(`runs/measure-20261116T172728/`) whole: the same build, every leg stating
the allowance `text[]`'s planned charge at the bound sizes. No production
code.

- **Every planned factor is confirmed as a bound; none is re-set**, `text[]`'s
  at the bound too. A one-partition query holds its line and the terms of its
  factor its input writes, no doubling among them, and every factor
  over-charges only by terms its input writes nothing of (the figure's prose
  names each). The charge D154 sets is a reservation's commit, which a
  resident set does not see past the pages written, so no reading can show a
  factor understated by less than those terms.
- **`text[]` at the bound holds the base and nine lines**, where its secant
  put it, inside the 15g container and the stated allowance.
- **A parse is flat in its line length**, holding no line at any length to
  the bound, default or strict.
- **No falsifier fires.** The residual rule that fired at `7f593832`, at four
  partitions over escaped text, reads 17.04 MiB here against its 32 MiB
  limit: one build's two sittings differ there by the column's rep spread at
  its 512 MiB anchor, which the figure's prose prices.

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
  2,090,480 KiB against the 2,097,152 stated. By the spec's rule as amended
  ("What a held line costs") 100 ms no longer holds, so the figure and the
  strict run wait on the delay 33.32.1 chooses and 33.33.1 sets.
- **Why that `--memory`**: the read's line charge is its longest line,
  462,157,089 bytes, at the plan's factor of one, 924,314,178 bytes (D160),
  which `Arrangement::resolve` takes off the margin's ceiling before solving
  the count, so the room holds that charge and the cache's statistics, and
  less than one reader's term past them. At 2 GiB that solves to eight
  readers (`EXPLAIN`'s `PgDumpExec` feeding eight partitions); at 1.5 GiB to
  one. Eight cut the block's 5.22 GB into pieces of about 650 MB
  (`cut_blocks`), so the cluster at its end — two lines of about 110 MiB, two
  of 220 MiB, two of 440 MiB, the longest 3.79 GiB into the block — falls
  across about three pieces, placed from the survey's line numbers
  (`runs/datanommer-survey/long-lines.tsv`) and the longest line's start
  (`pgdt info --detail`) rather than read, each piece reaching a long line
  early: the two 440 MiB lines alone charge about twice what the room holds,
  so readers wait by the cut rather than by timing. Nothing records a wait
  (the spec's `Rejected`).
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
  lines and four copies**, increasingly with the line over bytea, where over
  text it holds four lines exactly; unattributed. Whether each partition's
  copy is live at the peak is the question, for the `introspect` build.
- **A parse's slow growth with its line**, which the figure states, still
  has no name; the `introspect` build's live peak at 512 MiB and at the
  bound would say whether it is Rust's heap.
- **A query over escaped text took longer under the resident wrapper** than
  at `93428ef7`, at both sittings of this build: wall clocks the figure does
  not publish, priced to the 40 commits between and to no one of them. A
  count pair over a query row would place it.

## Negative results

- **The query run's shape**, one block through `pgdt sql` rather than a
  whole-table read or `pgdt query`, is the spec's, "How it is shown".
