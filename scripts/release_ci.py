#!/usr/bin/env python3
"""What a release does with what `release.py` built: the archives, their
checksums, the smoke run, the draft Release, the arm64 archive's attachment
and the publication.

**Build first, tag second.** A dispatched workflow builds a pushed commit's
x86-64 archive into a draft; the maintainer attaches the arm64 archive, built
from the same commit on their own machine (`docs/design/decisions.md`,
"D109"), then pushes an annotated tag on it, whose message is the Release
body, and the tag's workflow runs the arm64 archive on arm64 and publishes the
draft. A pushed tag is a `releases.atom` entry at once, carrying the
annotation, under the entry id a Release on that tag later takes
(`docs/design/release-invariants.md`, "RL3"), so the annotation is where the
text is when a feed reader first takes the entry, and nothing is announced
before its binaries exist. Rejected: a build the tag triggers (a release
announced while building, and a failed build an announced tag nobody can
retract); the text in a file or a workflow input (the tag's entry read empty).
Everything that leaves the machine is the maintainer's
(`.claude/skills/release/SKILL.md`).

**Verification needs no key we hold**: `SHA256SUMS`, an attestation of the
x86-64 archive made from the build's commit before the tag exists (so verified
by `--source-digest`, not by tag), GitHub's attestation of every asset of the
published release, the arm64 archive's only one (D109,
`release-invariants.md`, "RL1" and "RL2"), and immutable releases, under
which a mistake in a published release is a new version. Rejected: a GPG
signature over the checksums, a key to keep and publish that adds nothing an
identity-bound signature lacks. The binary keeps its symbols, the release
profile stripping nothing beyond Cargo's default, so a user's panic backtrace
names its functions.

**The arm64 archive is attached to CI's draft, never uploaded before it**
(`attach`): run where the rehearsal ran, on the clean tree whose `HEAD` the
draft was built from, it uploads `dist/`'s arm64 archive, downloads the
draft's x86-64 one, and replaces the draft's `SHA256SUMS` with one over both.
It compares CI's x86-64 archive with the rehearsal's and reports, never
refuses: equal bytes are the evidence that this machine's pipeline makes CI's
from one commit, and the property is not yet one anything relies on.

The workflows
(`.github/workflows/release-build.yml`, `release-publish.yml`) hold no logic of
their own: each `run:` step is one line calling `release.py` or this script,
the rest being pinned actions that check out, upload, download and attest, so
what a workflow does is what these tests hold it to, and the first live run is
not the first time any of it has run but for the GitHub calls the tests fake
(`Gh`). **A workflow never interpolates an input into a
shell line**: the commit a build is dispatched for and the tag a publish is
triggered by arrive as environment variables (`INPUT_REF`, `GITHUB_REF_NAME`)
and are validated here before anything is built or published from them.

**A draft is matched by name.** The build creates it as `v<manifest version>`;
`attach` and the publish find it by that name and then check its commit, so a
tag naming another version finds nothing, and the tag, the draft and the
archives carry one string. Publishing is refused unless the tag is annotated
(its message is the Release body, and a lightweight tag has none), the draft
exists unpublished, was built from the tag's commit, and holds exactly the
assets this script names: a draft failing any of these is not published.

**An archive is deterministic**: one top-level directory, entries in a fixed
order with fixed ownership, and the commit's own time, so the same inputs give
the same bytes.

Usage (`archive` reads `release.py`'s output under the release state's target
directory; `archive` empties `dist/` and writes it, `checksums` adds to it,
`smoke`, `draft` and `attach` read it, and `fetch` writes a draft's asset
into it):

    python3 scripts/release_ci.py preflight                   # INPUT_REF is HEAD; no Release for the version yet
    python3 scripts/release_ci.py perf-events                 # a runner lets the suite count its own events
    python3 scripts/release_ci.py archive [--target T]...     # dist/pgdt-v<version>-<target>.tar.xz
    python3 scripts/release_ci.py smoke --target T            # the archive, run in a fresh debian:trixie
    python3 scripts/release_ci.py checksums [--target T]...   # dist/SHA256SUMS over those archives, every target's by default
    python3 scripts/release_ci.py draft                       # a draft Release v<version> holding CI's archive and its sum
    python3 scripts/release_ci.py attach                      # the arm64 archive and SHA256SUMS over both, onto HEAD's draft
    python3 scripts/release_ci.py fetch --target T            # GITHUB_REF_NAME's draft, checked, and its T archive into dist/
    python3 scripts/release_ci.py publish                     # GITHUB_REF_NAME's draft, published
    cd scripts && uv run python -m unittest test_release_ci
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

import release
from release import ReleaseError

REPO = release.REPO
#: Where the archives and their checksums are written, and read from.
DIST = REPO / "dist"
CHECKSUMS = "SHA256SUMS"
#: What a draft's body says until its publication replaces it with the tag's
#: annotation.
DRAFT_BODY = "Draft. The release text is the tag's annotation, set when the tag is pushed."

#: The targets CI builds, attests and drafts.
CI_TARGETS = (release.NATIVE_TARGET,)
#: The targets built on the maintainer's machine and attached to CI's draft,
#: whose fat link CI's runner cannot hold (D109).
ATTACHED_TARGETS = (release.CROSS_TARGET,)

SHA_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:([0-9a-f]{64})$")
#: `origin`'s URL on GitHub, in each form `git` writes one.
REMOTE_RE = re.compile(r"^(?:git@github\.com:|ssh://git@github\.com/|https://github\.com/)([\w.-]+/[\w.-]+?)(?:\.git)?/?$")
TAG_RE = re.compile(r"^v(\d+\.\d+\.\d+)$")
SIGNATURE_RE = re.compile(r"\n?-----BEGIN (?:PGP|SSH|SIGNED) [A-Z ]*-----.*\Z", re.DOTALL)

#: The smoke run's input and what it must read back: a committed fixture, so a
#: change to it fails `test_release_ci` rather than a release.
SMOKE_FIXTURE = Path("fixtures/16/edge_cases/default.sql")
SMOKE_TABLE = "public.widgets"
SMOKE_PARSE_LINE = "public.widgets (5 rows)"
#: Read off stdout, where rows go (`query`'s own `5 row(s)` is stderr's): the
#: first row and the last, so a truncated scan fails it.
SMOKE_QUERY_ROWS = ("1\talpha\t", "5\t\tempty name above")


# --- names -----------------------------------------------------------------


def tag_of(version: str) -> str:
    return f"v{version}"


def archive_stem(version: str, target: str) -> str:
    """The archive's name without its extension, and its top-level directory."""
    return f"pgdt-{tag_of(version)}-{target}"


