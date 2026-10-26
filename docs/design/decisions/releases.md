# Releases (`release/`, `scripts/release.py`, `scripts/release_ci.py`, `scripts/debian_window.py`)

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. What a release is and how it is built, attested and
tested: the pinned Debian image and the glibc floor, the fat-LTO profile, arm64's archive, the release suite, and
CMake's place. `release/`, `scripts/release.py`, `scripts/release_ci.py`.

### D104 A release is `pgdt` for Linux, linked against one digest-pinned Debian-stable image that also builds what the register times
Two targets, Linux on x86-64 and arm64; macOS waits on P34, Windows on an I/O layer that is `std::os::unix`. glibc, not
static musl: the shipped binary is the timed one (D13). The floor follows Debian stable: at least six months' grace for
a new stable, moved by twelve (`debian_window.py`). One digest-pinned `debian:trixie` image builds both targets and
every binary the register times, so one pin names the build's glibc, the floor and every figure's runtime, as
`rust-toolchain.toml` names one compiler; a target directory it fills is cleared once what compiles there moves
(`key_target`). Rejected: `cargo zigbuild` (zig's clang for every C dependency); a build runner per architecture (arm64
rehearsed nowhere locally); cargo-dist (no glibc floor); the Rust pin in the image alone; keying it on the image's tag
(moved by tools compiling nothing).

### D105 The shipped build is fat LTO in one codegen unit, the leg retiring the fewest instructions on the typed query
`[profile.release]` restates `release-fat`, the leg retiring the fewest `instructions:u` on every query row, `pgdt sql`
first (`release-profile`). The count ranks profiles because wall clock cannot: which leg leads in time moves with the
tree, and a profile outlives every tree. Nothing else is ranked by count, and the default leg is held to fat by the
roadmap's downstream-build rule. Fat is DataFusion's own profile (D13). Rejected: Cargo's default (more retired on every
row); thin LTO (more than fat on every query row; its quicker build our cost, not a user's); user time (as
layout-exposed). Reopens: thin's or the default's count at or below fat's on `pgdt sql`, or ahead in wall clock at
successive trees; DataFusion moving its profile. Code: `test_the_shipped_profile_is_the_fat_leg_restated`.

### D109 arm64's archive is built on the maintainer's machine and attested by its release alone; x86-64's is CI's, with build provenance
CI's runner cannot hold arm64's fat link (D105), so the arm64 archive is built in the pinned image on the maintainer's
machine and attached to CI's draft. Its one attestation is the immutable release's, which `verify-asset` and mise's
`github:` backend accept alone (RL1, RL2). `attach` reports CI's x86-64 archive differing from the rehearsal's, never
refusing: the two differ by the build clock gcc stamps into mimalloc (`__DATE__`), and nothing relies on them agreeing.
Rejected: swap on the runner (a fat link paging); `ubuntu-24.04-arm`, or a paid or self-hosted runner; an arm64-only
profile (reopens D105); a keyless signature as the maintainer (mise's backend fails it); `actions/attest` over the local
build (provenance verifying and false). Reopens: a hosted runner holding the link; RL1 or RL2 failing. Code:
`step_attach`.

### D110 The release suite is the round's, less the never-shipped measurement apparatus
`release.py suite` leaves out each member whose manifest sets `[package.metadata.pgdt] apparatus`; `mise run check`
still tests it. An attested build tests what ships: performance and its apparatus are verified on the maintainer's
machine before a cut, and the apparatus needs facilities a runner need not have (GitHub's x86-64 runner opened no
counter). Rejected: lowering the runner's `perf_event_paranoid` (no counter behind it); a counting test that skips where
the counter does not open (a broken counter passing here too); the apparatus listed in `release.py` (a new instrument
tested on the runner until someone edits it). Reopens: an instrument shipping, or a shipped member needing a facility a
runner lacks. Evidence: `test_the_apparatus_the_suite_leaves_out_is_a_dependency_of_no_member`.

### D111 CMake is a `mise.toml` pin for the apparatus, and a build is refused where a crate `pgdt` depends on ran it
CMake is a `mise.toml` pin, so the image has it, for the apparatus: zlib-ng's `libz-ng-sys` builds under nothing else.
The shipped build must not build `aws-lc-sys` under its CMake builder, whose compilation options are not its `cc`
builder's; while no shipped crate needs CMake, `release.py build` refuses any crate of `pgdt`'s normal-dependency
closure that left a `CMakeCache.txt`. Rejected: CMake absent (zlib-ng unbuildable where every figure's binary is built);
`AWS_LC_SYS_CMAKE_BUILDER=0` in the image (one crate guarded by name, another's fallback onto CMake unseen); Debian's
`cmake` (a version no checkout shares). Reopens: a shipped dependency built by CMake, the check then narrowed to
`aws-lc-sys`. Code: `cmake_caches`. Evidence: `test_release.py`'s `CMake`.
