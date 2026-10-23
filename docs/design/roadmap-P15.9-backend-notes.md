# P15.9 notes — `gz-seek`'s backend figure

The instrument is landed and the figure registered untaken (`measure.UNTAKEN`):
`gzseek-backend`, `gz-seek`'s first pass and its resumed unit decodes on each
engine, against a serial inflate of the same member. Its readings and the
backend decision they settle remain.

- **The engine is a feature of `gz-seek`**: `zlib-rs` (the default), `zlib-ng`
  and `zlib`, exactly one, `src/zlib.rs` refusing any other count at compile
  time. The module is written once over the zlib API, aliased per engine as
  `sys`; what differs is in it. `gz_seek::engine()` answers the linked engine's
  `zlibVersion`.
- **The instrument is `crates/gz-seek/examples/gz_read.rs`**: `--shape forward`
  is the first pass at `--spacing`, each window copied out; `--shape units` is
  that pass and then every unit decoded alone, its range read positionally.
  It prints `engine=`, `delivered=` and `units=`, and `--engine` alone answers
  what the harness interrogates before timing (`measure.gzseek_engine_problem`).
- **The figure** (`measure.run_gzseek_backend`) is warm only, five reps, at
  `measure.GZSEEK_SPACING`: serial inflate rows for `zlib-ng` and `zlib-rs` by
  the `gzip-inflate` instrument, then for each engine its first pass, its first
  pass with every unit, and the units alone as the difference of those two
  medians. Each leg is built in the release image in a target dir of its own,
  `alloc-builds/image/gzseek-<engine>` (`measure.ensure_gzseek_binary`). It
  refuses a run whose `delivered=` is not `control`'s size, an engine that is
  not its leg's, and engines disagreeing on the unit count.
- **Every engine passes the crate's suite**, `cargo nextest run -p gz-seek
  --no-default-features --features <engine>` in the release image, 35 of 35
  each (`runs/p15.9-backend-legs/suite-<engine>.log`). That suite is RT23's
  re-verification, so its claim (the stops at block ends, no unused bit at a
  stream's end, the primed resume) is observed on all three engines, though
  RT23 is written for zlib-rs only. Each leg's example read `control_gz` whole
  in both shapes in the pinned image, delivering `control`'s size, with 96
  units at 32 MiB on every engine (`runs/p15.9-backend-legs/smoke.log`) —
  untimed.

## The build routes

| Engine | Route | Tried | Result |
|---|---|---|---|
| zlib-rs | `libz-rs-sys` 0.6.8, Rust, no C build | built, suite | works: the default leg |
| zlib-ng | `libz-sys` 1.1.30 `zlib-ng`: CMake, zlib-compatible build, zlib's own symbol names | built, suite | works: the `zlib-ng` leg; its target dir holds a `CMakeCache.txt` |
| zlib-ng | `libz-ng-sys` 1.1.30: CMake, native API, `zng_` symbols | built, in the image, for `inflate-throughput` | works; not a leg (below) |
| zlib-ng | `libz-sys` 1.1.30 `zlib-ng-no-cmake-experimental-community-maintained`: `cc` | built (`runs/p15.9-zlib-ng-cc-probe/`) | broken: the function table fails to initialise; with `WITH_OPTIM`, AVX-512 symbols undefined |
| zlib-ng | a system zlib-ng | read | none in Debian trixie (`roadmap-P15.1.1-inflate-notes.md`) |
| zlib | `libz-sys` 1.1.30 `stock-zlib` + `static`: `cc`, its bundled zlib 1.3.2 | built, suite | works: the `zlib` leg |
| zlib | `libz-sys`'s default, the system's zlib by `pkg-config` | built, `ldd` (`runs/p15.9-backend-legs/route-zlib-system.log`) | the image carries `zlib1g` but no `zlib1g-dev`, so `libz-sys` falls back to its bundled copy and the binary links no `libz.so` |

**Recommended, an engine at a time**: zlib-rs through `libz-rs-sys`; zlib-ng
through `libz-sys`'s CMake build; stock zlib through `libz-sys`'s static
build. Which engine ships is the figure's.

- **`libz-ng-sys` is not a leg** because its native API is not the one this
  module is written over: its init, its version call and its stream's field
  types differ, and the same engine is reached through the compatible build.
  It is the zlib-ng route to weigh for **shipping**, should zlib-ng win: its
  `zng_` symbols cannot collide with another zlib linked into `pgdt`, where the
  compatible build exports zlib's own names.
- **A build of our own for a CMake-free zlib-ng**, weighed: a `[patch]` of
  `libz-sys`'s `zng/cc.rs`, or zlib-ng's own `configure` and `make` driven by
  a build script of ours. Either keeps a source list in step with zlib-ng's
  by hand. It buys only CMake's absence from a shipped build, which D111's
  Reopens already answers another way — the check narrowed to `aws-lc-sys` by
  name. Worth building only if zlib-ng wins and that narrowing is refused.
- **The system zlib** would need `zlib1g-dev` in the release image and would
  ship a `pgdt` needing `libz.so.1` beside glibc, a dependency a release does
  not promise today (D104).
- **The `cc` build's defect is an upstream report, not a `UF<k>`**: no plan
  waits on it and nothing in the tree works around it, the CMake build being
  `libz-sys`'s own route to the same engine. Drafted for the maintainer to
  file, `runs/p15.9-zlib-ng-cc-probe/issue.md`; no such issue was open on
  2026-10-23.

## What the readings round inherits

- **The readings**: `cd scripts && uv run measure.py --figure gzseek-backend`,
  from the commit carrying this instrument. It times forty runs of under ten
  seconds each, plus the three leg builds, the CMake one the longest: well
  under half an hour, so no handoff.
- **Taking them moves the figure** from `UNTAKEN` into `FIGURES`, with its
  section in `measurements.md`.
- **The decision is a `D<k>` of its own**; `decisions.md` sits one line under
  its cap, so the entry means striking or trimming another. Then UF4 keeps
  only why the engine is reached through its C API, RT23 is rewritten for the
  engine that ships, and `crates/gz-seek/Cargo.toml`'s `default` names it.
  Should zlib-ng win, D111's check meets CMake in `pgdt`'s closure once 15.5
  links `gz-seek`, so the route weighed above becomes that entry's question.

## Negative results

- **`libz-sys` 1.1.30 declares neither `inflateGetDictionary` nor `crc32_z`**,
  in either of its modes; `src/zlib.rs` declares both for its `libz-sys`
  legs, every engine it builds exporting them.
- **`libz-sys`'s `z_stream` takes no null allocator**, so its legs hand the
  stream `calloc` and `free`, which is zlib's own default.
- **`libz-sys` given both modes builds stock zlib** (its `build.rs`: zlib-ng
  only without `stock-zlib`, which its default carries), so the `zlib-ng`
  feature turns its defaults off, and a leg that lost its feature would read as
  the `zlib` one — which is why the harness asks each build its engine.
