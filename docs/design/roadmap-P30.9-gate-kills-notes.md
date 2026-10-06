# P30.9 notes — the gate reads kills

What the wrap inherits from reading the gate on kills alone
([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md), "P30's
gate reads kills"). The re-rendered sitting is
`runs/measure-20261005T155330/tables.md`, `tables-as-taken.md` beside it.

## What the wrap inherits

- **P30's gate passes** on the `1c9fc9be` sitting: no leg of `reserve` or
  `parallel-peak-rss` is killed on either build
  ([`measurements.md`](measurements.md), `reserve`). Every slice's box is
  ticked, so the phase is a wrap waiting.
- **The margin is reported, never a verdict.** `REPORTED_MARGIN_PERCENT`
  names the figures carrying one, `reserve` alone; `_margin_short` reads a leg
  against it and `margin_note` writes the column beside the gate's verdict.
  `_gate_cell` fails a leg on a kill alone. One leg is short on the shipped
  build alone, 128 MiB blocks in `-m 1536m`, which is `KD34`'s.
- **Nothing of 30.8 survives**: its legs, verdict, constants, tests, notes
  and `introspect`'s `mimalloc_purge_delay` line went by reversing
  `30f73fb6^..532f5132` on `scripts/` and `pgdt/src/introspect.rs`, which
  applied cleanly. `30f73fb6` holds the read-back if P23 wants it
  ([`roadmap-P23-resident-reserve-inbox.md`](roadmap-P23-resident-reserve-inbox.md)).
- **30.7's diagnostic legs stay**, and `run_reserve` now renders a sitting
  that never took them without their table rather than raising, so a
  sitting before 30.7 re-renders.
- **Four texts still say a failure on the shipped build alone reopens D13**:
  `gate_section`'s blocking summary and `test_a_kill_on_the_shipped_build_alone_blocks`,
  `Session`'s classification of a kill outside the gate figures, `GATE_LEG`'s
  comment, and [`measurements.md`](measurements.md), "The apparatus"'s twin
  paragraph. The spec now reopens D13 only where no library change suiting
  both allocators and no option reaches the mechanism
  ([`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md), "What the move
  owes before a release"). No leg fails, so none of them prints; the wrap's
  repoint corrects them.

## Negative results

- **Only `reserve`'s gate table and "What the sitting settles" were
  re-folded.** The re-render moved `nested-decode-micro`'s cells again, its
  `cargo bench` being today's host's
  ([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md),
  "`--render` runs three renderers' host steps"); the rest of the diff
  against the previous rendering is fold-in lists naming the P30 notes docs.
- **`parallel-peak-rss`'s gate section is unchanged** by the re-render: it
  carried no margin, so its verdicts were already kill-only.