def archive_name(version: str, target: str) -> str:
    return f"{archive_stem(version, target)}.tar.xz"


def expected_assets(version: str) -> list[str]:
    """What a release holds: one archive for each target, and the checksums."""
    return sorted(archive_name(version, t) for t in release.TARGETS) + [CHECKSUMS]


def draft_assets(version: str) -> list[str]:
    """What CI drafts: its own archives, and the checksums over them alone
    until `attach` replaces them."""
    return sorted(archive_name(version, t) for t in CI_TARGETS) + [CHECKSUMS]


def host_target_dir() -> Path:
    """The cargo target directory `release.py` mounts into the image, where the
    host reads what its steps wrote."""
    return release.STATE.resolve() / "target"


def check_ref(ref: str | None, head: str) -> str:
    """`ref`, the commit a build was dispatched for, which must be a full
    commit id and the one checked out. A branch or an abbreviation names
    something that moves."""
    if not ref or not SHA_RE.match(ref):
        raise ReleaseError(f"the ref must be a full 40-hex commit id, not {ref!r}")
    if ref != head:
        raise ReleaseError(f"the checkout is {head}, not the requested {ref}")
    return ref


def git(*args: str, repo: Path = REPO) -> str:
    return subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, text=True).stdout


def head_commit(repo: Path = REPO) -> str:
    return git("rev-parse", "HEAD", repo=repo).strip()


def tree_is_clean(repo: Path = REPO) -> bool:
    return not git("status", "--porcelain", repo=repo).strip()


def repo_of_remote(url: str) -> str:
    """`owner/name` of a GitHub remote's URL, refusing any other host."""
    m = REMOTE_RE.match(url.strip())
    if not m:
        raise ReleaseError(f"origin is {url.strip()!r}, not a GitHub repository")
    return m.group(1)


