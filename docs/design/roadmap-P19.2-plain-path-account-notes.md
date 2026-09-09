# P19.2 — the plain path's account

The spec owes an account of `parallel-scan-throughput`'s two bad plain numbers
before any plain default is set, and it names two instruments because only one
half is a discovery. **Both halves are settled and are written below**, and so
is where the account leaves the spec's fork — both branches, on different terms,
under "Where the account lands".

No library code changes here. What lands is a harness change: the profile
recipe now prints a *pair* of profiles read against each other
(`measure.PROFILE_AXIS`), which is what a "where do the extra 41 ms go" question
needs and what a single profile could not answer.

## The `parse` regression is a double read

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

**The sitting has been read, and it confirms the depth term without accounting
for the step that term was charged with.** The depth term is real and it is
measured; the 4→8 step survives the clamp being lifted. Everything above stands
except that one attribution — see "What the sitting says" below, which is why
this slice's fork is not routed here.

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

## What the sitting says

Two `--alone` legs of `parallel-scan-throughput`, both **NOT PUBLISHABLE** and
read against each other rather than against `measurements.md`: a stock control
(`runs/measure-20260909T041637`) and a scratch build whose
`LocalFileSource::hint_parallelism` passes `POOL_DEPTH.max(jobs)` instead of
`POOL_DEPTH` (`runs/measure-20260909T044346`). Two legs rather than one because
the published table was taken at `20fd77c` and `stream.rs` and `main.rs` have
moved since, so the scratch build has nothing published to be read against. The
clamp is lifted on the plain source alone and not by raising the constant, which
also floors the `.xz` source's block pool — so the two compressed columns are a
control on the sitting itself.

Median wall over five reps, plain columns, seconds:

| `--jobs` | 1 | 2 | 4 | 8 | 12 | 16 | 24 |
|---|---|---|---|---|---|---|---|
| `parse`, stock | 0.432 | 0.457 | 0.482 | 0.569 | 0.564 | 0.567 | 0.560 |
| `parse`, clamp lifted | 0.443 | 0.498 | 0.470 | 0.546 | 0.522 | 0.510 | 0.522 |
| typed `query`, stock | 4.75 | 4.85 | 4.80 | 4.82 | 4.81 | 4.87 | 4.85 |
| typed `query`, clamp lifted | 4.79 | 4.88 | 4.78 | 4.84 | 4.82 | 4.83 | 4.87 |

### The typed-`query` suspect is refuted

**`POOL_DEPTH` is not what makes a plain typed `query` flat.** Lifting it moves
no cell by more than 0.04 s (0.8%), every difference is inside the per-rep
spread, the ratio stays 0.98–1.00× across the whole axis exactly as the
published table reads, and the two legs plan identical sub-stream counts (8, 12,
14, 14). The spec's named suspect for this half is spent, and the host probes'
prediction is confirmed inside the apparatus.

**What it is short of, host probes say, is any concurrency at all.** At
`--jobs 16` a plain typed `query` plans fifteen sub-streams — the budget note
names the number — and total CPU never reaches one core: 3.78 s of user in
4.68 s of wall at one worker, 4.18 s in 4.79 s at sixteen. A four-slot pool
would still have shown roughly four workers' worth of decode, so ~0.87 cores
points at a stage every row passes through serially rather than at the pool.
These are host readings, uncontained and ungated: **no document may quote them
as measurements**, and they are kept here only because they name where to look
next.

### The depth term is real, and it is not the 4→8 step

**Where the clamp binds, lifting it helps; where it cannot bind, it changes
nothing.** Below five workers `POOL_DEPTH.max(jobs)` *is* `POOL_DEPTH`, and the
1-, 2- and 4-job rows differ by no more than the sitting's noise — the lifted
leg is the *slower* of the two at one and two workers. Above four it is faster
at every count, and by more as the count rises: 4.0%, 7.4%, 10.1% and 6.8% at 8,
12, 16 and 24. The per-rep ranges are **disjoint** at 12 (0.557–0.577 against
0.504–0.528), at 16 (0.551–0.580 against 0.500–0.521) and at 24 (0.554–0.582
against 0.520–0.529); they overlap only at 8. A leg-level offset does not
explain it — the plain typed `query` column, taken in the same two sittings,
shows none at all (±0.8%, unsigned), and an offset would be flat in the worker
count where this grows with it.

