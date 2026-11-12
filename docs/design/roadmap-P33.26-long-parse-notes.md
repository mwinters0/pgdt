# P33.26 notes — a long field's parse priced

**The instrument has landed and its readings have not been taken**: a slice
that builds an instrument stops before its readings, which are taken from a
commit holding it (`.claude/skills/go/SKILL.md`, "Pick the work").

- **`--count-pair` has four more standing rows** (`COUNT_PAIR_SHAPES`): the
  data-level parse and a strict one (`STRICT_PARSE`, the data level's
  unwrapped `parse` with `--postgres-invalid-values strict`) over
  `long_escaped_32` and `long_bytea_32` (`COUNT_PAIR_LONG_INPUTS`), each
  under `STATISTICS_MEMORY` as the control's data-level row is. A 32 MiB
  line is past a read chunk and under today's 64 MiB line limit, so every
  commit back to `6dd4523c` reads it unflagged. `--callgrind-recipe` has a
  block for each.
- **`--count-span <base>..<tip>` counts a run of landed commits**, both legs
  from checkouts as a named change's are, refused unless `base` is an older
  commit on `tip`'s first-parent chain (`count_span_named`), and its log
  says it is an account over that many commits, never one change's price.
  `--count-pair <parent>..<change>` still refuses anything but the first
  parent. Held by `test_measure.py`'s `CountPair`.

## What the readings inherit

- **The commands**, from a clean checkout of a commit holding this
  instrument, one at a time: each pair overwrites the two leg binaries
  (`count_pair_leg_binary`) that `--callgrind-recipe` places a rise in, so a
  pair's resolved rows are placed before the next pair runs.

  ```sh
  cd scripts && uv run measure.py --count-pair 6d4a1cf4..e3a5ce6d   # 33.13
  cd scripts && uv run measure.py --count-pair a36e8580..62e8875c   # 33.18
  cd scripts && uv run measure.py --count-pair c356be7a..e80138b2   # 33.19
  cd scripts && uv run measure.py --count-span 6dd4523c..93428ef7   # the account, 58 commits
  ```

  Each was refused by neither guard here: every commit named has the
  tree's image inputs, and `6dd4523c` is on `93428ef7`'s first-parent chain.
  A pair over six rows took 29–36 minutes (`runs/count-pair-33.25-20261111/log`,
  `runs/count-pair-33.16.3-20261111-0143/log`), so the four together, with
  their callgrind blocks, go detached per `.claude/skills/gosub/handoff.md`.
- **The span holds more than the three**: its first-parent run also lands
  33.3.1, 33.10, 33.12, 33.11, 33.9, 33.3.2, 33.3.3, 33.13.1, 33.21, 33.13.3,
  33.13.4, 33.13.5 and 33.23, each changing a file a long field's parse
  runs — `scan/chunked.rs`'s held line (33.10, 33.12; a data-level parse held
  its line until 33.19), `copy.rs`, `gather/strict.rs` and `value/bytea.rs`
  (33.9). Where the three pairs do not sum to the span within spread, these
  are the residual's candidates, and it owes a name or the words "no name"
  (`.claude/skills/evidence/SKILL.md`, rule 6). Within spread, the account
  closes unplaced. Past it, **the span's own legs place it**: the span runs
  last, so `--callgrind-recipe` reads its binaries, and a function moved past
  what the three pairs' placements hold is named by the candidate whose diff
  touches it. A `--count-pair` on a candidate is taken only where two touch
  one function, and "no name" is written only where neither separates them.
- **33.19's pair is read on every row**, the control's included: its
  trigger is D108's, any hot-path change.

