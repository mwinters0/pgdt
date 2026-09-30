# P28.9.1 — The query figures over a data-level cache: notes

What the readings and the fold inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Evidence"; the instrument's rules are `measure.DATA_LEVEL_QUERIES`' comment.
The instrument has landed and its readings have not. Every table in
`measurements.md` is still the `da05a72` sitting's, taken with `--dtcache none`.

## What exists

- **Every `query-typed`, `query-strings`, `query-project-*`, `query-where-*`
  and `query-typed-jobs-*` shape** runs `measure.DATA_LEVEL_BUILDER`, an
  untimed `parse` stating `--jobs 1` and `GATHER_STATISTICS`, then `&&`, then
  its timed `query` stating `DATA_LEVEL_QUERY` in place of `--dtcache none`.
  That changes six figures: `nested-end-to-end`, `cross-file-floor`,
  `projection-widths`, `predicate-terms`, `allocator`'s two query rows and
  `parallel-scan-throughput`'s typed-`query` legs. `query-nomatch*` keeps
  `--dtcache none`, since it times the map and its saves.
- **The six figures declare `CACHED_QUERY`** (the map, the cache, the
  statistics). Each one's generated notes say what its query reads
  (`_cached_query_note`). `statistics_flag_problems` admits
  `GATHER_STATISTICS` on these shapes' builder, and "The apparatus" says so.
- **`--profile-recipe` writes the same cache before each query profile**,
  unprofiled (`profile_builder_argv`). `ProfileRecipe` holds both builders to
  `_script`'s.

## What the readings inherit

- **One sweep takes 28.9's readings and these**, as 28.9's notes say, run
  detached from a commit carrying both. `DataLevelQueries` holds the shapes.
- **The fold rewrites what reads these figures as mapping inside the timer**:
  the hand-written reproduce loop under `cross-file-floor` in
  `measurements.md`, which spells `--dtcache none`, and any prose attributing
  a row to a mapping pass or the census.
- **`KD17`'s reading is the old column's.** "About a tenth by four
  sub-streams" was read off a plain typed `query` that mapped before it
  replayed. The fold re-reads it against the new column, along with its marker
  in `stream.rs` (`plan_partitions`) and P22's inbox entry, which is
  contingent on it.
- **`QUERY_SUBSTREAM_CAP` stands.** With no mapping pass, no chunk is
  announced before the replay plans (`KD41`), so the plain source's slot is
  `POOL_MAX_BYTES` rather than the default chunk. `LocalFileSource::partitions`
  bills 8 MiB either way (`io.rs`), and `derived_source_span` floors at the
  options' chunk. A debug build over a small control file planned 7
  sub-streams at `--jobs 8` both with a cache and without, at the figure's
  stated allowance.
- **What a query reads carries decoding the whole cache, statistics
  included.** This is what a query after a default `parse` pays. The builder
  discovers its statistics allowance from its container, so a figure's
  container sizes the cache it decodes. `statistics-pruning`'s legs bound
  that decode from above.

## Negative results

- **A builder at the row's own `--jobs` was refused**: the `--jobs` axis
  would then read a different cache on each row, a second variable. The
  price is untimed sweep time, a serial data-level `parse` per rep.
- **`;` between builder and query was refused for `&&`**: with `;`, a
  builder that failed would leave the query to map cold and save, inside the
  timer, and the harness would publish that as a cached read.
  `statistics-pruning`'s builder still uses `;`. It is unchanged here because
  changing it would move that figure's shape.