def origin_repo(repo: Path = REPO) -> str:
    return repo_of_remote(git("remote", "get-url", "origin", repo=repo))


# --- the archives ----------------------------------------------------------


@dataclass(frozen=True)
class Member:
    """One file of an archive: its name inside the top-level directory, its
    source, and its mode."""

    name: str
    source: Path
    mode: int


def members(binary: Path, notices: Path, repo: Path = REPO) -> list[Member]:
    """An archive's files: the binary, the root licence and README, and the
    notices made for this target."""
    found = [
        Member("pgdt", binary, 0o755),
        Member("LICENSE", repo / "LICENSE", 0o644),
        Member(release.NOTICES_NAME, notices, 0o644),
        Member("README.md", repo / "README.md", 0o644),
    ]
    for m in found:
        if not m.source.is_file():
            raise ReleaseError(f"{m.source} is missing: {m.name} is part of every archive")
    return sorted(found, key=lambda m: m.name)


def write_archive(dest: Path, stem: str, files: Sequence[Member], mtime: int) -> None:
    """A `.tar.xz` holding `stem/` and each file under it, deterministic: a
    fixed order, root's ownership and one timestamp."""
    dest.parent.mkdir(parents=True, exist_ok=True)

    def entry(name: str, mode: int, kind: bytes = tarfile.REGTYPE, size: int = 0) -> tarfile.TarInfo:
        info = tarfile.TarInfo(name)
        info.type, info.mode, info.size, info.mtime = kind, mode, size, mtime
        info.uid = info.gid = 0
        info.uname = info.gname = "root"
        return info

    with tarfile.open(dest, "w:xz", format=tarfile.PAX_FORMAT) as tar:
        tar.addfile(entry(stem, 0o755, tarfile.DIRTYPE))
        for m in files:
            with m.source.open("rb") as f:
                tar.addfile(entry(f"{stem}/{m.name}", m.mode, size=m.source.stat().st_size), f)


def check_layout(names: Sequence[str], stem: str) -> None:
    """Refuse an archive that is not one top-level directory holding exactly
    what an archive holds."""
    want = {stem, *(f"{stem}/{n}" for n in ("pgdt", "LICENSE", release.NOTICES_NAME, "README.md"))}
    if set(names) != want:
        extra, missing = sorted(set(names) - want), sorted(want - set(names))
        raise ReleaseError(f"the archive's members are not {stem}/'s: extra {extra}, missing {missing}")


def commit_time(repo: Path = REPO) -> int:
    return int(git("log", "-1", "--format=%ct", "HEAD", repo=repo))


def step_archive(targets: Sequence[str], dist: Path = DIST) -> int:
    """Each target's archive, in a `dist/` emptied first: one call archives a
    release, and an earlier run's files are never checksummed beside it."""
    version, target_dir, mtime = release.workspace_version(), host_target_dir(), commit_time()
    if dist.exists():
        shutil.rmtree(dist)
    for target in targets:
        files = members(release.Build(target).binary(target_dir), release.notices_path(target, target_dir))
        dest = dist / archive_name(version, target)
        write_archive(dest, archive_stem(version, target), files, mtime)
        print(f"{target}: {dest} ({dest.stat().st_size} bytes)", flush=True)
    return 0


# --- the checksums ---------------------------------------------------------


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def checksum_lines(digests: Mapping[str, str]) -> str:
    """`sha256sum`'s format, one line a file in name order, which `sha256sum -c`
    reads from the directory holding the files."""
    return "".join(f"{digests[name]}  {name}\n" for name in sorted(digests))


def step_checksums(
    targets: Sequence[str] = tuple(release.TARGETS), version: str | None = None, dist: Path = DIST
) -> int:
    """`SHA256SUMS` over the `targets`' archives, refusing a `dist/` holding
    any other: a stray one would be checksummed and shipped."""
    version = version or release.workspace_version()
    archives = sorted(p.name for p in dist.glob("*.tar.xz"))
    want = sorted(archive_name(version, t) for t in targets)
    if archives != want:
        raise ReleaseError(f"{dist} holds {archives}, not the release's archives {want}")
    (dist / CHECKSUMS).write_text(checksum_lines({n: sha256_of(dist / n) for n in archives}))
    print(f"{dist / CHECKSUMS}", flush=True)
    return 0


