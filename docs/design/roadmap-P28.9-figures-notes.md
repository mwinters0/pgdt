# P28.9 — The figures: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Evidence" and "Facts found while grilling"; the attribution rule it applies is
[`roadmap.md`](roadmap.md), "Attribution is introspective; only the gate is
blind". The instrument and its readings have both landed: one `measure.py
--all` sweep at `183a50eb`, with 28.9.1's, re-took every register figure but
`session-drift` (`runs/measure-20261001T010810/`), and the profile recipe ran
after it (`runs/28.9-sweep-20261001-0107/`, its handoff and logs).

## What exists

- **No census figure and no census-off build.** `census-brace-free`,
  `census-arrays` and `census-attribution`, their runners, the `nocensus`
  binary kind, `Config.bin_nocensus`, `census_binary_problem` and its
  preflight are gone from `measure.py`, and their sections from
  `measurements.md`. `runs/pgdt-nocensus` and its stamp are read by nothing.
- **The throughput tables take their own `COPY` rows**; the allocator table's
  reference `parse` row borrows `scan-throughput-warm`'s.
- **M180's bar is lifted whole**: `BARRED`, `BAR_LIFTED_BY`,
  `barred_problems` and their text in `--list`, `--stale` and `main` are
  gone, so `--all` selects every figure again.
- **`statistics-gathering` prices the data level**: its legs are
  `metadata` and `data` (`STATISTICS_LEGS`), its rows the three throughput
  inputs and `arrays`, and it declares `unrepresentable.rs`. Its id is
  unchanged; its section is "What the data level costs a parse".
- **`statistics-pruning` has two legs a filter**, the `uncarried` leg gone;
  a test holds that no shape queries a cache a metadata-level `parse` wrote.
- **`--profile-recipe` profiles the data level**:
  `parse-statistics-data-rss`, the figure's `data` leg without its resident
  wrapper, over `control` and `arrays`, landing as
  `runs/profile-parse-statistics-data-rss-<input>.{data,txt}`.
- **The census's price is a profile share**, the samples under
  `map::census_row` read against the data leg's wall, and the count's share
  inside it; the shares and what they come to are
  [`measurements.md`](measurements.md), "What the data level costs a parse".
  They were read by bucketing `perf script --inline` samples on
  `census_row` and, inside it, its `tier` test.

## What the next slices inherit

- **A lever on what the data level costs is a statistics lever.** The census
  is a small share of a data-level `parse` and the count under half of that;
  the statistics are most of it ("What the data level costs a parse").
- **The scan figures time the metadata level**, which censuses nothing, and
  their warm absolutes moved against `da05a72`'s by less than a warm absolute
  resolves across sessions; no move is attributed to the census they no longer
  run.
- **`peak-rss`'s 4,000-block row rose against `da05a72`'s, unattributed**, the
  rise on the full `parse` and the cached no-match `query` alone and not on
  jemalloc's `parse` ([`measurements.md`](measurements.md), "What a scan holds
  resident, per byte and per block"). Building the commits from `da05a72`
  under the figure's wrapper is what would name it.
- **`statistics-pruning`'s prose keeps the upper bound** on decoding the
  cache's statistics, the pruned legs' floor, and says that nothing prices it
  below.
- **`measure.py`'s `section` for `nested-end-to-end` quotes a stale number**,
  so `tables.md` heads that figure with one the document does not; the marker,
  not the heading, addresses a figure, and a fold rewrites the document's.

## Negative results

- **No builder writes a table's census without its statistics**, so a leg
  pricing what carrying statistics costs a query has no cache to read: a
  metadata-level one times a census re-read, and a column override's repeats
  the subtraction the retired leg's reading left unresolved (`measure.py`,
  beside `PRUNING_LEGS`).
