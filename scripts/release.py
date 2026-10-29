#!/usr/bin/env python3
"""The release build: its image, `pgdt` built in it for both targets, and the
suite run there.

`docs/design/decisions.md`, "D104", is what this implements: one image,
`release/Dockerfile`, pinned by digest, builds every binary and notice a
release ships, and the glibc it links against is the floor a release promises. Each step is an argv run in that image, so a rehearsal here and CI
run the same commands: on the host a step is wrapped in a container run (CI's
runner is such a host, `PGDT_RELEASE_CONTAINER=docker`; a `container:` job
takes an image reference and the image is never pushed), and `--inside` runs
it as it stands, which is what the image itself does.

**The image is tagged by its inputs** -- the Dockerfile and the two files it
copies, `rust-toolchain.toml` and `mise.toml`, the Dockerfile copying itself
as well -- so a changed pin builds a new image and an unchanged one is
reused, never a stale image under a name that outlived what built it.
`--inside` refuses an image built from other copies of the three than the
tree's.

**Every cargo target directory a step builds into is keyed on what compiles
objects** (`image_key`), read inside the image: the base digest, rustc's
version and installed components, each C compiler's version and each
target's libc-dev's, and the build's clock. One holding another key, or none,
is cleared before the step runs, and what moved is logged (`key_target`), so
no object another image compiled reaches a release or a figure
(`docs/design/decisions.md`, "D104"): cargo's own fingerprint misses these
inputs.

**Every step builds under one clock**, `SOURCE_DATE_EPOCH` set to the constant
`CLOCK` inside the image (`ready_inside`), so what a C compiler stamps with
`__DATE__` and `__TIME__` is the same in every build, a rehearsal's and CI's
alike, and the two make one commit's x86-64 archive byte for byte
(`docs/design/decisions.md`, "D109").

**Every binary `build` or `bench` makes is held to the floor** (`check_floor`): no `GLIBC_`
version it needs may be past the glibc of the image's `libc6-dev` its target
links against. The link is against that glibc, so the check holds by
construction; it is the guard that says so, and the one a host build fails.
**And every `build` is refused where a crate `pgdt` depends on was built
under CMake** (`cmake_caches`): the image has it, `mise.toml` pinning it for
the measurement apparatus, and `aws-lc-sys` must keep its `cc` builder (D111).

On the host each run is the invoking user's, under a memory limit, with the
build's state under `target/release-image/` -- its cargo target directory,
`CARGO_HOME` and `uv`'s cache and environment -- never the host build's, which
another compiler's C and another glibc fill.

**The workflows run these steps** (`.github/workflows/`): a job on a runner
runs `release.py <step>` as a host does, building the image from the
Dockerfile there. `build --release` is the one release-only step: it hands the
container the workspace version as `RELEASE_VARIABLE`, the only thing of the
host's a build is given. `suite` builds and runs the round's tests, less the
measurement apparatus (D110), for the architecture the image is on, so an
arm64 host's image is how the second target's code runs its tests: nothing of
the suite crosses between hosts. What becomes of the output -- the archives,
the checksums, the draft, the publish -- is `scripts/release_ci.py`.

**The register builds here too** (`scripts/measure.py`): the binary it times as
the shipped one is `build --target x86_64-unknown-linux-gnu`'s, a release's
build but for `--release`'s variable, which only `--version` reads; each
variant it builds beside that one -- an allocator leg, the instrument (never
timed), the `xz_decode` and `gz_read` examples, the `gzip-inflate` member, a
`release-profile` leg -- is this recipe with only its
package, example, features, profile or target directory changed (`Build`), so
two binaries a figure compares differ by what the figure names.
A `cargo bench` a figure runs runs here as well (`Bench`), on the same compiler
and glibc, criterion's output landing in the release state's target directory.

**So does the round's check, its cargo side** (`scripts/check.py`, "The cargo
checks run in the release image"): `run` runs one command, from the directory
it names, under the same limit and in the same state as `suite`.

**Each archive's `THIRD-PARTY-NOTICES` is made here too** (`step_notices`):
`cargo-about`, pinned in `mise.toml`, reads every crate's licence under
`release/about.toml`'s allow-list, and this script cuts that reading to what
the target's `pgdt` links and adds what `cargo-about` does not carry.

Usage:

    cd scripts && uv run release.py image                 # build the image, unless built
    cd scripts && uv run release.py build [--target T]    # release-build pgdt, each held to the floor
    cd scripts && uv run release.py build --no-default-features --features system --target-dir D
    cd scripts && uv run release.py build --profile release-fat --target-dir D
    cd scripts && uv run release.py build --release       # a release's build: names the version
    cd scripts && uv run release.py suite                 # the host-native suite, in the image
    cd scripts && uv run release.py run --cwd=scripts -- uv run module_graph.py   # one command, in the image
    cd scripts && uv run release.py notices [--target T]  # each target's THIRD-PARTY-NOTICES, beside its binary
    cd scripts && uv run release.py bench --package pgdt-codec --bench decoders --filter nested
    cd scripts && uv run release.py floor <binary> --floor 2.41   # the floor check alone
    python3 scripts/release.py build --inside             # a step, already in the image
    cd scripts && uv run python -m unittest test_release
"""

from __future__ import annotations

import argparse
import dataclasses
import fcntl
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import textwrap
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Mapping, Sequence

import check

REPO = Path(__file__).resolve().parent.parent
DOCKERFILE = REPO / "release" / "Dockerfile"
#: What the Dockerfile copies into the image, from the repository root, and
#: where the image keeps them (`--inside`'s guard compares the two). It keeps
#: a copy of itself there too, which is where the base digest is read from
#: inside (`image_key`).
CONTEXT_FILES = ("rust-toolchain.toml", "mise.toml")
IN_IMAGE_COPIES = Path("/opt/toolchain")
IN_IMAGE_DOCKERFILE = IN_IMAGE_COPIES / "Dockerfile"
IMAGE_NAME = "pgdt-release"

#: Each release target, and the Debian package carrying the glibc it links
#: against in the image -- the floor its binary is held to.
TARGETS: dict[str, str] = {
    "x86_64-unknown-linux-gnu": "libc6-dev",
    "aarch64-unknown-linux-gnu": "libc6-dev-arm64-cross",
}
#: The one target an x86-64 image runs: the other is cross-built there.
NATIVE_TARGET = "x86_64-unknown-linux-gnu"
#: The target the x86-64 image cross-builds, and the one an arm64 host is.
CROSS_TARGET = "aarch64-unknown-linux-gnu"
#: The variable `pgdt` reads to know it is a release's build, which must name
#: the workspace version (`crates/pgdt/src/version.rs`, `RELEASED`).
RELEASE_VARIABLE = "PGDT_RELEASE_VERSION"
#: The clock every step builds under (`ready_inside`): the time CI's v0.2.0
#: build read, 2026-10-21 05:11:04 UTC, so that build is reproducible here. A
#: constant, not the commit's time: no build script declares the variable to
#: cargo, so a C object keeps the clock it was first compiled under for as
#: long as its target directory's key holds (D109).
CLOCK_VARIABLE = "SOURCE_DATE_EPOCH"
CLOCK = 1792559464

