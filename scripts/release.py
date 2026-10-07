#!/usr/bin/env python3
"""The release build: its image, `pgdt` built in it for both targets, and the
suite run there.

`docs/design/roadmap-P29-releases.md`, "The build image", is what this
implements: one image, `release/Dockerfile`, pinned by digest, builds every
release artifact, and the glibc it links against is the floor a release
promises. Each step is an argv run in that image, so a rehearsal here and CI
run the same commands: on the host a step is wrapped in a container run (CI's
runner is such a host, `PGDT_RELEASE_CONTAINER=docker`; a `container:` job
takes an image reference and the image is never pushed), and `--inside` runs
it as it stands, which is what the image itself does.

**The image is tagged by its inputs** -- the Dockerfile and the two files it
copies, `rust-toolchain.toml` and `mise.toml` -- so a changed pin builds a new
image and an unchanged one is reused, never a stale image under a name that
outlived what built it. `--inside` refuses an image built from other copies
of those files than the tree's.

**Every binary built is held to the floor** (`check_floor`): no `GLIBC_`
version it needs may be past the glibc of the image's `libc6-dev` its target
links against. The link is against that glibc, so the check holds by
construction; it is the guard that says so, and the one a host build fails.

On the host each run is the invoking user's, under a memory limit, with the
build's state under `target/release-image/` -- its cargo target directory,
`CARGO_HOME` and `uv`'s cache and environment -- never the host build's, which
another compiler's C and another glibc fill.

**The workflows run these steps** (`.github/workflows/`): a job on a runner
runs `release.py <step>` as a host does, building the image from the
Dockerfile there. `build --release` is the one release-only step: it hands the
container the workspace version as `RELEASE_VARIABLE`, the only thing of the
host's it is ever given. `suite-archive` builds the cross target's
tests into a nextest archive and `suite --archived` runs it, on an arm64 host's
image, so the second target's code runs its tests somewhere; its doctests do
not run, a doctest being compiled where it runs. What becomes of the output --
the archives, the checksums, the draft, the publish -- is `scripts/release_ci.py`.

**The register builds here too** (`scripts/measure.py`): the shipped binary it
times is `build --target x86_64-unknown-linux-gnu`'s, and each variant it times
beside that one -- an allocator leg, the instrument, the `xz_decode` example --
is this recipe with only its package, features or target directory changed
(`Build`), so two binaries a figure compares differ by what the figure names.
A `cargo bench` a figure runs runs here as well (`Bench`), on the same compiler
and glibc, criterion's output landing in the release state's target directory.

**Each archive's `THIRD-PARTY-NOTICES` is made here too** (`step_notices`):
`cargo-about`, pinned in `mise.toml`, reads every crate's licence under
`release/about.toml`'s allow-list, and this script cuts that reading to what
the target's `pgdt` links and adds what `cargo-about` does not carry.

Usage:

    cd scripts && uv run release.py image                 # build the image, unless built
    cd scripts && uv run release.py build [--target T]    # release-build pgdt, each held to the floor
    cd scripts && uv run release.py build --no-default-features --features system --target-dir D
    cd scripts && uv run release.py build --release       # a release's build: names the version
    cd scripts && uv run release.py suite                 # the host-native suite, in the image
    cd scripts && uv run release.py suite-archive         # the cross target's tests, as a nextest archive
    cd scripts && uv run release.py suite --archived      # that archive's tests, on its own architecture
    cd scripts && uv run release.py notices [--target T]  # each target's THIRD-PARTY-NOTICES, beside its binary
    cd scripts && uv run release.py bench --package pgdump_query --bench decoders --filter nested
    cd scripts && uv run release.py floor <binary> --floor 2.41   # the floor check alone
    python3 scripts/release.py build --inside             # a step, already in the image
    cd scripts && uv run python -m unittest test_release
"""

from __future__ import annotations

import argparse
import dataclasses
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
from typing import Mapping, Sequence

import check

REPO = Path(__file__).resolve().parent.parent
DOCKERFILE = REPO / "release" / "Dockerfile"
#: What the Dockerfile copies into the image, from the repository root, and
#: where the image keeps them (`--inside`'s guard compares the two).
CONTEXT_FILES = ("rust-toolchain.toml", "mise.toml")
IN_IMAGE_COPIES = Path("/opt/toolchain")
IMAGE_NAME = "pgdt-release"

#: Each release target, and the Debian package carrying the glibc it links
#: against in the image -- the floor its binary is held to.
TARGETS: dict[str, str] = {
    "x86_64-unknown-linux-gnu": "libc6-dev",
    "aarch64-unknown-linux-gnu": "libc6-dev-arm64-cross",
}
#: The one target an x86-64 image runs: the other is cross-built there.
NATIVE_TARGET = "x86_64-unknown-linux-gnu"
#: The target the x86-64 image cross-builds and an arm64 host's image runs.
CROSS_TARGET = "aarch64-unknown-linux-gnu"
#: The variable `pgdt` reads to know it is a release's build, which must name
#: the workspace version (`pgdt/src/main.rs`, `RELEASED`).
RELEASE_VARIABLE = "PGDT_RELEASE_VERSION"

