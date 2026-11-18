# P33.32 notes — mimalloc's purge delay priced

**100 ms is the delay, chosen on throughput, and what it keeps is billed
rather than fitted out** ("The choice"). The slice landed the two figures
that price the delay, then took them and 33.7's query run at each delay from a
later commit, a figure being taken from a commit and never from a tree carrying
its own apparatus (`measurements.md`, "A figure may be published outside the
sweep"). No production code; 33.33 sets the value.

- **`purge-delay`**, `measure.run_purge_delay`: the shipped binary's wall
  clock on `release-profile`'s four shapes, serial in the register's
  container, and on `parse` and the provider's typed scan at four workers and
  at twenty-four (`roadmap.md`, "Four workers is the optimization baseline;
  twenty-four is the guard") in `parallel-scan-throughput`'s, five reps. Each
  row's verdict line says per leg whether its gap to the shipped leg is
  resolved (`resolved_above`) or within the noise.
- **`purge-delay-resident`**, `measure.run_purge_delay_resident`: peak resident
  over `long-line-resident`'s legs but its anchors beside the bound, at the
  allowance that figure sizes from the factors its instrument legs read in the
  same sitting, three reps, each leg checked against that allowance over every
  rep.
- **The legs**: the shipped binary stating nothing, then
  `MIMALLOC_PURGE_DELAY` at `PURGE_DELAY_LEGS`, 0, 10 and 100 ms, each given to
  the container (`RunSpec.env`, keyed where set) and so outside the shape. Each
  leg is asked which delay it runs under before its first reading
  (`binary_purge_delay`; at this sitting under `MIMALLOC_VERBOSE=1`, by
  `pgdt --version` from 33.33), a stating leg answering another refused, the
  answers kept with the sitting (`Session.purge_delay`). Asked of `7860c5fb`'s
  binary: the shipped leg runs at mimalloc v3's 1000 ms, each other at the
  delay it states.
- **`long_line_leg_problems`** is `run_long_line_resident`'s leg check, moved
  out so both resident figures hold their legs to it; `long_line_factors`
  takes the figure whose instrument legs it reads.

Both figures are taken at `7860c5fb` and in `measure.FIGURES`, their section
`measurements.md`, "What mimalloc's purge delay costs, and what it leaves
resident", which states what each reads.

## The query run at each delay

**The shipped delay fails 33.7's query run and every stated one holds it.**
Taken at `7860c5fb` by hand (`runs/p33.32-query-delays-20261117-0959/`:
`run.sh`, `readings.txt`, each run's `.csv`, `.err` and `.exit`, each leg's
`asked-*.txt`): 33.7's stated run three times a leg, the reps interleaved, the
middle one reversed, each leg asked its delay first. Every run exited 0 with no
refusal and the unlimited run's rows. Against the 2,097,152 KiB it states, the
shipped leg peaked past it on all three (2,366,204 to 2,416,108 KiB) and the
others within it: 0 ms at most 1,952,428, 10 ms 2,021,064, and 100 ms
2,085,576, 11,576 KiB under it.

- **What a delay keeps is not proportional to it**: over the zero leg's median,
  100 ms keeps about a third of what 1000 ms does, 10 ms about a fifth.
- **No delay above 100 ms holds it.** `purge-delay` resolved a cost at
  100 ms, so the leg grid's rule (below) read candidates above it by hand from
  the same binary, 100 ms again beside them
  (`runs/p33.32-query-above-100-20261117-1334/`, laid out as above): 125 and
  150 ms held one rep of three, 200 and 250 ms none, and 100 ms all three. So
  none joins `PURGE_DELAY_LEGS` and neither figure is re-taken. Over six reps
  in two sittings, 100 ms peaks at 2,015,800 to 2,090,104 KiB, the worst
  7,048 KiB under the line.

## The choice

**100 ms, on throughput** (the spec's "What a held line costs"), what it keeps
resident billed rather than fitted out under one run's allowance.

- **The shipped delay fails the query run; 0, 10 and 100 ms hold it and every
  resident leg in this slice's reps.** None of the three is within the noise
  on every `purge-delay` row.
- **100 ms is the cheapest**, its cost resolved on two rows: the serial
  `pgdt sql -c` scan and the twenty-four-partition one (`measurements.md`,
  "What mimalloc's purge delay costs, and what it leaves resident"). On every
  other row it is within the noise; 10 ms resolves four rows and 0 ms five. At
  twenty-four partitions the three cost alike, so no holding delay avoids that
  row's cost.
- **The margin is thin by construction, and 33.7's reps are the rule's
  evidence**, reviewed with the maintainer (STATUS's "Decisions worth another
  look", 2026-11-17). The rule takes the largest delay that holds, so it lands
  where the run stops holding, and the run's `--memory` was chosen to carve
  eight readers, not to price retention: its margin says how likely 33.7's
  re-take is to pass, not what a user's run keeps. 100 ms's six reps span ten
  times its worst margin, so a further rep past the line needs nothing to
  change. That is no reason for 10 ms, a margin the rule does not state
  demanded of one arbitrary run at two more resolved rows on every release.
  33.7's re-take at the new default put one rep of three past the line, and
  fitting the delay under the run was then given up: 100 ms stays, chosen on
  throughput, and what it keeps is a term the binary states (the spec's "What
  a held line costs"), read by 33.34 and billed by 33.35.
- **What a user's run keeps is the margin's, not the delay's.** The query run
  holds past `margin_allowance`'s predicted resident at every delay, the zero
  one included, so no delay meets `MEMORY_MARGIN_PERCENT` on it; that excess
  is `KD34`'s, filed in P23's inbox ("33.7's query run holds past the
  predicted resident at every purge delay").

**The spec's account of the retention is confirmed.** It says why no library
change suiting both allocators reaches the retention: it is what the allocator
keeps of buffers a scan frees at its rate, spread over the view builder's
blocks, the decoded-block buffers and DataFusion's own
(`roadmap-P33.31-blocks-notes.md`, "The readings"). The readings bear that out. The delay alone takes the query run
under its line. Over `long-line-resident`'s legs, where a held line and its
copy are live rather than freed, it moves a parse by a few MiB and nothing the
charge covers.

## The leg grid

**The grid stops at 100 ms, the four parallel rows stay**, reviewed with
the maintainer (STATUS's "Decisions worth another look", 2026-11-17). No
reading fits the delay under 33.7's query run any more ("The choice"), so a
leg is added only where the choice moves.

- **Where the query run stops holding is at most about a quarter to a third of
  a second.** mimalloc v3 (`libmimalloc-sys` 0.1.49, `v3/src/arena.c`,
  `mi_arena_schedule_purge`, `mi_arenas_try_purge`) arms an arena's expiry
  at its first pending free and never extends it, purging on a later page
  allocation or free, one thread at a time under a global guard; at 0 ms it
  purges inside the free instead. What a delay keeps is therefore at most the
  scan's free rate times the delay. Taking the rate from what the shipped
  delay keeps over the zero-delay runs (`runs/p33.7-query-hand-run-20261117-0320/`,
  each `.err`'s `maxrss_kib`), the zero-delay headroom under the stated
  `--memory` runs out at 230–370 ms, an upper bound, reuse within a window
  only raising the rate. Read by hand, the edge is between 100 and 125 ms
  (above).
- **The parallel rows are where a positive delay's cost shows first**, frees
  crossing threads and purges serialized, and cost minutes of the figure's
  quarter hour; dropping them would hide the one outcome that refines the grid.

## Negative results

- **The reference leg states nothing rather than 1000 ms**, so it is the
  binary as a release runs it, and stays so once 33.33 sets the default — at
  which point `binary_purge_delay`, reading mimalloc's startup options, would
  name mimalloc's default rather than the one `pgdt` sets after it, and
  33.33 has to say how its default is read.
- **`purge-delay-resident`'s falsifier fires and moves nothing.** Its rule, a
  stating leg's reps wholly above the shipped leg's on any row, fires on fewer
  pairs than three-rep spreads part by chance. The one pattern chance leaves,
  `jsonb`'s ordering filter at 0 ms, is a few MiB and unattributed. The rule
  stays as written, its reading beside it in `run_purge_delay_resident`.
- **The parallel rows price a stated delay against a run its shipped leg's
  delay outlasts**: at four and twenty-four partitions the scan ends inside
  1000 ms, so those rows read what purging costs at all, not what a shorter
  delay adds over a longer run's purges.
- **No count of `instructions:u`**: a purge is a system call and the pages it
  re-faults, which `count-instructions`' user-space count does not see; the
  record's `cpu_seconds` carries system time beside the wall clock.
