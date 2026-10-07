# P29.5 notes — `/release` and the user's page

What 29.6 inherits. The mechanism is `.claude/skills/release/SKILL.md` (the
procedure), `scripts/debian_window.py` (the Debian window, tested by
`scripts/test_debian_window.py`) and `docs/manual/installing.md` (what a user
runs). Nothing below was run against a real release.

## What 29.6 inherits

- **`/release` has never run.** Its rehearsal is the workflows' own commands in
  one detached job (`runs/release-v<V>/rehearsal.log`, ending `REHEARSAL OK
  <commit>`), run after the release commit, and its duration is unmeasured; the commands it joins each ran
  in 29.4, the join did not.
- **`v0.1.0` is the unreleased-version path**: the manifest already says
  `0.1.0` and no tag exists, so `/release` cuts it as it stands, makes no
  commit, and prints the tag command against `HEAD`.
- **The user's page is unverified against a published release.** Its three
  commands — `sha256sum --ignore-missing -c`, `gh attestation verify … -R
  mwinters0/pgdt --source-digest <sha>` with `<sha>` from `git ls-remote …
  refs/tags/v$v^{}`, and `gh release verify-asset v$v <file> -R mwinters0/pgdt`
  — are checked against `gh` 2.101.0's `--help` for flags and argument order
  only. 29.6 runs each on both architectures' archives; the peeled-tag `sha`
  is the one most likely to need a fix, whether the peeled commit is what
  `--source-digest` compares against being unread in `gh`'s source.
- **The README's install section and the manual's links name releases that
  do not exist** until 29.6 publishes one; the wrap ticks the README's status
  list for releases.

## Negative results

- **No `distro-info-data` on this host** (Arch: no `/usr/share/distro-info`),
  so the window check fetches `debian.csv` from the data's own repository
  rather than reading an installed copy, which on Debian is as stale as the
  distribution. An unreadable source refuses.
- **`git ls-remote origin` fails from an agent's sandbox** (no ssh key), so
  step 2's "is `V` released" cannot be rehearsed unattended, and the skill
  treats a failure as a stop.