#: The round's own suite (`check.py`'s `CHECKS`), so the image runs what a
#: round does: nextest, then the doctests.
SUITE: tuple[tuple[str, ...], ...] = tuple(
    c.argv for c in check.CHECKS if c.name in ("nextest", "doctest")
)

CONTAINER = os.environ.get("PGDT_RELEASE_CONTAINER", "sudo nerdctl")
#: The container's memory limit: a whole-workspace test build links dozens of
#: binaries at once, and this machine's rule is a limit on every container run.
MEMORY = os.environ.get("PGDT_RELEASE_MEMORY", "20g")
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


def run_in_image(
    container: Sequence[str],
    tag: str,
    step: Sequence[str],
    *,
    state: Path = STATE,
    target: Path | None = None,
    memory: str = MEMORY,
    uid: int | None = None,
    gid: int | None = None,
    git: Path | None = None,
    env: Mapping[str, str] = {},
) -> list[str]:
    """The container run that executes `release.py <step> --inside` in the
    image, as the invoking user, the tree mounted at `WORKDIR`. `env` is the
    host's only say in the container's environment.

    `target` replaces `state`'s cargo target directory, `CARGO_HOME` staying
    shared: a build of other features writes the same output path, so one
    whose binary must survive beside the release's gets a directory of its
    own.

    **Seccomp is unconfined**: the suite runs `pgdt` as a user namespace's
    init (`pgdt/tests/namespace_init.rs`), which the runtime's default
    profile refuses to create."""
    uid = os.getuid() if uid is None else uid
    gid = os.getgid() if gid is None else gid
    argv = [
        *container, "run", "--rm",
        "-m", memory, "--memory-swap", memory,
        "--security-opt", "seccomp=unconfined",
        "--user", f"{uid}:{gid}",
        "-e", "HOME=/tmp",
        "-v", f"{REPO}:{WORKDIR}",
        "-w", WORKDIR,
    ]  # fmt: skip
    if git is not None:
        argv += ["-v", f"{git.resolve()}:{WORKDIR}/.git"]
    for key, value in env.items():
        argv += ["-e", f"{key}={value}"]
    dirs = state_dirs(state, target)
    for name, (at, env) in MOUNTS.items():
        argv += ["-v", f"{dirs[name]}:{at}"]
        for key, value in env.items():
            argv += ["-e", f"{key}={value}"]
    return [*argv, tag, "python3", "scripts/release.py", *step, "--inside"]


def check_inside(copies: Path = IN_IMAGE_COPIES, repo: Path = REPO) -> None:
    """Refuse to run a step in an image built from other pins than the tree's."""
    for name in CONTEXT_FILES:
        have = copies / name
        if not have.is_file():
            raise ReleaseError(f"--inside, but {have} is absent: this is not the release image")
        if have.read_bytes() != (repo / name).read_bytes():
            raise ReleaseError(
                f"the image was built from another {name} than the tree's; rebuild it "
                "(`cd scripts && uv run release.py image`)"
            )


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
    """One release-profile `cargo build` in the image, for one target.

    `Build(target)` is a release's own: `pgdt` at its default features. Every
    other value is a variant the register times beside it, which shares the
    image, the compiler, `--locked` and the explicit `--target` with it, so it
    differs from the release's binary by its package and features alone."""

    target: str = NATIVE_TARGET
    package: str = "pgdt"
    #: An example of `package` to build in place of its binary.
    example: str | None = None
    features: tuple[str, ...] = ()
    default_features: bool = True

    def cargo_argv(self) -> list[str]:
        argv = ["cargo", "build", "--release", "--locked", "-p", self.package, "--target", self.target]
        if self.example is not None:
            argv += ["--example", self.example]
        if not self.default_features:
            argv.append("--no-default-features")
        if self.features:
            argv += ["--features", ",".join(self.features)]
        return argv

    def binary(self, target_dir: Path) -> Path:
        """Where the build writes its binary under `target_dir`."""
        out = target_dir / self.target / "release"
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
        which the image sets for every step (`MOUNTS`)."""
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


def step_build(builds: Sequence[Build]) -> int:
    """Each build, its binary held to the floor of the glibc its target links."""
    failed = 0
    for build in builds:
        run_step(build.cargo_argv())
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


def linked_closure(target: str) -> set[Crate]:
    """Every crate the shipped binary links for `target`: its normal
    dependencies at its own features, a proc macro and what only it uses
    left out, since none of their code is in the binary. `cargo-about` reads
    the whole graph a build resolves, and this is what its output is cut to."""
    out = subprocess.run(
        [
            "cargo", "tree", "--locked", "--offline", "-p", SHIPPED, "--target", target,
            "-e", "normal,no-proc-macro", "--prefix", "none", "-f", "{p}",
        ],
        cwd=REPO, check=True, stdout=subprocess.PIPE, text=True,
    ).stdout  # fmt: skip
    return parse_tree(out)


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
            "-m", str(REPO / SHIPPED / "Cargo.toml"), "--target", target, "-c", str(config),
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


def suite_archive_path(target_dir: Path) -> Path:
    """Where the cross target's nextest archive is written and read, under
    the cargo target directory both steps see."""
    return target_dir / "nextest" / f"{CROSS_TARGET}.tar.zst"


def suite_archive_argv(archive: Path) -> list[str]:
    """The cross target's tests, built and archived rather than run: `--locked`
    and an explicit `--target`, as the release build's."""
    return [
        "cargo", "nextest", "archive", "--locked", "--workspace",
        "--target", CROSS_TARGET, "--archive-file", str(archive),
    ]  # fmt: skip


