# P28.9 — The figures: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Evidence" and "Facts found while grilling"; the attribution rule it applies is
[`roadmap.md`](roadmap.md), "Attribution is introspective; only the gate is
blind". The instrument has landed and its readings have not: nothing in
`measurements.md`'s tables was re-taken, so every table there is still the
`da05a72` sitting's, the retired figures' provenance notes included.

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
  `metadata` and `data` (`STATISTICS_LEGS`), its caption "What the data level
  costs a parse", its rows the three throughput inputs and `arrays`, and it
  declares `unrepresentable.rs`. Its id is unchanged.
- **`statistics-pruning` has two legs a filter**, the `uncarried` leg gone;
  a test holds that no shape queries a cache a metadata-level `parse` wrote.
- **`--profile-recipe` profiles the data level**:
  `parse-statistics-data-rss`, the figure's `data` leg without its resident
  wrapper, over `control` and `arrays`, landing as
  `runs/profile-parse-statistics-data-rss-<input>.{data,txt}`.

## What the next slices inherit

- **The readings wait on 28.9.1**, which moves the query figures, so one
  sweep takes both: `cd scripts && uv run measure.py --all`, detached per
  `CLAUDE.md`, "Long-running processes", from a commit carrying both. Then
  `uv run measure.py --profile-recipe`, run as printed; the census's price,
  the count inside it, is the share of the data leg's profile under
  `map::census_row` and the count it drives, read against that leg's wall.
- **The fold renames `measurements.md`'s statistics-gathering section** to
  the harness's caption, re-running `citations.py`, and rewrites the prose of
  both statistics sections against their new legs. D35's Evidence already
  cites `statistics-gathering`. **`statistics-pruning`'s rewritten prose
  keeps the upper bound** on decoding the cache's statistics, the pruned legs'
  floor, so the figure does not stop saying what carrying them costs.
- **The arrays row is where the census inspects array shapes**; the
  control's rows, holding counted columns and no array, are split too at the
  data level, since the count joined the census
  ([`roadmap-P28.3-count-notes.md`](roadmap-P28.3-count-notes.md)), so the
  control's row does not price the census's pre-filter alone.

## Negative results

- **No builder writes a table's census without its statistics**, so a leg
  pricing what carrying statistics costs a query has no cache to read: a
  metadata-level one times a census re-read, and a column override's repeats
  the subtraction the retired leg's reading left unresolved (`measure.py`,
  beside `PRUNING_LEGS`).
