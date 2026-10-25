# P15.17 notes — a frontier that never waits for a speculation

`gz_seek::Parallel::look` no longer waits for a speculation, a chunk's own or
a continuation. Standing where a decode handed out begins, the frontier waits
for that decode. Anywhere else it decodes on, and a speculation that lands
behind it is dropped (D117). A speculation on a chunk's own start records the
chunk the frontier stood in when it was handed out. When it lands, the pass
keeps how many chunks the frontier crossed meanwhile (`Parallel::lag`), and
the next is handed out at least that far past the chunk after the frontier's.
`pgdt-io`'s pass behaves as it did, `Ahead::wait_for` now saying whether it
ran the decode itself, for a test-only count of the waits. Its pulled
frontier already ran a decode no blocking thread had started, which is what
"in flight or unstarted" asks. The row was split
(`../status/history/2026-10-25.md`, "15.17 split"). Its gate, no slower than
serial at two and four jobs and falling toward the floor at 24, is 15.17.1's.

- **The evidence**: `the_frontier_never_waits_for_a_speculation` takes every
  task and runs none. The pass names no task to wait for and builds the
  serial index. `the_index_is_the_serial_pass_s_at_every_worker_count` gains
  that driver as `Hoarding`. `Lazily` now runs a speculation as soon as it is
  handed out, since the pass no longer waits on one for the driver to run.
  `a_speculation_is_handed_out_as_far_ahead_as_the_last_one_lagged` checks
  that the lag is read off a landing and applied to the next hand-out.
- **`pgdt-io`'s one-thread test had to be rebuilt.**
  `a_frontier_waits_for_a_task_that_nothing_has_started_on_either_feed` used
  to wait for the speculations it held up. Unchanged, it would have reached
  no wait at all. It now lets one read's speculations land and then reads
  wide, and counts the frontier's waits (`Frontier::waits`, test-only). A
  pulled frontier waits, and runs every decode it waits for itself. **A
  fetched frontier's wait for a decode is reached by no test**: it goes back
  to the runtime a piece at a time, and the decode's own pieces land first.
  Its rustdoc says so.

## What the probe and the profile found

A hand probe, outside the apparatus, of a cold-cache `parse` over `control_gz`
at 1, 2, 4, 8 and 24 jobs, three reps each, with the plain parse of `control`
at one reader as the floor: `runs/p15.17/jobs-probe/times.txt`, the recipe in
`runs/p15.17/probe.sh`. It ran on a profiling build of this tree. Two jobs is
level with serial, where 15.7's probe read it about six times slower. Four
jobs is still slower than serial in every rep. Eight is level. At 24 the pass
is no faster than serial, nowhere near the floor.

The account is from temporary counters, never committed
(`runs/p15.17/diag/README.txt`, and the `t-j*.log` beside it):

- **Two jobs run one task.** Each speculation lands alone, so no two guesses
  ever chain and no decode is handed out. The frontier inflates the whole
  file itself.
- **At four jobs** the tasks give the frontier about one chunk in seven.
  Almost every decode taken was waited for. A queued decode no worker was
  free for, the frontier decoded itself. The frontier's own inflate costs more
  per byte than at one job, and so do its serving and the scan beside it.
- **At 24 jobs** the frontier takes almost every chunk and almost never
  waits. But handing out a decoded chunk's plaintext costs a large fraction
  of what inflating it serially does, and the scan's share outside the pass's
  reads grows several times. In most of `Ahead::top_up`'s calls the guesses
  and decodes held for the frontier keep it at its limit of twice the tasks,
  with workers free.
- **Every per-byte cost rises with the count**: a speculation, a decode, the
  frontier's inflate, its serving and the scan. Whether that is the cores'
  clock, their shared caches or memory bandwidth, nothing here separates.

**What a speculation costs**, from two profiles on the same build
(`runs/p15.17/profile-first-pass-j{2,24}.txt`). At two jobs a speculation is
several of the engine's inflates of its chunk. About 95% of its own time is
the marked sink: `Marked::copy` writing one 16-bit value at a time, checking
each for a marker, then pushing it. Most of the rest is the block search. So
on this file a decode almost never goes clean, and the chunk is written
marked from start to end. The hot lines are mapped in
`runs/p15.17/annotate-supply-j2.txt`, through the binary's debug info. Growing
the marked buffer, a `Vec<u16>`, is a small share at two jobs. At 24 jobs it
is most of the profile's `memmove`, the second-largest symbol in it. **The engine's decode is not the cost**, so D117's
reopening on it has not fired.

## What 15.17.1 inherits

- The frontier already decodes on wherever no decode is handed out. What is
  left to price is what the tasks cost the frontier and the scan when they
  buy nothing, and how few chunks they buy below the count where guesses
  chain. Both live in the speculation: what one decodes, how, and what it
  writes. Before reworking it, read D117's "Rejected" and `KD139`/`KD140`.
- The counters above are what attributed the remainder. A probe that cannot
  see `taken` and the frontier's engine and served bytes reads only the wall
  clock, and on this file the wall clock moves very little between counts.

## Negative results

- **Pushing speculations further ahead does not make two or four jobs
  faster.** With fewer workers than a speculation costs inflates, guesses
  land too far apart to chain, wherever they are handed out. The lag only
  stops a speculation's result from landing behind the frontier.
