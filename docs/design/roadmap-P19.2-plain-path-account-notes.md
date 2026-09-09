# P19.2 — the plain path's account

The spec owes an account of `parallel-scan-throughput`'s two bad plain numbers
before any plain default is set, and it names two instruments because only one
half is a discovery. **The `parse` half is settled and is written below; the
typed-`query` half is measured and not yet read** — see "What is still open".

No library code changes here. What lands is a harness change: the profile
recipe now prints a *pair* of profiles read against each other
(`measure.PROFILE_AXIS`), which is what a "where do the extra 41 ms go" question
needs and what a single profile could not answer.

## The `parse` regression is a double read, and it is not either branch the spec drew

**A plain `parse` at `--jobs ≥ 2` reads every byte of the file exactly twice.**
Measured on the 3.00 GiB warm control, by summing `pread64` returns:

| | `pread64` calls | bytes read |
|---|---|---|
| `--jobs 1` | 196 | 3.02 GiB |
| `--jobs 2` | 387 | 6.02 GiB |

at a 16 MiB chunk, and 3076 against 6143 calls at the shipped 1 MiB chunk. The
ratio is **flat in the worker count** — 6143 calls at two workers, 6147 at four
— so it is charged per *partition*, not per worker.

**The mechanism.** `LocalFileSource::partitions` answers
`Partitioning::anywhere(pool.slot_bytes())`, so a plain source's partition is
exactly one read chunk. `leader::scan_partition` reads its range, and
`scan_piece` cannot finish inside it: a piece runs through the line ending at or
after its limit, and the bytes handed to it stop *at* the limit. So `done` is
false on the first read, and the loop takes a second read of
`want = options.chunk_size` — a whole further chunk, of which one row is used.
That second read is the next partition's body, read again by the piece before
it.

The tail read is sized as a chunk while the piece is sized as a *partition*, so
the overhead is `chunk / partition`. On a block-shaped `.xz` source a partition
is a whole 24 MiB block and the tail is 1 MiB — 4%, invisible. On a plain source
partition **equals** chunk and the overhead is 100%. That is the whole
asymmetry between the figure's `.xz` legs (5.82× at 24 workers) and its plain
`parse` leg (0.81×).

**It is charged in system time, which is where a warm `parse` already lives.**
Host readings on the release binary, warm control, three reps:

| `--jobs` | wall | user | sys |
|---|---|---|---|
| 1 | 0.38 s | 0.10 | 0.28 |
| 2 | 0.43 s | 0.16 | 0.66 |
| 3 | 0.47 s | 0.23 | 1.04 |
| 4 | 0.44 s | 0.29 | 1.30 |

The read doubles once and stops; the *system* time keeps climbing with workers,
which is the second, smaller term — `futex` calls go 12.6k → 83.7k → 142k over
the same three counts, and that one is per worker. Neither term is user-space
work, which is why the profile the spec asked for could not see them:
`perf_event_paranoid = 2` gives `exclude_kernel = 1`, so a `perf record` on this
machine samples user space only. **A profile of this shape is therefore blind to
its own answer**, and the pair of profiles is still worth having for what it did
show — the two `--jobs` legs run *different* code (`stream::map_forward` at 7.9%
of the serial leg's cycles, `leader::scan_piece` at 4.7% of the parallel one's,
and three scanning threads at `--jobs 2` rather than two) with total user cycles
up only 8%.

## The double read is one of two terms, and it is the smaller one

**The published column degrades in two steps, and the double read can only be
the first.** `parallel-scan-throughput`'s plain `parse` leg, which is the number
this account owes an explanation for:

| `--jobs` | 1 | 2 | 4 | 8 | 12 | 16 | 24 |
|---|---|---|---|---|---|---|---|
| wall | 0.471 | 0.512 | 0.512 | 0.580 | 0.578 | 0.577 | 0.579 |
| ratio | 1.00× | 0.92× | 0.92× | **0.81×** | 0.81× | 0.82× | 0.81× |

That is +0.041 s at 1→2, flat through 4, then **+0.068 s at 4→8**, then flat to
24. Both steps are far outside the per-rep spreads. The double read is flat in
the worker count by its own measurement, so it accounts for the first step —
roughly 38% of the total regression — and is silent on the rest.

