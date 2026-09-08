# P16.14 — koji verification

The phase's only check at real scale. `16.12` asserts cache byte-identity
across `--jobs` over 109 kilobyte-scale fixtures plus one generated dump; this
slice asserts it over a 40 GB compressed file whose 784 GB of plaintext holds
74 blocks, one of them tens of gigabytes on its own. Nothing in the fixture
tree reaches a block that spans dozens of read windows, a cut that lands inside
a multi-gigabyte region, or a `.xz` seek table with 31,150 entries in it.

**It passes on both halves.** The durable record is
[`measurements.md`](measurements.md), "koji full scan", under "The `.xz` copy
scans to the same answer serially and four ways" — that section is **outside
the register**, so every reading in it carries its disqualification at the
number and no document may quote one as a measurement.

## The check, and why it is two checks

The spec row was amended on the day of the run
([`../status/history/2026-09-08.md`](../status/history/2026-09-08.md), "A
compressed cache and a plain one were never byte-comparable"): it had asked for
byte-identity against the cache the serial 784 GB *plain* scan left behind,
which is an artifact nothing produces. So:

- **Across arrangements** — two legs over the *same* `.xz`, taken in one run,
  whose `.dqcache` files must be byte-identical to each other. This is the
  determinism claim, and it is the half that needs both legs taken fresh.
- **Across formats** — each leg's reported block, row and byte counts must
  equal the plain scan's published totals. This is the half that says the
  compressed path reaches the same answer as the plain one, and it works
  because those totals are properties of a fixed file rather than readings off
  a moving build.

Both hold. Serial and `--jobs 4` produced the same 2,144,936-byte cache byte
for byte, and both reported 74 blocks, 19,575,829,920 rows and — read back with
`pgdq info --dqcache … --detail` — `Scan completion: 100% (784019857152
bytes)`, which are the three numbers the "koji full scan" table already
carried.

A third datum came free. The first attempt's serial leg completed cleanly
before its parallel leg was killed, and its artifacts were archived rather than
deleted (`runs/koji-xz-attempt1/`), so this run also compared serial against
serial across two separate runs hours apart: byte-identical.

**This is `M70`'s fix at real scale.** `M70` — the block pool granted a wait it
could never satisfy — was closed against a 200 MiB scratch `.xz`. The
arrangement that deadlocked there is exactly this leg's (`--jobs 4` on a
block-decoding source), and here it ran a 40 GB file to completion.

## What the run was, mechanically

`runs/koji-xz-parallel-verify.sh`, detached per `CLAUDE.md`'s long-running-job
protocol, 14:19:17 → 15:25:25 UTC on 2026-09-08;
`runs/koji-xz-verify-20260908-b/orchestrator.log` is the whole account. The
binary was `target/release/pgdq` built at `e29939c` — the commit carrying
`M70`'s fix and `16.13`'s apparatus, and the same code HEAD has, every commit
since being documentation.

The input is `koji-2026-07-23.dump.multistream.xz`: the upstream download,
40,397,009,888 bytes, 31,150 concatenated one-block streams of ~24 MiB
plaintext each. **Not the recompressed `blocks128.xz`**, and that is a
constraint rather than a preference — 128 MiB blocks would need most of a
512 MB cgroup for four concurrent block buffers, so the run would have been
budget-clamped to two workers and would not have exercised the arrangement it
was convened to check.

Both legs ran in `postgres:16` with `-m 512m --memory-swap 512m --cpus 4 -e
MALLOC_ARENA_MAX=2`, the dump mounted read-only and `--dqcache` pointing into
the mounted `/out`, with `pgdq` `exec`'d as PID 1. Those last two are
`CLAUDE.md`'s load-bearing recipe details and they are load-bearing here for
the usual reasons; what is new is the two settings in front of them.

## The two container settings, and which one is doing the work

**`MALLOC_ARENA_MAX=2` is why the run fits.** The first attempt's parallel leg
was OOM-killed at 509 MiB anonymous resident inside the same cgroup, and the
cause is filed beside the mechanism
([`architecture.md`](architecture.md), "Execution model and API surface"):
`#[tokio::main]` sizes its runtime from the CPUs the process can see, each
worker seeds a glibc arena before a byte is scanned, and an arena retains what
its owning thread allocated. Roughly 200 MiB of that resident set was arena
retention rather than anything the library holds.

**`--cpus 4` is not the fix and is not carried as one.** Measured on its own it
cuts threads 31 → 10 and arenas 29 → 8 and still reaches ~476 MiB — what an
arena retains is not proportional to how many there are. It is in the recipe to
hold the thread count fixed across the whole scan, so the memory sampling
describes one arrangement rather than an average of several.

