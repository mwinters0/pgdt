# P35.8 notes — the `release-profile` figure's instrument

What the readings and 35.9 inherit from building the figure the spec's "Why
now" asks for. The instrument is landed and waits in `measure.UNTAKEN`; the
first sitting is not taken, a figure being taken from the commit that lands
its apparatus ([`measurements.md`](measurements.md), "A figure may be
published outside the sweep").

## What landed

- **Three profiles in the root `Cargo.toml`**, `release-default`,
  `release-thin` and `release-fat`, each inheriting `release` and pinning
  both `lto` and `codegen-units`, so no leg moves when 35.9 gives `release`
  a profile of its own. `release` itself is unchanged and states nothing.
- **`pgdt --version` names the profile**, `(profile: <name>)` after the
  allocator's markers, from `pgdt/build.rs` through `src/profile.rs`. Every
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

## What the readings round does

- **`cd scripts && uv run measure.py --figure release-profile`**, from the
  commit that lands this, on a quiet machine. It stands in no sharing edge,
  so it publishes alone with that commit in its marker. Folding it in moves
  the entry from `UNTAKEN` to `FIGURES` and gives it a section in
  `measurements.md`, beside "Which allocator a figure was taken under".
- **No leg has been built yet.** How long the three image builds take is
  unmeasured, `release-fat`'s single codegen unit over DataFusion likely the
  longest of any build the register makes; a round expecting the sitting to
  pass thirty minutes hands it off detached (`gosub`'s `handoff.md`).
- **That sitting is the one-crate tree's**, so it must be taken before
  35.4's carve lands; 35.9's sitting is the split tree's.

## What 35.9 inherits

- **The two sittings compare by their gaps, never their absolutes**: each is
  read against itself (`run_release_profile`'s rustdoc), and the spec's
  criterion is the default leg's gap to `release-fat` before and after.
- **The shipped binary is no column.** `release-default` is `release`
  restated, so it is not another figure's reference reading, and a decision
  that moves `release` leaves every leg where it is.
- **The carve moves `depends`' library paths, not the legs'**: `Cargo.toml`
  and `pgdt/build.rs` stay where they are, and 35.5 repoints the rest with
  every other figure's.

## Negative results

- **Cargo names no custom profile to a build script**: `PROFILE` reads
  `release` for every profile inheriting it, and `OPT_LEVEL` and `DEBUG`
  cannot tell the three legs apart, so `build.rs` reads the profile
  directory out of `OUT_DIR` (its rustdoc says how, and what `dev` reads
  as). A layout Cargo moves reads `unknown`, which the harness refuses and
  `profile.rs`'s test fails on.
