# P35.8 notes — the `release-profile` figure and its first sitting

What 35.9 inherits from the figure the spec's "Why now" asks for and from
its first sitting, the one-crate tree's, taken at `881c76f1` and published as
[`measurements.md`](measurements.md), "What link-time optimization buys the
shipped build".

## What landed

- **Three profiles in the root `Cargo.toml`**, `release-default`,
  `release-thin` and `release-fat`, each inheriting `release` and pinning
  both `lto` and `codegen-units`, so no leg moves when 35.9 gives `release`
  a profile of its own. `release` itself was left to 35.9, which made it
  `release-fat` restated (D105).
- **`pgdt --version` names the profile**, `(profile: <name>)` after the
  allocator's markers, from `crates/pgdt/build.rs` through `src/profile.rs`. Every
  build carries it, a release's included.
- **`release.py build --profile <name>`**: `Build.profile` replaces
  `--release` with `--profile` and lands the binary in the profile's own
  directory, so a leg is the shipped recipe with one flag changed.
- **`measure.py`'s `PROFILE_LEGS`, `binary_profile`,
  `ensure_profile_binary` and `run_release_profile`**: each leg built in the
  release image into its own target dir, refused unless `--version` names
  its profile and the shipped allocator, and timed over `_ALLOCATOR_SHAPES`
  with a co-measured `dd` floor; each cell's ratio is against
  `release-default`. Its tests are `test_measure.ReleaseProfile` and
  `test_release`'s profile-leg cases.

## What the first sitting licenses

The section's prose is the account; this is what follows from it for the
phase.

- **On this tree the default leg's gap to `release-fat` is resolved on
  `typed` only.** On `strings` and `parse` it lies inside the cells' own
  spreads, so the one-crate binary is not shown to leave anything on the
  table there. That is the "before" 35.9's criterion reads.
- **What "within the figure's noise" means is per shape.** The query shapes
  resolve a gap of a few percent; `parse` resolves nothing a codegen change
  is likely to move, being mostly kernel time. A split cost confined to
  `parse` is invisible to this figure by construction, and the spec's
  criterion can be met there vacuously.
- **It does not license a ranking of thin against fat.** Fat reads slower
  than thin on `strings` in user time, unattributed between fat LTO's code
  and one codegen unit's layout; the sitting took no `instructions:u`, which
  is what would tell them apart. A profile decision that leans on that cell
  needs that count first.
- **It does not license comparing the default leg with the shipped binary or
  with any other figure's absolutes.** The default leg is a separate build of
  Cargo's default, and a warm absolute across sittings resolves no better
  than the register's floor ([`measurements.md`](measurements.md), "A move
  smaller than the apparatus resolves is not a finding").
- **The apparatus line's I/O stall disqualifies nothing.** It is a
  per-container term the gate does not arm, it peaks on a run that reads its
  cell's median, and the one cell it could bias still orders the same
  without that run; what the term is made of is unattributed.

## What 35.9 inherits

- **The two sittings compare by their gaps, never their absolutes**: each is
  read against itself (`run_release_profile`'s rustdoc), and the spec's
  criterion is the default leg's gap to `release-fat` before and after.
- **The shipped binary is no column.** Each leg is built apart from it, so
  no leg is another figure's reference reading, and a decision that moves
  `release` leaves every leg where it is.
- **35.9's sitting replaces this table**, the figure now being in
  `measure.FIGURES` (so a sweep re-takes it too). The one-crate gaps then
  live only in git at the commit that folds this sitting and in
  `runs/measure-20261010T034321/raw.json`, so 35.9's fold carries them into
  the section's prose, naming `881c76f1`, as "Which allocator a figure was
  taken under" carries its earlier sittings.
- **A sitting takes about half an hour, most of it `release-fat`'s build**
  (`runs/release-profile-35.8-20261010-0341/log`), so it is handed off
  detached (`gosub`'s `handoff.md`).
- **The carve moves `depends`' library paths, not the legs'**: `Cargo.toml`
  and `crates/pgdt/build.rs` stay where they are, and 35.5 repoints the rest with
  every other figure's.

## Negative results

- **Cargo names no custom profile to a build script**: `PROFILE` reads
  `release` for every profile inheriting it, and `OPT_LEVEL` and `DEBUG`
  cannot tell the three legs apart, so `build.rs` reads the profile
  directory out of `OUT_DIR` (its rustdoc says how, and what `dev` reads
  as). A layout Cargo moves reads `unknown`, which the harness refuses and
  `profile.rs`'s test fails on.
