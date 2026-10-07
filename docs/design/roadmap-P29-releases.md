# P29 — Versioned binary releases

Publishing `pgdt` as versioned GitHub Releases, starting with `v0.1.0`. **The
maintainer owns the timing and the text; CI owns the bytes.** No crate is
published.

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. Grilled 2026-10-06.

## What ships

**One binary, `pgdt`**, composing the DataFusion CLI as `pgdt sql` and linking
mimalloc without `override` ([`decisions.md`](decisions.md), "D13"). Nothing
else is a release artifact: not `datafusion-cli-pgdump` as a binary, not
`peak-rss`, not a library crate.

**Two targets: Linux on x86-64 and on arm64.** macOS is not shipped by this
phase. A flagless run there finds no memory limit and no available memory —
discovery reads `/proc` and the cgroup tree only — and falls to the fixed
default, which [`roadmap.md`](roadmap.md), "A default runs as fast as the
allocation permits" forbids being the shipped behaviour; what "available" means
on a host with compressed memory and no cgroups is a design question of its
own, and it is P34's. Windows is not a target: the I/O layer is
`std::os::unix`.

## The Linux libc

**glibc, linked against a floor that follows Debian stable** — today trixie,
glibc 2.41. **A new Debian stable gets a grace period of at least six
months**: the floor stays on the previous codename for six months or more after
Debian releases its successor, so a user on the new oldstable keeps being
served through that window, and never moves between Debian releases. **Twelve
months after Debian's release the floor must have moved**: `/release` refuses
while the pin still names the predecessor past that, and between six and twelve
months the day is the maintainer's. The move is its own change and a register
sitting (below, "The build image").

Why glibc rather than static musl: every figure is a glibc build, and the
shipped binary is the timed one (D13), so musl would be a libc swap no sitting
has priced with mimalloc in front of it — musl's own `malloc` was measured
materially slower, and liblzma and aws-lc would still allocate through it.
Why a floor at all: a build on this host links `libm` symbols at `GLIBC_2.43`
and `GLIBC_2.44`, `__isoc23_*` at 2.38 and `pidfd_spawnp` at 2.39, so it loads
almost nowhere. Why Debian stable: it is the maintainer's chosen line, and the
`postgres:*` images are built on it (`postgres:16` is Debian 13.6, glibc 2.41),
so a release loads in the image a user runs PostgreSQL from.

**The register follows the floor.** The shipped binary is what every figure
times (D13), so the build the apparatus times is the release recipe's, and
moving to it is a sitting.

## Building, tagging and publishing

