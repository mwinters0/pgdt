# P19.16 — the reserve constant, read off five builds

The number `19.13` ships. The spec row is
[`roadmap-P19-efficient-defaults.md`](roadmap-P19-efficient-defaults.md),
"Slices"; the constant is `MEMORY_RESERVE` in `pgdump_query/src/io.rs`, and the
rule that spends it is [`architecture.md`](architecture.md), "Execution model
and API surface".

**The pick is 384 MiB** — the smallest of five candidates whose worst rep leaves
at least 20% of the limit at every one of its eight legs, with nothing exiting
non-zero and nothing OOM-killed. **Its margin at the two legs that decide it is
0.2 and 0.3 percentage points, and the apparatus's own noise at a comparable
arrangement is 2.1** — so the criterion selects 384 and the reading cannot
separate it from a failure. That is the one thing this slice hands over that is
not a number: see "The pick is inside the noise", below, and the entry under
`STATUS.md`'s "Decisions worth another look".

## Apparatus

A `runs/` probe, **not a figure** — no document may quote a number below as a
measurement, and nothing in the repo consumes the output
([`measurements.md`](measurements.md)).

- `runs/19.16-reserve-constant-probe.py`, detached; the sitting is
  `runs/19.16-reserve-constant-20260911-2210/` — `probe.log` (the per-rep lines,
  the per-candidate verdicts and the `PICK:` line) and `readings.json` (all 400
  runs, each with its resolved count, its budget, its `argv` and its stderr).