**What lifting the clamp does not do is remove the step.** Measured against each
leg's own four-job row, which is what the depth term was charged with:

| above-four penalty (s) | 8 | 12 | 16 | 24 |
|---|---|---|---|---|
| stock | +0.087 | +0.082 | +0.085 | +0.078 |
| clamp lifted | +0.076 | +0.052 | +0.040 | +0.052 |

The stock leg is flat: workers past four buy nothing and cost a fixed ~0.08 s.
The lifted leg *recovers* as workers are added, which is the shape of a clamp
being released and is the whole of the evidence that the depth term exists. But
the 4→8 step itself is +0.076 s with the clamp lifted against +0.087 s with it
in place — a difference inside the spreads — and the column never turns
positive: its best point above four is **0.87× at sixteen workers**, still worse
than four workers and worse than serial.

So the depth term removes between a third and a half of the above-four penalty
from twelve workers up, and nothing measurable at eight. It is not what the 4→8
step is made of.

**Apparatus.** Both legs passed the harness's contention gate; the lifted leg
ran on the noisier machine of the two (CPU stall ≤8.10% against ≤1.20%, I/O
stall ≤21.15% against ≤9.63%), which biases against the effect reported rather
than for it. Two `.xz` typed-`query` reps in the lifted leg are outliers
(31.30 s and 33.27 s) and sit at the top of their ranges; medians are unmoved.
Neither table may be folded into `measurements.md` — `--alone` marks a
diagnostic sitting by construction, and the lifted leg is a build this project
does not ship.

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

## Where the account lands: both branches, on different terms

**The fork is not exclusive, and reading it as a choice between two arms is what
made the readings look contradictory.** The spec's sentence is *"only one of the
two branches is this phase's"* — a claim about which **repair** the phase owns,
not a claim that the account has a single cause. The account names three costs,
and they route differently:

- **The sizing term** (partition equals chunk, the 1→2 step) and **the depth
  term** (four slots whatever is stated, real above four workers) are the spec's
  **first branch**: both are arguments of `pool.set_limits`, which is what "how a
  stated budget is turned into slots" names. Repair 1 folds into `19.5`.
- **The unattributed 4→8 step**, which survives the clamp being lifted, and the
  **refuted typed-`query` suspect** are the **second branch**: not a defaults
  question, out of this phase, and `19.8` ships the plain default with the
  reason written down.

**Attributing the 4→8 step does not gate anything and needs no re-sitting.**
Whatever it is, it is already outside the phase. The natural suspect is the
per-worker coordination term, and the instrument that would settle it is the one
this slice already used — `futex` counts and system time by worker count — run
at 8, 12, 16 and 24, where the step is. It was run only at 1–4, which is exactly
where the step is not.

## The plain source's worker default is serial, and this settles it

**A plain `parse` is slower than serial at every worker count in both builds.**
Stock's best above four is 0.560 s at twenty-four against 0.432 s serial; the
clamp-lifted leg's best is 0.510 s at sixteen — **0.87×**, still below one
worker. Repair 1 shrinks the sizing term; it does not turn 0.87× into 1.0×.

So `19.8`'s plain arm is **serial**, decided by evidence rather than by how the
fork routes, and it is not blocked on the routing. This is what the spec asked
for: it refuses a plain default of *"serial, because the number is bad and we do
not know why"*, and what stands in its place is "serial, because parallel is
measurably slower at every count on both builds, and here is what the cost is
made of". Only a new reading showing a plain parallel `parse` beating serial
would reopen it.

## What this leaves

**`KD17`** — the typed-`query` half, allocated here rather than deferred. Its
suspect is refuted and the finding is stable, so nothing the parse routing
decides changes its text: they are different mechanisms, read sizing and pool
depth on `LocalFileSource` against a stage every row passes through serially in
the query path. `(c) unowned`; detail beside its mechanism
([`architecture.md`](architecture.md), "What parallelism buys, and where it
stops").

**Repair 2** is out of this phase either way — a `KD<k>` if it is wanted, and it
may not be, since amortizing the tail under repair 1 makes `.xz`'s 4% smaller
still.

The two legs are on disk (`runs/measure-20260909T041637`,
`runs/measure-20260909T044346`) and the sitting need not be re-taken.