CONTAINER = os.environ.get("PGDT_RELEASE_CONTAINER", "sudo nerdctl")
#: The memory limit of every container run that builds or tests: a
#: whole-workspace test build links dozens of binaries at once, and this
#: machine's rule is a limit on every such run. The smoke run alone has none
#: (`release_ci.py`, `smoke_argv`).
#: It is above an arm64 runner's memory, so running out there loses the runner
#: rather than killing `ld`; nothing has read that job's time, memory or disk,
#: and a runner that cannot hold the build wants a job cap or a lighter test
#: profile, measured first.
MEMORY = os.environ.get("PGDT_RELEASE_MEMORY", "20g")
#: Physical cores a container run leaves the host, each with all its SMT
#: siblings: a whole-workspace build or suite otherwise takes every CPU, and
#: the machine stops answering until it ends. A timed run is the exception and
#: takes them all (`Bench`). A CI runner is the build's alone and sets 0.
RESERVE_CORES = int(os.environ.get("PGDT_RELEASE_RESERVE_CORES", "1"))
CPU_SYSFS = Path("/sys/devices/system/cpu")
#: The build's state on the host. Under the repository's `target/`, so it is
#: ignored and lives wherever that directory does.
STATE = Path(os.environ.get("PGDT_RELEASE_STATE", str(REPO / "target" / "release-image")))
#: Where that state is mounted in the image, and the variables naming it.
MOUNTS: dict[str, tuple[str, dict[str, str]]] = {
    "target": ("/state/target", {"CARGO_TARGET_DIR": "/state/target"}),
    "cargo-home": ("/state/cargo-home", {"CARGO_HOME": "/state/cargo-home"}),
    # The suite's `uv run`s: a cache, and an environment kept out of
    # `scripts/.venv`, which is the host's.
    "uv": (
        "/state/uv",
        {"UV_CACHE_DIR": "/state/uv/cache", "UV_PROJECT_ENVIRONMENT": "/state/uv/venv"},
    ),
}
WORKDIR = "/work"

FROM_RE = re.compile(r"^FROM\s+(\S+)\s*$", re.MULTILINE)
DIGEST_RE = re.compile(r"^[a-z0-9./-]+:[a-z0-9.-]+@sha256:[0-9a-f]{64}$")


class ReleaseError(Exception):
    pass


# --- the image -------------------------------------------------------------


def pinned_base(dockerfile: str) -> str:
    """The one `FROM` reference, which must name its image by digest."""
    refs = FROM_RE.findall(dockerfile)
    if len(refs) != 1:
        raise ReleaseError(f"the Dockerfile has {len(refs)} FROM lines, not one")
    if not DIGEST_RE.match(refs[0]):
        raise ReleaseError(f"the Dockerfile's base {refs[0]} is not pinned by digest")
    return refs[0]


def image_tag(inputs: dict[str, bytes]) -> str:
    """`pgdt-release:<digest of the inputs>`, each input named and sized so
    no two sets of files share one."""
    h = hashlib.sha256()
    for name in sorted(inputs):
        data = inputs[name]
        h.update(f"{name}\0{len(data)}\0".encode())
        h.update(data)
    return f"{IMAGE_NAME}:{h.hexdigest()[:16]}"


def image_inputs(repo: Path = REPO) -> dict[str, bytes]:
    inputs = {"Dockerfile": (repo / "release" / "Dockerfile").read_bytes()}
    for name in CONTEXT_FILES:
        inputs[name] = (repo / name).read_bytes()
    return inputs


def ensure_image(container: Sequence[str]) -> str:
    """The image's tag, built first if no image carries it."""
    inputs = image_inputs()
    pinned_base(inputs["Dockerfile"].decode())
    tag = image_tag(inputs)
    have = subprocess.run(
        [*container, "image", "inspect", tag], capture_output=True, text=True
    )
    if have.returncode == 0:
        print(f"release image {tag} is built", file=sys.stderr)
        return tag
    with tempfile.TemporaryDirectory(prefix="pgdt-release-context-") as ctx:
        for name, data in inputs.items():
            (Path(ctx) / name).write_bytes(data)
        print(f"building release image {tag}", file=sys.stderr)
        subprocess.run([*container, "build", "-t", tag, ctx], check=True)
    return tag


#: Repository extensions a checkout may carry that the image's git cannot
#: read. `relativeworktrees` is what `worktree.useRelativePaths` writes, and
#: names only how linked worktrees find the repository.
UNREADABLE_EXTENSIONS = ("extensions.relativeworktrees",)


def git_view(state: Path = STATE, repo: Path = REPO) -> Path | None:
    """A copy of the checkout's `.git` the image's git can read, mounted over
    the original, or None where it reads the original.

    The suite asks git about this repository's history (`scripts/measure.py`'s
    commit checks), and a checkout carrying an extension newer than the image's
    git fails every such question. The copy drops the extension and nothing
    else, so it names the same commits, refs and index; the original is never
    touched."""
    config = repo / ".git" / "config"
    present = [
        key for key in UNREADABLE_EXTENSIONS
        if subprocess.run(
            ["git", "config", "--file", str(config), "--get", key], capture_output=True
        ).returncode == 0
    ]  # fmt: skip
    if not present:
        return None
    view = state / "git"
    shutil.rmtree(view, ignore_errors=True)
    shutil.copytree(repo / ".git", view, symlinks=True)
    for key in present:
        subprocess.run(["git", "config", "--file", str(view / "config"), "--unset", key], check=True)
    return view


def state_dirs(state: Path = STATE, target: Path | None = None) -> dict[str, Path]:
    """Each mount's host directory: `state`'s, the target directory replaced
    by `target` where one is given."""
    dirs = {name: state.resolve() / name for name in MOUNTS}
    if target is not None:
        dirs["target"] = target.resolve()
    return dirs


def prepare_state(state: Path = STATE, target: Path | None = None) -> None:
    """Create every directory a step mounts, so the runtime does not create
    one as root."""
    for path in state_dirs(state, target).values():
        path.mkdir(parents=True, exist_ok=True)


def parse_cpu_list(text: str) -> frozenset[int]:
    """A kernel CPU list (`0-2,12-14`) as the CPUs it names."""
    cpus: set[int] = set()
    for part in text.strip().split(","):
        if not part:
            continue
        lo, _, hi = part.partition("-")
        cpus.update(range(int(lo), int(hi or lo) + 1))
    return frozenset(cpus)


def cpu_list(cpus: Iterable[int]) -> str:
    """CPUs as a kernel CPU list, runs collapsed, which `--cpuset-cpus` takes."""
    out: list[str] = []
    ordered = sorted(cpus)
    i = 0
    while i < len(ordered):
        j = i
        while j + 1 < len(ordered) and ordered[j + 1] == ordered[j] + 1:
            j += 1
        out.append(str(ordered[i]) if i == j else f"{ordered[i]}-{ordered[j]}")
        i = j + 1
    return ",".join(out)


