# P15.1.1 notes — The serial inflate figure

The instrument half of the figures 15.1 named; its readings are not yet taken,
and the candidate spacings are 15.1.2
([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md), "15.1.1:
the inflate figure is built, untaken").

- **`inflate-throughput` is registered in `measure.UNTAKEN`**: one table, a row
  each for both files' `dd` floors, the plain `pgdt parse --jobs 1`, and one
  serial inflate a backend, a column a regime — warm, cold SSD, cold NVMe. An
  inflate cell carries its time as a multiple of the same regime's `parse`,
  the ratio declared as a `Subtraction` so both come from one sweep. Taken by
  `cd scripts && uv run measure.py --figure inflate-throughput`, from a commit
  carrying the instrument (`measurements.md`, "A figure may be published
  outside the sweep"). It stands in no borrow edge, so a sitting of its own
  publishes.
- **The instrument is `crates/pgdt-io/examples/gzip_inflate.rs`**, built in
  the release image like `xz_decode`: `--backend zlib-rs|miniz-oxide`, one
  thread, the header, CRC-32 and `ISIZE` checked, a second member refused,
  `delivered=` printed and held by the harness to `control`'s size. The
  backends and `crc32fast` are `pgdt-io` dev-dependencies pinned with `=`,
  and the figure declares that manifest.
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

- **zlib-ng is not a leg.** No binding to it builds in the release image:
  `libz-ng-sys` is CMake-only and the image installs no CMake, and `libz-sys`'s
  `zlib-ng-no-cmake-experimental-community-maintained` build is broken at both
  versions tried — at 1.1.30 (zlib-ng 2.3.3) `WITH_OPTIM` is never defined, so
  the function table is left incomplete and `inflateInit2` fails with
  `Z_VERSION_ERROR` after printing "Zlib-ng functable failed initialization!";
  at 1.1.23 (zlib-ng 2.2.5) it defines `X86_AVX512` without compiling
  `chunkset_avx512.c`, and the link fails on `inflate_fast_avx512`. What
  replaces it is STATUS's "Decisions worth another look".
- **`zlib-rs` and `miniz_oxide` are driven directly**, not through `flate2`,
  whose backend is a build-time feature: one binary taking a flag keeps the
  legs differing by the decoder alone.

## What the next round inherits

- **The readings** are `--figure inflate-throughput`; the sitting's
  `control_gz` generation and three cold regimes make it a long one, so it is
  handed off per `.claude/skills/gosub/handoff.md` if it runs past half an
  hour. Before reasoning from it, `measure.py --stale` as for any figure.
- **15.1.2, the spacing figure**, is unbuilt; what a spacing leg times — a
  checkpointing first pass, a unit decode resumed from a checkpoint, or both —
  is that round's design.
