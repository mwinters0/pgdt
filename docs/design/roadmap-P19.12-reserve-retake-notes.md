# P19.12 — the reserve, re-read against `19.7`

`19.6` took the reserve on a build whose pools under-reported what they held;
`19.7` made the report a bound and predicted the compressed legs would halve.
They did not. This slice re-takes the figure against `19.7`'s build, drops the
arena leg that could not bind, and chooses the constant `19.13` carries.

The sitting is `runs/measure-20260909T171423/` — **`--alone`, NOT PUBLISHABLE
by construction**, at commit `8a432a6` with this slice's harness change in the
tree. Three reps, interleaved and reversed, every reading accepted at the first
attempt: CPU stall ≤0.60%, I/O stall ≤5.83%, machine ≤36% busy, steal 0.00%,
busiest core ≥3.67 GHz, ≤70°C. `postgres:16` at `-m 3g --memory-swap 3g`, the
apparatus departure `test_measure.MEMORY_DEPARTURES` already names. No library
code changes here.

## The reading

Median peak resident set, three reps, `--jobs 24`, and beside each the reserve
— that reading minus the budget the run stated.

| Leg | 64 MiB | 128 MiB | 256 MiB | 512 MiB |
|---|---|---|---|---|
| `.xz`, uncapped | 263.33 · +199.33 | 494.78 · +366.78 | 809.80 · +553.80 | 1347.94 · +835.94 |
| `.xz`, `MALLOC_ARENA_MAX=2` | 238.02 · +174.02 | 408.14 · +280.14 | 716.82 · +460.82 | 1352.25 · +840.25 |
| plain, uncapped | 80.26 · +16.26 | 152.29 · +24.29 | 208.96 · −47.04 | 208.38 · −303.62 |
| plain, `MALLOC_ARENA_MAX=2` | 78.40 · +14.40 | 86.22 · −41.78 | 77.87 · −178.13 | 80.23 · −431.77 |

The shipped serial arrangement beside them — `control` at `--jobs 1` stating no
budget — holds **5.70 MiB** (5.62–6.00) against the library's 64 MiB default.
`19.6` read 5.86 and `peak-rss` publishes 5.85, so the instrument is still the
one that took those.

**The sub-stream counts are unchanged from `19.6`.** `XzSource::partitions`
still charges `cache.unit + chunk_bytes` — 24 + 1 MiB — so `worker_count` plans
**2, 5, 10 and 20**; the plain source is charged `POOL_MAX_BYTES`, so it plans
**8, 16, 24 and 24**. The wall clocks land within 5% of `19.6`'s at every cell,
which is the independent check that no count moved underneath the resident
readings.

## `19.7`'s prediction is refuted, and that is the finding

The coupling of the block pool's free and retained lists was expected to halve
the compressed legs. Fitted against the sub-stream count, least squares over the
four cells:

| build | uncapped line |
|---|---|
| `19.6` (`8fe1f82`) | 160 MiB + **54.3** MiB a sub-stream |
| this sitting (`8a432a6`) | 180 MiB + **59.4** MiB a sub-stream |

Both terms went **up** by about a tenth. The doubled ceiling `19.7` removed —
`2 × slots × unit`, 2 × 24 MiB against a charged 25 — was arithmetically the
right size for the excess, and removing it did not return the bytes. The
coherent account is that the pool's ceiling was never what held them resident:
what `release` now declines to keep is freed into the calling thread's glibc
arena and retained there, so a tighter pool converts pooled bytes into arena
bytes at roughly par. The next paragraph is that account's evidence.

**The arena null is reversed, and `19.7` is what reversed it.** `19.6` read the
three arena legs agreeing within their spreads, and `M76` found the same at
`--jobs 4`; here the cap moves both sources and in one direction.

- **Plain is the clean case.** Uncapped, resident is **16 MiB + 8.0 MiB a
  worker** — 8.03 over the 8→24 span, and `POOL_MAX_BYTES` is 8 MiB exactly.
  Capped at two arenas it is **flat at 78–86 MiB** across the whole axis. So
  every byte of the plain path's growth in the worker count is arena retention
  of the partition buffer `19.7` stopped pooling, and two arenas recycle it.
- **Compressed is the same effect on the fixed term only.** Capped fits
  **103 MiB + 62.2** a sub-stream against uncapped's **180 + 59.4**: the cap
  takes ~77 MiB off the constant and leaves the per-sub-stream term alone. That
  term is the block pool genuinely holding decoded blocks, which no arena
  setting reaches.