def physical_cores(cpus: frozenset[int], root: Path = CPU_SYSFS) -> list[frozenset[int]]:
    """The physical cores among `cpus`, each as its SMT siblings in `cpus`, by
    lowest CPU. A CPU whose siblings sysfs does not say is a core of its own."""
    cores: dict[int, frozenset[int]] = {}
    for cpu in cpus:
        path = root / f"cpu{cpu}" / "topology" / "thread_siblings_list"
        siblings = parse_cpu_list(path.read_text()) & cpus if path.is_file() else frozenset({cpu})
        cores[min(siblings)] = siblings
    return [cores[k] for k in sorted(cores)]


def run_cpus(
    reserve: int, cpus: frozenset[int] | None = None, root: Path = CPU_SYSFS
) -> frozenset[int] | None:
    """The CPUs a container run is given: `cpus`, the host's affinity unless
    named, less its last `reserve` physical cores whole. `None` is every CPU,
    stated by no cpuset: nothing reserved, or no core left to run on once it
    is."""
    if reserve <= 0 or (cpus is None and not hasattr(os, "sched_getaffinity")):
        return None
    cores = physical_cores(frozenset(os.sched_getaffinity(0)) if cpus is None else cpus, root)
    if len(cores) <= reserve:
        return None
    return frozenset().union(*cores[:-reserve])


def run_in_image(
    container: Sequence[str],
    tag: str,
    step: Sequence[str],
    *,
    state: Path = STATE,
    target: Path | None = None,
    memory: str = MEMORY,
    reserve: int = RESERVE_CORES,
    uid: int | None = None,
    gid: int | None = None,
    git: Path | None = None,
    env: Mapping[str, str] = {},
    tree: Path = REPO,
) -> list[str]:
    """The container run that executes `release.py <step> --inside` in the
    image, as the invoking user, the tree mounted at `WORKDIR`. `env` is the
    host's only say in the container's environment.

    `tree` is the checkout mounted, the repository's own unless another is
    named: a worktree of another commit, built at the same path in the image
    so the two builds differ by their sources alone (`scripts/measure.py`,
    `cmd_count_pair`). Its own `release.py` runs the step.

    `target` replaces `state`'s cargo target directory, `CARGO_HOME` staying
    shared: a build of other features writes the same output path, so one
    whose binary must survive beside the release's gets a directory of its
    own.

    `reserve` is how many physical cores the run leaves the host
    (`RESERVE_CORES`, `run_cpus`); a timed run passes 0.

    **Seccomp is unconfined**: the suite runs `pgdt` as a user namespace's
    init (`crates/pgdt/tests/namespace_init.rs`), which the runtime's default
    profile refuses to create."""
    uid = os.getuid() if uid is None else uid
    gid = os.getgid() if gid is None else gid
    argv = [
        *container, "run", "--rm",
        "-m", memory, "--memory-swap", memory,
        "--security-opt", "seccomp=unconfined",
        "--user", f"{uid}:{gid}",
        "-e", "HOME=/tmp",
        "-v", f"{tree}:{WORKDIR}",
        "-w", WORKDIR,
    ]  # fmt: skip
    cpus = run_cpus(reserve)
    if cpus is not None:
        argv += ["--cpuset-cpus", cpu_list(cpus)]
    if git is not None:
        argv += ["-v", f"{git.resolve()}:{WORKDIR}/.git"]
    for key, value in env.items():
        argv += ["-e", f"{key}={value}"]
    dirs = state_dirs(state, target)
    for name, (at, env) in MOUNTS.items():
        argv += ["-v", f"{dirs[name]}:{at}"]
        for key, value in env.items():
            argv += ["-e", f"{key}={value}"]
    # `--inside` goes before a `--`, past which a `run` step's command is.
    at = step.index("--") if "--" in step else len(step)
    return [*argv, tag, "python3", "scripts/release.py", *step[:at], "--inside", *step[at:]]


def check_inside(copies: Path = IN_IMAGE_COPIES, repo: Path = REPO) -> None:
    """Refuse to run a step in an image built from other copies of
    `CONTEXT_FILES`, or of the Dockerfile, than the tree's."""
    pairs = [(name, repo / name) for name in CONTEXT_FILES]
    pairs.append(("Dockerfile", repo / "release" / "Dockerfile"))
    for name, theirs in pairs:
        have = copies / name
        if not have.is_file():
            raise ReleaseError(f"--inside, but {have} is absent: this is not the release image")
        if have.read_bytes() != theirs.read_bytes():
            raise ReleaseError(
                f"the image was built from another {name} than the tree's; rebuild it "
                "(`cd scripts && uv run release.py image`)"
            )


# --- the target directory's key --------------------------------------------

#: The file in a cargo target directory naming what compiled its objects, and
#: the lock every step holds on it while it builds there.
TARGET_KEY = "pgdt-image-key"
TARGET_LOCK = ".pgdt-image-key.lock"


def image_key(
    dockerfile: str,
    rustc: str,
    components: str,
    compilers: Mapping[str, str],
    libcs: Mapping[str, str],
    clock: int = CLOCK,
) -> str:
    """What compiles objects in an image, one line each: the base it is built
    from, `rustc -vV`, the sysroot's installed components, each C compiler's
    `--version` line by its name, each libc-dev package's version, and the
    clock a step builds under; not the image's tag
    (`docs/design/decisions.md`, "D104")."""
    lines = [f"base {pinned_base(dockerfile)}"]
    lines += [f"rustc {line.strip()}" for line in rustc.splitlines() if line.strip()]
    lines += [f"component {c}" for c in sorted(set(components.split()))]
    lines += [f"compiler {name}: {version}" for name, version in sorted(compilers.items())]
    lines += [f"libc {package} {version}" for package, version in sorted(libcs.items())]
    lines.append(f"clock {CLOCK_VARIABLE}={clock}")
    return "\n".join(lines) + "\n"


def read_image_key(dockerfile: Path = IN_IMAGE_DOCKERFILE) -> str:
    """`image_key` of the image this runs in: the base its own copy of the
    Dockerfile names, `rustc` as the tree's toolchain pin resolves it, `cc`
    and the cross target's compiler where the image has one, and each
    target's libc-dev package that is installed."""

    def out(argv: Sequence[str]) -> subprocess.CompletedProcess:
        return subprocess.run(argv, cwd=REPO, capture_output=True, text=True)

    if not dockerfile.is_file():
        raise ReleaseError(f"--inside, but {dockerfile} is absent: rebuild the release image")
    rustc = out(["rustc", "-vV"])
    sysroot = out(["rustc", "--print", "sysroot"])
    if rustc.returncode or sysroot.returncode:
        raise ReleaseError(f"rustc did not say what it is: {rustc.stderr.strip() or sysroot.stderr.strip()}")
    components = Path(sysroot.stdout.strip()) / "lib" / "rustlib" / "components"
    compilers = {}
    for name in ("cc", os.environ.get(f"CC_{CROSS_TARGET.replace('-', '_')}")):
        if name and shutil.which(name):
            compilers[name] = (out([name, "--version"]).stdout.splitlines() or [""])[0].strip()
    libcs = {}
    for package in sorted(set(TARGETS.values())):
        got = out(["dpkg-query", "-W", "-f", "${Version}", package])
        if got.returncode == 0 and got.stdout.strip():
            libcs[package] = got.stdout.strip()
    return image_key(dockerfile.read_text(), rustc.stdout, components.read_text(), compilers, libcs)


