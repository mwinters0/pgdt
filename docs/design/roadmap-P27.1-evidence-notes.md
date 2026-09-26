# P27.1 — The evidence: notes

What the slices after this one inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md),
"Evidence". The figures are in [`measurements.md`](measurements.md), "What
DataFusion's dynamic filters buy a query", taken at `2f94f14`.

## What exists

- **`datafusion-pgdump/tests/dynamic_filters.rs`** runs 27 queries over every
  major's `statistics` fixtures, both flag sets, gathered at 1 KiB groups, in
  two sessions per join mode — the three producer flags off and on — and holds
  both to one answer as a multiset. The join modes are the planner's choice
  and every hash join `Partitioned` (both single-partition thresholds `0`).
- **Each query asserts the shape its filter took after the query ran**, read
  off the flags-on scan's `EXPLAIN` line — bounds, an `IN` list,
  `hash_lookup`, the partitioned `CASE`, `struct(…) IN`, a null-equal join's
  `IS NULL`, a TopK's or an aggregate's threshold, and a TopK's `false` — so
  the harness cannot pass by exercising nothing
  ([`roadmap-P27.2-receiving-notes.md`](roadmap-P27.2-receiving-notes.md)).
- **`dynamic-filter-join` and `dynamic-filter-topk`** stand in no sharing
  edge, so each is re-taken alone from its own commit. Each leg runs an untimed
  `pgdt parse` stating `GATHER_STATISTICS` into `/dump.sql.dtcache`, where
  `--dump` looks, then times `datafusion-cli-pgdump -c` with its producer's
  flag `false` or `true` and `target_partitions` at `SWEEP_JOBS`, and hashes
  the answer outside the timer; `dynfilter_problems` refuses a sitting whose
  legs answered differently or not at all.
- **The input is `dynfilter`** (`scripts/generate_dynamic_filter_bench.py`):
  the control's rows with `u_key` and `bucket` appended, and three build
  tables written after the probe — `near`, 100 consecutive ids at the middle;
  `scattered`, the `u_key`s of 100 ids spread over the table; `every`, all 150
  buckets. The join rows are the clustered, unclustered and costing inputs the
  spec names; the TopK orders by `u_key`.
- **The second program is held to its own worker count**:
  `worker_count_problems` and `pinned_count_problems` read
  `DATAFUSION_EXECUTION_TARGET_PARTITIONS` off every run of it, since the
  builder's `--jobs 1` would otherwise satisfy the check for a run inheriting
  the core count.

## What the harness showed of DataFusion 55.1

- **At `2f94f14` a join computed no filter at all**: the probe subtree held no
  node whose `apply_expressions` visits it, so `plan_contains_expression_id`
  was false. The figures' join "on" leg is therefore that plan with the flag
  merely set; a TopK and an aggregate maintain theirs regardless.
- **A float join's bounds and a TopK's threshold order `NaN` above `inf` and
  `-0` below `0`** — `f8 >= -0 AND f8 <= NaN`, `f8 < NaN` — Arrow's total
  order, which the translator must keep rather than read as IEEE comparison.
- **A TopK whose heap no row can enter publishes `false`**, not a threshold:
  `ORDER BY gappy NULLS FIRST LIMIT 9` over 142 NULLs. Translated, that rules
  out every group left, the cheapest early stop there is.
- **The partitioned `CASE` ends `ELSE false`**, and a null-equal join wraps
  it: `k IS NULL OR CASE …`. A build side of one partition publishes no `CASE`
  even under `Partitioned`.
- **An enum declared sorted plans no TopK ascending**: the provider declares
  its label order, so `ORDER BY m LIMIT` is a plain limit.
- **`datafusion-cli-pgdump` does not run in `postgres:16`** built on this
  machine: it links `libm` symbols at `GLIBC_2.43`/`2.44` against the image's
  2.41, so its legs run in `archlinux:base`
  ([`measurements.md`](measurements.md), "The apparatus").

## Tests

- `datafusion-pgdump/tests/dynamic_filters.rs`: the answer check above, and
  `the_costing_input_is_the_largest_in_list_a_join_publishes`, holding the
  generator's `BUCKETS` to DataFusion's
  `hash_join_inlist_pushdown_max_distinct_values`.
- `scripts/test_measure.py`'s `DynamicFilterFigures`: the legs differ by the
  flag alone, the builder is untimed and writes where `--dump` looks, every run
  of the second program states its partitions and one inheriting them is
  reported, the SQL survives the shell's quoting and names the generator's
  columns, the table's `mimalloc` and image are true, and each refusal fires.
- `scripts/test_generate_dynamic_filter_bench.py`: the probe is the control's
  rows with the keys appended, all DDL precedes the data, `u_key` is a
  bijection, every bucket is in every 150 rows, and each selective join matches
  exactly 100 probe rows.
- A `--dry-run` and a 0.25 GiB two-rep sitting of both rendered; the second
  found the reported key a digit kept `parse_reported` from reading.

## For 27.5

- **The "before" is flat**: every leg of both figures is the scan, the flag
  moving nothing six reps resolve. So what 27.5 prices is the on leg alone
  moving; a join's count stays 100, 100 and 811,470, which the re-take's table
  shows beside the timing.
- **A join's answer is `count(*)` first**, the column its table reports, and
  `count(p.v_text)` beside it: `v_text` is nullable, and counting it alone
  read 99 rows for a join matching 100.
