#!/usr/bin/env python3
"""The container image a release publishes: `ghcr.io/<owner>/<repo>`, one
manifest per release target, each the target's release archive's files on
`release/container/Dockerfile`'s pinned slim Debian
(`docs/design/decisions.md`, "D171"). **Not `release.py`'s image**, which
builds the binaries; this one only carries them.

**It is assembled from the archives, never built from source**: CI cannot
build the arm64 binary (D109), and an image of other bytes than the release's
would be a second, unverified build. So the Dockerfile only copies, one host
builds both platforms without emulation, and the smoke run checks that the
image's `pgdt` is its archive's byte for byte before running it.

**Pushed by digest, smoked, then tagged, before the Release is published**
(`.github/workflows/release-publish.yml`): `push` builds both platforms from
the draft's archives and pushes the index untagged, the smoke run takes it
by digest on each architecture's runner, and `tag` gives that digest the
release's tags, refusing any tag whose index is not the pushed one, as the
last step before the publication. A release whose image fails is never
published, and no tag names an image no smoke run took.

**Only the amd64 manifest is attested** (by `actions/attest`, from `push`'s
outputs): the arm64 manifest carries a binary no runner built, which D109
refuses to attest as CI's. Its binary is the arm64 archive's, which the
immutable release attests.

**The image is reproducible**: each staged file carries the commit's time,
the build reads it as `SOURCE_DATE_EPOCH`, and the exporter rewrites every
layer's timestamps to it (`rewrite-timestamp`), without which a directory
`COPY` touches carries the build's own clock. So two builds of one commit's
archives under one BuildKit make one index digest. `build` and `push` each
print theirs; whether `nerdctl`'s and `docker buildx`'s agree is not asserted.

Usage (each reads `dist/`'s archives, as `release_ci.py` leaves or fetches
them; `push`, `tag`, and `smoke` given `CONTAINER_DIGEST`, reach the
registry as `GITHUB_ACTOR` with `GH_TOKEN`):

    cd scripts && uv run release_container.py build   # both platforms, as the local image pgdt-container:<version> (nerdctl)
    python3 scripts/release_container.py push         # both platforms, pushed untagged; outputs the digests
    python3 scripts/release_container.py smoke        # this host's platform: the local image, or CONTAINER_DIGEST's
    python3 scripts/release_container.py tag          # CONTAINER_DIGEST under the release's tags
    cd scripts && uv run python -m unittest test_release_container
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import Mapping, Sequence

import release
import release_ci
from release import ReleaseError

REPO = release.REPO
DOCKERFILE = REPO / "release" / "container" / "Dockerfile"
REGISTRY = "ghcr.io"
#: The local image `build` makes and `smoke` runs without a digest.
LOCAL_NAME = "pgdt-container"
#: Each release target's Docker architecture, the directory its files are
#: staged under and the platform it is built for.
ARCHES: dict[str, str] = {
    "x86_64-unknown-linux-gnu": "amd64",
    "aarch64-unknown-linux-gnu": "arm64",
}
#: The architecture an attestation covers (the module docstring).
ATTESTED_ARCH = "amd64"
#: Where the binary sits in the image, which the smoke run hashes.
IMAGE_BINARY = "/usr/local/bin/pgdt"
#: The builder `push` creates: Docker's own driver builds no multi-platform
#: image without the containerd store, which the runner's daemon does not use.
#: Its BuildKit pinned as an action is, by digest, and bumped as the bases are.
BUILDER = "pgdt-container"
BUILDKIT = "moby/buildkit:v0.35.1@sha256:baf6138eb8c37644800c7c913fba051e840ebb9ba13eaf92d92ef9a1712d0b4c"

DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
CODENAME_RE = re.compile(r"^debian:([a-z]+)(?:-slim)?@sha256:[0-9a-f]{64}$")
MACHINES = {"x86_64": "amd64", "amd64": "amd64", "aarch64": "arm64", "arm64": "arm64"}


# --- names -----------------------------------------------------------------


def image_name(repo: str) -> str:
    """The package's name: GHCR takes only lower case."""
    return f"{REGISTRY}/{repo.lower()}"


def release_tags(version: str) -> list[str]:
    """The exact version, its `<major>.<minor>` -- the line pre-1.0 breaks
    nothing within -- and `latest`."""
    major, minor, _patch = version.split(".")
    return [version, f"{major}.{minor}", "latest"]


def platforms() -> str:
    return ",".join(f"linux/{a}" for a in ARCHES.values())