def clear_target(directory: Path) -> int:
    """Remove everything in `directory` but its key and lock, leaving the
    directory itself, which a container run mounts. How many entries went."""
    gone = 0
    for entry in directory.iterdir():
        if entry.name in (TARGET_KEY, TARGET_LOCK):
            continue
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()
        gone += 1
    return gone


def key_moves(held: str | None, key: str) -> list[str]:
    """The lines saying why a directory keyed `held` is cleared for `key`."""
    if held is None:
        return ["  it named no image"]
    old, new = held.splitlines(), key.splitlines()
    return [f"  - {line}" for line in old if line not in new] + [f"  + {line}" for line in new if line not in old]


def key_target(directory: Path, key: str) -> int:
    """Hold `directory` to `key`, clearing it first where it holds another
    or none, and return the lock held on it: shared, for as long as the step
    builds there, so another image's step cannot clear it meanwhile. The
    check is made again under each lock taken, a step of another image
    clearing it between the two."""
    directory.mkdir(parents=True, exist_ok=True)
    keyfile = directory / TARGET_KEY
    fd = os.open(directory / TARGET_LOCK, os.O_RDWR | os.O_CREAT, 0o644)

    def held() -> str | None:
        return keyfile.read_text() if keyfile.is_file() else None

    while True:
        fcntl.flock(fd, fcntl.LOCK_SH)
        if held() == key:
            return fd
        fcntl.flock(fd, fcntl.LOCK_EX)
        before = held()
        if before == key:
            continue
        gone = clear_target(directory)
        written = directory / f"{TARGET_KEY}.new"
        written.write_text(key)
        written.replace(keyfile)
        if gone or before is not None:
            print(
                f"release.py: cleared {directory} ({gone} entries), keyed to another image:",
                *key_moves(before, key), sep="\n", file=sys.stderr, flush=True,
            )  # fmt: skip


def ready_inside() -> int:
    """What every step checks in the image before it runs: that the image is
    the tree's (`check_inside`), and that the target directory
    `CARGO_TARGET_DIR` names, which a container run always does, is keyed to
    it (`key_target`). The lock it returns is held until the step exits.
    Every command the step runs inherits `CLOCK`, whatever clock it was
    handed."""
    check_inside()
    named = os.environ.get("CARGO_TARGET_DIR")
    if not named:
        raise ReleaseError("--inside, but no CARGO_TARGET_DIR names the target directory to key")
    os.environ[CLOCK_VARIABLE] = str(CLOCK)
    return key_target(Path(named), read_image_key())


# --- the floor -------------------------------------------------------------

VERSION_NEEDS_RE = re.compile(r"^Version needs section '\.gnu\.version_r'")
NEED_FILE_RE = re.compile(r"\sFile:\s+(\S+)")
NEED_NAME_RE = re.compile(r"\sName:\s+(\S+)")
SYMBOL_RE = re.compile(r"\s(\S+?)@(GLIBC_\S+?)(?:\s|$)")
GLIBC_RE = re.compile(r"^GLIBC_(\d+(?:\.\d+)+)$")
DEB_VERSION_RE = re.compile(r"^(?:\d+:)?(\d+)\.(\d+)")


@dataclass(frozen=True)
class Need:
    """One `GLIBC_` version a binary needs, from one library."""

    file: str
    name: str

    @property
    def version(self) -> tuple[int, ...] | None:
        """None for a name that is no release, `GLIBC_PRIVATE`."""
        m = GLIBC_RE.match(self.name)
        return tuple(int(p) for p in m.group(1).split(".")) if m else None


def glibc_needs(readelf: str) -> list[Need]:
    """Every `GLIBC_*` name in `readelf --version-info`'s version-needs
    section, with the library naming it. Another library's names
    (`libgcc_s`'s `GCC_*`) are not glibc's and are left out."""
    needs: list[Need] = []
    inside = False
    current: str | None = None
    for line in readelf.splitlines():
        if VERSION_NEEDS_RE.match(line):
            inside = True
            continue
        if not inside:
            continue
        if not line.strip():
            if needs or current:
                break
            continue
        if m := NEED_FILE_RE.search(line):
            current = m.group(1)
        elif (m := NEED_NAME_RE.search(line)) and m.group(1).startswith("GLIBC_"):
            if current is None:
                raise ReleaseError(f"a version need before any File: line: {line.strip()}")
            needs.append(Need(current, m.group(1)))
    return needs


def symbols_by_version(readelf: str) -> dict[str, list[str]]:
    """Each `GLIBC_*` name, and the dynamic symbols bound to it."""
    out: dict[str, list[str]] = {}
    for line in readelf.splitlines():
        if " UND " not in line:
            continue
        if m := SYMBOL_RE.search(line):
            out.setdefault(m.group(2), []).append(m.group(1))
    return out


def deb_glibc(version: str) -> tuple[int, int]:
    """The glibc release a Debian package version carries: `2.41-12+deb13u4`
    and `2.41-11cross1` are both 2.41."""
    m = DEB_VERSION_RE.match(version.strip())
    if not m:
        raise ReleaseError(f"no glibc release in the package version {version!r}")
    return int(m.group(1)), int(m.group(2))


def parse_floor(text: str) -> tuple[int, ...]:
    if not re.fullmatch(r"\d+(\.\d+)+", text):
        raise ReleaseError(f"a floor is a glibc release such as 2.41, not {text!r}")
    return tuple(int(p) for p in text.split("."))


def check_floor(readelf: str, floor: tuple[int, ...]) -> tuple[bool, list[str]]:
    """Whether a binary needs no `GLIBC_` version past `floor`, and the lines
    saying so: the highest it needs, or each version past the floor with its
    library and the symbols bound to it."""
    needs = glibc_needs(readelf)
    if not needs:
        return False, ["no GLIBC_ version needs: not a dynamically linked glibc binary"]
    floor_s = ".".join(map(str, floor))
    past = [n for n in needs if n.version is None or n.version > floor]
    if not past:
        top = max(needs, key=lambda n: n.version or ())
        return True, [f"needs {top.name} at most ({top.file}); floor glibc {floor_s}: ok"]
    syms = symbols_by_version(readelf)
    lines = [f"needs past the floor glibc {floor_s}:"]
    for n in sorted(set(past), key=lambda n: (n.version or (999,), n.file)):
        bound = syms.get(n.name, [])
        shown = ", ".join(sorted(bound)[:8]) + (", ..." if len(bound) > 8 else "")
        lines.append(f"  {n.name} ({n.file}): {shown or 'no dynamic symbol named'}")
    return False, lines


def readelf_of(binary: Path) -> str:
    return subprocess.run(
        ["readelf", "--version-info", "--dyn-syms", "--wide", str(binary)],
        check=True, capture_output=True, text=True,
    ).stdout  # fmt: skip


