# P6.9 — Bytewise bounds for text whatever its collation: notes

A slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), admitted in
[`../status/history/2026-09-21.md`](../status/history/2026-09-21.md),
"Collated text gets bytewise bounds". Gathering now keeps bounds and row order
for every `CompareKind::Text` plan, whatever its divergence
(`ComparisonPlan::gathers_bounds`), and `bounds_ordered_in` believes them in
Arrow's semantics alone. A `parse` over a cache that holds such a column
without bounds re-reads the block (`StatisticsRequest::backfill`, now handed
`bounded_columns`). The why is [`decisions.md`](decisions.md), "D79". **The
`statistics-gathering` re-take is P23's**, held with the other three figures
([`roadmap.md`](roadmap.md), "P23 — Statistics coverage and the resident
reserve").

## What 6.7 and 6.8 inherit

- **Which plans are bounded, and where they are believed.**
  `text_is_bounded_whatever_its_collation_and_believed_in_arrow_alone`
  (`pgtype.rs`) pins it. Every `Text` plan is gathered: `UnknownCollation`,
  `NonBytewiseCollation`, `NonDeterministicCollation` and `json`'s `AsText`.
  Only an undivergent one is believed in PostgreSQL's semantics. So `pgdt
  query` prunes exactly what it pruned before, and the provider prunes and
  stops on text under any collation. `collated_text_prunes_by_its_bytewise_bounds_in_arrow_semantics_alone`
  (`tests/pruning.rs`) checks both semantics on six majors, serial and split.
  For 6.7 that makes `bounds_ordered_in(Arrow)` the `Exact` predicate for a
  text column's bounds.
- **Still unbounded in both semantics**: `jsonb` (`JsonbStringCollation`),
  `character(n)` off `C` (a `PaddedText` plan), nested columns, and every
  planless or `Refused` column. These are Arrow-bytewise too, and a `Text`
  gatherer would serve them. 6.10 bounds all but the nested ones, and gives
  a second set to every kind whose PostgreSQL bounds are not Arrow's
  ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md),
  "Which columns carry bounds in Arrow's order").
- **`json` gets bounds and still no dictionary.** Its `AsText` affects
  equality, so D79 keeps no dictionary. In Arrow semantics its `=` prunes by
  bounds alone.
- **What the back-fill resolves.** `bounded_columns` resolves a block's columns
  against the map's DDL, as `observer_tracking` does, for every block of a
  gathering `parse`. With no DDL no column is bounded, so a data-only dump
  never reads as lacking. A held column with bounds is never re-read for this.
- **`info --detail` and `--json` now show bytewise bounds and a row order for
  collated text.** Nothing there says in which semantics they hold. The manual
  says it once, in "`--statistics`: what `parse` records for later queries".

## Negative results

- **No `CACHE_FORMAT_VERSION` bump**, per D78: no kind's order or equality
  moved. `golden_order_is_pinned_to_the_format_version` is unchanged.
- **`statistics-pruning`'s input moved and its semantics did not.** Its
  `v_category` is bare `text`, so it now carries bounds, and the fidelity guard
  (`pgdt/tests/perf_generator_fidelity.rs`) stopped asserting `no bounds`.
  `pgdt query` still reads only the dictionary for it. What the heavier cache
  costs that figure's legs is unpriced.
- **`statistics-gathering` is expected to move.** The control's `v_text`,
  `v_long_text` and `v_escaped` are bare `text`, three of its 16 columns, and
  each is now bounded.
