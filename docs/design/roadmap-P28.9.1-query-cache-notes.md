# P28.9.1 — The query figures over a data-level cache: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Evidence"; the instrument's rules are `measure.DATA_LEVEL_QUERIES`' comment.
The instrument and its readings have both landed, taken in 28.9's sweep at
`183a50eb` (`runs/measure-20261001T010810/`).

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
- **The readings are folded.** Every one of the six fell against `da05a72`'s
  by about the mapping pass it no longer times, while every per-row and
  per-column difference read off them reproduced; `measurements.md`'s prose
  says so section by section, and its hand-written reproduce loop under
  `cross-file-floor` runs the builder before each timed `query`.

## What the next slices inherit

- **A typed `pgdt query` over `.xz` does not scale with `--jobs`**, and never
  did: the earlier column's gain was its mapping pass
  ([`measurements.md`](measurements.md), "What a second scan worker buys").
  What holds it, and the plain column's tenth, is `pgdt query`'s in-order
  merge, which reads one sub-stream at a time past its first round (`KD57`);
  the library's sub-streams run concurrently under DataFusion, and `M187`
  moves the figure's query legs there.
- **`QUERY_SUBSTREAM_CAP` stands.** With no mapping pass, no chunk is
  announced before the replay plans (`KD41`), so the plain source's slot is
  `POOL_MAX_BYTES` rather than the default chunk. `LocalFileSource::partitions`
  bills 8 MiB either way (`io.rs`), and `derived_source_span` floors at the
  options' chunk; the plain column planned 7 sub-streams from eight jobs, as
  before.
- **What a query reads carries decoding the whole cache, statistics
  included.** This is what a query after a default `parse` pays. The builder
  discovers its statistics allowance from its container, so a figure's
  container sizes the cache it decodes. `statistics-pruning`'s pruned legs
  bound that decode from above.
- **About one cached-`query` rep in thirty reads roughly half a second slow**,
  across the nested, cross-file, allocator and parallel figures, where the
  `da05a72` sitting's uncached shapes read one such rep; medians absorb it,
  and one lands on `cross-file-floor`'s floor row. It is unattributed. It is
  not a mapping pass: a probe of the same shape found none in a slow rep's
  `stderr`.
- **`raw.json`'s `reported` resolution is the builder's on these shapes.**
  `parse_resolution` reads the first `scan started` line in the container's
  `stderr`, and a cached `query` maps nothing and prints none, so the line is
  the untimed builder's `--jobs 1` and every typed-`query` leg of
  `parallel-scan-throughput` reports one reader. Only `reserve`, whose legs
  are `parse`s, consumes the pair, so no table is wrong; what is lost is the
  check its docstring names, a row labelled 24 that ran fewer, on these legs.

## Negative results

- **A builder at the row's own `--jobs` was refused**: the `--jobs` axis
  would then read a different cache on each row, a second variable. The
  price is untimed sweep time, a serial data-level `parse` per rep.
- **`;` between builder and query was refused for `&&`**: with `;`, a
  builder that failed would leave the query to map cold and save, inside the
  timer, and the harness would publish that as a cached read.
  `statistics-pruning`'s builder still uses `;`. It is unchanged here because
  changing it would move that figure's shape.