def image_floor(target: str) -> tuple[int, int]:
    """The glibc `target` links against in the image, by its package."""
    package = TARGETS[target]
    version = subprocess.run(
        ["dpkg-query", "-W", "-f", "${Version}", package],
        check=True, capture_output=True, text=True,
    ).stdout  # fmt: skip
    return deb_glibc(version)


# --- the steps, in the image -----------------------------------------------


def target_dir() -> Path:
    return Path(os.environ.get("CARGO_TARGET_DIR", str(REPO / "target")))


@dataclass(frozen=True)
class Build:
    """One `cargo build` in the image, for one target.

    `Build(target)` is a release's own: `pgdt` at its default features, under
    `release`. Every other value is a variant the register times beside it,
    which shares the image, the compiler, `--locked` and the explicit
    `--target` with it, so it differs from that build by its package,
    example, features and profile alone."""

    target: str = NATIVE_TARGET
    package: str = "pgdt"
    #: An example of `package` to build in place of its binary.
    example: str | None = None
    features: tuple[str, ...] = ()
    default_features: bool = True
    #: A Cargo profile inheriting `release`, whose output directory is its
    #: name: a `release-profile` figure leg (`scripts/measure.py`,
    #: `PROFILE_LEGS`).
    profile: str = "release"
    #: `RUSTFLAGS` for this build alone, in a target directory of its own: the
    #: profile recipe's frame pointers, which no Cargo profile key states
    #: (`scripts/measure.py`, `profile_recipe`).
    rustflags: str = ""

    def cargo_argv(self) -> list[str]:
        profile = ["--release"] if self.profile == "release" else ["--profile", self.profile]
        argv = ["cargo", "build", *profile, "--locked", "-p", self.package, "--target", self.target]
        if self.example is not None:
            argv += ["--example", self.example]
        if not self.default_features:
            argv.append("--no-default-features")
        if self.features:
            argv += ["--features", ",".join(self.features)]
        return argv

    def binary(self, target_dir: Path) -> Path:
        """Where the build writes its binary under `target_dir`."""
        out = target_dir / self.target / self.profile
        return out / "examples" / self.example if self.example is not None else out / self.package

    def options(self) -> list[str]:
        """The `build` step's flags naming this build but for its target."""
        opts = [f"--package={self.package}"]
        if self.example is not None:
            opts.append(f"--example={self.example}")
        if not self.default_features:
            opts.append("--no-default-features")
        if self.features:
            opts.append(f"--features={','.join(self.features)}")
        if self.profile != "release":
            opts.append(f"--profile={self.profile}")
        if self.rustflags:
            opts.append(f"--rustflags={self.rustflags}")
        return opts


def build_step(builds: Sequence[Build]) -> list[str]:
    """The `build` step running `builds`, which differ by target alone."""
    if not builds or {dataclasses.replace(b, target=NATIVE_TARGET) for b in builds} != {
        dataclasses.replace(builds[0], target=NATIVE_TARGET)
    }:
        raise ReleaseError("one build step runs one build for each of its targets")
    return ["build", *(f"--target={b.target}" for b in builds), *builds[0].options()]


@dataclass(frozen=True)
class Bench:
    """One `cargo bench` target run in the image, for the native target.

    The register runs one as a figure's program (`scripts/measure.py`,
    `NESTED_BENCH`), so it takes what a `Build` does -- the image's compiler,
    `--locked`, the explicit `--target` -- and shares the release state's
    target directory with the shipped build. criterion writes its estimates
    under that directory (`criterion_root`), which is where a figure reads
    them back on the host."""

    package: str
    bench: str
    #: criterion's filter: which of the bench's benchmark ids run.
    filter: str | None = None

    def cargo_argv(self, *, no_run: bool = False) -> list[str]:
        """The bench run; `no_run` builds it and names its executable on
        stdout instead, for the floor check before it runs."""
        argv = [
            "cargo", "bench", "--locked", "-p", self.package,
            "--target", NATIVE_TARGET, "--bench", self.bench,
        ]  # fmt: skip
        if no_run:
            return [*argv, "--no-run", "--message-format=json-render-diagnostics"]
        return [*argv, "--", self.filter] if self.filter is not None else argv

    def criterion_root(self, target_dir: Path) -> Path:
        """Where criterion writes under `target_dir`: `$CARGO_TARGET_DIR/criterion`,
        which every step's container run sets (`MOUNTS`)."""
        return target_dir / "criterion"

    def step(self) -> list[str]:
        """The `bench` step running this bench."""
        argv = ["bench", f"--package={self.package}", f"--bench={self.bench}"]
        return [*argv, f"--filter={self.filter}"] if self.filter is not None else argv


def bench_executables(messages: str, bench: str) -> list[Path]:
    """The executables `cargo bench --no-run --message-format=json` built for
    the bench target `bench`, out of the JSON messages it printed."""
    found = []
    for line in messages.splitlines():
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = msg.get("target") or {}
        if (
            msg.get("reason") == "compiler-artifact"
            and msg.get("executable")
            and "bench" in target.get("kind", ())
            and target.get("name") == bench
        ):
            found.append(Path(msg["executable"]))
    return found


def run_step(argv: Sequence[str]) -> None:
    print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
    subprocess.run(argv, cwd=REPO, check=True)


#: What a `cmake`-driven build script leaves in its `OUT_DIR`.
CMAKE_CACHE = "CMakeCache.txt"


def package_of(package_id: str) -> Crate:
    """The crate a Package ID Specification names, as cargo's JSON messages
    print one: `<source>#<name>@<version>`, or `<source>#<version>` where the
    name is the source's last path segment."""
    source, hash_, fragment = package_id.rpartition("#")
    if not hash_:
        raise ReleaseError(f"not a package ID specification: {package_id!r}")
    name, at, version = fragment.rpartition("@")
    if not at:
        name, version = source.split("?")[0].rstrip("/").rsplit("/", 1)[-1], fragment
    return Crate(name, version)


def cmake_caches(messages: str, closure: set[Crate]) -> list[tuple[Crate, Path]]:
    """Each `CMakeCache.txt` a build script of a crate in `closure` left in
    its `OUT_DIR`, out of `cargo build --message-format=json`'s messages,
    which name every build script the build ran or found fresh. Only those
    directories are read: another build in the same target directory, the
    measurement apparatus's, may build under CMake."""
    found = []
    for line in messages.splitlines():
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(msg, dict) or msg.get("reason") != "build-script-executed":
            continue
        crate = package_of(msg["package_id"])
        if crate in closure and msg.get("out_dir"):
            found += [(crate, p) for p in sorted(Path(msg["out_dir"]).rglob(CMAKE_CACHE))]
    return found