**The second step lands exactly at `POOL_DEPTH`, and the tree already says so.**
`LocalFileSource::hint_parallelism` passes `POOL_DEPTH` — 4, unconditionally —
as the pool's depth, and the doc comment above it says that above four workers
the extras "block for a slot rather than allocating … a ceiling on plain-file
worker throughput that no figure has priced yet". `measurements.md` publishes
the same claim about this very column: *"`POOL_DEPTH` clamps the chunk pool to
four slots, so a fifth fused worker on a plain source waits: the rows above four
say what that ceiling costs."*

**So the account is of two terms, and both are the spec's first branch.** A
*sizing* term — partition equals chunk, flat in workers, the 1→2 step — and a
*depth* term — four slots whatever is stated, the 4→8 step. `slot_bytes` and
`depth` are literally the two arguments of `pool.set_limits`, which is what "how
a stated budget is turned into slots" names. Nothing yet points at the second
branch: futex counts do grow per worker, but wall is flat from 8 to 24 where
that term would still be climbing, so per-worker coordination is not what the
plateau is made of.

**This is provisional until the sitting's plain `parse` column is read**, and
that column is the discriminator. The scratch build is `POOL_DEPTH.max(jobs)` in
`LocalFileSource::hint_parallelism` — the plain source's own clamp — so the
sitting is an instrument for *both* halves of this slice, not only the
typed-`query` one, and it carries the only measurement in the apparatus that
reaches the 4→8 step. If the plain `parse` leg improves above four workers under
the lifted clamp, the depth term is confirmed and the account lands on the pool
depth. If it does not, the 4→8 step is something else and the second branch is
back in play.

*Rejected:* routing the repair on the sizing defect alone, before the sitting is
read. That was this slice's first call and it was reversed on review: the
account as written explains a third of the number it is the account of, and the
spec's fork asks where *the account* lands. The measurement that settles it was
already running when the call was made.

**Two repairs are available and the sitting does not decide between them**, so
the slice that takes it should:

- **make a plain source's partition several chunks** — the tail is then
  amortized to `chunk / partition` exactly as it is on `.xz`, and nothing about
  `scan_partition` changes; or
- **size the tail read to a row rather than to a chunk** — `want` on the second
  iteration is currently `options.chunk_size`, which is the read loop's steady
  state and not what a resync needs. `scan_partition` already grows `want` when
  a line boundary is not found, so a small tail that doubles is the shape the
  code is written for.

The first is a defaults change and reaches only the plain source; the second
reaches every source and would shrink `.xz`'s 4% too. Neither is written here.

**The two are not on the same side of the fork, and only the first is this
phase's.** Repair 1 edits `LocalFileSource::partitions` — a source's own answer
about itself, which is the defaults shape. Repair 2 edits
`leader::scan_partition`, reaches every source, and changes what the leader does
per piece: that is the mechanism-redesign shape the fork's *second* branch
describes, so it takes a `KD<k>` if it is wanted at all — and it may not be,
since amortizing the tail under repair 1 makes `.xz`'s 4% smaller still.

**Repair 1 folds into `19.5` rather than taking a slice number of its own.**
`19.5` is already opening `Partitioning` to state its retained unit, and its
rejected-alternatives paragraph turns on exactly what a plain source's partition
means economically — two slices editing that type's meaning a week apart is how
the second contradicts the first. This supersedes the spec's pre-authorization
to admit the repair as the next free number ahead of `19.8`, which was written
before anyone knew the repair touches the type `19.5` already opens.

**What it does *not* mean.** `architecture.md`'s "A worker is two reads, and the
second is why a partition's footprint is a block *plus* a chunk" stays true —
the footprint accounting is right and the *sizing* is what is wrong. Nothing
about the interior split's contract, the piece semantics or the fold is touched
by either repair.

## What is still open

