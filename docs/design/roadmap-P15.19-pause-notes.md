# P15.19 notes — speculation paused where it finds nothing

**The opening account is taken, and the pause is built; the readings that
gate the row are not.** A speculation landing having found nothing stops the
next being handed out until the frontier next crosses a block start or a
member start (`docs/design/decisions.md`, "D125"). Whether that holds the
level-1 file to its serial pass is `gzip-first-pass`'s to say, re-taken from
a commit carrying the pause:

    cd scripts && uv run measure.py --figure gzip-first-pass

Whether it holds the level-6 files to the tree it opens on is `probe.sh`'s,
re-run on release-image builds of `d17eccb5` and `1c086123`
([history](../status/history/2026-10-28.md), "15.19's level-6 half,
reviewed"). The row's bar is 15.7.1's, `measure.gzpass_beats`, which pairs
reps within one sitting: the level-1 file's `1` and `12` rows pair in the
re-take, and the level-6 files' `12` rows pair only in the probe, which
alternates the two builds. The figure's cells at `c4069143` stay at
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

## What the next round inherits

- **The gate is two readings.** `gzip-first-pass`, re-taken from a commit
  carrying the pause, reads the level-1 file's `12` row against its serial
  pass. `runs/p15.19/probe.sh`, seven reps at `12` over the three level-6
  files and the level-1 file, alternating release-image builds of `d17eccb5`
  and `1c086123` and read by `pairs.py`, reads the level-6 rows against the
  opening tree. The probe above timed frame-pointer `profiling` builds, so
  it is the account, not the gate.
- **15.7.2** reads `KD147` after the re-take: struck if the level-1 file's
  default row is no slower than serial, rewritten to the reading otherwise.