def step_build(builds: Sequence[Build]) -> int:
    """Each build, its binary held to the floor of the glibc its target links,
    and refused where a crate the shipped package depends on was built under
    CMake, which `mise.toml` pins for the measurement apparatus alone
    (`docs/design/decisions.md`, "D111")."""
    failed = 0
    for build in builds:
        argv = [*build.cargo_argv(), "--message-format=json-render-diagnostics"]
        env = {**os.environ, "RUSTFLAGS": build.rustflags} if build.rustflags else None
        shown = f"RUSTFLAGS={shlex.quote(build.rustflags)} " if build.rustflags else ""
        print(f"$ {shown}{shlex.join(argv)}", file=sys.stderr, flush=True)
        messages = subprocess.run(
            argv, cwd=REPO, check=True, stdout=subprocess.PIPE, text=True, env=env
        ).stdout
        closure = shipped_tree(build.target, "normal", offline=False)
        for crate, cache in cmake_caches(messages, closure):
            failed += 1
            print(
                f"{build.target}: {crate}, which {SHIPPED} depends on, built under CMake: {cache}",
                flush=True,
            )
        binary = build.binary(target_dir())
        ok, lines = check_floor(readelf_of(binary), image_floor(build.target))
        failed += not ok
        print(f"{build.target}: {binary}", flush=True)
        for line in lines:
            print(f"  {line}", flush=True)
    return 1 if failed else 0


def step_bench(bench: Bench) -> int:
    """The bench built, its executable held to the floor, and then run."""
    argv = bench.cargo_argv(no_run=True)
    print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
    built = subprocess.run(argv, cwd=REPO, check=True, capture_output=True, text=True).stdout
    executables = bench_executables(built, bench.bench)
    if not executables:
        raise ReleaseError(f"cargo built no executable for the bench {bench.bench!r}")
    failed = 0
    for binary in executables:
        ok, lines = check_floor(readelf_of(binary), image_floor(NATIVE_TARGET))
        failed += not ok
        print(f"{NATIVE_TARGET}: {binary}", flush=True)
        for line in lines:
            print(f"  {line}", flush=True)
    if failed:
        return 1
    run_step(bench.cargo_argv())
    return 0


# --- the notices -----------------------------------------------------------

ABOUT_CONFIG = REPO / "release" / "about.toml"
NOTICES_NAME = "THIRD-PARTY-NOTICES"
#: The package a release ships, whose linked closure the notices cover.
SHIPPED = "pgdt"
SHIPPED_MANIFEST = REPO / "crates" / SHIPPED / "Cargo.toml"
#: A licence whose duty in executable form includes saying where the source
#: is: each crate under one gets a line naming it.
SOURCE_POINTER_LICENSES = ("MPL-2.0",)
#: The elections a dual licence leaves to us and no crate's declared licence
#: states, by the crate that compiles the component in.
ELECTIONS: dict[str, str] = {
    "zstd-sys": (
        "zstd's C sources, which zstd-sys compiles in, are offered under "
        "BSD-3-Clause or GPL-2.0-only; they are distributed here under "
        "BSD-3-Clause, not GPL-2.0-only."
    ),
}

TREE_LINE_RE = re.compile(r"^(\S+) v(\S+)")


@dataclass(frozen=True, order=True)
class Crate:
    name: str
    version: str

    def __str__(self) -> str:
        return f"{self.name} {self.version}"


def parse_tree(text: str) -> set[Crate]:
    """The crates in `cargo tree --prefix none -f '{p}'`'s output."""
    found = set()
    for line in text.splitlines():
        if m := TREE_LINE_RE.match(line.strip()):
            found.add(Crate(m.group(1), m.group(2)))
    return found


def shipped_tree(target: str, edges: str, *, offline: bool) -> set[Crate]:
    """The shipped package's dependencies for `target` along `edges`, at its
    own features: `cargo tree` resolves them as `cargo build -p pgdt` does,
    where `cargo metadata` resolves the whole workspace's at once."""
    out = subprocess.run(
        [
            "cargo", "tree", "--locked", *(["--offline"] if offline else []),
            "-p", SHIPPED, "--target", target,
            "-e", edges, "--prefix", "none", "-f", "{p}",
        ],
        cwd=REPO, check=True, stdout=subprocess.PIPE, text=True,
    ).stdout  # fmt: skip
    return parse_tree(out)


def linked_closure(target: str) -> set[Crate]:
    """Every crate the shipped binary links for `target`: its normal
    dependencies at its own features, a proc macro and what only it uses
    left out, since none of their code is in the binary. `cargo-about` reads
    the whole graph a build resolves, and this is what its output is cut to."""
    return shipped_tree(target, "normal,no-proc-macro", offline=True)


def workspace_crates() -> tuple[set[Crate], str]:
    """The workspace's members, which are ours rather than third parties',
    and the shipped package's version."""
    out = subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--no-deps", "--format-version", "1"],
        cwd=REPO, check=True, stdout=subprocess.PIPE, text=True,
    ).stdout  # fmt: skip
    meta = json.loads(out)
    members = {Crate(p["name"], p["version"]) for p in meta["packages"]}
    version = next(p["version"] for p in meta["packages"] if p["name"] == SHIPPED)
    return members, version


def about_json(target: str, config: Path = ABOUT_CONFIG) -> dict:
    """`cargo-about`'s reading of the shipped package's graph for `target`,
    as JSON. `--fail` makes a licence the configuration does not accept a
    failed run rather than a warning (a clarification whose files moved is
    still a warning: `check_clarified`), and `--offline` reads only what the
    crates' sources carry, so one lockfile gives one reading."""
    out = subprocess.run(
        [
            "cargo-about", "generate", "--fail", "--format", "json", "--locked", "--offline",
            "-m", str(SHIPPED_MANIFEST), "--target", target, "-c", str(config),
        ],
        cwd=REPO, check=True, stdout=subprocess.PIPE, text=True,
    ).stdout  # fmt: skip
    return json.loads(out)


@dataclass
class Text:
    """One licence or `NOTICE` text, the licences it was reproduced under and
    the crates it was reproduced for."""

    text: str
    licenses: set[str] = dataclasses.field(default_factory=set)
    crates: set[Crate] = dataclasses.field(default_factory=set)


def licence_texts(about: dict, closure: set[Crate]) -> tuple[list[Text], dict[Crate, set[str]]]:
    """`cargo-about`'s licence texts cut to `closure`, one entry per distinct
    text, and the licences each crate is distributed under.

    `cargo-about` keeps a text once per licence it was reproduced under, so a
    file a clarification names under several (an attribution file) comes
    back once for each; here it is one entry. **A crate in `closure` with no
    text is refused**: the closure is what ships, and a crate it holds that
    the reading missed would ship unattributed."""
    by_text: dict[str, Text] = {}
    licences: dict[Crate, set[str]] = {}
    for lic in about["licenses"]:
        users = {Crate(u["crate"]["name"], u["crate"]["version"]) for u in lic["used_by"]} & closure
        if not users:
            continue
        entry = by_text.setdefault(lic["text"], Text(lic["text"]))
        entry.licenses.add(lic["id"])
        entry.crates |= users
        for crate in users:
            licences.setdefault(crate, set()).add(lic["id"])
    missing = sorted(closure - licences.keys())
    if missing:
        raise ReleaseError(
            "cargo-about gave no licence text for " + ", ".join(map(str, missing))
            + f": each crate {SHIPPED} links must have one"
        )
    return sorted(by_text.values(), key=lambda t: (sorted(t.licenses), min(t.crates))), licences


