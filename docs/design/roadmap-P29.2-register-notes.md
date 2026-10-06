# P29.2 notes — the register moves to the image

What the move's sitting found and what the later slices inherit. The mechanism is
`scripts/measure.py` (`register_image`, `build_in_image`, `image_target_dir`,
`bench_in_image`, `apparatus_preflight`) and `scripts/release.py` (`Build`,
`build_step`, `Bench`, `step_bench`, `run_in_image`'s `target`), whose
docstrings say how each step runs; the readings below are
`runs/29.2-register-image/` (`image.log`; `builds.log`, which `builds.py`
there wrote by calling the harness's own build functions; `bench.log`, one
`release.py bench` of the figure's group, not a figure).

## The sitting

**Every figure but `session-drift` was re-taken at `3a34f062` and folded in
whole, re-stamping `measurements.md`**: `runs/measure-20261006T193318/`
(`tables.md`, `raw.json`, `log.txt`, the instrument reports under
`instrument/`), launched detached from a tree carrying only the history entry
and the STATUS line, neither a declared path. No leg was killed on either
build, so both gates pass; `parallel-scan-throughput` came in with the rest
and its marker carries no sitting of its own. The two sittings either side of
the move differ in the build environment, the image, the glibc and the
timer's resolution at once, so a cell that moved against `1c9fc9be` names
none of them alone, and the doc's prose says so where it sets the two side by
side.

What the sitting found, each filed beside the figure it is read off:

- **`reserve`'s model is refuted at 128 MiB blocks**: the worst rep of each
  flagless leg at four and five readers lands over `MEMORY_RESERVE`, the
  `rule` band, its other two reps in the `bound` band the previous stamp read,
  and the `system` twins inside the bound. The figure publishes, a kill alone
  keeping one out; the finding is `KD34`'s, rewritten to it, and v0.1.0 ships
  with the reserve as it stands ([`decisions.md`](decisions.md), "D3").
- **The instrument legs at 128 MiB blocks read both of `KD34`'s units** — the
  program's, `KD111`, and the one mimalloc keeps — and never the worst rep's
  excess, which nothing names (`measurements.md`, "What a scan holds above the
  budget it was given").
- **The warm `INSERT` scan's multiple reproduces** on the release image's
  build, so the move the previous stamp left unattributed is not that
  sitting's build environment, image or glibc (`measurements.md`, "Scan
  throughput by input shape").
- **glibc 2.41's arena ceiling is read**: the instrument opens more arenas at
  twenty-four readers than 2.44's ceiling allows (`runtime-invariants.md`,
  "RT10", where the observation is recorded).
- **`nested-decode-micro`'s first reading on mimalloc** moves `render`, the
  side that allocates, and starts its copy control's series afresh
  (`measurements.md`, "Nested decode costs what it copies").
- **Every warm `dd` floor reads faster than the previous stamp's**, a shared
  move in the direction that disqualifies nothing, unattributed between the
  two images (`measurements.md`, "The floor is read directionally").
- **The `allocator` table's apparatus line carries a discarded rep**: its CPU
  stall is a `dd` rep the contention gate threw out and re-took, since
  `measure.apparatus_note` reads every run a figure's records hold, retakes
  included (`log.txt`, the `allocator` stage's `DISCARDED` line). No reading in
  the table was taken under it.

## What later slices inherit

- **`release.py build` takes a package, an example, features and a host target
  directory** (`Build`), so 29.4's workflow and 29.5's `/release` call one
  recipe the register already times. A step runs one build per target and
  refuses two builds differing otherwise.
- **The release image carries `make`**, for the `jemalloc` leg alone, under
  the spec's admission rule
  ([`roadmap-P29-releases.md`](roadmap-P29-releases.md), "The build image").
- **A sitting builds four binaries in the image besides the shipped one**: the
  `system` and `jemalloc` legs and the instrument, each in its own target
  directory under `alloc-builds/image/`, and the `xz_decode` example in the
  release state beside the shipped build. `builds.log` is the first such pass,
  each binary's `--version` read back as its leg; the sitting's own were
  incremental on those.
- **`nested-decode-micro` runs in the release image itself**, not its base:
  `cargo bench` builds and runs in one step, so the image's tag is the place
  `glibc_of` asks, and the marker names nothing while that answers the
  stamp's glibc. criterion writes under the release state's target directory,
  `target/release-image/target/criterion/`, which the figure reads; the
  host's `target/criterion/` is read by nothing.
- **Every timed binary is held to the floor** by `release.py`'s build step,
  and the bench's executable by its bench step before it runs: a variant
  needing a `GLIBC_` version past the image's fails its build rather than its
  first rep.
- **The timer resolves a millisecond**: trixie's bash is 5.2, which clamps
  `TIMEFORMAT`'s six places to three. Every table quotes three places below
  two seconds, so no rendered cell loses a digit; the `--arms` reports print
  six, the last three zeros.
- **koji's wrap leg still exits 143**: the base image sets no `STOPSIGNAL`
  either.

## Negative results

- **The image had no `make`**, which `tikv-jemalloc-sys` runs; nothing in the
  release build or the suite had needed it.
- **A leg's target directory cannot be the host leg's**: a host build's build
  scripts link the host's glibc, and the image's cargo could find one fresh
  and run it under 2.41. The image's legs build under `alloc-builds/image/`,
  apart from the recipes' host builds under the same root.
- **What stays on the host is what times nothing**: `peak-rss`, static,
  links no glibc; and the profile and heaptrack recipes, which print host
  builds.
