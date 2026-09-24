# P25.1 — The evidence harness: notes

The slice built by the spec's "Evidence"
([`roadmap-P25-plan-answers.md`](roadmap-P25-plan-answers.md)). It adds no
product code. The oracles are in `datafusion-pgdump/tests/statistics.rs`. The
fixture shapes are in `scripts/fixture_schema_statistics.sql`, and
`pgdump_query/tests/statistics_fixture.rs` checks them on the bytes at every
major.

## What the next slices inherit

- **The `statistics` schema has a second flag set,
  `load-via-partition-root`.** Under that flag `public.spans` is three
  `COPY public.spans` blocks. Its `id`, `reversed`, `label` and `part` stay
  sorted across the block boundaries. `local` is sorted inside each block and
  not across them. `gappy` holds one NULL. Without the flag the same rows are
  the three tables `spans_1`–`spans_3`. So the `default` flag set now has
  eight blocks, and the tests that counted three read a `BLOCKS` constant.
- **Four targets, one per kind of answer.** Each later slice extends the one
  its answer belongs to:
  - `statistics_never_change_an_answer` now also asks `COUNT(DISTINCT c)` of
    every column and `SUM(c)` of every numeric one. It compares each answer
    with the blind session over every fixture, typed and as text. Nothing
    answers them from statistics yet, so it records no `Seen` flag for them.
    25.3 and 25.6 each add the flag and assert it. A typed `COUNT(DISTINCT)`
    over a column the typed read refuses (`KD8`) is not checked against the
    text read, because a text read counts spellings.
  - `an_estimate_is_never_below_what_the_scan_emits` checks `num_rows`,
    `total_byte_size` and each column's `byte_size`. An `Exact` value must
    equal what the scan emits and an `Inexact` one must be at least that. It
    checks the whole node and each partition, over `planned_fixtures()`
    gathered at 1 KiB groups: unfiltered, fetched, and under every term the
    provider pushes plus seeded trees of those terms. After a filter, or a
    fetch that cuts rows, no column statistic may be `Exact`. A byte size is
    measured by `emitted_bytes`: rows times width for a fixed-width type, and
    the summed value lengths of a variable-width one. A byte size stated for a
    type with no measure fails the check. 25.2's bound is checked here as
    soon as it lands. That slice should also assert that some filtered bound
    falls below the table's rows, which the tally does not record yet.
  - `a_declared_ordering_holds_in_every_partition_and_drops_the_sort` checks
    every declared ordering over each partition with `arrow::row`'s
    comparator. It also requires `ORDER BY c ASC` and `ORDER BY c DESC` to
    plan a `SortExec` exactly where no ordering is declared. Today nothing is
    declared, so only the second half and the crossing assertion are live.
    25.4 adds an assertion that an ordering is declared and read over a
    partition that crosses a boundary.
  - `a_join_builds_the_side_its_statistics_call_smaller` pins today's shape:
    `ordered ⋈ long_value` builds `long_value` and collects it whole
    (`CollectLeft`), because only rows compare. 25.6's byte size should move
    both, and that slice rewrites the assertion.
- **`KD42` is asserted as reached, not tolerated.** `agrees` returns
  `Answer::OtherZero` when statistics answer a float extreme with the other
  zero. The target asserts that `public.zeros` produces this. 25.5 deletes
  that arm and the assertion together. `public.long_min`'s minimum is longer
  than `DICTIONARY_ENTRY_MAX_BYTES`, so under 25.5 its text `MIN` must still
  come from reading rows.
- **The data behind 25.3's `timestamptz` invariant.** `spans.stamp` writes two
  instants from different offsets and gets one text back at all six majors.
  The invariant itself still needs `pg_dump`'s source as its proof.

## Negative results

- **A multi-threaded runtime makes the two sessions refuse on different
  rows.** A query that refuses (`KD8`) reads its partitions concurrently, so
  the first failing row depends on timing, and the reading and blind errors
  stop matching. `per_major` gives each major its own thread with a
  current-thread runtime. That keeps each comparison deterministic and runs
  the six majors in parallel.
- **Partition cuts can all land on block boundaries.** On `public.spans`,
  some partition counts give no partition that crosses a boundary. The
  ordering target runs `ORDERING_PARTITIONS` and asserts that a count above
  one does cross, so the boundary proof gets exercised.
- **A fetch the table's rows fit under keeps `Exact`.** This is correct
  (`exec.rs`, `fetched`), so the estimate target applies its "nothing exact"
  check only where the fetch cuts rows.
- **A blank-padded column's own bounds are unpadded only under a bytewise
  collation.** Under the database's collation, the column diverges and its
  only set is Arrow's, which holds padded text (`ComparisonPlan::bounds_kinds`).
  The fixture's `padded` and `bare` columns are `COLLATE "C"`, so the
  file-level oracle in `pgdump_query/tests/statistics.rs` meets the
  `PaddedText` set. That oracle does not model a non-bytewise `character`
  column.
- **The estimate target does not reuse the library's generated filters.**
  `pgdump_query/tests/pruning.rs` builds library `Expr` terms, and those
  never reach a provider. This target draws DataFusion terms from each
  column's emitted values, as `tests/pushdown.rs` does, and combines them the
  way `pruning.rs` does. The target's floor requires at least a fifth of the
  filtered scans to prune a group, so the bound is tested where it differs
  from the table's.
