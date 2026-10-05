# P30.6 notes — the re-take

What the phase's wrap and P23 inherit. The sitting is
`runs/measure-20261005T155330/` (`tables.md`, `raw.json`, `log.txt`):
`measure.py --all` from `1c9fc9be`, the shipped mimalloc build in
`archlinux:base`, every figure but `session-drift` folded into
[`measurements.md`](measurements.md) under one stamp. Its handoff is
`runs/30.6-retake-sweep-20261005-1600/HANDOFF.md`.

## The gate

- **It passes**: both "The gate." tables (`reserve`'s flagless legs,
  `parallel-peak-rss`'s legs) read every leg on the shipped build and its
  `system` twin, and no leg was killed anywhere in the sitting, so
  `Session._gate_twin_of_kill` never ran and nothing failed on both legs to be
  filed as predating P30.
- **What the gate does not read is the charge criterion**, and that is where
  the allocator shows: `reserve`'s 128 MiB-block legs at four and five readers
  land in the `bound` band on the shipped build alone, their twins having
  resolved the same arrangements and staying inside the bound, and the worse
  of them leaves less than `MEMORY_MARGIN_PERCENT` of its allocation
  ([`measurements.md`](measurements.md), `reserve`). Filed into `KD34` and
  P23's sketch ([`roadmap.md`](roadmap.md), "P23 — Statistics coverage and
  the resident reserve"); that it does not block P30 is under STATUS's
  "Decisions worth another look".
- **The builds part by block size**, unattributed: the shipped build holds
  less than `system` on 24 MiB blocks and more on 128 MiB blocks wherever more
  than one reader runs, in `reserve` and `parallel-peak-rss` alike. Every
  instrument leg is on 24 MiB blocks, so no reading names what mimalloc keeps
  of a 128 MiB block; P23's two-heap account is where it would be named.

## What the wrap inherits

- 30.6 is P30's last slice, so the phase is ready to wrap: the slice notes
  consolidated, the index row `Complete`, the README's status list ticked
  for the one binary.
- **The cold-NVMe parallel regime's rejection no longer passes its own
  test** on this sitting's provider readings
  ([`measurements.md`](measurements.md), "Scan throughput by input shape");
  it is left standing under STATUS's "Decisions worth another look".

## Negative results

- **No twin is swept for `parallel-scan-throughput`**: it reads no resident
  set, so its gate reading is whether a leg was killed, which the kill twin
  reads when one is (`Session._gate_twin_of_kill`).
- **The warm and NVMe `INSERT`-run legs slowed sharply against every earlier
  stamp and nothing in this sitting attributes it**: the sitting differs from
  the last by the allocator, by DataFusion linked in and by `8cffbac`'s lexer
  outside `COPY`, and the `allocator` figure times no `INSERT` run
  ([`measurements.md`](measurements.md), "Scan throughput by input shape").
  A profile ranks the candidates; no slice has taken one.
- **A small `parse` holds more on the shipped build than on `system`** in the
  same sitting, a step between one block and 500 that `info` does not take
  ([`measurements.md`](measurements.md), `rss-attribution`). Nothing gates on
  it, and nothing attributes the step.
