# P30.6 notes — the re-take

What the round taking the readings inherits. The gate's `system` legs are
built (`scripts/measure.py`, `GATE_LEG`, `gate_twin`, `gate_section`,
`Session._gate_twin_of_kill`); the readings are not taken, a figure being
taken from a commit and never from a tree carrying its own apparatus
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep").

## What the readings round inherits

- **The command is `cd scripts && uv run measure.py --all`**, from the commit
  carrying this apparatus, launched detached per `CLAUDE.md`, "Long-running
  processes", with a handoff per `.claude/skills/gosub/handoff.md`. `reserve`
  and `parallel-peak-rss` sweep their twins inside it, so the gate and the
  figure set are one sitting on the composed binary.
- **Where the gate is read**: each of the two figures prints a "The gate."
  table, a row a leg, the shipped build beside `system`, each cell the worst
  surviving rep and its headroom against that leg's limit, or the kill, and a
  verdict line under it — "The gate blocks" names every leg failing on the
  shipped build alone. A kill anywhere else fails that figure, and `emit`
  goes on to the next; the error, in the sitting's "Figures that failed",
  says ahead of the run's output whether the same leg survived on `system`.
- **What a result means is the spec's**
  ([`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md), "What the move
  owes before a release"): a failure on both legs is filed (`KD34` or a new
  `KD<k>`) and does not block; one on the shipped build alone blocks P30 and
  earns the diagnostic slice on the `introspect` build.
- **A `system` twin killed in `reserve` bars that figure from publication**,
  as any killed leg does (`KILL_TOLERANT`); the gate table still renders.
- **The sweep is longer by the twins**: twelve flagless legs and fourteen
  `parallel-peak-rss` legs, three reps each.

## Negative results

- **No twin is swept for `parallel-scan-throughput`**: it reads no resident
  set, so its gate reading is whether a leg was killed, which the kill twin
  reads when one is (`Session._gate_twin_of_kill`).