def host_arch(machine: str | None = None) -> str:
    machine = machine or platform.machine()
    if machine not in MACHINES:
        raise ReleaseError(f"no release target runs on {machine}")
    return MACHINES[machine]


def target_of(arch: str) -> str:
    return next(t for t, a in ARCHES.items() if a == arch)


def codename(base: str) -> str:
    m = CODENAME_RE.match(base)
    if not m:
        raise ReleaseError(f"{base} is not a `debian:<codename>[-slim]@sha256:…` reference")
    return m.group(1)


def check_codenames(build_dockerfile: str, container_dockerfile: str) -> str:
    """The container's base on the codename the binaries are built on, so its
    glibc is at or past the floor they link (D104)."""
    built = codename(release.pinned_base(build_dockerfile))
    runs = codename(release.pinned_base(container_dockerfile))
    if built != runs:
        raise ReleaseError(f"the container's base is {runs}, but the binaries are built on {built}")
    return runs


# --- the context -----------------------------------------------------------


def stage_context(dist: Path, version: str, dest: Path, mtime: int, dockerfile: Path = DOCKERFILE) -> dict[str, str]:
    """`dest` made the build's context: the Dockerfile, and each target's
    archive extracted under its architecture, every file at `mtime`. Returns
    each architecture's binary's SHA-256."""
    check_codenames(release.DOCKERFILE.read_text(), dockerfile.read_text())
    shutil.copy(dockerfile, dest / "Dockerfile")
    os.utime(dest / "Dockerfile", (mtime, mtime))
    digests = {}
    for target, arch in ARCHES.items():
        archive = dist / release_ci.archive_name(version, target)
        if not archive.is_file():
            raise ReleaseError(f"{archive} is missing: the image is built from both archives")
        stem = release_ci.archive_stem(version, target)
        with tarfile.open(archive) as tar:
            release_ci.check_layout(tar.getnames(), stem)
            tar.extractall(dest / "unpacked", filter="data")
        (dest / "unpacked" / stem).rename(dest / arch)
        for f in (dest / arch).iterdir():
            os.utime(f, (mtime, mtime))
        os.utime(dest / arch, (mtime, mtime))
        digests[arch] = release_ci.sha256_of(dest / arch / "pgdt")
    (dest / "unpacked").rmdir()
    return digests


def build_args(version: str, revision: str, repo: str, mtime: int) -> list[str]:
    values = {
        "SOURCE_DATE_EPOCH": str(mtime),
        "VERSION": version,
        "REVISION": revision,
        "SOURCE": f"https://github.com/{repo}",
    }
    return [a for k, v in values.items() for a in ("--build-arg", f"{k}={v}")]


def archive_binary_digest(dist: Path, version: str, target: str) -> str:
    """The SHA-256 of `pgdt` inside `target`'s archive."""
    stem, archive = release_ci.archive_stem(version, target), dist / release_ci.archive_name(version, target)
    if not archive.is_file():
        raise ReleaseError(f"{archive} is missing: the image's binary is held to it")
    with tarfile.open(archive) as tar:
        release_ci.check_layout(tar.getnames(), stem)
        f = tar.extractfile(f"{stem}/pgdt")
        assert f is not None
        h = hashlib.sha256()
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


# --- the registry ----------------------------------------------------------


def login(container: Sequence[str], env: Mapping[str, str]) -> None:
    """The runtime logged in to GHCR as the workflow's token, read from stdin."""
    actor, token = env.get("GITHUB_ACTOR", ""), env.get("GH_TOKEN", "")
    if not actor or not token:
        raise ReleaseError("the registry needs GITHUB_ACTOR and GH_TOKEN")
    argv = [*container, "login", REGISTRY, "--username", actor, "--password-stdin"]
    print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
    done = subprocess.run(argv, input=token, capture_output=True, text=True)
    if done.returncode != 0:
        raise ReleaseError(f"`{shlex.join(argv)}` failed:\n{done.stderr}")


def run(argv: Sequence[str]) -> str:
    print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
    done = subprocess.run(list(argv), capture_output=True, text=True)
    if done.returncode != 0:
        raise ReleaseError(f"`{shlex.join(argv)}` exited {done.returncode}:\n{done.stderr}")
    return done.stdout


