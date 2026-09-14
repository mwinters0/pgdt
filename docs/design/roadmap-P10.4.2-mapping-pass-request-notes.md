# P10.4.2 — The mapping pass's statistics request: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"How gathering is asked for". Nothing `pgdq` does changed.

## What exists

- **`map_file(source, scan_options, cache, statistics: &StatisticsRequest)`**;
  `ScanOptions` has no statistics field. `StatisticsRequest::default()` is
  `StatisticsRequest::ALL`, every column at the default group size, and
  `StatisticsRequest::NONE` gathers nothing.
- **Gathering nothing is a selection, not an `Option`**:
  `StatisticsSelection::None` is the third variant, so the type's `Default` is
  the default the spec states. `map_forward` takes the same reference, and a
  query's pass hands it `NONE`. The CLI's `StatisticsFlag` is gone,
  `--statistics none` parsing to that variant. Rejected, on review:
  `Option<&StatisticsRequest>` with `None` gathering nothing, since `Option`'s
  default is the opposite of the spec's; and with `None` gathering everything,
  which spells the default two ways and makes `map_file(…, None)` read as the
  request `NONE` names. `Option`-means-unstated belongs in a resolver
  (`CacheMode::resolve`, the CLI's `statistics_request`), and an entry point
  takes the resolved value, as `map_file` takes `&CacheMode`.
- **A group size is ignored in a block the request does not track, and only
  the CLI refuses one**, beside `--statistics none`, where a person's intent is
  ambiguous. Rejected, on review: refusing it in the library, or making it
  unrepresentable with a `None` request variant carrying no size. Targets naming
  no table in the dump leave the same size sizing nothing, so either would close
  one instance of it.
- **`build_index` and `preamble_only` gather nothing**, as before: neither is
  the mapping pass the spec names, and the eager producer is what
  `tests/map_file.rs` compares a mapped index against.

## For the slices after

- **10.7.** The back-fill entry point takes a `&StatisticsRequest` as
  `map_file` does, and `gathers()` is the one test that it asks for anything.
  A block mapped under `NONE` holds `None` exactly as before.
- **10.8.** The spec's "`pgdq query --statistics none`, and its library option"
  is a switch on *pruning*, a query option; it is not a `StatisticsRequest`,
  which no query entry point accepts.
- **10.6.** Every `tests/map_file.rs` scan and both `tests/wait_policy.rs`
  scans state `NONE`, since a gathered block is never offered to the leader.
  When 10.6 re-offers it, those are the tests to widen to the default request.

## Negative results

- **One hand mutation**, `StatisticsSelection`'s `#[default]` moved to `None`,
  failed `tests/statistics.rs`'s
  `the_default_request_gathers_every_column_at_a_mebibyte` and two of the CLI's
  `tests/statistics.rs` cases.