# --- the smoke run ---------------------------------------------------------


def check_version_line(output: str, version: str) -> None:
    """A release says it is one: `pgdt <version> (` with no `(unreleased)`."""
    first = output.splitlines()[0] if output.strip() else ""
    if not first.startswith(f"pgdt {version} ("):
        raise ReleaseError(f"--version says {first!r}, not pgdt {version}")
    if "(unreleased)" in first:
        raise ReleaseError(f"--version marks a release's binary unreleased: {first!r}")


def check_output_has(what: str, output: str, wanted: Sequence[str]) -> None:
    missing = [w for w in wanted if w not in output]
    if missing:
        raise ReleaseError(f"{what}'s output lacks {missing}:\n{output}")


def smoke_argv(
    container: Sequence[str], base: str, uid: int, gid: int, bin_dir: Path, data_dir: Path,
    args: Sequence[str],
) -> list[str]:  # fmt: skip
    """`pgdt <args>` run from the archive in a fresh image of the release's own
    base, as the invoking user: the binary mounted read-only, the data beside
    it writable for the cache `parse` leaves."""
    return [
        *container, "run", "--rm", "--user", f"{uid}:{gid}",
        "-v", f"{bin_dir}:/pgdt:ro", "-v", f"{data_dir}:/data",
        base, "/pgdt/pgdt", *args,
    ]  # fmt: skip


def step_smoke(target: str, dist: Path = DIST, repo: Path = REPO) -> int:
    """The `target` archive, extracted and run in a fresh `debian:trixie` on
    this machine's architecture: `--version`, a `parse` and a `query` of a
    committed fixture. It tests the bytes that ship."""
    version = release.workspace_version()
    archive, stem = dist / archive_name(version, target), archive_stem(version, target)
    base = release.pinned_base((repo / "release" / "Dockerfile").read_text())
    container = shlex.split(release.CONTAINER)
    # Beside the archive, not in `/tmp`: the container runtime mounts it from
    # its own namespace, and a shell's `/tmp` may be private (systemd's
    # `PrivateTmp`), so a bind of it finds nothing.
    with tarfile.open(archive) as tar, tempfile.TemporaryDirectory(prefix="pgdt-smoke-", dir=dist) as tmp:
        check_layout(tar.getnames(), stem)
        tar.extractall(tmp, filter="data")
        data = Path(tmp) / "data"
        data.mkdir()
        shutil.copy(repo / SMOKE_FIXTURE, data / SMOKE_FIXTURE.name)
        source = f"/data/{SMOKE_FIXTURE.name}"

        def run(*args: str) -> str:
            argv = smoke_argv(container, base, os.getuid(), os.getgid(), Path(tmp) / stem, data, args)
            print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
            done = subprocess.run(argv, capture_output=True, text=True)
            if done.returncode != 0:
                raise ReleaseError(f"`pgdt {shlex.join(args)}` exited {done.returncode}:\n{done.stderr}")
            return done.stdout

        check_version_line(run("--version"), version)
        check_output_has("parse", run("parse", "--source", source), [SMOKE_PARSE_LINE])
        check_output_has("query", run("query", "--source", source, "--table", SMOKE_TABLE), SMOKE_QUERY_ROWS)
    print(f"{target}: {archive.name} ran in {base}", flush=True)
    return 0


# --- GitHub ----------------------------------------------------------------


def flatten_pages(data: Any) -> list[dict]:
    """`gh api --paginate --slurp`'s output, an array of pages, as one list."""
    if isinstance(data, list) and all(isinstance(p, list) for p in data):
        return [item for page in data for item in page]
    if isinstance(data, list):
        return data
    raise ReleaseError(f"expected a list of releases from the API, got {type(data).__name__}")


