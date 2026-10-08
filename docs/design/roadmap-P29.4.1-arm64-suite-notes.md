# P29.4.1 notes — the arm64 suite built where it runs

What 29.6 inherits. The mechanism is `.github/workflows/release-build.yml`'s
`arm64-suite` and `arm64-smoke` jobs and `release.py`'s `suite`; the decision is
[`roadmap-P29-releases.md`](roadmap-P29-releases.md), "The build image", and
[`../status/history/2026-10-08.md`](../status/history/2026-10-08.md), "The arm64
suite is built where it runs". Nothing below ran on an arm64 host.

## What 29.6 inherits

- **`arm64-suite` is the first arm64 build of the whole workspace's tests**,
  needing only `preflight`, so it runs beside the cross build rather than after
  it; `arm64-smoke` alone waits on the archive.
  The build's cold time, its memory against the runner's 16 GB at four cargo jobs and
  its disk are unmeasured; the earlier 24-job cross build's out-of-memory is in
  [`roadmap-P29.4-workflows-notes.md`](roadmap-P29.4-workflows-notes.md), "What
  29.6 inherits". A runner that cannot hold the build is a job cap or a lighter
  test profile for the suite, a measurement first. `release.py`'s container limit
  (`MEMORY`) is above a runner's 16 GB, so on either suite job it limits
  nothing, and running out kills the runner rather than the container: a lost
  runner rather than `ld`'s signal 9 is how that reads in the run's log.
- **The arm64 suite now runs the doctests** as the x86-64 job does, so a library
  first holding one needs no decision about arm64.
- **The arm64 image now compiles**, where it only ran an archive before: the
  cross linker and `CC_`/`AR_` variables the Dockerfile sets for
  `aarch64-unknown-linux-gnu` are read there by the native build, and resolve
  to the `aarch64-linux-gnu-gcc` and `-ar` that Debian's arm64 `gcc` and
  `binutils` pull in (`gcc-aarch64-linux-gnu`, `binutils-aarch64-linux-gnu`,
  read from packages.debian.org's trixie file lists, not run). Nothing here
  has built that image.
- **The tests are compiled by the native toolchain**, not the cross one that
  builds the shipped binary: the suite tests the source per architecture, and
  the smoke run the bytes that ship, as the spec's "The build image" already
  divides them.

## Negative results

- **qemu-user was not taken.** It would run the cross-built tests on this host,
  but under this host's `/proc`, cgroups and page size, and without arm's
  memory ordering, which are what the runtime-discovery and allocator tests
  look at; it needs a system-wide `binfmt` registration this machine lacks.
  `/release` still rehearses arm64 as a build only.