`19.10` inherits this. The manual's `MALLOC_ARENA_MAX` recommendation was
"half-confirmed and half-stale" after `19.6`; on this build the cap buys
**129 MiB** on a 24-worker plain scan and **~77 MiB** on a compressed one, both
uncapped-minus-capped at the 512 MiB cell. That is a diagnostic reading and no
document may quote it as a measurement — what `19.10` may say is that the
recommendation buys something again, and `19.11`'s sitting is what puts a number
in the register.

## The plain path's regression, priced

`19.6` read plain flat at 37.4 MiB across the whole axis. It is now 80–209 MiB,
growing 8 MiB a worker, and that is exactly the partition-buffer pooling
`19.7`'s tightened `keeps` gave up
([`architecture.md`](architecture.md), "The interior split"). The shape is
`--jobs ≥ 2` on a plain file, which `19.8` made non-default, so nobody meets it
without stating the flag — but the roadmap Future item "A two-unit plain source"
now has a price beside it rather than an adjective: **8 MiB of resident a
worker, or a `MALLOC_ARENA_MAX=2` the user sets themselves.**

## The constant `19.13` carries

**256 MiB, from the uncapped compressed leg's fixed term.**

The harness's renderer calls the constant "the uncapped leg's worst cell", which
would be **+836 MiB**. That reading is right only where resident is flat in the
stated budget, and it is flat on neither source now. What the rule actually
needs is the part of resident the budget does **not** name — the intercept, 180
MiB — because the part that scales with the budget is what
`min(fraction × limit, limit − reserve)` has a fraction for. The spec asks for
"one constant, taken from the compressed leg"; the intercept is that constant
and the worst cell is the intercept plus a term the rule already has a slot for.
256 MiB is 180 rounded up past the fit's residuals, which reach ±36 MiB at the
256 MiB cell where the per-rep spread is 732–892.

**The fraction is now the term that binds, and `19.13` has to give it a number.**
The spec calls it "a ceiling only" that "bounds nothing anyone has measured";
against this line it is the only thing bounding a compressed parallel scan.
Substituting resident ≈ 180 + 2.376 × budget (MiB, while the sub-stream count is
below `--jobs`) into `resident ≤ limit` with `budget = f × limit`:

| limit | largest safe `f` |
|---|---|
| 512 MiB | 0.27 |
| 1 GiB | 0.35 |
| 3 GiB | 0.40 |
| unbounded | 0.42 |

The count saturates at `--jobs`, so resident tops out near 180 + 24 × 59.4 ≈
1.6 GiB and every limit above that is safe at any fraction. **The binding region
is a limit between about 256 MiB and 1.6 GiB**, and a fraction around **0.25**
covers all of it with margin.

**Below about 300 MiB no fraction saves it**, and `19.13` should decide what
that means rather than discover it: at a 256 MiB limit the 64 MiB floor plans
two sub-streams and reads ~300 MiB resident, so a 24-worker compressed parse
does not fit however the budget is set. The fixed 180 MiB term is what does not
fit, and the lever that reaches it is the *worker count*, not the budget —
`worker_count` already divides, but the intercept is charged whether one
sub-stream runs or twenty. The below-floor `PlanNote` `19.9` owns is what tells
that user their allocation bound the scan; whether the discovered default should
also refuse to go parallel at all under such a limit is a decision this reading
raises and does not settle.

## The harness change

`RESERVE_ARENAS` is two legs, uncapped and `MALLOC_ARENA_MAX=2`. The
worker-count-plus-one leg is gone and did not wait on a reading: after `19.3`
the threads are the blocking pool's and are created on demand, so a cap at or
above the concurrency dispatched cannot bind, and the leg priced a setting inert
by construction. `test_measure.Reserve` holds the replacement both ways —
`test_no_leg_caps_at_or_above_the_worker_count` is the mechanism, and
`test_the_capped_leg_is_the_tightest_an_operator_would_set` is that the pair is
uncapped against the tightest value anyone would set. Both legs stay: a one-leg
figure cannot report a null, and this phase has now had an arena claim outlive
its build twice and a null outlive its build once.

`measurements.md` already described the figure as carrying "an uncapped leg and
a `MALLOC_ARENA_MAX=2` leg", so this change closes a gap between that sentence
and the register rather than opening one.
