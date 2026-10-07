# P29.3 notes — licence, manifests and identity

What the later slices inherit from the licence, the manifests and the version
marker. The mechanism is the root `Cargo.toml`'s `[workspace.package]`,
`pgdt/src/main.rs` (`version`, `RELEASED`), `release/about.toml` and
`scripts/release.py`'s notices section (`step_notices` and what it calls), whose
comments say how each piece works.

## What 29.4 inherits

- **A release build sets `PGDT_RELEASE_VERSION` to the workspace version**,
  read by `option_env!`; the `--version` line then drops `(unreleased)`. A
  value naming any other version, an empty one included, fails the build, so
  the workflow reads the version from the manifest (`cargo metadata`) rather
  than from the tag or an input. `run_in_image` passes no host variable into
  the container, so a CI step running `release.py build --inside` sets it in
  the job's own environment; the register's builds never see it.
- **`release.py notices [--target T]` writes
  `$CARGO_TARGET_DIR/<target>/release/THIRD-PARTY-NOTICES`**, beside the
  binary `build` writes, so the archive step finds both in one directory. It
  runs `cargo fetch --locked` first, which downloads every target's packages
  (Windows crates included): `cargo-about` reads the graph through
  `cargo metadata`, and nothing else in the step touches the network.
- **The archive's `LICENSE` is the root `LICENSE`**, Apache-2.0 with the
  maintainer's copyright line, the same text as `vendor/xz-seek/LICENSE-APACHE`.

## What 29.5 inherits

- **The notices are checked only where they are made.** `mise run check` does
  not run `cargo-about`, so a dependency upgrade that brings an unlisted
  licence, or moves a file `release/about.toml` clarifies, fails `/release`'s
  rehearsal rather than the round that made it. The failure names the crate and
  the file to re-read.

## Readings

Not figures. Both targets' notices list the same 343 crates: the linked
closure less the five workspace members, `xz-seek` counted as a third party
since it is not one. 49 crates carry a `NOTICE`, four distinct texts. The
notices made in the image and on the host are byte-identical for x86-64. A
copy of `release/about.toml` without `Zlib` failed `cargo-about` on its three
`Zlib` crates; one with a clarification's checksum altered passed
`cargo-about` and was refused by `check_clarified`.

## Negative results

- **mise's registry has no `cargo-about`**; it is pinned through the `github:`
  backend, which installs the upstream release's musl binary.
- **`cargo-about` 0.9.2 cannot say what is linked**: it reads build
  dependencies and proc macros alike, and its config ignores only the first.
  The recipe cuts its output to `cargo tree -e normal,no-proc-macro`, whose
  set differs from `cargo-about`'s by exactly 30 crates: the proc macros and
  what only they reach.
- **It reproduces no `NOTICE` file**; the recipe reads them from each linked
  crate's root.
- **Its harvest of `ring` 0.17.14 is wrong**: it reproduces ISC headers cut
  out of sixteen source files, code included, and its built-in `ring`
  workaround checksums an older `LICENSE` layout. `release/about.toml`
  clarifies `ring`, `aws-lc-sys` and `liblzma-sys` from their own files.
- **A clarification whose checksum fails is a warning under `--fail`**, and
  the crate falls back to its declared licence, which would drop liblzma's
  0BSD text silently; `check_clarified` is the guard.
- **`private = { ignore = true }` skips `xz-seek` too**, being keyed on
  `publish = false` rather than membership, so the configuration leaves it
  off and the recipe drops the members itself.
- **Pinning `cargo-about` in `mise.toml` moves the release image's tag**, so
  the register's next sitting builds a new image; the tool is on no timed path.