def check_clarified(config: dict, dirs: dict[Crate, Path], texts: Sequence[Text]) -> None:
    """Refuse notices missing a file a clarification names.

    `cargo-about` drops a clarification whose checksum no longer matches with
    a warning, `--fail` or not, and falls back to the crate's declared
    licence, which is exactly what each clarification is there to correct;
    so every linked crate's clarified files must be among its texts. A file
    is compared whole, so a clarification may not cut one (`start`, `end`)."""
    for crate, root in sorted(dirs.items()):
        clarify = config.get(crate.name, {}).get("clarify")
        if clarify is None:
            continue
        held = {t.text for t in texts if crate in t.crates}
        for file in clarify.get("files", []):
            if "start" in file or "end" in file:
                raise ReleaseError(f"{crate}'s clarification cuts {file['path']}; the recipe compares whole files")
            if (root / file["path"]).read_text() not in held:
                raise ReleaseError(
                    f"{crate}'s {file['path']} is not in its notices: its clarification no longer "
                    f"applies (a checksum moved at an upgrade?); re-read the file and re-clarify it "
                    f"in {ABOUT_CONFIG.relative_to(REPO)}"
                )


def crate_dirs(about: dict) -> dict[Crate, Path]:
    """Each crate's source directory, from its manifest path."""
    return {
        Crate(c["package"]["name"], c["package"]["version"]): Path(c["package"]["manifest_path"]).parent
        for c in about["crates"]
    }


def notice_texts(dirs: dict[Crate, Path], closure: set[Crate]) -> list[Text]:
    """Every `NOTICE` file at the root of a crate in `closure`, one entry per
    distinct text: Apache-2.0's section 4(d) asks a redistribution to carry
    each one's attribution notices."""
    by_text: dict[str, Text] = {}
    for crate in sorted(closure):
        root = dirs.get(crate)
        if root is None:
            continue
        for path in sorted(root.iterdir()):
            if path.is_file() and path.name.upper().startswith("NOTICE"):
                text = path.read_text(errors="replace")
                by_text.setdefault(text, Text(text)).crates.add(crate)
    return sorted(by_text.values(), key=lambda t: min(t.crates))


def source_pointers(about: dict, licences: dict[Crate, set[str]]) -> list[str]:
    """A line saying where the source is, for each crate distributed under a
    licence asking for one."""
    packages = {Crate(c["package"]["name"], c["package"]["version"]): c["package"] for c in about["crates"]}
    lines = []
    for crate in sorted(licences):
        held = sorted(set(SOURCE_POINTER_LICENSES) & licences[crate])
        if not held:
            continue
        line = f"{crate} ({', '.join(held)}): https://crates.io/crates/{crate.name}/{crate.version}"
        if repo := packages.get(crate, {}).get("repository"):
            line += f", repository {repo}"
        lines.append(line)
    return lines


def elections(closure: set[Crate]) -> list[str]:
    """The elections, each refused where its crate is no longer linked: a
    stale one would be a statement about a component nobody ships."""
    linked = {c.name for c in closure}
    stale = sorted(set(ELECTIONS) - linked)
    if stale:
        raise ReleaseError(f"an election names a crate {SHIPPED} no longer links: {', '.join(stale)}")
    return [ELECTIONS[name] for name in sorted(ELECTIONS)]


WIDTH = 78
RULE = "=" * WIDTH
THIN = "-" * WIDTH


def render_notices(
    version: str,
    target: str,
    licences: dict[Crate, set[str]],
    texts: Sequence[Text],
    notices: Sequence[Text],
    pointers: Sequence[str],
    chosen: Sequence[str],
) -> str:
    """The `THIRD-PARTY-NOTICES` file: the components and their licences, the
    elections, where source is owed, every licence text and every `NOTICE`."""

    def heading(title: str) -> list[str]:
        return ["", RULE, title, RULE, ""]

    def para(text: str) -> list[str]:
        return textwrap.wrap(text, WIDTH, break_on_hyphens=False) + [""]

    def label(prefix: str, cs: set[Crate]) -> list[str]:
        return textwrap.wrap(f"{prefix} {', '.join(map(str, sorted(cs)))}", WIDTH, break_on_hyphens=False)

    out = [
        f"Third-party notices for {SHIPPED} {version}, {target}",
        "",
        f"{SHIPPED} is licensed under the Apache License, Version 2.0; its text is",
        "LICENSE, beside this file. It is built from the third-party components",
        "listed below, each distributed under the licence or licences named for",
        "it, whose texts follow. Where a component offers a choice of licences,",
        "it is distributed under the one named for it here.",
    ]
    out += heading("Components")
    out += [f"{crate}: {' AND '.join(sorted(licences[crate]))}" for crate in sorted(licences)]
    out += heading("Elections")
    for line in chosen or ["None."]:
        out += para(line)
    out += heading("Source code")
    out += para(
        "The source of each component below is available where its entry says, "
        "under the licence named in it."
    )
    for line in pointers or ["None."]:
        out += para(line)
    out += heading("Licence texts")
    for t in texts:
        out += [THIN, *label(f"{' / '.join(sorted(t.licenses))}, for:", t.crates), THIN, "", t.text.strip("\n"), ""]
    out += heading("NOTICE files")
    for t in notices:
        out += [THIN, *label("NOTICE of:", t.crates), THIN, "", t.text.strip("\n"), ""]
    return "\n".join(out).rstrip("\n") + "\n"


def notices_path(target: str, target_dir: Path) -> Path:
    """Where the notices for `target` are written: beside its release binary."""
    return target_dir / target / "release" / NOTICES_NAME


def step_notices(targets: Sequence[str]) -> int:
    """Each target's `THIRD-PARTY-NOTICES`, written beside its binary.

    Every package the lockfile names is fetched first: `cargo-about` reads
    the graph through `cargo metadata`, which wants the sources of every
    target's and every member's dependencies, not only what a build fetched.
    Nothing else here touches the network.

    Rendered here from `cargo-about`'s JSON, which decides the licences and
    harvests the texts. Rejected: a `cargo-about` template, which can only
    render what it read, proc macros included, no `NOTICE` file, and a
    clarification whose checksum moved silently dropped."""
    run_step(["cargo", "fetch", "--locked"])
    members, version = workspace_crates()
    for target in targets:
        closure = linked_closure(target) - members
        about = about_json(target)
        texts, licences = licence_texts(about, closure)
        dirs = {c: d for c, d in crate_dirs(about).items() if c in closure}
        check_clarified(tomllib.loads(ABOUT_CONFIG.read_text()), dirs, texts)
        out = render_notices(
            version, target, licences, texts,
            notice_texts(dirs, closure),
            source_pointers(about, licences), elections(closure),
        )  # fmt: skip
        path = notices_path(target, target_dir())
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(out)
        print(f"{target}: {path} ({len(licences)} components)", flush=True)
    return 0


def workspace_version(repo: Path = REPO) -> str:
    """`[workspace.package] version`, the one place the version lives."""
    return tomllib.loads((repo / "Cargo.toml").read_text())["workspace"]["package"]["version"]