class Gh:
    """The GitHub API as the `gh` CLI reaches it, as the token it is given: one
    repository, one call. Tests stand a fake in its place."""

    def __init__(self, repo: str):
        self.repo = repo

    def api(self, path: str, *, method: str = "GET", payload: Any = None, paginate: bool = False) -> Any:
        argv = ["gh", "api", "-X", method, f"repos/{self.repo}/{path}"]
        if paginate:
            argv += ["--paginate", "--slurp"]
        if payload is not None:
            argv += ["--input", "-"]
        done = subprocess.run(
            argv, capture_output=True, text=True,
            input=json.dumps(payload) if payload is not None else None,
        )  # fmt: skip
        if done.returncode != 0:
            raise ReleaseError(f"`{shlex.join(argv)}` failed:\n{done.stderr}")
        return json.loads(done.stdout) if done.stdout.strip() else None

    def releases(self) -> list[dict]:
        """Every release, drafts included."""
        return flatten_pages(self.api("releases", paginate=True))

    def create_draft(self, tag: str, sha: str, files: Sequence[Path]) -> None:
        argv = [
            "gh", "release", "create", tag, *map(str, files), "--repo", self.repo,
            "--draft", "--target", sha, "--title", tag, "--notes", DRAFT_BODY,
        ]  # fmt: skip
        print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
        subprocess.run(argv, check=True)

    def upload(self, tag: str, files: Sequence[Path]) -> None:
        """Each file onto the Release named `tag`, a draft included, replacing
        an asset of its name."""
        argv = ["gh", "release", "upload", tag, *map(str, files), "--repo", self.repo, "--clobber"]
        print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
        subprocess.run(argv, check=True)

    def download_asset(self, asset_id: int, dest: Path) -> None:
        """An asset's bytes by its id, which reaches a draft's for a token with
        push access, as no download URL does."""
        argv = [
            "gh", "api", "-H", "Accept: application/octet-stream", f"repos/{self.repo}/releases/assets/{asset_id}",
        ]  # fmt: skip
        with dest.open("wb") as f:
            done = subprocess.run(argv, stdout=f, stderr=subprocess.PIPE)
        if done.returncode != 0:
            raise ReleaseError(f"`{shlex.join(argv)}` failed:\n{done.stderr.decode(errors='replace')}")


def repo_from_env(env: Mapping[str, str] = os.environ) -> str:
    repo = env.get("GITHUB_REPOSITORY", "")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repo):
        raise ReleaseError(f"GITHUB_REPOSITORY is {repo!r}, not owner/name")
    return repo


# --- the draft -------------------------------------------------------------


def plan_draft(version: str, releases: Sequence[dict]) -> str:
    """The tag the draft is created under, refused where any Release, draft or
    published, already carries it: a release is never repaired in place, and a
    stale draft is the maintainer's to delete."""
    tag = tag_of(version)
    held = [r for r in releases if r.get("tag_name") == tag]
    if held:
        state = "a draft" if held[0].get("draft") else "published"
        raise ReleaseError(
            f"{tag} already has a Release ({state}); delete a stale draft by hand, "
            "or release the next version"
        )
    return tag


def draft_files(version: str, dist: Path = DIST) -> list[Path]:
    """CI's assets in `dist/`, exactly: refusing a missing one."""
    files = [dist / name for name in draft_assets(version)]
    missing = [f.name for f in files if not f.is_file()]
    if missing:
        raise ReleaseError(f"{dist} lacks {missing}: build, archive and checksum first")
    return files


def step_preflight(gh: Gh, ref: str | None) -> int:
    version = release.workspace_version()
    check_ref(ref, head_commit())
    tag = plan_draft(version, gh.releases())
    print(f"{ref}: {tag} is free", flush=True)
    return 0


def step_draft(gh: Gh, ref: str | None, dist: Path = DIST) -> int:
    version = release.workspace_version()
    sha = check_ref(ref, head_commit())
    tag = plan_draft(version, gh.releases())
    gh.create_draft(tag, sha, draft_files(version, dist))
    print(f"{tag}: a draft of {sha}", flush=True)
    return 0


# --- the attachment --------------------------------------------------------


