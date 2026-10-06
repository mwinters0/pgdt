#!/usr/bin/env python3
"""The release build: its image, `pgdt` built in it for both targets, and the
suite run there.

`docs/design/roadmap-P29-releases.md`, "The build image", is what this
implements: one image, `release/Dockerfile`, pinned by digest, builds every
release artifact, and the glibc it links against is the floor a release
promises. Each step is an argv run in that image, so a rehearsal here and a CI
container job run the same commands: on the host a step is wrapped in a
container run, and `--inside` runs it as it stands, which is what the image
itself does.

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

**The register builds here too** (`scripts/measure.py`): the shipped binary it
times is `build --target x86_64-unknown-linux-gnu`'s, and each variant it times
beside that one -- an allocator leg, the instrument, the `xz_decode` example --
is this recipe with only its package, features or target directory changed
(`Build`), so two binaries a figure compares differ by what the figure names.
A `cargo bench` a figure runs runs here as well (`Bench`), on the same compiler
and glibc, criterion's output landing in the release state's target directory.

Usage:

    cd scripts && uv run release.py image                 # build the image, unless built
    cd scripts && uv run release.py build [--target T]    # release-build pgdt, each held to the floor
    cd scripts && uv run release.py build --no-default-features --features system --target-dir D
    cd scripts && uv run release.py suite                 # the x86-64 suite, in the image
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
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

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
#: The one target the image runs: the other is cross-built.
NATIVE_TARGET = "x86_64-unknown-linux-gnu"

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
) -> list[str]:
    """The container run that executes `release.py <step> --inside` in the
    image, as the invoking user, the tree mounted at `WORKDIR`.

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


def step_suite() -> int:
    status = 0
    for argv in SUITE:
        print(f"$ {shlex.join(argv)}", file=sys.stderr, flush=True)
        status |= subprocess.run(argv, cwd=REPO).returncode
    return 1 if status else 0


# --- the command line ------------------------------------------------------


def main(argv: Sequence[str] | None = None) -> int:
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
    b.add_argument("--inside", action="store_true", help="run here: this is the image")
    r = sub.add_parser(
        "bench", help=f"one cargo bench, run for {NATIVE_TARGET} in the image, held to the floor"
    )
    r.add_argument("--package", required=True, help="the package carrying the bench")
    r.add_argument("--bench", required=True, help="the bench target")
    r.add_argument("--filter", help="criterion's filter: which benchmark ids run")
    r.add_argument("--inside", action="store_true", help="run here: this is the image")
    s = sub.add_parser("suite", help=f"the {NATIVE_TARGET} suite, in the image")
    s.add_argument("--inside", action="store_true", help="run here: this is the image")
    f = sub.add_parser("floor", help="hold one binary to a glibc floor")
    f.add_argument("binary", type=Path)
    f.add_argument("--floor", required=True, help="a glibc release, such as 2.41")
    args = p.parse_args(argv)

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
        bench = Bench(args.package, args.bench, args.filter) if args.cmd == "bench" else None
        if args.inside:
            check_inside()
            if bench is not None:
                return step_bench(bench)
            return step_suite() if args.cmd == "suite" else step_build(builds)
        tag = ensure_image(container)
        prepare_state(target=target)
        if args.cmd == "suite":
            step, git = ["suite"], git_view()
        elif bench is not None:
            step, git = bench.step(), None
        else:
            step, git = build_step(builds), None
        return subprocess.run(run_in_image(container, tag, step, target=target, git=git)).returncode
    except ReleaseError as e:
        print(f"release.py: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