def suite(
    repo: Path = REPO, load: Callable[[Path], list[check.Package]] = check.workspace
) -> list[tuple[str, ...]]:
    """The round's own suite, as a run without `--affected` runs it
    (`check.every_check`), less the measurement apparatus
    (`check.Package.apparatus`, `docs/design/decisions.md`, "D110"): nextest
    excluding each such member, then the doctests of the other libraries
    holding one, or none. Read when the suite runs, `cargo metadata` being
    the image's to run."""
    packages = load(repo)
    tested = [p for p in packages if not p.apparatus]
    excluded = tuple(a for p in packages if p.apparatus for a in ("--exclude", p.name))
    return [
        c.argv + excluded if c.name == "nextest" else c.argv
        for c in check.every_check(repo, lambda _: tested)
        if c.name in ("nextest", "doctest")
    ]


def step_suite() -> int:
    """The suite, built and run for the architecture the image is on."""
    status = 0
    for argv in suite():
        print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
        status |= subprocess.run(argv, cwd=REPO).returncode
    return 1 if status else 0


#: What the host passes a `run` step of its own environment: the colour
#: settings `check.py` sets, so a log reads as it did on the host.
RUN_PASSES = ("CARGO_TERM_COLOR", "NO_COLOR")


def step_run(command: Sequence[str], cwd: str) -> int:
    """One command, from `cwd` under the tree: a check `check.py` runs in the
    image (`check.Check.image`)."""
    return subprocess.run(command, cwd=REPO / cwd).returncode


# --- the command line ------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    """The command line, which the workflows' step lines are tested against."""
    p = argparse.ArgumentParser(
        prog="release.py",
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("image", help="build the release image, unless one of these inputs is built")
    b = sub.add_parser("build", help="release-build pgdt in the image, each binary held to the floor")
    b.add_argument(
        "--target", action="append", choices=sorted(TARGETS),
        help="one target (repeatable); every release target by default",
    )  # fmt: skip
    b.add_argument("--package", default="pgdt", help="the package (default: pgdt)")
    b.add_argument("--example", help="an example of the package, in place of its binary")
    b.add_argument("--features", default="", help="comma-separated features, as cargo takes them")
    b.add_argument("--no-default-features", action="store_true")
    b.add_argument(
        "--profile", default="release",
        help="a Cargo profile inheriting release, in its place (default: release)",
    )  # fmt: skip
    b.add_argument(
        "--rustflags", default="",
        help="RUSTFLAGS for this build alone; give it a --target-dir of its own",
    )  # fmt: skip
    b.add_argument(
        "--target-dir", type=Path,
        help="a host cargo target directory of its own, in place of the release state's",
    )  # fmt: skip
    b.add_argument(
        "--release", action="store_true",
        help=f"a release's build: {RELEASE_VARIABLE} names the workspace version, so --version drops (unreleased)",
    )  # fmt: skip
    b.add_argument("--inside", action="store_true", help="run here: this is the image")
    r = sub.add_parser(
        "bench", help=f"one cargo bench, run for {NATIVE_TARGET} in the image, held to the floor"
    )
    r.add_argument("--package", required=True, help="the package carrying the bench")
    r.add_argument("--bench", required=True, help="the bench target")
    r.add_argument("--filter", help="criterion's filter: which benchmark ids run")
    r.add_argument("--inside", action="store_true", help="run here: this is the image")
    n = sub.add_parser(
        "notices", help=f"each target's {NOTICES_NAME}, from cargo-about, under release/about.toml"
    )
    n.add_argument(
        "--target", action="append", choices=sorted(TARGETS),
        help="one target (repeatable); every release target by default",
    )  # fmt: skip
    n.add_argument("--inside", action="store_true", help="run here: this is the image")
    s = sub.add_parser("suite", help="the suite, built and run in the image for its architecture")
    s.add_argument("--inside", action="store_true", help="run here: this is the image")
    u = sub.add_parser("run", help="one command in the image: a check of check.py's")
    u.add_argument("--cwd", default=".", help="where it runs, under the tree (default: its root)")
    u.add_argument("--inside", action="store_true", help="run here: this is the image")
    u.add_argument("command", nargs=argparse.REMAINDER, help="the command, after --")
    f = sub.add_parser("floor", help="hold one binary to a glibc floor")
    f.add_argument("binary", type=Path)
    f.add_argument("--floor", required=True, help="a glibc release, such as 2.41")
    return p


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)

    try:
        if args.cmd == "floor":
            ok, lines = check_floor(readelf_of(args.binary), parse_floor(args.floor))
            print("\n".join(lines))
            return 0 if ok else 1
        container = shlex.split(CONTAINER)
        if args.cmd == "image":
            print(ensure_image(container))
            return 0
        builds: list[Build] = []
        target: Path | None = None
        if args.cmd == "build":
            features = tuple(f for f in args.features.split(",") if f)
            builds = [
                Build(
                    t, args.package, args.example, features, not args.no_default_features,
                    args.profile, args.rustflags,
                )
                for t in (args.target or TARGETS)
            ]
            target = args.target_dir
            if args.rustflags and target is None and not args.inside:
                raise ReleaseError("--rustflags rebuilds the release state's binaries; give it a --target-dir")
            if args.inside and target is not None:
                raise ReleaseError("--target-dir is the host's; inside, CARGO_TARGET_DIR names it")
            if args.inside and args.release:
                raise ReleaseError(f"--release is the host's; inside, {RELEASE_VARIABLE} names the version")
        bench = Bench(args.package, args.bench, args.filter) if args.cmd == "bench" else None
        command: list[str] = []
        if args.cmd == "run":
            command = args.command[1:] if args.command[:1] == ["--"] else args.command
            if not command:
                raise ReleaseError("run takes a command, after --")
        if args.inside:
            ready_inside()
            if args.cmd == "run":
                return step_run(command, args.cwd)
            if bench is not None:
                return step_bench(bench)
            if args.cmd == "notices":
                return step_notices(args.target or list(TARGETS))
            return step_suite() if args.cmd == "suite" else step_build(builds)
        tag = ensure_image(container)
        prepare_state(target=target)
        env: dict[str, str] = {}
        if args.cmd == "suite":
            step, git = ["suite"], git_view()
        elif args.cmd == "run":
            step, git = ["run", f"--cwd={args.cwd}", "--", *command], git_view()
            env = {key: os.environ[key] for key in RUN_PASSES if key in os.environ}
        elif bench is not None:
            step, git = bench.step(), None
        elif args.cmd == "notices":
            step, git = ["notices", *(f"--target={t}" for t in args.target or ())], None
        else:
            step, git = build_step(builds), None
            if args.release:
                env[RELEASE_VARIABLE] = workspace_version()
        # A bench is timed, so it takes every core.
        reserve = 0 if bench is not None else RESERVE_CORES
        argv = run_in_image(container, tag, step, target=target, git=git, env=env, reserve=reserve)
        return subprocess.run(argv).returncode
    except ReleaseError as e:
        print(f"release.py: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
