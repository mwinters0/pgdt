# P15.19 notes — speculation paused where it finds nothing

**The pause is built and its gate is met; the level-1 file's default count
still pays a remainder over serial, which is 15.19.1's.** A speculation
landing having found nothing stops the next being handed out until the
frontier next crosses a block start or a member start
(`docs/design/decisions.md`, "D125"). Over the level-1 file that took the
upper counts level with the default, and the level-6 files' default rows
are level with the tree the slice opened on. The default is still slower
than serial (`docs/design/measurements.md`, "What a parallel gzip first pass
buys", at `eb3f9c7b`), and that clause of the row moved to 15.19.1
([history](../status/history/2026-10-28.md), "15.19's level-1 half,
reviewed").

The row's bar is 15.7.1's, `measure.gzpass_beats`, which pairs reps within
one sitting: the level-1 file's `1` and `12` rows pair in the figure, and the
level-6 files' `12` rows pair only in the probe below, which alternates the
two builds ([history](../status/history/2026-10-28.md), "15.19's level-6 half,
reviewed"). The figure's cells at `c4069143`, before the pause, stay at
`git show 1dc3778d:docs/design/measurements.md`.

## The opening account

`runs/p15.19/profile.sh`, contained, `perf record` of a cold `pgdt parse` over
`control_gz_quick` at one, four and twelve jobs, on a profiling build of the
tree this slice opened on (`runs/p15.19/head/`). User cycles only. Not a
figure; read for proportions.

- **The serial pass is mostly D118's walk.** About three fifths of its
  samples are `Engine::step_into`'s own, whose source lines
  (`profile-j1-step_into-srcline.txt`) are `native/walk.rs`'s `Walk::run` and
  the fixed-code table it decodes through. zlib-rs's `inflate_fast` is about
  a third, the CRC-32 a fiftieth. The member is one static block, so the walk
  covers it whole beside the engine: that, not the larger compressed size, is
  why its serial pass is several times a level-6 file's. D118 reopens on
  exactly this, which goes to the maintainer under "Decisions worth another
  look", not to this row.
- **What slows the frontier as workers are added** is the engine's inflate.
  The frontier's own work in cycles (`profile-j*-period.txt`): the walk's is
  flat across the three counts, within a third of a percent; `inflate_fast`'s
  rises about 5% at four jobs and 7.5% at twelve. Cycles a sample are equal
  at one and twelve jobs, so the clock did not move. So the searches cost the
  frontier's inflate through something shared: a cache or memory bandwidth.
  No symbol of the frontier's handling of failed guesses reaches half a
  percent.
- **What the workers do**: `ChunkTask::supply` and the native decoder's marked
  and plain `codes`, with the sinks' growth copies, and a quarter of the
  samples at four jobs, over half at twelve. The searches do decode: false
  candidates inside the static block run some way before being refused.
- **Unattributed**: about half the wall-clock difference between one job and
  four or twelve is not in the frontier's user cycles. Kernel time, the
  frontier's hand-off between blocking threads (it is sampled on all of them,
  evenly) and wake-ups are not sampled by `cycles:Pu`; a profile with kernel
  samples would attribute it.

## What landed

- **`Parallel::speculation_through` answers `None` while paused.** The pass
  records where its frontier stood (`at`) when a speculation landed with
  nothing found, and hands out none while it still stands there. `at` moves
  only at a block start or member start the frontier crosses, by its own
  decode or a taken chunk's, so no other signal is needed. A continuation is
  paused too.
- **Two tests** (`parallel/tests.rs`):
  `a_member_ending_no_block_is_speculated_on_until_one_finds_nothing`, over
  zlib-rs's level 1 run as each task is handed out, at most one speculation a
  block start crossed, against many on a level-6 file; and
  `a_speculation_finding_nothing_pauses_speculation_until_the_next_block_start`,
  the pause lifting at the first block start after it.
- **Two tests' chunks widened to 32 KiB**:
  `a_decode_run_ahead_is_ready_before_the_frontier_takes_it` and
  `a_speculation_is_handed_out_as_far_ahead_as_the_last_one_lagged` cut their
  level-6 file into 8 KiB chunks, most of which hold no block start, so their
  first speculation now paused the rest. Their files' blocks end at most about
  27 KiB apart. `pgdt-io`'s
  `a_frontier_waits_for_a_task_that_nothing_has_started_on_either_feed`
  likewise reads ten copies of its payload in 64 KiB chunks, so the guesses
  its first read lands still chain. **The shipped chunk is a megabyte**, many
  blocks of every level-6 writer measured, so on those files a speculation
  finding nothing is the exception the tests had to be cut small to reach.

## What the hand probe says

Not the gate: `runs/p15.19/probe.sh`, contained, seven reps alternating a
profiling build of this tree (`new`) against the opening one's (`head`), read
by `measure.gzpass_beats` in `runs/p15.19/pairs.py` (`pairs.txt`).

- **The level-6 files' `12` rows are level with the opening tree's** on all
  three writers: none beaten.
- **The level-1 file's `12` row beats the opening tree's**, faster in every
  pair, and is **level with its own serial pass**, where the opening tree's
  was beaten by it in every pair. Level only just: serial is faster in five
  pairs of seven, by a median paired difference a little over the margin, so
  the figure may read it either way.
- **What remains at twelve jobs** (`runs/p15.19/new/`): the speculations handed
  out before the first lands, about a fifteenth of the pass's user cycles,
  spent at its start. The frontier's walk and inflate are within noise of the
  serial pass's.

The probe above timed frame-pointer `profiling` builds, so it is the
account, not the gate.

## The gate's level-6 half

`runs/p15.19/probe.sh`, contained, seven reps at `12` over the three level-6
files and the level-1 file, alternating release-image builds of `d17eccb5`
(`new`) and `1c086123` (`head`), each from a worktree into a target dir of
its own (`runs/p15.19/build-release-*.log`), read by `pairs.py`
(`runs/p15.19/pairs-release.txt`).

- **The level-6 files' `12` rows are level with the opening tree's** on all
  three writers, none beaten either way: `new` is faster in four or five
  pairs of seven on each, short of the six a verdict needs, and the median
  paired difference favours it on all three, under the margin. That half of
  the row is met.
- **The level-1 file's `12` row is level with the opening tree's here**,
  faster in six pairs of seven but under the margin, where the profiling
  builds read it beating. Nothing gates on this pair; what the row asks of
  that file is its `12` row against its own serial pass, the re-take's.
- **Every run exited cleanly**: the probe stops at the first that does not,
  and wrote all 56 lines.

## The gate's level-1 half

`gzip-first-pass`, re-taken at `eb3f9c7b`, whose code is `d17eccb5`'s
(`runs/measure-20261028T193159/`, launched from
`runs/gzip-first-pass-20261028-1930/`). All 196 readings were taken
cleanly: none killed, none discarded for contention, and each count's
resolved workers checked.

- **The level-1 file's `12` row is beaten by its serial pass**, faster in one
  pair of seven by a median paired difference past the margin: 15.19.1's
  clause. The hand probe on profiling builds had read it level, on the
  margin.
- **The pause did what D125 says.** The upper counts are level with the
  default, where before it they were beaten by it. What it did not do is
  take the default to serial.
- **The remainder's shape**: paired against serial, it rises from four jobs
  to twelve and is flat from twelve to twenty-four, so it does not grow
  with the speculations handed out before the first lands, whose number
  the count sets.
- **What it is not.** Not the frontier's user cycles: `step_into` and
  `inflate_fast` are level at one and twelve jobs
  (`runs/p15.19/new/profile-j{1,12}-period.txt`, profiling builds), and the
  extra cycles at twelve are the searches, on other cores. Not a wait on a
  task: the frontier waits only for a decode (`gz-seek`'s `parallel.rs`,
  module doc), and over this file no decode is handed out.
- **The frontier changes thread at twelve jobs and not at one.** The
  opening account's profiles, read by thread (`runs/p15.19/new/hops.txt`):
  at one job one thread holds 98.6% of the samples; at twelve, once the
  speculations handed out before the first lands are done, one CPU's worth
  of work spreads evenly over twelve blocking threads, 2619 changes of thread
  in 8 s, a median 1.1 ms on one. The pulled frontier runs each scan read in
  a `spawn_blocking` of its own (`first_pass.rs`, `run_pulled`), so the
  runtime's blocking pool picks its thread a read at a time. A change of
  thread's wake-up and cold core sit off the frontier's user cycles, which
  is where the remainder is, and more idle threads in the rotation fits the
  rise from four jobs to twelve; nothing here explains twelve and twenty-four
  being level, and nothing has priced a change of thread.
- **Unattributed, and what would attribute it**: the frontier's off-CPU time
  at one and twelve jobs on a release-image build. `perf stat`'s software
  counters (task clock, context switches, migrations) are permitted at this
  machine's `perf_event_paranoid` of 2; kernel samples are not.
- **The level-6 rows and the default stand.** Each level-6 file's `12` row
  beats serial and both upper counts, the verdict is `shared` as at
  `c4069143`, so D2 is unchanged, and the 4 MiB chunk is slower or level on
  every file, so D121 is unchanged.

## What the next round inherits

- **15.19.1 opens on the change of thread**: an account from the code of
  the changes a pass makes, then `perf stat` at one and twelve jobs, with
  its falsifier fixed in the spec row before the run. A change of thread
  also happens on the level-6 files, hidden under their lift, so whatever
  15.19.1 lands is gated on their rows too.
- **`KD147` is 15.19.1's**, rewritten to the re-take's reading; 15.7.2
  reads it once 15.19.1 closes.
