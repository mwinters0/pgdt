# P29 inbox — facts filed for its grilling

Evidence P29 (versioned binary releases) will need. **This is a queue, not a
document**: when P29 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## A pushed tag is a Releases-feed entry, carrying its annotation, before any Release exists

**Fact.** A repository's `releases.atom` lists bare tags as entries: the Linux
kernel's repository, which has tags and no Releases, carries one entry per tag
whose content is the annotated tag's message ("Linux 7.3-rc5"), and whose id
is keyed by the tag name alone
(`tag:github.com,2008:Repository/<id>/<tag>`) — the same id a Release on that
tag would carry, so a reader that has already taken the entry need not fetch
it again when the Release replaces it. Observed, not documented by GitHub.

**Why P29 cares.** The maintainer's doc reaches feed readers at the moment the
tag is pushed, and only if it is already there. That decides where the doc
lives — an annotated tag's message lands with the tag, while a file or a
workflow input lands with the Release, after the tag's entry has been read
empty — and whether binaries trailing the tag by a build's duration is
acceptable or the build must run before the tag is pushed.

**Origin.** Filed 2026-10-01 by the session sketching P29, from
`https://github.com/torvalds/linux/releases.atom`. Contingent on GitHub's feed
behaviour, which can change without notice; re-check against a tag-only
repository's feed before relying on it.

---

## Every figure is a glibc build on the platform allocator, and musl measured slower