def platform_digests(raw: str, digest: str) -> dict[str, str]:
    """Each platform's manifest digest from an index's raw bytes, held to the
    index's own `digest`, refusing an index of other platforms than the
    release's."""
    got = "sha256:" + hashlib.sha256(raw.encode()).hexdigest()
    if got != digest:
        raise ReleaseError(f"the index read back as {got}, not {digest}")
    index = json.loads(raw)
    found = {
        m["platform"]["architecture"]: m["digest"]
        for m in index.get("manifests", [])
        if m.get("platform", {}).get("os") == "linux"
    }
    if sorted(found) != sorted(ARCHES.values()) or len(index.get("manifests", [])) != len(ARCHES):
        raise ReleaseError(f"the index holds {sorted(found)}, not {sorted(ARCHES.values())} alone")
    return found


def write_outputs(outputs: Mapping[str, str], env: Mapping[str, str]) -> None:
    """Each value as a step output, where a workflow runs this."""
    path = env.get("GITHUB_OUTPUT")
    for k, v in outputs.items():
        print(f"{k}={v}", flush=True)
    if path:
        with open(path, "a") as f:
            f.writelines(f"{k}={v}\n" for k, v in outputs.items())


def digest_from_env(env: Mapping[str, str]) -> str | None:
    digest = env.get("CONTAINER_DIGEST") or None
    if digest is not None and not DIGEST_RE.match(digest):
        raise ReleaseError(f"CONTAINER_DIGEST is {digest!r}, not sha256:<hex>")
    return digest


# --- the steps -------------------------------------------------------------


def step_build(dist: Path = release_ci.DIST) -> int:
    """Both platforms, as the local image `pgdt-container:<version>`: the
    rehearsal's, which no registry sees. Exported as an OCI archive and
    loaded, since a builder's own image store need not be the runtime's;
    `nerdctl`'s `load` takes every platform."""
    container = shlex.split(release.CONTAINER)
    version, mtime = release.workspace_version(), release_ci.commit_time()
    args = build_args(version, release_ci.head_commit(), release_ci.origin_repo(), mtime)
    tag = f"{LOCAL_NAME}:{version}"
    with tempfile.TemporaryDirectory(prefix="pgdt-container-", dir=dist) as tmp:
        ctx, oci = Path(tmp) / "context", Path(tmp) / "image.tar"
        ctx.mkdir()
        stage_context(dist, version, ctx, mtime)
        run([*container, "build", "--platform", platforms(), *args,
             "--output", f"type=oci,dest={oci},name={tag},rewrite-timestamp=true", str(ctx)])  # fmt: skip
        with tarfile.open(oci) as tar:
            index = json.load(tar.extractfile("index.json"))  # type: ignore[arg-type]
        run([*container, "load", "--all-platforms", "--input", str(oci)])
    digest = index["manifests"][0]["digest"]
    print(f"{tag}: {platforms()}, index {digest}", flush=True)
    return 0


def step_push(env: Mapping[str, str], dist: Path = release_ci.DIST) -> int:
    """Both platforms built by a pinned BuildKit and pushed untagged; the
    index's digest, the attested manifest's and the name, as outputs."""
    container = shlex.split(release.CONTAINER)
    if Path(container[-1]).name != "docker":
        raise ReleaseError(f"push builds with `docker buildx`, not {release.CONTAINER}")
    repo = release_ci.repo_from_env(env)
    name, version, mtime = image_name(repo), release.workspace_version(), release_ci.commit_time()
    login(container, env)
    run([*container, "buildx", "create", "--name", BUILDER, "--driver", "docker-container",
         "--driver-opt", f"image={BUILDKIT}"])  # fmt: skip
    with tempfile.TemporaryDirectory(prefix="pgdt-container-", dir=dist) as ctx:
        stage_context(dist, version, Path(ctx), mtime)
        meta = Path(ctx) / "metadata.json"
        run([
            *container, "buildx", "build", "--builder", BUILDER, "--platform", platforms(),
            "--provenance=false", "--sbom=false",
            *build_args(version, release_ci.head_commit(), repo, mtime),
            "--output", f"type=image,name={name},push-by-digest=true,name-canonical=true,push=true,rewrite-timestamp=true",
            "--metadata-file", str(meta), ctx,
        ])  # fmt: skip
        digest = json.loads(meta.read_text()).get("containerimage.digest", "")
    if not DIGEST_RE.match(digest):
        raise ReleaseError(f"the build reported the digest {digest!r}")
    raw = run([*container, "buildx", "imagetools", "inspect", "--raw", f"{name}@{digest}"])
    manifests = platform_digests(raw, digest)
    write_outputs({"name": name, "digest": digest, f"{ATTESTED_ARCH}_digest": manifests[ATTESTED_ARCH]}, env)
    return 0


