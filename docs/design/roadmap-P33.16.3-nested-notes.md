# P33.16.3 notes — 33.16's count pair over a nested column

**The walk is cheaper than the staged elements it replaced on a nested
column**: `dbdaead4..067b8f69` resolves a fall on the `arrays` row and no rise
there, so nothing on it is placed or accepted (D108). The log is
`runs/count-pair-33.16.3-20261111-0143/log`, both legs compiled from their
checkouts.

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
- **The pair is `dbdaead4..067b8f69`, the row's `02cdaa3d..067b8f69`**:
  `067b8f69`'s first parent is `dbdaead4`, which differs from `02cdaa3d` in
  `.claude/skills/gosub/SKILL.md` alone, so the parent leg built `02cdaa3d`'s
  crates.
- **The five control rows are 33.16's rises, already placed and accepted**
  ([`roadmap-P33.16-builders-notes.md`](roadmap-P33.16-builders-notes.md)):
  the same three query rows resolve a rise and both parses none, each row's
  median change differing from `runs/count-pair-33.16-20261110/log`'s by less
  than its parent leg's spread.
  The two pairs share the parent's crates, the input and the counter, so
  their agreement says this commit is the change 33.16 priced, not that the
  pricing is right; the placement is that pair's callgrind's.

## What 33.16.1 inherits

- **The `arrays` row prices a change to what a query's batch decodes a
  nested column with**, which the control cannot; a rework of the walk is
  priced there, by `<parent>..<change>` once it is committed.

## Negative results

- **The fall is unplaced**: D108 places a rise, and no callgrind was run on
  the `arrays` row. `--callgrind-recipe`'s block for it on the two legs
  would place it; the legs are rebuilt by the same command.
- **`pgdt sql -c` over `arrays` is no row**: its query counts the sixteen
  scalar columns alone (`_script`'s `dfcli-query-typed`), so no builder of a
  nested column runs under it.
