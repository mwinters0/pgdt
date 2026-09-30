# P28.4 — Statistics' views: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decision is `decisions.md`, "D97".

## What exists

- **`ColumnStatistics::unrepresentable`**: one `Unrepresentable { format,
  engine }` per group, counted by the same leaf test the census counts by
  (`unrepresentable::ColumnTier`, built from the resolution the observer
  gathers against), nested columns included; `None` where no group of the
  block holds one. The groups' counts sum to the block's census count,
  column by column (`statistics_count_by_group_what_the_census_counts_by_block`).
  `CACHE_FORMAT_VERSION` 32.
- **`ColumnBounds` is over the values the type holds**, with
  `every: Option<BoundsView>` beside it where some group holds a value past
  the format spec and `displayable` where some group holds one past the
  calendar only, each listing bounds for such a group alone. Read them
  through `ColumnStatistics::group_bounds(set, view, group)`,
  `sortedness(set, view)` and `null_count(group, view)`, never through
  `ColumnBounds::groups` directly: that list is the representable view, and
  a reader indexing it reads a different view than `Every`.
- **Pruning reads a stated view**: `prune_block` and `DynamicPruning::new`
  take a `StatisticsView`, and `QueryOptions::statistics_view` answers it —
  `Every` for every query, which is what every query read before this slice,
  its filter ranking a special value and a read decoding one refusing. The
  provider's summary (`summary.rs`, "D89") and `pgdt info --detail`'s bounds
  and order counts read `Every` too. So no answer moved.
- **Only a keyed set holds views**: a bytewise kind's type holds every value.
  Both of an `interval`'s sets do, the Arrow set (`IntervalFields`) among them.

## What the next slices inherit

- **28.5 sets the view from the mode and the front end**:
  `QueryOptions::statistics_view` is the one place, `Representable` under the
  typed mode in `pgdt` and the library, `Displayable` in the provider,
  `Every` under the refuse mode and wherever a filter compares in
  PostgreSQL's order over a widened column. The summary's `READING` is the
  provider's half.
- **"D89"'s statistics are not yet the typed mode's**: a sum is still dropped
  by a `NaN` (`gather::Summand::value`), and a dictionary still holds an
  unrepresentable value's text, so a distinct count derived from it counts
  one the typed mode reads as NULL. Neither is wrong under `Every`; both are
  28.5's, and changing either bumps `CACHE_FORMAT_VERSION`.
- **A dictionary is sound under every view as it is**: its extra entries only
  add truths to a group's set, and a typed-mode NULL's `Unknown` comes from
  the NULL count the view adds to.
- **Every value's view is unbounded over a group holding a value no key
  orders** — a timestamp past `i64` microseconds, an `interval` time part
  past nanoseconds — as any value the bounds cannot cover leaves a group
  (STATUS, "Decisions worth another look"). The refuse mode's filter over
  such a column (28.6) needs those values keyed, and keying them fills the
  view with no change here but the key.
- **A gathered block tests each value of a counted column twice**, once in
  the census and once in its observer, as it splits each row twice;
  unpriced, and 28.9's profile is where it shows.

## Negative results

- **A set of bounds per view, gathered from every such column's first row,
  holds three sets where one serves**, every `date` and timestamp column
  paying for a value almost none holds; the base-and-extremes shape
  allocates nothing until a group meets one ("D97").