**Build first, tag second.** A manually triggered workflow builds the binaries
from a pushed *commit* into a **draft** Release. The maintainer then pushes an
**annotated tag** on that commit whose message is the release text; a second
workflow, triggered by the tag push, checks the draft was built from the tag's
commit, takes the tag message as the Release body, and publishes it. A tag with
no matching draft fails that workflow loudly. **A draft is matched by name**:
the build creates it as `v<manifest version>`, and the publish workflow finds it
by the pushed tag's name and then checks its commit, so a tag naming another
version finds nothing and the tag, the draft and the archives carry one string
([`../status/history/2026-10-07.md`](../status/history/2026-10-07.md), "29.3's
unattended calls, reviewed").

Why: a pushed tag is a `releases.atom` entry at once, carrying the annotated
tag's message, under the entry id a Release on that tag later takes (observed
on `torvalds/linux`, which has tags and no Releases; GitHub does not document
it, so re-check against a tag-only repository's feed before relying on it).
The annotation is therefore the one place the text can be when a feed reader
first takes the entry, and building first means nothing is announced before its
binaries exist. Rejected: tag-triggered builds (the feed announces a release
whose binaries are still building, and a failed build leaves an announced tag
nobody can retract); the text in a repository file or a workflow input (the
tag's entry is read empty).

`/release` (below) stops before anything leaves the machine; the push, the
workflow dispatch and the tag are the maintainer's.

## Licence and notices

**Every workspace member is `Apache-2.0`**, the licence DataFusion and Arrow
carry and `datafusion-cli-pgdump` already declares, its `lib.rs` being a copy
of `datafusion-cli`'s `main.rs`; the repository root carries the licence text.
`vendor/xz-seek` keeps its own `MIT OR Apache-2.0`.

**Each archive carries a generated `THIRD-PARTY-NOTICES`**, produced by
`cargo-about` (pinned in `mise.toml`) from the linked closure of the target it
ships, never committed. It reproduces every licence text and every Apache-2.0
`NOTICE` file, states the elections a dual licence leaves to us — zstd's C
sources taken under BSD-3-Clause, not GPLv2 — and says where the MPL-2.0
crate's source is. **The recipe refuses a licence outside an explicit
allow-list**, so a new dependency under an unlisted licence stops a release
rather than reaching one.

The closure as grilled (2026-10-06, `cargo tree -p pgdt -e normal,no-proc-macro`
joined to `cargo metadata`): no copyleft-only crate; one MPL-2.0 crate,
`option-ext`, through `datafusion-cli` → `dirs`, whose only duty in executable
form is the source pointer; 49 crates carrying a `NOTICE`; statically compiled
C from aws-lc (Apache-2.0 / ISC, its Jitter Entropy elected BSD-3-Clause by
Amazon), ring, liblzma (0BSD), zstd, mimalloc (MIT), BLAKE3 and `psm`. No
LGPL code is compiled, so nothing carries a relinking duty.

## The version and how a build names it

**`[workspace.package] version` is the one place the version lives**, every
member taking it, and `/release` is what moves it. **Pre-1.0 the minor moves
at every release, the patch only for a release whose changes are all fixes.**
The six intra-workspace path dependencies drop their `version =` requirements,
and every member is `publish = false`, so a bump is one edit and nothing can be
published by accident.

**A release says it is one; every other build says it is not.** The publishing
workflow sets one environment variable, read by `option_env!`: a release
prints `pgdt 0.2.0 (…)`, anything else `pgdt 0.2.0 (unreleased) (…)`, the
markers after it unchanged. Cargo tracks the variable and rebuilds on its
change. The harness reads only the `(allocator: …)` and `(instrument: …)`
markers (`scripts/measure.py`, `binary_allocator`), so neither moves. Rejected:
a `build.rs` reading `git describe`, which re-runs at every commit and names a
dirty tree after a commit it does not match. A version bump alone invalidates
no cache: caches are keyed on `CACHE_FORMAT_VERSION` alone.

## The compiler

**`rust-toolchain.toml` pins an exact Rust release for development, the
register and the release alike** (1.98.0 at grilling). Moving it is a
deliberate change made through `upgrade-deps`, with the walk of invariants and
figures any dependency upgrade owes: the shipped binary is the timed one only
if one compiler builds both. Rejected: a pin in the release image alone, under
which the register times host `stable` and the release ships another rustc.

## The build image

**One image builds every release artifact: `debian:trixie`, pinned by
digest**, carrying the `rust-toolchain.toml` Rust, Debian's gcc for the C
dependencies, and Debian's arm64 cross toolchain (`gcc-aarch64-linux-gnu`,
`libc6-dev-arm64-cross`) for the second target. The floor holds by
construction — the link is against the image's own glibc — so "follows Debian
stable" is one pin, moved inside the grace period above. The same image runs locally
under the container runtime and in CI through the same `release.py` steps
under the runner's own, so a rehearsal and the release run the same commands
([`../status/history/2026-10-07.md`](../status/history/2026-10-07.md), "29.4's unattended calls, reviewed").

Rejected: `cargo zigbuild --target …-gnu.2.41`, which compiles every C
dependency, mimalloc included, with zig's clang, keeps the floor as a suffix
synced to Debian by hand, and adds a zig pin (0.17 unsupported at grilling,
cargo-zigbuild #493) and a silent failure — a `CC` left in the environment
compiles C against the host's headers again; native runners per architecture,
which no local rehearsal reproduces for arm64; cargo-dist, which builds to no
glibc floor and takes no release text from an annotated tag.

**The register follows: it times the image's build, run in `debian:trixie` at
the same pin**, so one pin names the build's libc, the floor and the runtime
every figure is taken on — the oldest glibc a release promises. This replaces
the host build run in `archlinux:base`
([`../status/history/2026-10-06.md`](../status/history/2026-10-06.md), "The
register moves to the release image"). Development builds stay on the host. The
register's build is the release recipe without the publishing workflow's
variable, so its `--version` says `(unreleased)` and its bytes differ from a
release's in that string alone.

**What the build workflow proves before it drafts.** Both targets are
cross-built in the image. The x86-64 suite runs in the image; the arm64 suite
is cross-built as a nextest archive and run on GitHub's `ubuntu-24.04-arm`
runner in the arm64 variant of the same pinned image, so arm64 code runs its
tests somewhere; it is nextest alone, a doctest being compiled where it runs
([`../status/history/2026-10-07.md`](../status/history/2026-10-07.md), "29.4's unattended calls, reviewed"). Each archived `pgdt` then passes a smoke run on its own
architecture — `--version` carrying no `(unreleased)`, a `parse` and a `query`
of a committed fixture. Only then is the draft created. The suite runs
development-profile test binaries, so it tests the source per architecture; the
smoke run is what tests the bytes that ship.

**When the pin moves.** The digest within a codename (a Debian point release)
moves deliberately through `upgrade-deps`, as the Rust pin does, and never as a
side effect of a release: each move is an apparatus change. The codename moves
under the grace period above; `/release` reads Debian's release dates
(`distro-info-data`'s `debian.csv`, which records each codename's release day)
and holds the pin inside that window: refusing a codename released under six
months ago, and a predecessor kept past twelve.

**The digest pins the packages too**: apt reads the `snapshot.debian.org`
archive the base image records, so one digest installs one package set and a
move is the only way gcc or `libc6-dev` changes. Rejected: the live archive,
under which the compilers and the crt objects linked into every binary move at
each point release and security upload with the digest unchanged; and a built
image pushed to a registry and pulled by its own digest, a second artifact to
build and re-pin at every Rust or tool bump, as `M215` refused for the
register's image. Snapshot's availability is the cost taken; its speed was not
found to be one ([`../status/history/2026-10-06.md`](../status/history/2026-10-06.md),
"The release image's inputs, reviewed").

**The image carries what the release build, the suite and the register run**,
not only what a release artifact links: the suite's `git` and `python3`, and
the register's `make`, which the `jemalloc` allocator leg's build runs. Rejected:
an image derived from this one for that leg alone, from the same snapshot, a
second image and build path guarding against a build script in the shipped
closure that looks for `make`, of which there is none
([`../status/history/2026-10-06.md`](../status/history/2026-10-06.md), "The
register's image, reviewed").

## `/release`

A skill run on the maintainer's machine, in this order: refuse a dirty tree or
a failing `mise run check`; rehearse in the image — both targets' release
builds, the x86-64 suite, the notices under their allow-list, the archives;
ask the maintainer which kind of release this is and bump the version by the
rule above; commit; then **stop**, printing what is left to the maintainer:
`git push`, `gh workflow run release-build -f ref=<sha>`, and, once the draft
exists, `git tag -a v<x.y.z> <sha>` and `git push origin v<x.y.z>`. Everything
that leaves the machine is the maintainer's, the workflow dispatch included,
since a draft is visible to anyone with access to the repository. arm64 is
rehearsed as a build only: this host runs no arm64 code.

## The artifacts

**Per target, one `pgdt-v<version>-<target-triple>.tar.xz`** holding a
top-level directory with `pgdt`, `LICENSE`, `THIRD-PARTY-NOTICES` and
`README.md`. **Symbols are kept**: the release profile strips nothing beyond
Cargo's default, so a user's panic backtrace names its functions. They are
62 MiB of `.symtab`/`.strtab` uncompressed and about 4 MB of a 31 MB `.xz`
(host build, 2026-10-06), and no figure moves either way.

**Verification needs no key we hold.** Beside the archives, a `SHA256SUMS`;
each archive carries a GitHub artifact attestation (`actions/attest`,
Sigstore-signed) made once every build, suite and smoke run has passed, from
that run's commit before the tag exists ([`../status/history/2026-10-07.md`](../status/history/2026-10-07.md), "29.4's unattended calls, reviewed"), so it is checked with `gh attestation verify <file> -R mwinters0/pgdt
--source-digest <sha>`, not by tag; and the repository has **immutable
releases** on, so publishing locks the tag and its assets and GitHub adds a
release attestation `gh release verify-asset` checks. A mistake in a published
release is never repaired in place: it is a new version. Rejected: a GPG
signature over `SHA256SUMS`, a key to keep and publish that adds nothing
Sigstore's identity-bound signing lacks.

## Slicing

**Evidence first.** The image and its floor land before anything depends on
them, and the register moves onto the image's build before anything ships
under it, so what gcc 14 and glibc 2.41 cost is a figure before it is a
release. The licence, the manifests and the version marker are mechanical and
follow. The publish workflow cannot be run live without pushing a tag, which a
feed reader takes at once, so its logic is a script under `scripts/` with its
own tests, and its first live run is v0.1.0.

**The phase wraps on a published release.** Cutting v0.1.0 is the last slice,
run by the maintainer: an untested publish path is not a delivered one, so the
phase stays open until a release has gone through end to end and been verified
from outside — `gh attestation verify`, `gh release verify-asset`, and the
archive run in a fresh `debian:trixie` on both architectures.
