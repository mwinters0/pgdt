# P29.6 notes — v0.1.0

What the wrap inherits. `v0.1.0` is published from `44b5732d`, an annotated tag,
and the release is immutable. It was verified from outside on its three assets
as published, downloaded fresh: `docs/manual/installing.md`'s procedure, which
anyone can repeat against the release.

| Check | x86-64 | arm64 |
|---|---|---|
| `sha256sum -c SHA256SUMS` | OK | OK |
| `gh attestation verify <archive> -R mwinters0/pgdt --source-digest 44b5732d…` | pass | pass |
| `gh release verify-asset v0.1.0 <archive>` | pass | pass |
| `release_ci.step_smoke` (`--version`, `parse`, `query`) in a fresh `debian:trixie` at the release's pinned digest | pass | pass, under qemu-user |

- **One build attestation covers both archives.** It names
  `release-build.yml@refs/heads/main`, run by a manual dispatch from the
  commit the tag points at.
- **The image digest is a multi-arch index**, so the arm64 leg ran the same
  pin with `--platform linux/arm64`: `uname -m` gave `aarch64` and glibc 2.41.
- **arm64 was run under emulation, by the maintainer's call**: where emulation
  exists, nobody is required to own the hardware. Real arm64 ran the suite in
  CI's `arm64-suite` job, on `ubuntu-24.04-arm`.

## What the wrap inherits

- **Where the emulation call lives is open.** It binds any later release to
  another architecture, so it is a `D<k>` or a "Standing rules" line, which
  is for the maintainer to decide. The spec's "Slicing" says "on both
  architectures" and does not say whether emulation counts.
- **`/release`'s step 4 says this host runs no arm64 code.** That is no longer
  true here: `qemu-user-static` and `qemu-user-static-binfmt` are installed,
  and the `qemu-aarch64` rule carries `F`. `release_ci.py smoke` has no
  `--platform`, so for 29.6 the arm64 leg called `step_smoke` with one added
  by hand.

## Negative results

- **The first `release-build` run failed two of `test_measure`'s tests on both
  architectures.** The cause was `actions/checkout`'s single-ref fetch: the
  suite resolves the commits `measurements.md` declares. `44b5732d` gives the
  suite jobs the whole history, and the release was cut from that commit.