The typed-`query` half. The spec's named suspect is `POOL_DEPTH` clamping the
chunk pool to four slots, and its instrument is a `--figure
parallel-scan-throughput --alone` sitting against a scratch build with that
clamp lifted. **That sitting serves both halves**, which was not the intent when
it was launched: the figure carries a plain `parse` column too, and the clamp it
lifts is the one the `parse` account's second term is charged to. **The sitting was launched and not read** — it is two legs, a
stock control at this commit and the scratch build, because the published table
was taken at `20fd77c` and `stream.rs` and `main.rs` have moved since, so the
scratch build has nothing at this commit to be read against. Both legs are
`--alone` and NOT PUBLISHABLE by construction.

`runs/19.2-pooldepth-20260909-0416/HANDOFF.md` says how to read it.

**The scratch build is `POOL_DEPTH.max(jobs)` in `LocalFileSource::hint_parallelism`,
not a raised `POOL_DEPTH` constant.** Raising the constant reaches the `.xz`
source's chunk pool and, through `POOL_DEPTH.max(jobs)` in `apportion`, its
*block* pool floor — so it would move the two compressed legs the plain legs are
being read against. Lifting the clamp where the suspect is leaves those two legs
byte-identical, and it is also the shape the repair would take.

**Host probes predict the sitting will come back "not the pool depth", and that
is an answer the spec already provides for** — *"a profile that comes back
inconclusive is itself the answer"*. What they show is that a plain typed
`query` never runs concurrently at all, whatever the pool depth:

| `--jobs` | wall | user | CPU used |
|---|---|---|---|
| 1 | 4.68 s | 3.78 | 0.81 |
| 4 | 4.75 s | 4.01 | 0.85 |
| 8 | 4.77 s | 4.13 | 0.87 |
| 16 | 4.79 s | 4.18 | 0.87 |

Fifteen sub-streams are *planned* at `--jobs 16` — the budget note names the
number — and total CPU never reaches one core. A pool of four slots would still
have shown roughly four workers' worth of decode; ~0.87 cores says the
sub-streams are not overlapping, which points at the serialised stage every row
passes through rather than at the pool. These are host probes outside the
apparatus and no document may quote them; the sitting is what answers it inside
one.

## The harness change

`measure.PROFILE_AXIS` is a second, **explicitly paired** list beside
`PROFILE_SHAPES × PROFILE_INPUTS`, holding `("parse-jobs-1", "control")` and
`("parse-jobs-2", "control")`.

- **Paired, not a second cross product.** The three baseline shapes are crossed
  with both inputs because each asks what a *path* costs and the two files reach
  different paths. This one asks what one figure's one anomalous row is made of,
  and that row is on `control`.
- **`profile_argv` grows a `JOBS_AXIS` arm**, so the pair states
  `--parallel-memory` exactly as `_script` does — including on the one-worker
  leg, where `Serial` makes it inert. `ProfileRecipe`'s shape-equality
  reconciliation covers the two new shapes by construction, since it now
  iterates `PROFILE_SHAPES + PROFILE_AXIS`.
- Two new assertions: the pair's two argvs differ in **exactly one token pair**
  (a difference read bucket by bucket is only a difference if nothing else
  moved), and the pair's input is staged and torn down whether or not
  `PROFILE_INPUTS` already carries it.
- The staging and teardown steps now compute their input list from both, so a
  pair added on an input the cross product does not carry needs no second edit.

The recipe's own "the worker count, stated rather than inherited" bullet no
longer says every invocation states `SWEEP_JOBS`: the pair states the count in
its own name, which is the whole of what separates its two profiles.

## For the session that closes this slice

1. Read `runs/19.2-pooldepth-20260909-0416/HANDOFF.md` and both legs'
   `tables.md`. **Read the plain `parse` column as well as the typed-`query`
   one** — the scratch build lifts the plain source's own clamp, so that column
   is what discriminates the fork, and it is the only reading here that reaches
   past four workers.
2. Write both halves of the account into this doc, then tick `19.2`.
3. **Then** route the repair, which this slice deliberately does not do. If the
   plain `parse` leg improves above four workers, the account lands on the pool
   depth and repair 1 is **folded into `19.5`**, whose spec row already opens
   `Partitioning` — not admitted as a new number ahead of `19.8`. If it does
   not, the 4→8 step is unaccounted for and the second branch is live again;
   that is a call for the maintainer, not for the closing session.
4. Repair 2 is out of this phase either way — a `KD<k>` if it is wanted, and it
   may not be.
