# P29.2 notes — the register moves to the image

What the sitting and the later slices inherit from the move. The mechanism is
`scripts/measure.py` (`register_image`, `build_in_image`, `image_target_dir`,
`apparatus_preflight`) and `scripts/release.py` (`Build`, `build_step`,
`run_in_image`'s `target`), whose docstrings say how each step runs; the
readings below are `runs/29.2-register-image/` (`image.log`; `builds.log`,
which `builds.py` there wrote by calling the harness's own build functions).

## What remains: the sitting

**The apparatus has landed and no figure has been taken on it.** The sitting
is a whole sweep from a commit carrying this change — a figure is never taken
from a tree carrying its own uncommitted apparatus — launched detached per
`CLAUDE.md`, "Long-running processes", and handed off per
`.claude/skills/gosub/handoff.md`:

```sh
cd scripts && uv run measure.py --all
```

Its tables fold into `measurements.md` whole, re-stamping it: every figure but
`session-drift` moves from glibc 2.44 to the image's 2.41, so nothing in the
doc may be set beside a reading of the new sitting except through its own
stamp. `parallel-scan-throughput`'s sitting of its own is taken with the rest.

## What the sitting inherits

- **The first sitting builds four binaries from scratch in the image**: the
  `system` and `jemalloc` legs and the instrument, each in its own target
  directory under `alloc-builds/image/`, and the `xz_decode` example in the
  release state beside the shipped build. `builds.log` is one such pass on
  this tree, each build's wall clock beside it, every binary's `--version`
  read back as its leg, and the preflight passing in the image under glibc
  2.41; a later sitting's builds are incremental.
- **Every one is held to the floor** by `release.py`'s build step: a variant
  needing a `GLIBC_` version past 2.41 fails its build rather than its first
  rep. Each needed 2.39 at most, `xz_decode` 2.34.
- **The timer resolves a millisecond, not a microsecond**: trixie's bash is
  5.2, which clamps `TIMEFORMAT`'s six places to three (`measure.TIME_FORMAT`
  already said an image's bash decides it). Every table quotes three places
  below two seconds, so no rendered cell loses a digit; the `--arms` reports
  print six, the last three now zeros.
- **glibc's arena ceiling moves with it**: 2.41 computes the limit as `8 x
  ncores` where 2.44 computed `max(8, ncores)` (`runtime-invariants.md`,
  "RT10"), so the `system` leg, the gate's twin and the instrument may keep
  more arenas than the stamped sitting's did at the highest worker counts —
  the sentence in `measurements.md`, "The apparatus" naming `8 x ncores` is
  the one the sitting makes true. Where a `system` reading moves, this is a
  candidate term before any other.
- **koji's wrap leg still exits 143**: the base image sets no `STOPSIGNAL`
  either.

## What later slices inherit

- **`release.py build` takes a package, an example, features and a host target
  directory** (`Build`), so 29.4's workflow and 29.5's `/release` call one
  recipe the register already times. A step runs one build per target and
  refuses two builds differing otherwise.
- **The release image carries `make`**, for the `jemalloc` leg alone; the
  call is under STATUS's "Decisions worth another look".

## Negative results

- **The image had no `make`**, which `tikv-jemalloc-sys` runs; nothing in the
  release build or the suite had needed it.
- **A leg's target directory cannot be the host leg's**: a host build's build
  scripts link the host's glibc, and the image's cargo could find one fresh
  and run it under 2.41. The image's legs build under `alloc-builds/image/`,
  apart from the recipes' host builds under the same root.
- **What stays on the host is what times no shipped binary**: `peak-rss`,
  static, links no glibc; `nested-decode-micro`'s `cargo bench` names the
  host's glibc in its marker (STATUS's "Decisions worth another look"); and
  the profile and heaptrack recipes, which print host builds and time nothing.
