# P29.1 notes — the image and its floor

What the next slices inherit from the release image. The mechanism is
`release/Dockerfile`, `rust-toolchain.toml` and `scripts/release.py`, whose
docstrings say how each step runs; the readings below are
`runs/29.1-release-image/` (`readings.txt`, `build.log`, `suite.log`,
`suite-flake.log`).

## What 29.2 inherits

- **The image's `pgdt` is at
  `target/release-image/target/x86_64-unknown-linux-gnu/release/pgdt`**, the
  cargo target dir `release.py` mounts, built by `release.py build --target
  x86_64-unknown-linux-gnu`. `measure.py` names `target/release/pgdt` by path
  and runs in `Config.image`, so the move has a path to change as well as an
  image; the Dockerfile's digest is `release.pinned_base`.
- **It starts there and the host build does not**: the image's x86-64 binary
  prints `--version` and parses a fixture in plain `debian:trixie` at the same
  digest, and the host's fails to load on `GLIBC_2.43` and `GLIBC_2.44`
  (`readings.txt`). Both targets need `GLIBC_2.39` at most against a 2.41
  floor.
- **`peak-rss`'s musl target is in the toolchain file**, so the image and an
  x86-64 host carry it without a `rustup target add`.

## What 29.4 inherits

- **`--inside` refuses an image built from other copies of
  `rust-toolchain.toml` or `mise.toml` than the tree's.**
- **The suite's container needs `--security-opt seccomp=unconfined`**:
  `pgdt/tests/namespace_init.rs` unshares a user namespace, which the default
  profile refuses (all three failed without it); `run_in_image` passes it.

## Negative results

- **`debian:trixie` installs no `unshare`**: `util-linux` is not in the base
  image, and the suite needs it.
- **trixie packages none of `mise`, `uv` or `cargo-nextest`**, and its
  `rustup` is 1.27.1, which installs a toolchain file's Rust on its first
  proxy call.
- **trixie's git (2.47) cannot read a checkout carrying
  `extensions.relativeworktrees`**, which `worktree.useRelativePaths` writes
  and this machine's global git config sets; trixie-backports carries no newer
  git. `release.git_view` mounts a copy
  of `.git` without it over the original for the suite, which asks git about
  this repository's history.
- **No release build runs CMake**: `aws-lc-sys` builds without it on both
  targets (no `CMakeCache.txt` under either), and nothing installs it.
- **Three `test_measure` classes reached the host** — a built `peak-rss`, and
  `sudo nerdctl` to ask the register image's glibc — and failed on a fresh
  target dir; they now patch both. Nothing else in the suite read host state.
