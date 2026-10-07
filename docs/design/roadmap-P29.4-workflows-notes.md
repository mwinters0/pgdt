# P29.4 notes — the two workflows

What the later slices inherit from the workflows. The mechanism is
`.github/workflows/release-build.yml` and `release-publish.yml`, which hold no
logic, `scripts/release.py` (`suite-archive`, `suite --archived`,
`build --release`) and `scripts/release_ci.py`, whose docstrings say what each
step does; the readings below are `runs/29.4-workflows/` (`image.log`,
`build.log`, `build-arm.log`, `notices.log`, `smoke.log`, `suite-archive.log` (the 24-job run that
failed), `suite-archive-j6.log`),
not figures.

## What was run, and what was not

**Run here, in the release image and a fresh `debian:trixie`**: the image built
from the two-architecture Dockerfile (x86-64 side); both targets' `build
--release`; both `notices`; both archives; `checksums`, whose output
`sha256sum -c` accepts; `smoke` on the x86-64 archive, whose `--version`
carries no `(unreleased)`; and `suite-archive`, which wrote a 2.4 GB archive
that `cargo nextest list --archive-file … --workspace-remap /work` extracted
and read (it then could not execute an arm64 test binary, as expected here). The scripts' logic is `scripts/test_release_ci.py`'s,
against stand-ins for `gh` and the container runtime.

**Never run**: the workflow files themselves (they parse; `uv run --with
pyyaml` read both), the `gh` calls against a live repository, the arm64 image
(no arm64 host or `binfmt` here), `suite --archived` on one, `actions/attest`,
and a runner's disk and memory under a whole job. **`v0.1.0` is the first live
run of the publish workflow, and `29.6`'s first dispatch of the build.** A build
run failing before `draft` leaves nothing to clean up; a draft left behind
blocks the same version's next dispatch at `preflight`, and is deleted by hand.

## What 29.5 inherits

- **The rehearsal is the workflow's own commands**, `release.py image`,
  `build --release --target=T`, `notices --target=T`, `suite`, then
  `release_ci.py archive`, `checksums` and `smoke --target=<x86-64>` (arm64
  cannot be smoked here), so `/release` calls what CI calls and no second
  recipe exists. `dist/` is the archives' directory, gitignored.
- **What a user verifies** is the archive's own layout (`pgdt-v<version>-<target>/`
  holding `pgdt`, `LICENSE`, `THIRD-PARTY-NOTICES`, `README.md`), the
  `SHA256SUMS` beside the archives (`sha256sum -c`), and the attestation made
  in the `draft` job from the commit: `gh attestation verify <archive> -R
  <owner>/pgdt --source-digest <sha>`.
- **The tag must be annotated** and its message is the Release body
  (`plan_publish` refuses a lightweight tag, an empty annotation, a tag with no
  draft by that name, a draft of another commit and a draft whose assets are
  not exactly the version's three). `/release`'s printed `git tag -a` carries
  it.

## What 29.6 inherits

- **A runner's disk is the first-run risk.** Each build and each suite is a job
  of its own for it, none measured: the dev-profile test build is the large
  one. A `no space left` is a job to split further or a step to free the
  runner's preinstalled software. The arm64 tests cross to their runner as one
  artifact, its size unmeasured.
- **A cold cross build of the tests ran out of memory at its links** under the
  image's 20 GB limit with this host's 24 cargo jobs (`ld` killed, signal 9),
  and completed at six; a runner's four CPUs make four jobs, which is unmeasured
  against its 16 GB. The step takes no job cap; whether a runner needs one
  is unmeasured.
- **The arm64 image is an untested build**: the Dockerfile picks `mise`'s
  checksum and the cross toolchain by `dpkg --print-architecture`, and `mise
  install` fetches every tool in `mise.toml` for arm64, `cmake` and `uv`
  included, which this host cannot confirm.
- **The arm64 suite is nextest alone**: a doctest is compiled where it runs, so
  no archive holds one, and the x86-64 job's doctests are the only ones a
  release runs.
- **Attestation and the arm runner both want a public repository** (or a plan
  that provides them).

## Negative results

- **A `container:` job cannot run this image**: the image is built from the
  Dockerfile, never pushed (the spec's refusal of a registry), so a job builds
  it on the runner and runs each step through the host wrappers, exactly as a
  developer's machine does.
- **`smoke` first read `5 row(s)` off stdout.** It is `query`'s stderr; the
  unit tests' stand-in output had it on stdout, and only the real run in
  `debian:trixie` showed it. The smoke now reads the first and last row, off
  stdout, and `test_release_ci` holds both to the committed fixture.
- **`upload-artifact` zips and drops modes**, so an archive is one `.tar.xz`
  whose modes are inside it, never a directory handed over.
- **The registry's image moved**: the Dockerfile's text is an input of the
  image's tag, so the tag changed with the second architecture; the installed
  package set and the tools on `PATH` are identical to the previous tag's
  (`dpkg-query` over both images), so the register's next sitting rebuilds an
  image with no package or tool different.
- **No YAML parser is in the standard library**, and the scripts' environment
  pins two dependencies for the bytes they decide; the workflow tests read the
  files by line and hold each `run:` to the two scripts' own parsers
  (`build_parser`) instead.