- **Five binaries differing in nothing but the constant** — 256, 320, 384, 448,
  512 MiB — from a detached worktree at `bb8744f`, one `sed` on `MEMORY_RESERVE`
  each, kept as `runs/pgdq-19.16-r<N>`. A candidate is a binary rather than a
  `--parallel-memory` value because a *stated* budget keeps the source's
  unlowered worker count, which the flagless default never produces
  ([2026-09-11](../status/history/2026-09-11.md), "`19.16` is launched: five
  reserves, one criterion, and the choice is a binary").
- **The whole flagless family**: two block sizes (`control_xz.xz` at 24 MiB,
  `control_xz128.xz` at 128 MiB) × four container limits (512m, 1g, 1536m, 2g),
  ten reps, rep-outer and reversed on alternate reps. 400 timed runs, ~68
  minutes.
- Instrument and container parameters are `19.15`'s, which are the `reserve`
  figure's minus the two flags: `getrusage` through a forking `perl` wrapper,
  `-m X --memory-swap X`, `postgres:16`, `parse --source /dump.sql --dqcache
  /tmp/x.dqcache`. The machine was not quiet, which is the ordinary reason a
  probe is not a figure.

## The reading

Worst-rep headroom of ten, per candidate and leg. `<<` marks a leg under the
20% floor; the resolved reader count is in parentheses.

| leg | r256 | r320 | **r384** | r448 | r512 |
|---|---|---|---|---|---|
| xz24 512m | 23.9% (4) | 35.0% (3) | **50.8% (2)** | 87.7% (1) | 97.0% (1, declined) |
| xz24 1g | 9.0% << (13) | 19.2% << (12) | **20.2% (11)** | 36.1% (9) | 39.2% (8) |
| xz24 1536m | 11.4% << (22) | 16.7% << (20) | **20.3% (19)** | 18.6% << (18) | 21.8% (17) |
| xz24 2g | 27.0% (24) | 26.3% (24) | **26.0% (24)** | 26.2% (24) | 24.9% (24) |
| xz128 512m | 97.0% (1, declined) | 97.0% | **97.0%** | 97.0% | 97.0% |
| xz128 1g | 21.7% (2) | 21.7% (2) | **21.7% (2)** | 21.7% (2) | 72.7% (1) |
| xz128 1536m | 29.8% (4) | 29.9% (4) | **29.9% (4)** | 29.9% (4) | 38.8% (3) |
| xz128 2g | 21.5% (6) | 21.6% (6) | **21.6% (6)** | 21.5% (6) | 34.4% (5) |
| **verdict** | fails | fails | **PASSES** | fails | PASSES |

## The rule resolves exactly, at every one of the forty cells

`19.16`'s only apparatus check was that a candidate resolve the count predicted
for it. It is stronger than that: across all forty cells the resolved count is
`min(24, max(1, floor((limit − reserve) / per_reader)))` and the budget
requested is `count × per_reader` **to the byte**, never the cap —
58.03 MiB a reader at 24 MiB blocks and 266.03 MiB at 128 MiB, the two numbers
`XzSource::block_reader_bytes` bills. The three floors behave as their doc
comments say: at a 512 MiB limit a 448 MiB reserve leaves 58.0 MiB and resolves
**one** reader on the block path, and a 512 MiB reserve leaves zero, where
`BlockCache::affordable` declines block decode and the run holds 15 MiB.

So the account predicts the grid and the reading only had to price the arena
retention on top of it — which is what `19.18` said the reserve is for
([`roadmap-P19.18-compressed-account-notes.md`](roadmap-P19.18-compressed-account-notes.md)).

## The pick is inside the noise

Three readings from this sitting's own data, none of which needed another run:

**The gate is non-monotonic in the reserve.** 384 passes, 448 fails, 512 passes.
The failure is one leg, xz24 1536m, and it is a *tail*, not a shift: r448's
median there is 1119.8 MiB against r384's 1203.6 — 84 MiB roomier, exactly the
reader it dropped — while its worst rep is 1250.3 against r384's 1224.4. Eight
of r448's ten reps sit in 1096–1124 MiB and two sit at 1215.5 and 1250.3.
Medians are monotone in the reserve at every leg; only the worst-of-ten is not.

**The apparatus's noise is 2.1 percentage points, measured where the reserve is
provably inert.** At xz24 2g all five candidates resolve 24 readers on a
1392.8 MiB budget — the source's recommendation binds, so the reserve does not
enter — which makes those five groups of ten reps *the same arrangement measured
five times*. Their worst-rep headrooms are 27.0, 26.3, 26.0, 26.2 and 24.9%: a
2.1 pp range on a quantity the constant cannot move. The three xz128 controls
of the same kind (1g, 1536m and 2g at r256–r448, identical counts and budgets)
range 0.0, 0.1 and 0.1 pp, so the scatter belongs to the many-reader 24 MiB
family and not to the instrument.

**384's two deciding legs clear the floor by 0.2 and 0.3 pp.** At xz24 1g its
worst rep is itself an outlier — 816.9 MiB against a next-worst of 791.4 — and
the floor is 819.2, so **2.3 MiB more would have failed it** and made 512 the
pick, since 448 already fails. The other deciding leg has 4.4 MiB in hand. The
two candidates are not adjacent in what they cost: 512 also declines the block
path in a 512 MiB container and drops a reader on three of the four xz128 legs.

None of that overturns the pick. The criterion was registered before the
sitting, 384 met it and no smaller candidate did, and choosing differently after
reading the data is the move the phase has already been burned by twice. What it
does mean is that "384 passes and 448 fails" may not be re-derived as a property
of the constant, and that the maintainer has a real choice between the
criterion's answer and the one candidate with margin to spare.

## What the constant costs, in the column nobody was studying

Median wall clock, the 24 MiB-block file (the 128 MiB one is unchanged from 256
through 448, since 64 MiB steps of reserve never cross its 266 MiB reader):

| limit | r256 | r320 | **r384** | r448 | r512 |
|---|---|---|---|---|---|
| 512m | 9.91s (4) | 12.55s (3) | **17.49s (2)** | 16.95s (1) | 16.45s (declined) |
| 1g | 4.88s (13) | 4.92s (12) | **5.24s (11)** | 5.83s (9) | 6.08s (8) |
| 1536m | 3.85s (22) | 4.02s (20) | **4.01s (19)** | 4.21s (18) | 4.38s (17) |
| 2g | 3.83s | 3.82s | **3.80s** | 3.77s | 3.71s |

**384 costs one arrangement and leaves the rest alone**: the 512 MiB container
goes 4 readers → 2 and 1.77× slower, and every other leg is within 8% of what
256 does. Going on to 512 would cost a further 16% at 1g, 20% at xz128 1536m and
23% at xz128 2g. That asymmetry is the argument for the criterion's answer over
the roomier one, and it is why the choice is worth putting rather than
splitting.

## The block path buys nothing below three readers, on this file

The 512 MiB column above is a clean reader-count sweep — one limit, one file,
only the count varying — and it answers the reading
[`roadmap.md`](roadmap.md)'s "A block cache whose retention floor tracks the
reader count" asks for. Four readers is 1.66× the streaming fallback and three
is 1.31×; **two readers is 6% *slower* than declining and one reader is 3%
slower**, while holding 250 MiB and 63 MiB respectively against the fallback's
15. So the floor is not merely charged at low counts, it is charged for nothing:
below three readers the block path costs memory and returns no time. The Future
item is where that is filed; nothing in this phase acts on it.

## One reader on the block path, reached the other way

At a 448 MiB reserve the 512 MiB container resolves one reader and holds
**62.1–62.9 MiB across ten reps, spread 0.8 MiB**. The published
`parallel-peak-rss` figure's serial cell at 24 MiB blocks is **62.93 MiB
(62.50–63.25)**, and `19.18`'s blocksize-charge probe reads 62.9.

**What the three share is the instrument, and what they do not share is the
route.** All are `getrusage` maxrss on this machine over the same input, so this
is not independent mechanisms agreeing. The published cell is `--jobs 1` with a
**stated** `--parallel-memory` of 2 GiB in a 3 GiB container, and `19.18`'s is a
stated count in an 8 GiB one; this is a flagless run in a 512 MiB cgroup where
the *rule* resolved the count and left it 58.0 MiB. Agreeing to within a
megabyte across budgets that differ thirty-five-fold, in containers six and
sixteen times larger, is a reading about the budget rather than about the
instrument: above `jobs ×
what a reader holds` the bytes are inert, which is the property
`BufferPool::slots`' clamp is supposed to give and which nothing had checked at
one reader. This is the cell the spec row said no sitting had measured, and it
is the cell that falsified `19.15`'s `403 MiB + 31.2 MiB` a reader, which
predicts 436.

## What `19.13` inherits

- **The constant is 384 MiB**, and the pending choice above rides with it.
- **`MEMORY_RESERVE`'s doc comment carries a refuted number** and is corrected
  by the change that ships the constant. It cites `403 MiB + 31.2 MiB` a reader
  as what "a block-decoding `.xz` scan holds" and says a 512 MiB allocation comes
  within three megabytes of its limit; the fit is refuted
  ([`roadmap-P19.19-per-file-term-notes.md`](roadmap-P19.19-per-file-term-notes.md)),
  the account is `4 MiB + 49.0 MiB` a reader plus arena retention, and at
  384 MiB a 512 MiB allocation leaves 50.8%. Not corrected here because this
  slice ships no library code and `19.13` owns that comment.
- **The decline moves with the constant, as the spec said it would.** At 384 MiB
  a 512 MiB container still takes the block path on the 24 MiB file (two
  readers) and still declines it on the 128 MiB one; the reserve at which the
  24 MiB file declines is between 448 and 512. The decline's report is `19.13`'s
  row.
- **The candidate binaries and the sitting stay** under `runs/` until `19.13`
  has shipped: `runs/pgdq-19.16-r384` is the arrangement it must reproduce, and
  a resolved count that disagrees with the table above is an apparatus fault
  rather than a reading.
- **`19.11` inherits nothing publishable.** No figure moved and no harness path
  changed.
