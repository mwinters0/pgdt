# Installing `pgdt`

`pgdt` is one binary — the CLI, and `pgdt sql` inside it. Each
[release](https://github.com/mwinters0/pgdt/releases) carries it as a Linux
archive for two targets:

| Machine | Archive |
|---|---|
| x86-64 | `pgdt-v<version>-x86_64-unknown-linux-gnu.tar.xz` |
| arm64 | `pgdt-v<version>-aarch64-unknown-linux-gnu.tar.xz` |

**It needs glibc 2.41 or newer** — Debian 13 ("trixie") or later, or any
distribution whose `ldd --version` says 2.41 or more. That includes the
`postgres:*` images on Debian 13, so a release runs where PostgreSQL does. macOS
and Windows have no release; build from source there, with
`cargo build --release -p pgdt` (see [`CONTRIBUTING.md`](../../CONTRIBUTING.md)).

Beside the archives is a `SHA256SUMS`.

## Install

```sh
v=0.1.0 target=x86_64-unknown-linux-gnu
base=https://github.com/mwinters0/pgdt/releases/download/v$v
curl -fLO $base/pgdt-v$v-$target.tar.xz -O $base/SHA256SUMS

sha256sum --ignore-missing -c SHA256SUMS     # "…: OK"; see "Verify" for more
tar -xf pgdt-v$v-$target.tar.xz
install pgdt-v$v-$target/pgdt ~/.local/bin/
pgdt --version
```

The archive is one directory, `pgdt-v<version>-<target>/`, holding `pgdt`,
`LICENSE`, `THIRD-PARTY-NOTICES` — the licence of every crate the binary links,
and the elections a dual licence leaves us — and `README.md`. The binary keeps
its symbols, so a panic's backtrace names its functions.

`pgdt --version` prints `pgdt <version> (…)`. **A release's says nothing more
about itself; a build from source adds `(unreleased)` after the version.**

## Verify

**The checksum proves the download arrived whole; it does not prove who made
it**, since `SHA256SUMS` came from the same place. GitHub's attestations prove
more, and none needs a key of ours: each is signed through Sigstore and checked
with the [`gh`](https://cli.github.com/) CLI. Which ones an archive carries
depends on its target.

**The release's attestation**, on both archives, says the archive is an asset
of that release, which is **immutable**: once published, its tag and its files
cannot change, so a mistake in a release is fixed by a new version and never by
replacing one.

```sh
gh release verify-asset v$v pgdt-v$v-$target.tar.xz -R mwinters0/pgdt
```

**The build's attestation**, on the x86-64 archive alone, says it was built by
this repository's `release-build` workflow from a named commit. Give it the
commit the tag names:

```sh
sha=$(git ls-remote https://github.com/mwinters0/pgdt "refs/tags/v$v^{}" | cut -f1)
gh attestation verify pgdt-v$v-$target.tar.xz -R mwinters0/pgdt --source-digest $sha
```

**The arm64 archive is built outside GitHub's runners**, by the maintainer, so
it carries no build attestation, and the release's is its check. That proves
the archive is an asset of this immutable release, uploaded by someone with
write access to the repository; it does not prove which commit or which
machine built it.

Every check your archive carries must pass. A failure of any is a reason not to
run the binary, and to open an issue.
