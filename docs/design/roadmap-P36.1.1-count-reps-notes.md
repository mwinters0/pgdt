# P36.1.1 notes — the count pair's reps

The instrument is `scripts/measure.py`'s `cmd_count_pair`, now counting each
shape `RELEASE_PROFILE_REPS` times a leg, and `count_pair_resolution` and
`count_pair_table` beside it; `CountPair` in `scripts/test_measure.py`
asserts them. Their docstrings say what each prints and why. **The reading
this slice owes is not yet taken**, so the box stays open.

## What remains: the identical-source reading

From the commit that lands this instrument, the tree clean:

    cd scripts && uv run measure.py --count-pair HEAD~1

That commit touches only `scripts/` and the docs, so its two legs are two
builds of one Rust source. **Any row printed `resolved`, either way,
falsifies the instrument** (spec, "What a hot-path slice owes"), and no
hot-path slice (36.4, 36.5, 36.9) is counted until that is answered. The
reading also answers what 36.1's first pair left open: whether its change
column was layout or run-to-run (36.1's notes, "Two builds of one source
are not one binary"). A pair is over ten minutes by 36.1's builds alone, so
it is launched detached per `.claude/skills/gosub/handoff.md`, and the round
after reads the log's table and its "rows resolved" line, then ticks 36.1.1
with the reading filed here.

It is not taken from the uncommitted tree with `--count-pair HEAD`, though
that pair too has one Rust source: a reading is taken from a commit, never
from a tree carrying its own uncommitted apparatus
([`measurements.md`](measurements.md), "A figure may be published outside
the sweep").

## What the next round inherits

- **One test of resolution, shared.** `resolved_above` is the fewest above
  the most, and `downstream_verdict` reads both its gaps through it, so the
  pair's rule and the register's cannot drift apart.
- **Each row says the direction** — a rise, a fall, or unresolved. A slice
  is refused on a resolved rise alone; the identical pair on either.
- **The pair runs both legs each rep**, the first leg reversed for the
  second half of the reps, as `_sweep_one` orders a figure's, and refuses a
  shape any of whose runs answers differently from another, not only across
  legs.
- **`--reps` does not reach the pair.** `main` builds its `Config` from
  `--dry-run` alone, so the pair's spread is always `release-profile`'s
  rep count, the comparison the spec names.