def plan_attach(version: str, head: str, releases: Sequence[dict]) -> dict:
    """The draft `attach` adds to, or the reason it must not: the one Release
    named for the version, unpublished, built from `head`, holding CI's
    archives uploaded and nothing a release does not hold."""
    tag = tag_of(version)
    held = [r for r in releases if r.get("tag_name") == tag]
    if not held:
        raise ReleaseError(f"no draft Release is named {tag}: dispatch release-build for {head} first")
    if len(held) > 1:
        raise ReleaseError(f"{len(held)} Releases are named {tag}")
    draft = held[0]
    if not draft.get("draft"):
        raise ReleaseError(f"{tag} is already published")
    built = draft.get("target_commitish", "")
    if built != head:
        raise ReleaseError(f"the draft {tag} was built from {built!r}, not this checkout's HEAD {head}")
    assets = {a["name"]: a.get("state") for a in draft.get("assets", [])}
    stray = sorted(set(assets) - set(expected_assets(version)))
    if stray:
        raise ReleaseError(f"the draft {tag} holds {stray}, which no release does")
    ci = [archive_name(version, t) for t in CI_TARGETS]
    missing = sorted(n for n in ci if assets.get(n) != "uploaded")
    if missing:
        raise ReleaseError(f"the draft {tag} lacks CI's uploaded {missing}")
    return draft


def asset_named(draft: dict, name: str) -> dict:
    found = [a for a in draft.get("assets", []) if a["name"] == name]
    if len(found) != 1:
        raise ReleaseError(f"the draft {draft.get('tag_name')} holds {len(found)} assets named {name}")
    return found[0]


def fetch_asset(gh: Gh, asset: dict, dest: Path) -> str:
    """`asset` downloaded to `dest`, held to the digest the API states for it,
    and its SHA-256."""
    gh.download_asset(asset["id"], dest)
    got = sha256_of(dest)
    stated = asset.get("digest")
    if not stated:
        print(f"{asset['name']}: the API states no digest, so the download is held to none", file=sys.stderr)
        return got
    m = DIGEST_RE.match(stated)
    if not m:
        raise ReleaseError(f"{asset['name']}'s digest is {stated!r}, not sha256:<hex>")
    if m.group(1) != got:
        raise ReleaseError(f"{asset['name']} downloaded as {got}, not the {m.group(1)} the API states")
    return got


def check_archived_from(archive: Path, stem: str, mtime: int) -> None:
    """Refuse an archive that is not an archive of `stem` made at the commit
    time `mtime`: one left in `dist/` by another commit's rehearsal."""
    with tarfile.open(archive) as tar:
        check_layout(tar.getnames(), stem)
        times = {m.mtime for m in tar.getmembers()}
    if times != {mtime}:
        raise ReleaseError(f"{archive.name} was archived at {sorted(times)}, not at HEAD's commit time {mtime}")


def comparison_line(target: str, ci: str, rehearsed: Path) -> str:
    """Whether CI's archive for `target` is the rehearsal's, byte for byte."""
    if not rehearsed.is_file():
        return f"{target}: no rehearsal archive at {rehearsed} to compare CI's with"
    here = sha256_of(rehearsed)
    if here == ci:
        return f"{target}: CI's archive is byte-identical to the rehearsal's ({ci})"
    return f"{target}: CI's archive differs from the rehearsal's: CI {ci}, rehearsal {here}"


def step_attach(gh: Gh, dist: Path = DIST, repo: Path = REPO) -> int:
    """The attached targets' archives onto `HEAD`'s draft, and `SHA256SUMS`
    over them and CI's, replacing CI's own; CI's archive compared with the
    rehearsal's and reported. Refused on a dirty tree, an archive in `dist/`
    not made at `HEAD`, and a draft `plan_attach` refuses, before anything is
    uploaded."""
    version = release.workspace_version()
    if not tree_is_clean(repo):
        raise ReleaseError("the tree is not clean: an archive is attached from the checkout of the commit it is of")
    head = head_commit(repo)
    draft = plan_attach(version, head, gh.releases())
    tag, mtime = tag_of(version), commit_time(repo)
    local = []
    for target in ATTACHED_TARGETS:
        path = dist / archive_name(version, target)
        if not path.is_file():
            raise ReleaseError(f"{path} is missing: rehearse HEAD first (`/release`, step 4)")
        check_archived_from(path, archive_stem(version, target), mtime)
        local.append(path)
    digests = {p.name: sha256_of(p) for p in local}
    with tempfile.TemporaryDirectory(prefix="pgdt-attach-", dir=dist) as tmp:
        for target in CI_TARGETS:
            name = archive_name(version, target)
            digests[name] = fetch_asset(gh, asset_named(draft, name), Path(tmp) / name)
            print(comparison_line(target, digests[name], dist / name), flush=True)
        sums = Path(tmp) / CHECKSUMS
        sums.write_text(checksum_lines(digests))
        gh.upload(tag, [*local, sums])
    print(f"{tag}: {', '.join(p.name for p in local)} and {CHECKSUMS} attached to the draft of {head}", flush=True)
    return 0


