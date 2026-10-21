# Release invariants we rely on

Every entry is a property of something outside our control that a release
treats as guaranteed: what GitHub does with a release and its tag, and what an
installer a user runs checks of one. Each records the claim, the source that
proves it, the versions it was verified against, and how to re-verify it.

**This is [`postgres-invariants.md`](postgres-invariants.md)'s and
[`runtime-invariants.md`](runtime-invariants.md)'s sibling**, and neither holds
these: the first is `pg_dump`'s output, the second the environment the process
is given, while these are the platform a release is published on and the tools
it is installed with. A change there surfaces as a release users cannot verify,
or one announced before it exists, rather than as an error of ours. The
trigger to walk this file is a release whose rehearsal or publication meets
something unexpected, a `mise` upgrade (`RL1`), and a change GitHub announces
to releases, their attestations or their feed (`RL2`, `RL3`).
Only the first has a carrier, `/release`'s step 5; the other two are events no
session sees, and a release meeting the unexpected is their backstop.

**Identifiers are `RL<n>`**, allocated on discovery and never reused. **`RL1`–`RL3`
are allocated**, and nothing at or below `RL3` is reused.

**A citation into `mise` is a repo-relative path against a stated tag**, as the
siblings cite theirs; `https://github.com/jdx/mise` at that tag serves it.

## RL1 — mise's `github:` backend accepts an asset GitHub's release attestation alone attests, and names no expected signer

**Claim.** `mise install github:<owner>/<repo>@<version>`, with
`github.github_attestations` at its default (on), verifies an asset whose
digest carries any Sigstore bundle naming `<owner>/<repo>` as its source
repository, GitHub's release attestation among them, and checks no signing
workflow, runner type or SLSA provenance unless the user sets
`slsa_signer_identity` and `slsa_signer_issuer`. An asset with no attestation
at all is skipped, not refused, unless mise's registry marks the tool as
requiring them or a `mise.lock` recorded `github-attestations` provenance.

**Proof.** mise `v2026.10.15`: `src/backend/github.rs` ("We don't know the
expected workflow"); `crates/mise-sigstore/src/github.rs`,
`verify_github_attestation_sources_for_artifact`, into
`crates/mise-sigstore/src/certificate.rs`'s `bundle_source_repository`, which
accepts the release attestation's signed in-toto statement naming the
repository, and `crates/mise-util/src/github.rs`'s `attested_by_repository`.
Observed: v0.1.0's x86-64 archive installed with its attestations verified,
the digest carrying both the release attestation and the `draft` job's build
provenance, and the log holding no "verification method failed" line, which
mise writes for each bundle it rejects.

**Scope limit.** The `github:` backend alone; aqua's and ubi's are not claimed.
The observation is of a digest carrying both bundles: an asset carrying the
release attestation alone has not been installed, mise picking this host's
x86-64 asset.

**Verified against:** mise 2026.10.15, `mwinters0/pgdt` v0.1.0.

**Relied on by:** [`decisions.md`](decisions.md), "D109" — the arm64 archive
attested by its release alone, which a user installing through mise verifies
as one CI attests.

**Re-verify** — on an arm64 host after a release, so the asset carrying the
release attestation alone is the one mise picks:

```sh
d=$(mktemp -d)
MISE_DATA_DIR=$d/data MISE_CACHE_DIR=$d/cache MISE_CONFIG_DIR=$d/config MISE_STATE_DIR=$d/state \
  MISE_LOG_LEVEL=debug mise install github:mwinters0/pgdt@<version> 2>&1 | grep -iE 'attestation|verification method'
```

It holds while the output names `GitHub artifact attestations verified` and no
`verification method failed`.

## RL2 — an immutable release has GitHub attest every asset, naming the repository

**Claim.** Publishing a release in a repository with immutable releases on
makes GitHub sign an attestation of each asset, predicate type
`https://in-toto.io/attestation/release/v0.2`, by its own certificate
authority as `https://dotcom.releases.github.com`, its in-toto statement naming
the repository, whoever uploaded the asset; `gh release verify-asset` and
`gh attestation verify` check it.

**Proof.** Observed: v0.1.0's x86-64 archive's digest carries a bundle of
predicate type `https://in-toto.io/attestation/release/v0.2` beside the
`draft` job's build provenance, which mise read as naming `mwinters0/pgdt`
(`RL1`). mise `v2026.10.15`'s `crates/mise-sigstore/src/certificate.rs` is
the shape's statement in source: `bundle_source_repository` trusts such a
statement only from a certificate of GitHub's internal authority naming
`GITHUB_RELEASE_ATTESTER_IDENTITY`, and `release_statement_repository` reads
the repository from its predicate.

**Scope limit.** Assets present when the release is published; an immutable
release refuses a later one. Nothing about who uploaded an asset, from where,
or which commit built it. That an asset no workflow uploaded is attested alike
is not yet observed: v0.2.0's arm64 archive is the first, and the re-verify
below is the check.

**Verified against:** GitHub.com, `mwinters0/pgdt` v0.1.0.

**Relied on by:** [`decisions.md`](decisions.md), "D109" — the arm64 archive's
only attestation; [`../manual/installing.md`](../manual/installing.md),
"Verify" — the release's attestation, on both archives.

**Re-verify** — after a release, from a directory holding its arm64 archive:

```sh
gh release verify-asset v<version> pgdt-v<version>-aarch64-unknown-linux-gnu.tar.xz -R mwinters0/pgdt
gh attestation verify pgdt-v<version>-aarch64-unknown-linux-gnu.tar.xz -R mwinters0/pgdt \
  --predicate-type https://in-toto.io/attestation/release/v0.2
```

Both pass while it holds, for an asset no workflow built.

## RL3 — a pushed tag is a `releases.atom` entry at once, carrying its annotation, under the id a Release on it later takes

**Claim.** A repository's `https://github.com/<owner>/<repo>/releases.atom`
lists a pushed tag as an entry before any Release names it, its content the
annotated tag's message and its id keyed by the tag name alone
(`tag:github.com,2008:Repository/<id>/<tag>`): the id a Release on that tag
carries, so a reader that took the tag's entry does not take the Release's as
new.

**Proof.** Observed, and documented by GitHub nowhere: `torvalds/linux`'s
feed, a repository with tags and no Releases, carries one entry per tag whose
content is the tag's message ("Linux 7.3-rc5") under that id.

**Scope limit.** The feed alone; GitHub's notifications and the API are not
claimed. Feed readers' handling of an entry whose content changes under one id
is theirs.

**Verified against:** GitHub.com, `torvalds/linux`'s feed.

**Relied on by:** [`../../scripts/release_ci.py`](../../scripts/release_ci.py),
"Build first, tag second" — the release text in the tag's annotation, and the
tag pushed once the draft is whole.

**Re-verify:**

```sh
curl -fsS https://github.com/torvalds/linux/releases.atom | grep -m4 -E '<id>|<title>'
curl -fsS https://github.com/mwinters0/pgdt/releases.atom | grep -m4 -E '<id>|<title>'
```

It holds while the first's ids end in a tag name and its titles name tags no
Release carries, and the second's ids end in `v<version>`.
