# P15.7.1 notes — the throughput readings

**The instrument is landed and `gzip-first-pass` is registered untaken; the
readings remain.** They are taken from the commit carrying it
([`measurements.md`](measurements.md), "A figure may be published outside
the sweep"):

    cd scripts && uv run measure.py --figure gzip-first-pass

It is `pgdt parse` over a single gzip member, its first pass, at `--jobs` 1,
4, 12, 23 and 24 (`GZPASS_JOBS`), and at 4 MiB chunks at four, seven reps a
leg. There are four writers' files and a sweep for each, with the plain
`parse` of `control` read in every sweep as the scan's floor.
`measure.run_gzip_first_pass`'s docstring holds the falsification criteria.
They were written before any reading.

## What the readings round inherits

- **The figure settles the default itself, by a rule fixed before the
  sitting.** One leg beats another where it is faster in all but one of its
  paired reps (`GZPASS_SPARED`) *and* its median paired difference is over
  `GZPASS_MARGIN` of its own median; two legs neither of which beats the
  other are level. Each file's 12, 23 and 24 fall in one of the spec's three
  causes ([`roadmap-P15-gzip.md`](roadmap-P15-gzip.md), "Slice order") in
  `GZPASS_OUTCOMES`, every order settling: 12 not beating 24 is `stands`, and
  past that 12 beating 23 is `shared` and anything else `oversubscription`.
  A file on which 12 does not beat serial says nothing of the default:
  `regresses` where serial beats any of the three, named in the verdict and
  owed a `KD<k>` by the round reading it, and `no lift` otherwise.
  `gzpass_verdict` settles only where every file the pass lifts on agrees.
  Moving the default, amending D2 for this source and closing `KD145` are
  that round's, in the change that folds the table in.
- **The writers** (`GZPASS_WRITERS`, `generate_gzip_input.py --writer`):
  - zlib 1.3.1 at level 6, the existing `control_gz`, which is `pg_dump -Z`
    over stock zlib.
  - Fedora 44's `zlib-ng-compat` 2.3.3-3.fc44 at level 6, `pg_dump -Z` on
    Fedora. The generator copies that `libz.so.1` out of a `fedora:44` image
    pinned by digest, checks the package's version with `rpm`, and calls its
    `deflate` through `ctypes` (`LibzDeflater`), so generating it runs the
    container runtime `measure.py` does. Its bytes were checked once by hand
    against Python's own `zlib` loaded onto the same library, on an 11 MB
    payload, and matched. Over the host's stock zlib the `ctypes` path
    writes Python's bytes too, which a test holds.
  - GNU gzip 1.15 at level 6, `pg_dump | gzip`, the `gzip` on `PATH`.
  - zlib-ng 2.2.5 at level 1, the writer that ends no block, which 15.10's
    notes said this figure would read on the frontier alone. It comes from
    the `zlib-ng` Python package pinned in `scripts/pyproject.toml`, which
    bundles its own zlib-ng.

  Each writer is refused at any version but the one the generator names.
  Why these four and this bar:
  [`../status/history/2026-10-27.md`](../status/history/2026-10-27.md),
  "15.7.1's writers and its bar, reviewed".
- **The chunk-length leg is a build of its own**: `pgdt`'s
  `gzip-4mib-chunks` feature, which forwards to `pgdt-io`'s and switches
  `CHUNK_BYTES` in `first_pass/ahead.rs`. `pgdt --version` names it as
  `(gzip chunk: 4 MiB)` (`version.rs`, `CHUNK_MARKER`), and
  `ensure_chunk_binary` refuses a leg naming anything else. It is built into
  `image_target_dir`'s `gzip-4mib-chunks` and copied to
  `runs/pgdt-gzip-4mib-chunks`, beside the allocator legs. `check.py` lints
  it (`FEATURE_BUILDS`). It is read at four jobs only, the optimization
  baseline ([`roadmap.md`](roadmap.md), "Four workers is the optimization
  baseline; twenty-four is the guard"), and it differs from the shipped binary by its
  layout too, which no pair removes.
- **The budget is the figure's own**: `GZPASS_BUDGET`, 3 GiB, in a 5g
  container. `gzip-resident`'s 2 GiB affords `control_gz` twenty workers at
  `--jobs 24` under `margin_allowance`. A test holds the new budget to
  twenty-four at either chunk length, and every run's `scan started` count
  is checked against its row.
- **A sitting's cost is not measured.** The run builds the chunk leg in the
  image, a cold fat-LTO build in its own target dir. It generates three
  3 GiB compressions, and `control_gz` again because the generator's stamp
  moved. Then it takes 224 timed runs at two to five seconds each. That may
  pass half an hour, in which case the round hands it off
  (`.claude/skills/gosub/handoff.md`). The machine must be this one: the rows
  are its 24 threads on 12 cores, and `gzpass_topology_problem` refuses any
  other.

## Negative results

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
  change and after it write byte-identical files at its default and at
  `--bgzf`, checked on a 50 MB prefix of the plaintext. Every figure
  declaring the generator is red all the same.
