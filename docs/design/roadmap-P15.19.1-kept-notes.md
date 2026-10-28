# P15.19.1 notes — the frontier kept on one thread

**The frontier's every read now runs on one blocking thread, kept from read to
read (`docs/design/decisions.md`, "D126"), and the falsifier did not fire.** A
hand probe on release-image builds reads the level-1 file's default count
level with serial, where the opening tree's is beaten by it. The level-6
files' default rows are level with or beating the opening tree's. The gate is
still owed: `gzip-first-pass` re-taken from a commit carrying D126. So the box
stays empty.

## The account from the code

- **A scan read is one job**, and `FirstPass::run_pulled` ran each as a
  `spawn_blocking` of its own. A cold `parse` reads `SCAN_CHUNK_DEFAULT_SIZE_BYTES`
  at a time, about 3,100 reads over `control_gz_quick`'s plaintext.
- **tokio's blocking pool wakes one idle thread; Linux picks the longest idle.** Its
  idle threads wait on one `Condvar` (`tokio-1.53.1`,
  `runtime/blocking/pool.rs`, `Inner::run`), and `spawn_task` calls
  `notify_one`. The futex wakes its waiters in FIFO order, so successive jobs
  go round the idle threads in turn. `kept/tests.rs` asserts this rotation as
  the contrast to the kept thread.
- **So the frontier hops only where other blocking threads are idle.** At one
  job the pool holds the frontier's thread alone. With tasks ahead, each task
  leaves its thread idle once it lands. Over the level-1 file, the pause
  (D125) stops the tasks early, so the frontier goes round all twelve threads,
  a different core each read. On the 3900X, three cores share an L3, so most
  hops also leave the L3 the frontier's state sits in.

## The readings

`runs/p15.19.1/`, contained, on release-image builds: `pgdt-head` is the
shipped build the 2026-10-28 sitting made (`d17eccb5`'s code), `pgdt-kept` is
this tree. Both binaries are in `/mnt/ssd/fedora/scratch/pgdump_query/p15.19.1/`.
Not figures.

- **`perf stat` cannot count switches or migrations here.** At
  `perf_event_paranoid` 2 every event gets `:u`, and these software events are
  kernel-side, so both read zero (`head/stat-*.csv`). `sched.py` reads each
  thread's `/proc/<pid>/task/<tid>/sched` instead.
- **Both counts sleep and wake equally often**: about 6,200 voluntary
  switches a pass at one job and at twelve, two a read (`sched-head.txt`).
  So the wake-up count is not the difference. Migrations do not show the hop
  either: each thread mostly wakes on the core it last ran on, so the
  frontier's change of core shows up as a change of thread, not a migration.
- **The kept thread holds the frontier** (`sched-kept.txt`): at twelve jobs
  one thread runs about 8.9 s of the pass, against about 0.9 s on each of
  twelve before.
- **The probe** (`probe.sh`, `pairs.py`): seven alternated reps of both builds
  at one and twelve jobs over the four `gzip-first-pass` files, read by
  `measure.gzpass_beats` (`probe.txt`, `pairs.txt`).
  - The level-1 file's `12` row is **level with its serial pass**: faster in
    2/7, by +0.177 s, under the margin. The opening build's is beaten, as the
    figure read it.
  - The level-1 file's `12` row against the opening build's: level, faster
    in 6/7.
  - The level-6 files' `12` rows against the opening build's: **beating** on
    zlib and zlib-ng-compat (7/7 and 6/7, about 13%), level on GNU gzip
    (5/7, −0.111 s). The hop was hidden under their lift.
  - The serial rows of both builds are level on every file. Their spread is
    about ±10% on the level-6 files, by placement.

## Unattributed

The level-1 file's default row is still slower than serial in five pairs of
seven, by a median 2%. No term is named. Two candidates are already known,
and neither is priced on this build: the speculations handed out before the
first lands (the 15.19 notes, "What the hand probe says"), and the frontier's
inflate slowing beside them (the 15.19 notes, "The opening account"). The
re-take's paired rows read it by the bar. A profile of `pgdt-kept` at one and
twelve jobs would attribute it.

## What the next round inherits

- **The gate, in two halves read as 15.19's were.** Both are taken from a
  commit carrying this tree.
  - The level-1 half: `cd scripts && uv run measure.py --figure
    gzip-first-pass`, about an hour, handed off per
    `.claude/skills/gosub/handoff.md`. It is met when, by 7.1's bar, the
    level-1 file's `1` row does not beat its `12` row. If the `1` row beats
    it, the remainder goes to the maintainer unattributed, per the spec row.
  - The level-6 half: `probe.sh` re-run at `12` on a release-image build of
    that commit against `pgdt-head`. The figure pairs only within its own
    sitting, so it cannot compare two trees.
- **`KD147`** is struck by the re-take if it reads the default level, and
  rewritten to that reading otherwise.
- **`IDLE`** is a chosen constant (STATUS, "Decisions worth another look").
