# P33.32 notes — mimalloc's purge delay priced

**The instrument, ahead of its readings.** A figure is taken from a commit,
never from a tree carrying its own apparatus (`measurements.md`, "A figure may
be published outside the sweep"), so this slice lands the two figures that
price the delay and stops; its readings, the 33.7 query run at each delay and
the choice are the next round's. No production code.

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
  leg is asked under `MIMALLOC_VERBOSE=1` which delay mimalloc runs under
  before its first reading (`binary_purge_delay`), a stating leg answering
  another refused, the answers kept with the sitting (`Session.purge_delay`).
  Asked of today's binary: the shipped leg runs at mimalloc v3's 1000 ms, each
  other at the delay it states.
- **`long_line_leg_problems`** is `run_long_line_resident`'s leg check, moved
  out so both resident figures hold their legs to it; `long_line_factors`
  takes the figure whose instrument legs it reads.

Both figures are `measure.UNTAKEN` until their readings move them into
`FIGURES` and give them a section, "What mimalloc's purge delay costs, and
what it leaves resident".

## What the next round takes

From this slice's commit, each a detached job (`CLAUDE.md`, "Long-running
processes"):

- `cd scripts && uv run measure.py --figure purge-delay` — about a quarter
  of an hour, by `release-profile`'s and `parallel-scan-throughput`'s rows.
- `cd scripts && uv run measure.py --figure purge-delay-resident` — four times
  `long-line-resident`'s legs, about three hours by that figure's last take
  (`runs/long-line-resident-20261117-0207/log`).
- **33.7's query run at each delay**, by hand, never in `measure.py`:
  `runs/p33.7-query-hand-run-20261117-0320/run.sh`'s stated run (`q 4g`, `SET
  pgdump.memory = 2147483648`) three times stating nothing and three times
  under `-e MIMALLOC_PURGE_DELAY=<delay>` for each of `PURGE_DELAY_LEGS`,
  each run's `.err` its peak, against the 2,097,152 KiB it states, and its
  rows held to the unlimited run's.

**The choice**, by the spec's rule ("What a held line costs"): the largest
delay — the shipped one included — whose query runs and resident legs hold
their allowance in every rep and whose cost is within the noise on every
`purge-delay` row; else the cheapest that holds, its cost stated. The spec's
paragraph is also why no library change suiting both allocators reaches the
retention, which the resident legs and 33.31's readings bear on and the
readings round confirms or refutes there.

## The leg grid

**The grid stops at 100 ms, the four parallel rows stay**, reviewed with
the maintainer (STATUS's "Decisions worth another look", 2026-11-17).

- **Only 33.7's query run can reject a delay.** `long-line-resident`'s
  allowance is one number, the widest leg's, and its last take at the shipped
  delay holds every leg under it by gigabytes (`measurements.md`, "What a held
  line holds resident"); a lower delay leaves no more resident, which is
  `purge-delay-resident`'s falsifier. So that figure reads what each delay
  leaves resident and narrows nothing.
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
  only raising the rate: 500 ms cannot hold, 250 ms is at the edge.
- **So a value between 100 ms and that edge is wanted only where 100 ms
  resolves a cost on a `purge-delay` row yet holds the query run.** Where
  100 ms is within the noise, a larger delay buys purges no row resolves and
  spends the one binding run's margin. Where it resolves one, the readings
  round reads candidates above 100 ms on the query run by hand first, adds the
  one it keeps to `PURGE_DELAY_LEGS` in a commit, and re-takes both figures
  from it before the choice, so every table names each candidate.
- **The parallel rows are where a positive delay's cost shows first**, frees
  crossing threads and purges serialized, and cost minutes of the figure's
  quarter hour; dropping them would hide the one outcome that refines the grid.

## Negative results

- **The reference leg states nothing rather than 1000 ms**, so it is the
  binary as a release runs it, and stays so once 33.33 sets the default — at
  which point `binary_purge_delay`, reading mimalloc's startup options, would
  name mimalloc's default rather than the one `pgdt` sets after it, and
  33.33 has to say how its default is read.
- **No count of `instructions:u`**: a purge is a system call and the pages it
  re-faults, which `count-instructions`' user-space count does not see; the
  record's `cpu_seconds` carries system time beside the wall clock.