def smoke_image_argv(container: Sequence[str], arch: str, image: str, uid: int, gid: int, data: Path,
                     args: Sequence[str]) -> list[str]:  # fmt: skip
    """`pgdt <args>` as the image's entrypoint, on `arch`, as the invoking user,
    with the data mounted where the image's working directory is."""
    return [
        *container, "run", "--rm", "--platform", f"linux/{arch}", "--user", f"{uid}:{gid}",
        "-v", f"{data}:/data", image, *args,
    ]  # fmt: skip


def step_smoke(env: Mapping[str, str], dist: Path = release_ci.DIST, repo: Path = REPO) -> int:
    """This host's platform of the image -- the local one, or the pushed index
    `CONTAINER_DIGEST` names -- holding its archive's `pgdt` byte for byte and
    passing the archive's smoke run (`release_ci.py`, `step_smoke`)."""
    container = shlex.split(release.CONTAINER)
    version, arch = release.workspace_version(), host_arch()
    digest = digest_from_env(env)
    if digest:
        login(container, env)
        image = f"{image_name(release_ci.repo_from_env(env))}@{digest}"
    else:
        image = f"{LOCAL_NAME}:{version}"
    want = archive_binary_digest(dist, version, target_of(arch))
    got = run([*container, "run", "--rm", "--platform", f"linux/{arch}", "--entrypoint", "sha256sum",
               image, IMAGE_BINARY]).split()[0]  # fmt: skip
    if got != want:
        raise ReleaseError(f"{image}'s {arch} pgdt is {got}, not its archive's {want}")
    fixture = release_ci.SMOKE_FIXTURE
    with tempfile.TemporaryDirectory(prefix="pgdt-container-smoke-", dir=dist) as tmp:
        data = Path(tmp)
        shutil.copy(repo / fixture, data / fixture.name)
        source = f"/data/{fixture.name}"

        def pgdt(*args: str) -> str:
            return run(smoke_image_argv(container, arch, image, os.getuid(), os.getgid(), data, args))

        release_ci.check_version_line(pgdt("--version"), version)
        release_ci.check_output_has("parse", pgdt("parse", "--source", source), [release_ci.SMOKE_PARSE_LINE])
        release_ci.check_output_has(
            "query", pgdt("query", "--source", source, "--table", release_ci.SMOKE_TABLE), release_ci.SMOKE_QUERY_ROWS
        )
    print(f"{arch}: {image} holds the archive's pgdt and runs", flush=True)
    return 0


def step_tag(env: Mapping[str, str]) -> int:
    """The pushed index under the release's tags, each read back and refused
    unless it names the index `CONTAINER_DIGEST` does."""
    container = shlex.split(release.CONTAINER)
    digest = digest_from_env(env)
    if not digest:
        raise ReleaseError("CONTAINER_DIGEST names no pushed index")
    name, version = image_name(release_ci.repo_from_env(env)), release.workspace_version()
    login(container, env)
    tags = release_tags(version)
    run([*container, "buildx", "imagetools", "create", *(a for t in tags for a in ("--tag", f"{name}:{t}")),
         f"{name}@{digest}"])  # fmt: skip
    for t in tags:
        held = json.loads(run([*container, "buildx", "imagetools", "inspect", f"{name}:{t}",
                               "--format", "{{json .Manifest}}"])).get("digest")  # fmt: skip
        if held != digest:
            raise ReleaseError(f"{name}:{t} names {held}, not the pushed {digest}")
    print(f"{name}: {', '.join(tags)} name {digest}", flush=True)
    return 0


# --- the command line ------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    """The command line, which the publish workflow's step lines are tested against."""
    p = argparse.ArgumentParser(
        prog="release_container.py",
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("build", help=f"dist/'s archives as the local image {LOCAL_NAME}:<version>, both platforms")
    sub.add_parser("push", help="dist/'s archives built and pushed untagged to GHCR; the digests as outputs")
    sub.add_parser("smoke", help="this host's platform of the local image, or of CONTAINER_DIGEST's")
    sub.add_parser("tag", help="CONTAINER_DIGEST's index under the release's tags")
    return p


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.cmd == "build":
            return step_build()
        if args.cmd == "push":
            return step_push(env)
        if args.cmd == "smoke":
            return step_smoke(env)
        return step_tag(env)
    except ReleaseError as e:
        print(f"release_container.py: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
