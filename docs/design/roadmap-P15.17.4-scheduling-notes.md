# P15.17.4 notes — a four-job first pass faster again

A four-job gzip first pass over `control_gz` is now faster than the tree
this slice opened on (`35201239`) in every alternated pair, and faster than
serial. Twenty-four jobs is no slower. The frontier still waits a large share
of its time on taken decodes: the workers' time, not their scheduling, holds
it, and the second decode behind it stays (D117, `KD143`). The default worker
count is 15.7.1's (`KD145`).

What changed, D120:

- **Decodes have a room of their own.** `Ahead::top_up` hands out a decode
  while fewer than `DECODES_A_TASK` a task are out or held, and otherwise a
  speculation while fewer than `GUESSES_A_TASK` a task are out or held as
  guesses. `gz_seek::Parallel` hands the two out and counts them apart
  (`decode_task`, `speculation_through`, `decodes_pending`,
  `guesses_pending`). `Chunking::task_bytes` charges both multiples.
- **A speculation is placed a lead past where it would land**
  (`Parallel::set_lead`, `Chunking::SHIPPED`'s `lead`). Where the caller's
  bound does not reach that far, which a fetched pass's download sets, it is
  handed out where it lands, as before.
- **The instrument is committed**: `crates/gz-seek/examples/spec_cost.rs`.
  Its `par` shape drives the pass as `ahead.rs` does, with no scan beside
  it, and prints the account below. The room and the lead are its
  environment's to vary (`DECODES`, `GUESSES`, `LEAD`).

## The account at four jobs

None of it is a figure. `runs/p15.17.4/README.txt` maps every file.

- **Where the opening tree's time went** (`spec_cost par4`, `par.txt`). Nearly
  every chunk was speculated, and a little over half were taken. The frontier
  decoded the rest itself, and spent about a third of its time waiting on
  taken decodes. Each had run about a third of a decode's time when the
  frontier reached it. Workers were idle about a sixth of the time. A taken
  decode sat queued for several milliseconds before a worker took it. The
  guesses held filled the one room `top_up` shared between both kinds, so
  the room held decodes back. Runs of taken chunks averaged three or four.
- **The profile** (`account/cycles-j4-flat.txt`, `pgdt` at four jobs): the
  speculation's marked decode is under half the CPU and the engine's inflate
  a third. Feeding a task its input is a few percent.
- **Page faults by call stack** (`account/page-faults-j4.txt`): a few
  thousand over the pass, several times the serial pass's. The engine
  writing into a fresh decode buffer accounts for a fifth of them. A task's
  input growing accounts for about a third, and `run_task` zeroing its read
  buffer for about a sixth. At even a few microseconds a fault, that is well
  under a percent of the pass's CPU. That is arithmetic, not a reading. So
  the faults do not name a decode's buffer, and it stays unpooled (D117).
- **On this tree**, at the same knobs: speculations drop by about a fifth and
  taken chunks rise. Runs at the frontier average about six, workers idle
  under a tenth, and a waited decode has run about two thirds of its time.
  The frontier's waits fall from about a third of its time to about a
  quarter.

## What the probe found

`runs/p15.17.4/gate/`, read by `pairs.py`: 15.17.1's `probe.sh`, seven reps,
alternating this tree's `pgdt` and the opening tree's within each rep, every
leg in both.

- **Four jobs** beat the opening tree in 7 of 7 pairs, by about a quarter of
  its time. It beat this tree's serial pass in 7 of 7 pairs. The opening
  tree's four jobs was level with its serial pass.
- **Twenty-four jobs** beat the opening tree in 5 of 7 pairs, its median a
  little faster. Two, twelve and serial are level. Under the probe's
  `--memory`, this tree's larger task charge affords twenty workers at
  `--jobs 24`, where the opening tree ran twenty-four, so that leg compares
  twenty workers with twenty-four.
- **Twelve beat twenty-four in 7 of 7 pairs on both trees.** That falsifies
  D2's rejection of a first pass capped at its physical cores, which this
  change strikes. Whether the default moves is the maintainer's call.

## What the next slice inherits

- **The waits are the workers' time.** At four jobs the workers are nearly
  always busy, the pass doing over twice the serial pass's work: the
  speculation, and the engine's decode of each taken chunk again. So the
  frontier waits however its decodes are scheduled, and only removing work
  shortens them. Removing the engine's decode means handing out native
  plaintext, which D117 refuses; the speculation's cost is `KD143`'s.
- **The default worker count is 15.7.1's.** A local first pass still
  recommends its logical count (`default_workers`). On this machine that is
  slower than its physical count, in every pair on both trees (`KD145`).
  `--jobs` counts the frontier and not the scan, so the logical count runs
  one thread more than there are CPUs; 15.7.1's leg at the logical count less
  one tells that apart from a core shared.
- 15.7.1 and 15.18 read this tree. 15.18 settles `task_bytes`, which now
  charges two held decodes and five guesses a task (`KD141`). Under
  `gzip-resident`'s allowance that charge affords `control_gz` twenty workers
  at `--jobs 24`, so that cell refuses itself (`resolved_jobs`) until 15.18's
  sitting answers the count or the allowance. `test_measure.py`'s stated
  bills hold the one- and four-job bills this tree's runs stated.

## Negative results

All in the emulation (`par.txt`).

- **Runs with gaps** of a fixed length, at the tasks' count or a few more,
  with gaps of one to three chunks the frontier decodes unguessed: no
  faster than the free-running pass.
- **One run of guesses in flight at a time**: the frontier's waits fall to a
  small share at four jobs, but the pass is no faster there and slower at
  eight and twelve. A run is held until the frontier reaches the last one's
  start, so its guesses land late.
- **A worker kept for decodes** (`SPEC_CAP`): slower, as 15.17.2 found.
- **The frontier running a queued decode while it waits**: no faster.
- **A lead alone, the room unchanged**: slower than the opening tree. The
  guesses it places further ahead fill the shared room sooner.
- **A lead of two to six chunks past the lag** is one plateau at eight and
  twelve jobs. Four jobs gains a little across it. Three or four times
  the lag instead overshoots, and the workers idle.
