# P15.7.1 notes — the throughput readings

**`gzip-first-pass` is taken at `c4069143` and its verdict is `shared`**
([`measurements.md`](measurements.md), "What a parallel gzip first pass
buys"). On every file the pass lifts on, the physical count beats the logical
count and the logical count less one, and those two are level. So the cost
of the logical count is a core or a cache shared, not the scan's thread. A
local first pass now recommends the cores it was given, no more than the
physical cores its affinity allows, capped at its chunks
(`FirstPass::default_workers`, D2). The physical cores are read off `/proc`
and sysfs by `pgdt-io`'s `cores.rs` (`RT27`). `KD145` is struck. The chunk
stays at the shipped length (D121). The level-1 file regresses (`KD147`).

The figure is retaken from any commit carrying the instrument:

    cd scripts && uv run measure.py --figure gzip-first-pass

The falsification criteria are in `measure.run_gzip_first_pass`'s docstring,
and the writers in `generate_gzip_input.py`'s. Why these four writers and
this bar: [`../status/history/2026-10-27.md`](../status/history/2026-10-27.md),
"15.7.1's writers and its bar, reviewed".

## What later slices inherit

- **15.7.2**: a `.gz` file's first pass, run with no `--jobs`, takes the
  physical cores within its affinity. An `.xz` file still takes its cores.
  The manual's `--jobs` paragraph names only `.xz`'s default. A member whose
  writer ends no block before its last, `deflate_quick` at level 1, is read
  more slowly in parallel than serially (`KD147`). Its serial first pass is
  also several times slower than a level-6 file's, mostly D118's walk
  ([`roadmap-P15.19-pause-notes.md`](roadmap-P15.19-pause-notes.md), "The
  opening account").
- **15.18**: a flagless first pass now runs at most the physical count. So
  `gzip-resident`'s twenty-four-job leg prices a count that only a stated
  `--jobs` reaches.

## Negative results

- **The level-1 file's serial pass and its slowing frontier** were
  unattributed here; 15.19's profile attributes both
  ([`roadmap-P15.19-pause-notes.md`](roadmap-P15.19-pause-notes.md), "The
  opening account"). Every speculation over that file finds nothing, as
  15.10's notes foresaw, so the workers add no lift (`KD147`).
- **The floor is noisier than the passes.** One sweep's plain `parse` median
  sits above the others, so that writer's "over the floor" ratio reads lower.
  The paired verdicts do not read the floor.
- **`GZPASS_FAMILY` is not a `JOBS_AXIS` family.** Those families' rows are
  `PARALLEL_JOBS` under `PARALLEL_BUDGET`, and four tests hold every family
  to both. The 23-job row and the larger budget are this figure's alone, so
  the family is exempted from `SWEEP_JOBS` by its own prefix, as the decode
  instrument's is.
- **The PyPI zlib-ng is not Fedora's at level 6**, so it is not the
  `pg_dump -Z` its label named (`runs/p15.7.1-writers/reading.txt`). The
  `ctypes` path exists because the Python `uv` runs has its `zlib` built in,
  so `LD_LIBRARY_PATH` cannot point it at another `libz.so.1`, and the
  `fedora:44` image has no Python.
- **The default writer's bytes did not move.** The generator before this
  instrument and after it write byte-identical files at its default and at
  `--bgzf`, checked on a 50 MB prefix of the plaintext.