def archived_suite_argv(archive: Path) -> list[str]:
    """The round's nextest run (`SUITE`'s first command) over an archive,
    its sources remapped to the checkout at `WORKDIR`. The doctests are
    left out: rustdoc compiles them where it runs, so no archive holds one."""
    head = ("cargo", "nextest", "run")
    nextest = SUITE[0]
    if nextest[: len(head)] != head:
        raise ReleaseError(f"the round's first suite command is not a nextest run: {nextest}")
    rest = [a for a in nextest[len(head):] if a != "--workspace"]
    return [*head, "--archive-file", str(archive), "--workspace-remap", WORKDIR, *rest]


def step_suite(archived: bool = False) -> int:
    """The suite: the round's own, or, `archived`, the cross target's archived
    tests, which an image on that target's architecture runs."""
    argvs = [archived_suite_argv(suite_archive_path(target_dir()))] if archived else SUITE
    status = 0
    for argv in argvs:
        print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
        status |= subprocess.run(argv, cwd=REPO).returncode
    return 1 if status else 0


def step_suite_archive() -> int:
    """The cross target's tests built and archived, for `step_suite(archived)`."""
    archive = suite_archive_path(target_dir())
    archive.parent.mkdir(parents=True, exist_ok=True)
    run_step(suite_archive_argv(archive))
    print(f"{CROSS_TARGET}: {archive}", flush=True)
    return 0


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
    s = sub.add_parser("suite", help="the suite, in the image")
    s.add_argument(
        "--archived", action="store_true",
        help=f"the {CROSS_TARGET} nextest archive `suite-archive` wrote, run on that architecture",
    )  # fmt: skip
    s.add_argument("--inside", action="store_true", help="run here: this is the image")
    a = sub.add_parser(
        "suite-archive", help=f"build the {CROSS_TARGET} tests into a nextest archive, in the image"
    )
    a.add_argument("--inside", action="store_true", help="run here: this is the image")
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
                Build(t, args.package, args.example, features, not args.no_default_features)
                for t in (args.target or TARGETS)
            ]
            target = args.target_dir
            if args.inside and target is not None:
                raise ReleaseError("--target-dir is the host's; inside, CARGO_TARGET_DIR names it")
            if args.inside and args.release:
                raise ReleaseError(f"--release is the host's; inside, {RELEASE_VARIABLE} names the version")
        bench = Bench(args.package, args.bench, args.filter) if args.cmd == "bench" else None
        if args.inside:
            check_inside()
            if bench is not None:
                return step_bench(bench)
            if args.cmd == "notices":
                return step_notices(args.target or list(TARGETS))
            if args.cmd == "suite-archive":
                return step_suite_archive()
            return step_suite(args.archived) if args.cmd == "suite" else step_build(builds)
        tag = ensure_image(container)
        prepare_state(target=target)
        env: dict[str, str] = {}
        if args.cmd == "suite":
            step, git = ["suite", *(["--archived"] if args.archived else [])], git_view()
        elif args.cmd == "suite-archive":
            step, git = ["suite-archive"], None
        elif bench is not None:
            step, git = bench.step(), None
        elif args.cmd == "notices":
            step, git = ["notices", *(f"--target={t}" for t in args.target or ())], None
        else:
            step, git = build_step(builds), None
            if args.release:
                env[RELEASE_VARIABLE] = workspace_version()
        return subprocess.run(run_in_image(container, tag, step, target=target, git=git, env=env)).returncode
    except ReleaseError as e:
        print(f"release.py: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
