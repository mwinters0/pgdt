# P27 — DataFusion's dynamic filters: notes

What the phase leaves that neither the code nor the register holds: the
negative results with no other home, and the facts a later phase needs. The
spec is [`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md).
The decisions are [`decisions.md`](decisions.md), "D53" (the membership term),
"D93" (row evaluation and when a state is read), "D94" (the translation) and
"D95" (the entry point and the cut at the first poll); the figures and the
per-row account are [`measurements.md`](measurements.md), "What DataFusion's
dynamic filters buy a query".

## What DataFusion 55.1's producers publish, as the scan meets it

Read off the flags-on scan's `EXPLAIN` by
`datafusion-pgdump/tests/dynamic_filters.rs`, which asserts each shape.

- **A join computes no filter unless its probe side's leaf visits it** in
  `apply_expressions`; with none visiting it, a join's "on" leg is the plan
  with the flag merely set. A TopK and an aggregate keep theirs regardless.
- **A float's bounds and a TopK's threshold are Arrow's total order** —
  `NaN` above `inf`, `-0` below `0` — and **a float's `IN` is a set of bits**
  (`in_list/primitive_filter.rs`), `-0` not in a list holding `0`.
- **A TopK no row can enter publishes `false`**, which rules out every group
  left; **a partitioned join's `CASE` ends `ELSE false`**, a null-equal join
  wraps it in `k IS NULL OR …`, and a build side of one partition publishes no
  `CASE`. No producer publishes an `IN` beneath a `NOT`.
- **An enum declared sorted plans no TopK ascending**: the provider declares
  its label order, so `ORDER BY m LIMIT` is a plain limit.
- **A `SET` prints nothing under `--format csv`**, so a figure's rows leg
  reads back the query's answer alone; a release printing one fails the
  sitting (`dynfilter_problems`) rather than skewing it.

## Negative results

- **`DynamicFilterTracker`** answers "moved?" in one atomic load, but its
  `changed(&mut self)` is one consumer's subscription; shared by a scan's
  sub-streams it needs a lock, where `snapshot_generation` reads each filter
  under its own.
- **The generated check cannot catch a producer's unsoundness**: it holds a
  translation to DataFusion's evaluation of the filter, which equates the
  zeros as the library does, and passes with "D94"'s zero rule removed. Only
  a check against a producer's own answer can — the flags-on/off sweep and
  `tests/statistics.rs`'s blind session, which fail without it.
- **Asking the dynamic stop in every kept group** paid a per-row evaluation
  over every group before the bound's to save at most the rest of one group;
  it is armed only where a group's statistics say a row can reach the bound.
- **A decode failure taken as a rejection** would hide a refusal behind
  whether the state had narrowed; a field of the state that does not decode
  keeps its row (the P28 inbox).
- **A membership's group answer is no tighter than its `Or`'s**, though a row
  equals at most one value: tighter would break the per-group agreement its
  generated check proves, and whether it would prune more is unmeasured.
  **Only its row path is constant in the list's length**; a group is still
  answered term by term, a dictionary entry once per term. `Lookup::Each` is
  unreachable by construction and stands instead of a panic.
- **A float's `NOT IN` holding a `NULL` keeps every row**, the float rule
  asked first; no producer publishes one, so nothing a query prunes is lost.
- **`Trimmed` implies nothing**, so a join over `character(n)` keys keeps its
  bounds per row: speed, not rows. **The cut does not search the sorted list
  within a group's bounds**, which would stop its cost scaling with the list;
  what remains per term is a key comparison, no parse. **The row form is
  rebuilt at each state read of each block**, never per row.
- **No switch for a static filter**, whose evaluation is what answers it
  `Exact` ("D88"); **no `pgdt` flag**, `pgdt query` holding no dynamic
  filter.
- **The evaluation instrument** (`pgdump_query::instrument`): `Instant` is a
  vDSO call around the same counter `rdtsc` reads; `rdtscp` or a fence prices
  a span as if the pipeline held nothing else; an `#[inline(always)]` closure
  wrapper changed the code of every function it wrapped in a build without
  the feature, where `timed!` and `row_evaluated!` expand to the body alone;
  per-thread accumulators buy nothing over an uncontended relaxed atomic at
  one partition. **Its calibrated parts are not the account**: the
  calibration, taken back to back, subtracts more from `Row` than the whole
  build costs, so its counts are read and its split is the profile's. **The
  row split's walk is moved, not added**: what `push_row` loses of it the
  first `Locate` takes.

## For later phases

- **P28** inherits the third timing path, inside a group: where a session
  states `pgdump.dynamic_filter_rows`, a row the state rejects never decodes.
  Its inbox holds it; off by default, the path is reached only on request.
- **P26**: a join past the `IN` list's limit (`hash_lookup`) or on several
  keys (`struct(…) IN`) prunes by its bounds alone, and an `IN` over a column
  with no dictionary by nothing — the membership a per-group bloom filter
  would answer.
- **Row evaluation's cost that remains** is the lookup (`KD55`), the walk,
  and the replay's per-row reads outside evaluation, each priced in the
  account; "D93"'s **Reopens** says what turns it on.
- **Unattributed moves** stand beside their figures in
  [`measurements.md`](measurements.md): the unclustered join saving more than
  the costing row over the same removed leaves at `5e02bf9`, and the TopK's
  legs rising there over identical scan metrics.
