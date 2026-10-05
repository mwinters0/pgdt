# P30.8 notes — the confirming sitting

What the reading inherits. The instrument has landed and the readings have
not: they are a sitting from the commit carrying this change, never from a
tree holding its own uncommitted apparatus
([`measurements.md`](measurements.md), "A figure may be published outside the
sweep").

## The readings still owed

`cd scripts && uv run measure.py --figure reserve --alone`, from that commit,
launched detached per `.claude/skills/gosub/handoff.md`: it runs 30.7's
sitting plus the four confirming legs, about as long as 30.7's
(`runs/30.7-diagnostic-sitting-20261005-1935/HANDOFF.md`). NOT PUBLISHABLE, as
30.7's was, so nothing is folded into [`measurements.md`](measurements.md).
Every leg the verdict reads is taken in that one sitting, so nothing is
subtracted across sittings. The result is the section headed "Whether the
shipped build's retention is mimalloc's `purge_delay`" in its `tables.md`.

## What landed

- **`RESERVE_CONFIRMING_OPTIONS`** in `scripts/measure.py`: one row,
  `MIMALLOC_PURGE_DELAY=0`, a flagless-shape token set in front of the
  wrapper as the arena cap is. `_reserve_confirming_specs` runs it at
  `RESERVE_DIAGNOSTIC_INPUT` and `RESERVE_DIAGNOSTIC_LIMITS` on the shipped and
  the introspection build, each leg one variable from a leg the figure
  already takes. The system twin they are read against is the flagless leg's
  own, the `system` build serving no allocation from mimalloc.
- **The verdict is registered before the reading**, in `confirming_verdict`:
  each rep is read alone, and it counts as holding the term where its reading
  reaches `RESERVE_RETENTION_FRACTION` of a unit. A shipped rep's reading is
  its peak RSS less its `system` twin's worst rep; an instrument rep's is its
  peak RSS less Rust's live high-water and glibc's. The constants' comments
  say why. A share is confirmed only where `fewer_held_p`, the one-sided
  Fisher exact test on the pooled counts with the option and without it, is
  at or under `CONFIRMING_ALPHA`; gone in every rep confirms without it.
- **The instrument reads the option back**: `pgdt/src/introspect.rs` reports
  `mimalloc_purge_delay` through `mi_option_get`, and the renderer gives no
  verdict where a confirming leg's rep read back anything but the value set.
  Otherwise a variable lost on the way would read as the candidate ruled out.
  A probe of the harness's own shape through `peak-rss` in the pinned image,
  over a fixture, read back mimalloc's default without the variable and 0
  with it; the instrumented tests pass with it unset, 0 and 7. Neither is a
  figure.

## What the reading session does

Read the section's counts and verdict against
[`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md), "What the move owes
before a release", and the whole of `tables.md` (evidence rule 4): the
diagnostic checks still resolve the shipped arrangement, nothing is killed, the
gate's verdict is 30.7's. The verdict:

- **Confirmed** sends P30 to the remedy's grilling with `purge_delay` named.
- **A share**, **not confirmed for any share** and **ruled out** all leave a
  remainder, which is attributed next on the cross-thread free list, on an instrument built only then
  (`MI_STAT` 1; [`roadmap-P30.7-diagnostic-notes.md`](roadmap-P30.7-diagnostic-notes.md),
  "The candidates in mimalloc's source").
- **No verdict** on an option that did not reach mimalloc gets a new
  sitting, once its delivery is fixed. **No verdict** on a control too sparse
  for the share test, or on a kill, is grilled before anything is sat again
  or attributed, the whole `tables.md` read against 30.7's: the sitting no
  longer reproduces the arrangement it was to confirm.

## Negative results

- **`include/mimalloc.h`'s comment on `mi_option_purge_delay` reads `(=10)`.
  That is not its index**, which counts to 15 in 3.3.2's enum, and not its
  default, which is 1000 (`src/options.c`). `libmimalloc-sys` 0.1.49 declares
  14 and 16 and not 15. `the_purge_delay_read_back_is_purge_delay_s` holds the
  count against the default.
- **The `system` build links mimalloc**, through `datafusion-cli`'s
  dependency (`cargo tree -p pgdt --no-default-features --features system -i
  libmimalloc-sys`), but nothing installs it as the global allocator and
  nothing calls it. So the option still reaches no allocation on that build.
- **`--render` of 30.7's sitting, or any sitting taken before these legs,
  raises**: the renderer asks for legs that sitting never took, as the 30.7
  notes record for its own legs.
