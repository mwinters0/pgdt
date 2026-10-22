# P15.1.1 notes — The serial inflate figure

The serial inflate figure, taken at `d2cf15c4` and read in
[`measurements.md`](measurements.md), "What serial inflate costs against the
plain parse"; the candidate spacings are 15.1.2
([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md), "15.1.1:
the inflate figure, and 15.8 admitted").

- **`inflate-throughput` is a register figure published outside the sweep**:
  one table, a row each for both files' `dd` floors, the plain `pgdt parse
  --jobs 1`, and one serial inflate a backend, a column a regime — warm, cold
  SSD, cold NVMe. An inflate cell carries its time as a multiple of the same
  regime's `parse`, the ratio declared as a `Subtraction` so both come from one
  sweep. It stands in no borrow edge, so a sitting of its own publishes.
- **The instrument is the `gzip-inflate` member**, `crates/gzip-inflate/`,
  marked apparatus so the release suite leaves it out (`decisions.md`,
  "D110"), and built in the release image as every timed binary is:
  `--backend zlib-ng|zlib-rs|miniz-oxide`, one thread, the header, CRC-32 and
  `ISIZE` checked, a second member refused, `delivered=` printed and held by
  the harness to `control`'s size. The backends — `libz-ng-sys` (zlib-ng
  2.3.3, native API, CMake-built), `zlib-rs`, `miniz_oxide` — and `crc32fast`
  are its dependencies pinned with `=`, and the figure declares its manifest
  and `src/`. Its tests run every backend over members `flate2` writes;
  `flate2` became a workspace dependency to be named twice.
- **The input is `control_gz`**, `control` deflated by
  `scripts/generate_gzip_input.py` through Python's `zlib`, refused unless it
  is `ZLIB_VERSION` and unless the header is `PG_DUMP_HEADER`, which a test
  reads back off every major's `compress.sql.gz`. Generating it takes minutes;
  the sitting does it once and keeps it beside `control`.
- **The inflate shapes state no worker count** and are admitted to
  `measure._NO_WORKERS` beside `dd`; the test holding that tuple now refuses a
  `pgdt` subcommand there rather than the `/pgdt` mount point, which every
  figure's binary takes.

## Negative results

- **zlib-ng has no CMake-free binding**, which is why its leg needs CMake
  and the instrument a member of its own: `libz-sys`'s
  `zlib-ng-no-cmake-experimental-community-maintained` build is broken at both
  versions tried — at 1.1.30 (zlib-ng 2.3.3) `WITH_OPTIM` is never defined, so
  the function table is left incomplete and `inflateInit2` fails with
  `Z_VERSION_ERROR` after printing "Zlib-ng functable failed initialization!";
  at 1.1.23 (zlib-ng 2.2.5) it defines `X86_AVX512` without compiling
  `chunkset_avx512.c`, and the link fails on `inflate_fast_avx512`
  ([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md), "The
  zlib-ng leg, reviewed").
- **No zlib-ng reaches the image another way.** Debian trixie packages
  none (`apt-cache` in the pinned base lists `cmake` 3.31.6-2 and no
  zlib-ng), and zlib-ng's own `configure` + `make` build, CMake-free, is
  driven by no crate but `libz-sys`'s broken one — using it is build plumbing
  of our own.
- **Every backend is driven directly**, not through `flate2`,
  whose backend is a build-time feature: one binary taking a flag keeps the
  legs differing by the decoder alone.

## What the next round inherits

- **The reading earns a parallel first pass, admitted as 15.8**: by the
  figure's own criterion, an inflate row slower than the `parse` beside it,
  a single member's serial first pass is bound by its inflate on tmpfs and
  the NVMe, and only on the SATA SSD does the plain `parse`'s device bound
  leave it the faster of the two. The call is under STATUS's "Decisions
  worth another look".
- **`gz-seek`'s throughput floor is the `Inflate, zlib-ng` row**: its forward
  decode, checkpoints taken and every check made, no slower than zlib-ng's
  serial inflate of the same member. A ratio between two binaries holds only
  within one sitting, so the figure that reads the floor — 15.1.2's
  checkpointing first pass against a serial inflate — carries a zlib-ng leg of
  its own, which the `gzip-inflate` member already supplies.
- **A re-take is about ten minutes** once `control_gz` exists, and it is kept
  beside `control` in the SSD cache, so it needs no handoff. The gate discards
  a reading taken while anything else on the machine compiles; a sitting
  started under a neighbour's build fails on its first reading.
- **15.1.2, the spacing figure**, is taken on `gz-seek` after 15.4; what it
  times is its STATUS row
  ([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md), "The
  spacing figure, reordered").
