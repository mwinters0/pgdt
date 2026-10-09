# P29 notes — versioned binary releases

What the phase leaves the next release, the maintainer and the keystone. The
mechanism is `release/Dockerfile`, `scripts/release.py`, `scripts/release_ci.py`,
`scripts/debian_window.py`, `.github/workflows/release-build.yml` and
`release-publish.yml`, and `.claude/skills/release/SKILL.md`, whose comments say
how each step runs and carry most of what the slices found; the decisions are
[`roadmap-P29-releases.md`](roadmap-P29-releases.md)'s. What follows is only
what is in none of them. The slices' readings are under `runs/29.*/`.

## v0.1.0

Published from `44b5732d` under an annotated tag, immutable, and verified from
outside on its three assets as published, by
[`../manual/installing.md`](../manual/installing.md)'s procedure: `sha256sum -c`,
`gh attestation verify … --source-digest`, `gh release verify-asset`, and
`release_ci.step_smoke` in a fresh `debian:trixie` at the release's pin, each
passing on both archives. One build attestation covers both, naming
`release-build.yml@refs/heads/main`. The pinned digest is a multi-arch index,
so the arm64 leg ran the same pin under `--platform linux/arm64`.

## Open, for the maintainer

- **The arm64 archive was verified under qemu-user, by the maintainer's call**
  — where emulation exists, nobody is required to own the hardware; real arm64
  ran the suite in CI's `arm64-suite` job. The call binds any later release to
  another architecture, so it is a `D<k>` or a "Standing rules" line, which is
  the maintainer's to decide; the spec's "Slicing" says "on both
  architectures" and not whether emulation counts.
- **`/release`'s step 4 and the spec's "`/release`" say the maintainer's
  machine runs no arm64 code**, which stopped being true when qemu-user and its
  `binfmt` rule were installed for 29.6. `release_ci.py smoke` takes no
  `--platform`, so 29.6's arm64 leg called `step_smoke` with one added by hand.
  Whether the rehearsal smokes arm64 is a change to the skill.

## What the next release inherits

- **`arm64-suite` has built and passed once, and nothing read its time, memory
  or disk.** `release.py`'s container limit (`MEMORY`) is above an arm64
  runner's memory, so running out there loses the runner rather than killing
  `ld`. A cold cross build of the arm64 tests on this host ran out of memory at
  its links at 24 cargo jobs and completed at six (`runs/29.4-workflows/`); a
  runner that cannot hold the build wants a job cap or a lighter test profile,
  measured first.
- **A draft left behind blocks the same version's next dispatch** at
  `preflight`, and is deleted by hand; a run failing before `draft` leaves
  nothing.
- **`/release` cannot run unattended past step 2**: `git ls-remote origin` fails
  without the maintainer's ssh key, and the skill reads a failure as a stop.

## Negative results

For the keystone's harvest: each is a refusal no comment records.

- **qemu-user was not taken for the arm64 suite.** It would run cross-built
  tests on an x86-64 host, but under that host's `/proc`, cgroups and page
  size and without arm's memory ordering, which are what the runtime-discovery
  and allocator tests look at. The smoke run reads none of those.
- **The notices are not in `mise run check`, which stays offline**:
  `release.py notices` fetches every target's packages first. They are made at
  a `Cargo.lock` change (`upgrade-deps`) and at the rehearsal, every way they
  break being a dependency moving.
- **`cargo-about`'s `private = { ignore = true }` skips `xz-seek` too**, being
  keyed on `publish = false` rather than membership, so the configuration
  leaves it off and the recipe drops the workspace members itself.
- **mise's registry has no `cargo-about`**; it is pinned through the `github:`
  backend. Debian trixie packages none of `mise`, `uv` or `cargo-nextest`.
- **The workflow tests parse no YAML**: the standard library has no parser and
  the scripts' environment pins two dependencies for the bytes they decide, so
  the tests read the files by line and hold each `run:` to the scripts' own
  parsers.