# --- the publication -------------------------------------------------------


@dataclass(frozen=True)
class Tag:
    """What the API says of a pushed tag."""

    name: str
    #: Whether the tag is an object of its own, which carries a message.
    annotated: bool
    commit: str
    message: str


def fetch_tag(gh: Gh, name: str) -> Tag:
    ref = gh.api(f"git/ref/tags/{name}")["object"]
    if ref["type"] == "commit":
        return Tag(name, False, ref["sha"], "")
    if ref["type"] != "tag":
        raise ReleaseError(f"{name} points at a {ref['type']}, not a commit or an annotated tag")
    tag = gh.api(f"git/tags/{ref['sha']}")
    if tag["object"]["type"] != "commit":
        raise ReleaseError(f"{name} is an annotated tag of a {tag['object']['type']}, not of a commit")
    return Tag(name, True, tag["object"]["sha"], tag["message"])


def annotation_body(message: str) -> str:
    """The Release body: a tag's message without a trailing signature block."""
    return SIGNATURE_RE.sub("", message).strip()


@dataclass(frozen=True)
class Publication:
    release_id: int
    body: str


def plan_publish(name: str, tag: Tag, releases: Sequence[dict]) -> Publication:
    """The release `name`'s tag publishes, or the reason it must not.

    **Every refusal is a tag that is not a release's or does not match its
    draft**: a name not `v<major>.<minor>.<patch>`, an unannotated tag or an
    empty annotation (no text), a tag with no draft (nothing was built for it)
    or several, a draft already published, one built from another commit, or
    one not holding exactly the uploaded assets of this version. A published release is immutable, so
    each is checked before the one irreversible call."""
    m = TAG_RE.match(name)
    if not m:
        raise ReleaseError(f"{name!r} is not a release tag: v<major>.<minor>.<patch>")
    version = m.group(1)
    if not tag.annotated:
        raise ReleaseError(f"{name} is a lightweight tag; the release text is an annotated tag's message")
    body = annotation_body(tag.message)
    if not body:
        raise ReleaseError(f"{name}'s annotation is empty: it is the Release's text")
    held = [r for r in releases if r.get("tag_name") == name]
    if not held:
        raise ReleaseError(f"no draft Release is named {name}: run release-build for this version first")
    if len(held) > 1:
        raise ReleaseError(f"{len(held)} Releases are named {name}")
    draft = held[0]
    if not draft.get("draft"):
        raise ReleaseError(f"{name} is already published")
    built = draft.get("target_commitish", "")
    if not SHA_RE.match(built) or built != tag.commit:
        raise ReleaseError(
            f"the draft {name} was built from {built!r}, but the tag names {tag.commit}"
        )
    assets = {a["name"]: a.get("state") for a in draft.get("assets", [])}
    want = expected_assets(version)
    if sorted(assets) != sorted(want):
        raise ReleaseError(f"the draft {name} holds {sorted(assets)}, not {sorted(want)}")
    pending = sorted(n for n, state in assets.items() if state != "uploaded")
    if pending:
        raise ReleaseError(f"the draft {name}'s assets are not all uploaded: {pending}")
    return Publication(draft["id"], body)


def step_fetch(gh: Gh, name: str | None, target: str, dist: Path = DIST) -> int:
    """`target`'s archive from the draft `name`'s tag would publish, checked as
    the publish checks it, into `dist/`: the bytes that ship, for the smoke
    run."""
    if not name:
        raise ReleaseError("GITHUB_REF_NAME names no tag")
    releases = gh.releases()
    plan = plan_publish(name, fetch_tag(gh, name), releases)
    draft = next(r for r in releases if r.get("id") == plan.release_id)
    asset = asset_named(draft, archive_name(name[1:], target))
    dist.mkdir(parents=True, exist_ok=True)
    fetch_asset(gh, asset, dist / asset["name"])
    print(f"{name}: {dist / asset['name']}", flush=True)
    return 0


