# P35.9 notes — the second sitting and the shipped profile

What the next slices inherit from the `release-profile` figure's sitting on
the split tree, taken at `6b759eaf` and published as
[`measurements.md`](measurements.md), "What link-time optimization buys the
shipped build", and from the profile it decided,
[`decisions.md`](decisions.md), "D105".

## What landed

- **The sitting folded**, the one-crate sitting's gaps carried into the
  section's prose under `881c76f1`, the only place they now stand besides git
  and `runs/measure-20261010T034321/raw.json`.
- **`[profile.release]` states fat LTO and one codegen unit**, `release-fat`
  restated, held to that leg by `test_measure`'s
  `test_the_shipped_profile_is_the_fat_leg_restated`. The legs pin both knobs
  (35.8), so none moved.

## What the two sittings license

- **The spec's criterion is met** ([`roadmap-P35-seams.md`](roadmap-P35-seams.md),
  "Why now"): the default leg's gap to `release-fat` is no wider after the
  carve on any shape — on `typed` resolved and unchanged in user time, on
  `strings` inside the cells' spreads, on `parse` vacuously. This figure owes
  no `#[inline]` at a crate boundary.
- **The default leg stays inside the downstream rule's bound** of the shipped
  one on every shape the figure times, in both sittings
  ([`roadmap.md`](roadmap.md), "A downstream build is within 3% of ours").
- **Thin and fat are not ranked.** Level on every shape on the split tree;
  the first sitting's thin-ahead-of-fat on `strings` did not survive a
  rebuild, and stays unattributed with no `instructions:u` taken. D105 breaks
  the tie by precedent, not by a reading.
- **Nothing here is about `pgdt sql`.** No leg times a DataFusion query, the
  first-ranked workload; the typed `pgdt query` shape is the nearest, sharing
  its per-field decode and not its engine.

## What the next slices inherit

- **Every build inheriting `release` is now fat LTO in one codegen unit**:
  the shipped `pgdt` the register times, the `allocator` legs, the
  `introspect` instrument, `profiling`, and a `cargo bench`, the `bench`
  profile inheriting `release`. Each rebuilds after a workspace edit in many
  times what the default leg does (`runs/release-profile-35.9-20261010-1755/log`,
  the `Finished` lines), which a sitting that rebuilds them pays first.
- **The profile change is an apparatus change `--stale` cannot see.** The
  stamp's figures were taken under Cargo's default profile, but no figure
  except `release-profile` declares the root `Cargo.toml`, and the stamp names
  the allocator and glibc but no profile. All of them are red at this commit
  for the carve's paths, so the next sweep, the first under D105, re-takes
  every one; a later move of `release` after a sweep would go unflagged.
- **35.5 repoints figure paths as planned**; the legs, `Cargo.toml` and
  `crates/pgdt/build.rs` did not move.

## Negative results

- **A narrower wall-clock gap on `typed` is not the carve helping.** It
  narrowed between the sittings while the user-time gap, the one codegen
  moves, held; the difference is in system time and in which runs set each
  median.
