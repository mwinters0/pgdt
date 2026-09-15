# P20.2 — Koji's row density: notes

The tool is `scripts/row_density.py`, its tests `scripts/test_row_density.py`;
what it derives and how the criterion is read is its module docstring. The
artifact is `runs/row-density-20260915/`: the known shapes' readings in
`known-shapes.txt` beside their distributions as JSON; the koji selection in
`koji-selection.txt`; the gathering run's log `koji-scan.log` and its
`info --json`, `koji-info.json`, which is what a re-derivation reads, the cache
beside it being unreadable after 20.4's format bump; and the reading,
`koji-density.txt`, with every block's distribution at every size in
`koji-density.json`.

## The criterion, registered before koji is read

The spec's "close to the bound" names no number and "reasonable width" no
measure; both are fixed here, reviewed by the maintainer before koji's reading:

- **Close is `CLOSE_RATIO` = 0.75 of `2R/m`.** A uniform block that coarsens
  reaches at most a half, so three quarters is skew no uniform block shows.
- **Width is the median group's bytes over its rows at the gathered size**,
  against 1 KiB — never the block's mean row. The criterion exists for skewed
  blocks, and a few wide rows lift a mean past 1 KiB while the median group
  stays dense, so a mean would keep out exactly the block it is looking for.
  Each group records its `bytes` beside its `rows`, so this needs no re-read.

## Koji's reading

**Not met; the median stands.** The run exited 0 with the log's 74 `COPY`
blocks and 19,575,829,920 rows; 13 blocks hold no row, the other 61 are all
tracked, and each block's group rows sum to its row count. 59 of the 61 are of
reasonable width, 37 of those reach the minimum, and none comes near the bound:
the nearest is `public.user_krb_principals` at 0.442, a single group, which
cannot pass a half; of blocks with more than a handful of groups the nearest is
`public.build` at 0.173.

- **The median coarsens no koji block of reasonable width.** A median group of
  at most 1 KiB a row over about `N` bytes already holds about `m` rows, so
  such a block is chosen at the gathered size, and only groups below the
  median sitting near empty could bring it close. None does.
- **It coarsens exactly the two wide blocks**: `public.task` (median group
  1,969 B a row) to `2^22`, and `public.archiveinfo` (32,435 B, against a
  1,492 B mean) to `2^23`. A lower quantile coarsens those two further, and
  otherwise only two blocks of under ten groups.
- **The width measure moved nothing on koji**: every block falls on the same
  side of 1 KiB by its mean row as by its median group, so the verdict does not
  rest on the choice between them. No koji block's median group is empty.
- **`public.buildroot_listing` is dense and uniform** — at `2^20` its tenth
  and ninetieth percentile groups are within about a tenth of each other — and
  the minimum leaves it at the gathered size.

## The known shapes

Every fixture at every major, gathered at a stated 4,096-byte group and read at
a minimum of 16 rows, and two 64 MiB perf-generator inputs gathered flagless and
read at the default 1,024. Every block's width judgement and every verdict are
the same under the median group as under the mean row:

- **The generator's rows are not of reasonable width.** The control's median
  group holds 3,960 B a row and `--arrays`' 4,581, so both sit outside the
  criterion; the median chooses `2^22` and `2^23`, which is what their widths
  predict (about 265 and 230 rows a MiB). No generated input in the tree has
  narrow rows; the fixtures' `ordered` and `escapes` tables are the
  reasonable-width shapes read, and both sit far under the bound (0.096 and
  0.061).
- **A row longer than the group** (`statistics/default.sql`'s `long_value`)
  leaves groups no row starts in, so its median group is empty and has no
  width, and its median never reaches a minimum short of the whole block; the
  report marks both and keeps it out of the criterion.

## What the next slices inherit

- **The quantile is nearest-rank**, the median being the `ceil(G/2)`-th
  smallest group, and the bound is proved for that definition by counting the
  groups at or above it; 20.4's rule reads the same one or proves its own.
- **A uniform block that coarsens lands between a quarter and a half of
  `2R/m`**, one already dense at `2^20` lower still; only skew — half the
  groups near empty — comes near 1. `test_row_density.py` holds both.
- **On koji the minimum sizes one block.** `public.task`'s `2^20` groups are
  far past a cap of the order the spec argues, so the cap's size stands over
  the minimum's there; `public.archiveinfo` holds fewer groups than such a cap,
  so its `2^23` is the minimum's alone. 20.4's koji-facing check is that block.
- **`public.archiveinfo` is koji's clustered block**: at `2^20` its ninetieth
  percentile group holds some seventy times its median's rows. It is the real
  shape of what 20.5 calls rows that cluster, where a re-read at the size a
  stated maximum predicts can still miss.
- **The preamble lists a table-level `CONSTRAINT` as a column** named
  `constraint` whose declared type is the constraint's name (koji's
  `public.task`, visible in `info --json`'s `metadata`). Every consumer looks a
  column up by name and `pg_dump` writes columns before constraints, so nothing
  resolves against it, and on koji `select` picks none.
- **Which column a table tracks moves no group's rows**; the koji run tracks
  each table's narrowest fixed-width column to hold little.