def step_publish(gh: Gh, name: str | None) -> int:
    if not name:
        raise ReleaseError("GITHUB_REF_NAME names no tag")
    plan = plan_publish(name, fetch_tag(gh, name), gh.releases())
    gh.api(f"releases/{plan.release_id}", method="PATCH", payload={"draft": False, "body": plan.body})
    print(f"{name}: published", flush=True)
    return 0


# --- the runner ------------------------------------------------------------

#: The kernel's gate on `perf_event_open`, which no container namespaces.
PERF_PARANOID = Path("/proc/sys/kernel/perf_event_paranoid")
#: The highest level at which an unprivileged process may count its own
#: user-space events, as `count-instructions`' tests do
#: (`crates/count-instructions/tests/count_instructions.rs`).
PERF_PARANOID_MAX = 2


def step_perf_events(path: Path = PERF_PARANOID, run: Any = subprocess.run) -> int:
    """The runner's `perf_event_paranoid` lowered to `PERF_PARANOID_MAX` when
    it is above it, so the suite counts its own events where this machine
    does; GitHub's Ubuntu image ships it higher. A runner is the job's alone, so
    the change outlives nothing; a level already at or below is left as it is."""
    level = int(path.read_text())
    if level <= PERF_PARANOID_MAX:
        print(f"perf_event_paranoid is {level}: left as it is", flush=True)
        return 0
    run(["sudo", "sysctl", "-w", f"kernel.perf_event_paranoid={PERF_PARANOID_MAX}"], check=True)
    print(f"perf_event_paranoid lowered from {level} to {PERF_PARANOID_MAX}", flush=True)
    return 0


# --- the command line ------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    """The command line, which the workflows' step lines are tested against."""
    p = argparse.ArgumentParser(
        prog="release_ci.py",
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("preflight", help="INPUT_REF is the checkout, and no Release carries the version's tag")
    sub.add_parser("perf-events", help=f"the runner's perf_event_paranoid at most {PERF_PARANOID_MAX}, for the suite")
    a = sub.add_parser("archive", help="dist/pgdt-v<version>-<target>.tar.xz from release.py's output")
    a.add_argument(
        "--target", action="append", choices=sorted(release.TARGETS),
        help="one target (repeatable); every release target by default",
    )  # fmt: skip
    s = sub.add_parser("smoke", help="the target's archive, run in a fresh debian:trixie")
    s.add_argument("--target", required=True, choices=sorted(release.TARGETS))
    c = sub.add_parser("checksums", help=f"dist/{CHECKSUMS} over the targets' archives")
    c.add_argument(
        "--target", action="append", choices=sorted(release.TARGETS),
        help="one target (repeatable); every release target by default",
    )  # fmt: skip
    sub.add_parser("draft", help="a draft Release holding CI's archives and their checksums, from INPUT_REF")
    sub.add_parser("attach", help=f"dist/'s attached archives and {CHECKSUMS} over every archive, onto HEAD's draft")
    f = sub.add_parser("fetch", help="the target's archive from GITHUB_REF_NAME's draft, checked, into dist/")
    f.add_argument("--target", required=True, choices=sorted(release.TARGETS))
    sub.add_parser("publish", help="the draft named GITHUB_REF_NAME, published with the tag's annotation")
    return p


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.cmd == "archive":
            return step_archive(args.target or list(release.TARGETS))
        if args.cmd == "smoke":
            return step_smoke(args.target)
        if args.cmd == "checksums":
            return step_checksums(args.target or list(release.TARGETS))
        if args.cmd == "perf-events":
            return step_perf_events()
        if args.cmd == "attach":
            return step_attach(Gh(origin_repo()))
        gh = Gh(repo_from_env(env))
        if args.cmd == "preflight":
            return step_preflight(gh, env.get("INPUT_REF"))
        if args.cmd == "draft":
            return step_draft(gh, env.get("INPUT_REF"))
        if args.cmd == "fetch":
            return step_fetch(gh, env.get("GITHUB_REF_NAME"), args.target)
        return step_publish(gh, env.get("GITHUB_REF_NAME"))
    except ReleaseError as e:
        print(f"release_ci.py: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
