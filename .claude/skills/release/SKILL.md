---
name: release
description: Prepare a `pgdt` release on the maintainer's machine — check the tree and the Debian pin, bump the version and commit, rehearse what the release workflows run, and stop with the commands the maintainer runs. Use when the user invokes /release, or asks to cut, prepare or rehearse a release.
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

## 3. Bump and commit

Skip when `V` is cut as it stands. Otherwise edit that one `version = "…"` line,
then `cargo check --workspace` to move `Cargo.lock`'s member entries. `git diff`
must show those two files and nothing else; anything more is a mistake to
revert, not a release. Then `git add Cargo.toml Cargo.lock` and commit as
`Release v<V>`.

The rehearsal comes after the commit, since it reads the manifest's version and
the commit's time, so it builds what the SHA step 5 prints will build
(`docs/design/roadmap-P29-releases.md`, "`/release`").

## 4. Rehearse

The workflows' own commands, so no second recipe exists: the image, both
targets' release builds, both `THIRD-PARTY-NOTICES` under the allow-list, the
x86-64 suite, the archives, the smoke run of the x86-64 one, the checksums.
arm64 is a build only: this host runs no arm64 code.

**It runs long**, so per `CLAUDE.md`, "Long-running processes": detached and
logging under `runs/`. Unlike a sweep it is watched, by the maintainer's
choice: the release is the whole of the session's work and step 5 is owed as
soon as it ends. The log's last line is `REHEARSAL OK <ID>` or
`REHEARSAL FAILED <ID>`, so its end is read from the log, never from a process.

```sh
V=<the version>; mkdir -p runs/release-v$V
ID=$(git rev-parse HEAD)
setsid bash -c "set -euo pipefail
trap 'echo REHEARSAL FAILED $ID' EXIT
rm -rf dist
(cd scripts && uv run release.py image && uv run release.py build --release \
  && uv run release.py notices && uv run release.py suite \
  && uv run python -m unittest test_release test_release_ci test_debian_window)
python3 scripts/release_ci.py archive
python3 scripts/release_ci.py smoke --target=x86_64-unknown-linux-gnu
python3 scripts/release_ci.py checksums
trap - EXIT
echo REHEARSAL OK $ID" > runs/release-v$V/rehearsal.log 2>&1 < /dev/null &
```

Then `CronCreate` a recurring job every 30 minutes, on off-minutes
(`17,47 * * * *`), whose prompt names the log, `V` and `ID` and says: read the
log's last line; on `REHEARSAL OK <ID>`, `CronDelete` this job and do
`/release`'s step 5; on `REHEARSAL FAILED <ID>`, `CronDelete` it and report the
failing step from the log's tail; otherwise report one line of progress and
stop. Print the log's path and the job's ID, and end the turn: the job carries
the release from here. It lives only as long as the session, so **on a later
run,
with `V` and `ID` computed as above, step 4 is done if
`runs/release-v<V>/rehearsal.log` ends with `REHEARSAL OK <ID>`**; a log
without it, or for another commit, is a failure to read or a rehearsal to
launch again. **A failed rehearsal keeps the `Release v<V>` commit**: the next
run finds `V` unreleased, skips step 3 and rehearses `HEAD` again, so a fix
lands as a commit after it and is what step 5 names. Only to abandon the
release, with that commit still at `HEAD` and unpushed, `git reset --keep
HEAD~1` undoes it.

## 5. Stop

`SHA=$(git rev-parse HEAD)` and print, for the maintainer to run:

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