**`--parallel-memory 268435456` is not a lever either**, and the run confirms
it at scale: `BufferPool::slots()` clamps to `POOL_DEPTH.max(jobs)` = 4, so
128 MiB stated and 256 MiB stated afford the same four block slots. That the
stated number stops bounding the process above that point is `16.15`'s
question.

**That setting is flagged for review.** It is an entry under `STATUS.md`'s
"Decisions worth another look" — tuning the allocator, which
[`measurements.md`](measurements.md)'s "The apparatus" calls part of the
apparatus, in order to make a run pass. It was made because this slice checks
determinism and an arena count cannot change a cache byte. The entry is open
and this slice does not close it.

## What the memory sampling says, and what it does not

Each leg sampled every 60 s into `runs/pgdq-koji-xz-<leg>-mem.log` — the
cgroup's `anon`, the process's `VmRSS` and `VmHWM`, its thread count and its
arena count. The first attempt captured none of this, which is why diagnosing
it cost a second run; a leg that dies without a memory log leaves nothing to
diagnose but its exit code.

The parallel leg plateaued at ~330 MiB anon with 10–12 threads and **one
arena**, peaking at 404 MiB `VmHWM` — against the 509 MiB that got the first
attempt killed, and with ~108 MiB of headroom in the 512 MB cgroup. The serial
leg sat at 68–73 MiB anon, peak 78 MiB.

**`memory.current` is the wrong instrument and the logs show why**: it reads
503–512 MiB on *both* legs, because the cgroup is charged the page cache of a
40 GB read. A reader who takes that column for the process's own footprint
concludes the serial leg also filled the cgroup, when it held 73 MiB.
`measurements.md` already says `memory.peak` is charged this way; the sampled
`anon` and `VmHWM` columns are what to read.

## What is deliberately not claimed

**The wall times are not a throughput reading.** Leg 1 took 2644 s and leg 2
1322 s, and the ratio is not a speedup figure: no `cat`-to-`/dev/null` floor
was taken, the allocator was tuned away from the shipped default, the CPU count
was capped, the legs ran back to back with no quiet-machine gate and no
repetition, and leg 2 read a file leg 1 had just pulled through the page cache
in part. The registered figure for `--jobs` against throughput is
`parallel-scan-throughput`, taken warm at `e29939c`.

**Nor are they comparable to the plain koji rows.** Those scan 784 GB off the
HDD; these move 40 GB and spend the difference in the decoder. The section's
own standing rule — koji throughput rows are outside the register — covers
both, and these are further outside it than the rows already there.

**The `.xz` determinism assertion still does not exist in the test suite.** It
exists at koji scale, once, on one machine, in a run nobody can repeat cheaply.
`16.12`'s fixture sweep is plain `.sql`; `xz_source.rs` carries the compressed
source's end-to-end row parity and `parallelism.rs` its budget parity, neither
on cache bytes. A compressed `parse` at two job counts writing one cache is
still cheap to assert wherever a slice next touches that path, and doing so
would put the claim somewhere CI can see it.

## What `16.15` inherits

- **A confirmed reading to work against.** The process holds ~330 MiB anon at a
  stated 256 MiB budget with `--jobs 4`, sustained over a full scan rather than
  over a ten-minute probe. `16.15` makes the stated number bound both memory
  terms; this is the "before".
- **The clamp is the reason the stated number stops mattering**, confirmed at
  scale: four slots at 128 MiB stated and four at 256 MiB.
- **The allocator term is larger than the pool term under a memory cap**, and
  no change to how the budget divides touches it. `16.15` can make the stated
  number honest about what the pools hold and the process will still carry
  arena retention on top; the remedy for that stays `MALLOC_ARENA_MAX`, which
  is a property the manual already documents.

## For whoever runs the next koji verification

- **The script does not resume a killed leg.** Re-running it restarts leg 1
  too, which is an hour thrown away. A leg that has to be stopped is stopped
  with `sudo nerdctl stop`, which reaches the interrupt guard and banks the
  cache; the same command then resumes it.
- **Archive rather than delete.** Moving the previous attempt's caches aside
  instead of removing them is what bought the serial-versus-serial comparison
  here for nothing.
- **The orchestrator compares the caches itself** — it runs `cmp` and logs the
  verdict — but it only *prints* the expected counts beside each leg's tail for
  a reader to compare, and it does not read the byte total back at all. That
  last one is `pgdq info --dqcache <cache> --detail`, run afterwards on the
  host, and it is the third of the three cross-format numbers.
