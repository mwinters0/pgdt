---
name: release
description: Prepare a `pgdt` release on the maintainer's machine — check the tree and the Debian pin, bump the version, rehearse what the release workflows run, commit, and stop with the commands the maintainer runs. Use when the user invokes /release, or asks to cut, prepare or rehearse a release.
---

A release is cut in three hands: **this skill makes one commit and a
rehearsal; the maintainer pushes, dispatches the build and tags; CI owns the
bytes** (`docs/design/roadmap-P29-releases.md`, "`/release`"). **Nothing here
leaves the machine** — no `git push`, no `gh workflow run`, no tag — since a
draft is visible to anyone with access to the repository and a tag is a
`releases.atom` entry at once. It asks the maintainer one
question, in step 2.

## 1. Refuse

Stop, saying which, on any of:

- `git status --porcelain` printing anything.
- `mise run check` not ending green (the whole check, not `--affected`).
- `python3 scripts/debian_window.py` exiting non-zero: it reads the Dockerfile's
  Debian pin against Debian's release days and refuses a codename under six
  months old and a predecessor kept twelve months past its successor's
  release. Its `ok:` line, where the pin may move but need not, goes to the
  maintainer in step 5.

## 2. Say what is being released

`V` is `[workspace.package] version` in the root `Cargo.toml`. It is released
when `git ls-remote --tags origin refs/tags/v<V>` names a tag, and not
otherwise; read it from the remote, a local tag proving nothing, and a
`git ls-remote` that fails is a stop, not an empty answer.

- **`V` unreleased** (the first release, or one whose bump already landed):
  `V` is the next version. Cut it as it stands unless the maintainer asks to
  move it.
- **`V` released**: show `git log --oneline v<V>..HEAD` and ask which kind of
  release this is. **Pre-1.0 the minor moves at every release (`0.N.x` →
  `0.(N+1).0`), the patch only for a release whose changes are all fixes** —
  say which commits are not fixes if the maintainer picks the patch.

## 3. Bump

Skip when `V` is cut as it stands. Otherwise edit that one `version = "…"` line,
then `cargo check --workspace` to move `Cargo.lock`'s member entries. `git diff`
must show those two files and nothing else; anything more is a mistake to
revert, not a release.

The rehearsal comes after the bump, so what it builds and names is the commit's
own tree.

## 4. Rehearse

The workflows' own commands, so no second recipe exists: the image, both
targets' release builds, both `THIRD-PARTY-NOTICES` under the allow-list, the
x86-64 suite, the archives, the smoke run of the x86-64 one, the checksums.
arm64 is a build only: this host runs no arm64 code.

**It runs long**, so per `CLAUDE.md`, "Long-running processes": detached,
logging under `runs/`, never waited on. Launch it and stop here.

```sh
V=<the version>; mkdir -p runs/release-v$V
ID=$( (git rev-parse HEAD; git diff HEAD) | sha256sum | cut -c1-16 )
setsid bash -c "set -euo pipefail
rm -rf dist
(cd scripts && uv run release.py image && uv run release.py build --release \
  && uv run release.py notices && uv run release.py suite \
  && uv run python -m unittest test_release test_release_ci test_debian_window)
python3 scripts/release_ci.py archive
python3 scripts/release_ci.py smoke --target=x86_64-unknown-linux-gnu
python3 scripts/release_ci.py checksums
echo REHEARSAL OK $ID" > runs/release-v$V/rehearsal.log 2>&1 < /dev/null &
```

Print the log's path and that `/release` picks up from it. **On a later run,
with `V` and `ID` computed as above on the same tree, step 4 is done if
`runs/release-v<V>/rehearsal.log` ends with `REHEARSAL OK <ID>`**; a log
without it, or for another tree, is a failure to read or a rehearsal to
launch again. A failed rehearsal after a bump leaves the bump in the tree:
`git checkout -- Cargo.toml Cargo.lock` undoes it.

## 5. Commit and stop

If step 3 bumped: `git add Cargo.toml Cargo.lock` and commit as `Release
v<V>`. Then `SHA=$(git rev-parse HEAD)` and print, for the maintainer to run:

```sh
git push origin main
gh workflow run release-build -f ref=<SHA>
# once the draft exists: gh release view v<V>
git tag -a v<V> <SHA>       # the editor opens: the message is the Release body
git push origin v<V>
```

and `git log --oneline v<previous>..<SHA>` as an aid to the text, which is the
maintainer's. **The tag must be annotated**: the publish workflow refuses a
lightweight one, and a draft built from another commit. Say that
**publishing makes the release immutable**, so a mistake in it is a new
version, and that [`docs/manual/installing.md`](../../../docs/manual/installing.md)
is how it is verified from outside. Relay step 1's `ok:` line if the Debian pin
is in its six-to-twelve-month band.
