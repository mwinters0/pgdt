# P33.16.3 notes — 33.16's count pair over a nested column

**The instrument has landed and its readings have not been taken**: a slice
that builds an instrument stops before its readings, which are taken from a
commit holding it (`.claude/skills/go/SKILL.md`, "Pick the work").

- **`--count-pair` has a sixth row, `query --schema-mode typed` over
  `arrays`** (`COUNT_PAIR_SHAPES`), the one whose batch decodes a nested
  column: each shape names its input, the other five the control, and
  `--callgrind-recipe` links each row's input and names each row's files by
  shape and input (`count_pair_row`).
- **`--count-pair <parent>..<change>` prices a committed change**: both legs
  built from checkouts made just before each build, refused unless `parent`
  is `change`'s first parent (`count_pair_named`) or where either commit's
  image inputs differ from the tree's. The change's leg builds in a target
  directory of its own (`COUNT_PAIR_CHANGE_TARGET`), so the checkout's tree
  leg never reads it as fresh. Held by `test_measure.py`'s `CountPair` and
  `CallgrindRecipe`.

## What the readings inherit

- **The pair is `dbdaead4..067b8f69`, not `02cdaa3d..067b8f69`**:
  `067b8f69`'s parent is `dbdaead4`, and `git diff 02cdaa3d dbdaead4` is
  `.claude/skills/gosub/SKILL.md` alone, so the parent leg builds
  `02cdaa3d`'s crates; the named form refuses the row's spelling as two
  commits. The command, from a clean checkout of a commit holding this
  instrument:

  ```sh
  cd scripts && uv run measure.py --count-pair dbdaead4..067b8f69
  ```

- **It runs past half an hour**: 33.16's own pair took 26.3 minutes over five
  rows (`runs/count-pair-33.16-20261110/log`), this one adds a sixth, and
  its first step regenerates the `arrays` input, whose stamp the generator
  has moved since. So it goes detached per `.claude/skills/gosub/handoff.md`.
- **The five control rows re-count 33.16 committed**, and are read against
  that log's table; the `arrays` row is the walk against the staged
  elements, a resolved rise there placed by `--callgrind-recipe`'s block for
  it and reworked or accepted (D108).

## Negative results

- **`pgdt sql -c` over `arrays` is no row**: its query counts the sixteen
  scalar columns alone (`_script`'s `dfcli-query-typed`), so no builder of a
  nested column runs under it.
