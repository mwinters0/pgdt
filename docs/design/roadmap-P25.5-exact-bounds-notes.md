# P25.5 — The first format bump, exact bounds: notes

The slice built the spec's first two "Same-pass gathering" items
([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)): whether a
stored lower bound is the value it came from, and the zero's sign at a float's
extremes. `CACHE_FORMAT_VERSION` is 25. The flag is `Bounds::min_exact`. The
float order is `ValueKey::stored_order`, and the reason it is one set rather
than a second is `decisions.md`, "D79". `KD39` and `KD42` are struck.

## What the next slices inherit

- **`min_exact` holds the head's wholeness while a block is gathered, the
  same way `max_exact` does**, until `finish` clips the bounds. That replaced
  the gatherer's private `MIN_WHOLE` flag. `LOST` is now the only closed-group
  flag. A keyed kind stores both flags `true`.
- **Nothing in the library orders by the filter's order alone any more.**
  Gathering, `table_summary`'s fold and `partition_orders`' boundary proof all
  use `stored_order`. `ValueKey::compare` is test-only. The tests use it as
  PostgreSQL's order and check `stored_order`'s results against it.
- **`GOLDEN_ORDER` is `(25, …)`.** Values are now sorted in the stored order,
  and a tie that only the stored order breaks is marked `~`. 25.6's bump
  changes no order, so it re-pins the version and leaves the digest alone.
- **Floats are declared orderings now.** 25.4's `matches!` in
  `column_order` is gone. The ordering target asserts that `public.specials`'
  `f8` and `f4` are declared ascending and `f8_unsorted` is not. Each
  partition's rows are read under Arrow's comparator.
- **What the harness now pins.** `statistics_never_change_an_answer` has no
  `Answer::OtherZero` arm. It asserts `Seen::text_min`, a text or binary `MIN`
  answered from statistics, in both schema modes. The new target
  `an_extreme_answers_where_its_stored_bound_is_the_value` checks four things
  against the blind session. `long_min`'s `MIN(v)` and `MIN(vc)` are read.
  Its `MAX(v)` and `ordered`'s `MIN(c_text)` are answered. Each of `zeros`'
  six columns is answered. The library oracle asserts that `min_exact` holds
  exactly where the group holds the text, and that a float's bounds are
  extremes under `total_cmp`. Two mutation runs each failed a target:
  `min_exact` always `true`, and `stored_order` without the float tiebreak.
- **`info --json` exports `min_exact`** with no code change, because it
  prints the internal struct (D67). The manual's field list names it.

## Negative results

- **A float block that is sorted only in PostgreSQL's order is now recorded
  `Unsorted`.** `0` then `-0` is an example. So pruning's sorted stop, which
  reads the same order, loses that block. `public.zeros.min_pos_first` pins
  it in `sortedness_is_the_blocks_row_order`.
- **Across groups, an exact bound and a clipped one with the same text keep
  whichever came first.** If the clipped one came first, the column's
  extreme stays `Inexact` even though the exact one would be correct. No
  fixture reaches this, so preferring the exact one on a tie was not built.