**Fact.** Every performance figure is the default `glibc` build, run in a
glibc image, and every one published so far was taken on the platform
allocator, which `pgdt`'s default build does not link; static musl measured
materially slower on the workloads that move real bytes, and is in no recipe
([`measurements.md`](measurements.md), "The apparatus";
[`decisions.md`](decisions.md), "D13"). `pgdt`'s `system` and `jemalloc`
features are measured legs, not a supported matrix
([`measurements.md`](measurements.md), "Which allocator a figure was taken
under").

**Why P29 cares.** The usual portable Linux binary is static musl, whose own
malloc is what the figures measured as slower. A glibc artifact needs a symbol
floor (built against an old glibc, or targeting one explicitly), and that floor
decides which distributions run it. mimalloc is the shipped binary's
global allocator, with no `override`
([`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md)), so a static musl
build keeps the Rust heap off musl's malloc and needs no floor. On musl it is
unmeasured, and the C dependencies (`liblzma`, `aws-lc`) would still allocate
through musl's malloc.

**Origin.** Filed 2026-10-01 by the session sketching P29.

---

## Memory discovery reads Linux files only, and finds nothing elsewhere

**Fact.** A flagless run sizes itself from `/proc/meminfo`'s `MemAvailable`
and the cgroup limit (`pgdump_query/src/io.rs`, `available_memory_in`,
`Parallelism::discover_holding_in`; `datafusion-pgdump/src/budget.rs`,
`ScanBudget::discover_in`). With neither present it falls back to
`DEFAULT_MEMORY_BUDGET`, or to `AllowanceOrigin::NoneFound` in the provider. No
other OS has an answer, though
[`runtime-invariants.md`](runtime-invariants.md)'s preamble anticipates one.
Elsewhere the code is `std::os::unix`, with Linux-only arms in
`namespace-init/src/lib.rs` (`ending_signals`) and glibc-only ones in
`pgdt/src/introspect.rs`. Whether the workspace compiles for an Apple target
has never been tried.

**Why P29 cares.** A macOS binary would run every flagless scan at the fixed
default regardless of the machine, which
[`roadmap.md`](roadmap.md), "Two tunables fit pgdt to hardware: memory and
parallelism", says a person should not have to correct by hand. The grilling
decides whether macOS ships degraded with that stated, waits on a discovery
path and its `RT<n>` entries, or does not ship yet. Before any of that, a
compile against both Apple targets is the cheapest first slice.

**Origin.** Filed 2026-10-01 by the session sketching P29.

---

## The binaries pull in C builds, one of them the hard one to cross-compile

**Fact.** `pgdt` enables `pgdump_query/http`, and `datafusion-cli-pgdump`,
which it links for `sql`, enables `datafusion-pgdump/http`. That puts
`aws-lc-sys` (a cmake-driven C build, `pgdump_query/Cargo.toml`) into `pgdt`.
`xz-seek`'s default `liblzma` feature is a C build, and `pgdt` links
`mimalloc` as its global allocator. `mise.toml` already pins `cmake` for the
first.

**Why P29 cares.** The cross toolchain must build these for four targets from
one Linux host. `aws-lc-sys` against an Apple target is the build most likely
to need more than a linker swap. Whether the remote source ships in the
release binaries is a feature decision, and it is the one that sets this cost.

**Origin.** Filed 2026-10-01 by the session sketching P29.

---

## `--version` is a contract the measurement harness parses

**Fact.** `pgdt --version` prints `alloc::VERSION`,
`<CARGO_PKG_VERSION> (allocator: <name>)`, plus `(instrument:
counting-allocator)` on an introspection build (`pgdt/src/alloc.rs`).
`scripts/measure.py`'s `binary_allocator` reads the allocator back and refuses
an instrumented binary, and the tests in `alloc.rs` and `test_measure.py`
assert both markers. `datafusion-cli-pgdump` uses clap's bare `version`
(`src/lib.rs`, `Args`), naming its own crate version and not DataFusion's.

**Why P29 cares.** Distinguishing a release from a development `--release`
build adds build identity to that string: a tag, a `git describe`, or a
release-only marker. Whatever is added must leave both markers parseable, and
a build script reading git state re-runs on every commit, which a stamp-keyed
harness should be checked against.

**Origin.** Filed 2026-10-01 by the session sketching P29.

---

## The version is already one field, but the path dependencies restate it

**Fact.** `[workspace.package] version = "0.1.0"`, and all five members take
`version.workspace = true`. Six intra-workspace path dependencies also carry
`version = "0.1.0"` (in `pgdt`, `datafusion-pgdump` and
`datafusion-cli-pgdump`'s manifests), a requirement Cargo checks against the
path crate. Pre-1.0, `^0.1.0` admits `0.1.x` and refuses `0.2.0`. The vendored
`vendor/xz-seek` is excluded from the workspace and declares its own `0.1.0`.
The repository has no tags.

**Why P29 cares.** Lockstep means one edit only if those six requirements go,
which nothing needs while no crate is published, or move with it. Otherwise
the first minor bump breaks the build. The `/release` skill's bump is where
this is decided.

**Origin.** Filed 2026-10-01 by the session sketching P29.

---

## P30 decided the one artifact and its allocator

**Fact.** A release ships one binary, `pgdt`, with the DataFusion CLI as
`pgdt sql`. The composition is unconditional, `datafusion-cli-pgdump` becomes
a library, and the binary links mimalloc, with no `override`, so C
dependencies keep libc's malloc ([`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md)).
`pgdt --version` gains a `(datafusion: <version>)` marker after the existing
ones. The composed binary is what every figure times. Its size is mostly
DataFusion's `.text`, and the release profile sets no `strip`: re-check with
`ls -l target/release/pgdt` and `size -A`.

**Why P29 cares.** The artifact list, the build's feature set and its
allocator are settled. Left to P29: whether the release profile strips, and
whatever libc question mimalloc leaves (below).

**Origin.** Filed 2026-10-05 by P30's grilling, replacing the 2026-10-01
entry that deferred this to P30.

---

## No licence is declared for most of what a release would distribute

**Fact.** The repository root has no `LICENSE` file. Of the five members only
`datafusion-cli-pgdump` declares a licence (`Apache-2.0`, its `lib.rs` being
a copy of `datafusion-cli`'s `main.rs`), and `vendor/xz-seek` declares
`MIT OR Apache-2.0`. The GitHub repository is public.

**Why P29 cares.** A published binary distributes the project's code under
whatever licence it carries, which is currently none, and statically links
dependencies whose licences mostly require their notices to travel with it.
Choosing the licence is the maintainer's call. Generating the notices
(`cargo-about` or similar) is a build step the release recipe owns.

**Origin.** Filed 2026-10-01 by the session sketching P29.

---

## The register times a host build, run in the host's distribution's image

**Fact.** Every figure times `cargo build --release -p pgdt` built on the
measurement host and run in an image of the host's distribution
(`archlinux:base`, glibc 2.44), the composed `pgdt` linking `libm` symbol
versions an older glibc lacks (`runs/30.5-register-image/readings.txt`). A
toolchain container of another distribution, or a link against an explicit
glibc floor, was refused for the register because each compiles the C
dependencies, mimalloc included, with a second C compiler
([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md), "The
composed `pgdt` does not start in the register's image").

**Why P29 cares.** The shipped binary is the timed one
([`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md), "The shipped binary
is the timed binary"), and the release is built in CI against a libc floor P29
has yet to pick. Whatever floor and C toolchain it picks, the register's build
follows, so that choice moves the apparatus — the image, the C compiler and
the glibc the stamp names — and wants a sitting on the release build.

**Origin.** Filed 2026-10-05 by `/dwal` on 30.5's register-image entry.
Contingent on the release build differing from the host's.
