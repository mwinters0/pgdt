"""The release-image builds the harness owns: the shipped `pgdt`, the static resident-set and
instruction-count instruments, and the apparatus members a figure times instead of `pgdt`."""

from __future__ import annotations

import platform
import shutil
from pathlib import Path
from typing import Callable
from typing import TYPE_CHECKING

import release

from measurement import config
from measurement import host
from measurement import parsing
from measurement import shapes

if TYPE_CHECKING:
    from measurement.config import Config


#: Whether this process has already built the shipped binary. Per process for
#: `_ALLOC_BUILT`'s reason, one level up: this is the binary every `pgdt`
#: figure but the allocator's other legs is timed against, and a stale one is
#: what a sitting cannot see.
_PGDT_BUILT = False


def ensure_pgdt_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """The shipped binary, built in the release image before the first
    reading rather than found (`build_in_image`).

    **What this closes is a sitting that does not look lost.** The harness
    timed whatever `target/release/pgdt` happened to be and asked only that the
    file exist, while the session stamp named `git rev-parse HEAD` regardless —
    so a run on a tree-old binary emits a full table, a stamp naming a commit
    it did not execute, and a verdict. The worked instance is the 2026-09-12
    gate sitting, which timed a charge model three commits stale against
    `charge_model`'s repaired mirror of it and reported the difference as an
    over-bill of the whole bill.

    **Built, not stamped.** A build the harness performs itself is one whose
    provenance it knows, where a stamp only records a hand's claim; and
    `ensure_allocator_binary` rebuilds every leg once per process precisely
    because "short-circuiting on the file's existence would have silently timed
    the previous session's binary against this one's reference". That is this
    failure, already written down as a rejected alternative — for the legs of a
    comparison whose reference side did it.

    Once per **process**, for that reason. `cargo` is incremental, so a tree
    that has not moved costs about a second; a build between two timed reps
    would move the second one.

    **`PGDT_MEASURE_BIN` pointed anywhere else builds nothing.** The only path
    that build writes is `REGISTER_BIN`, so a harness that ran it and then
    timed a different file would be asserting a provenance it does not have.
    There the binary is the caller's, and its absence is `main`'s error rather
    than a build.
    """
    global _PGDT_BUILT
    if _PGDT_BUILT or cfg.bin_pgdt != config.REGISTER_BIN:
        return cfg.bin_pgdt
    if cfg.dry_run:
        # A dry run measures nothing and must work where no binary exists, so
        # it announces instead — the same arm `ensure_allocator_binary` has,
        # for the same reason.
        log(f"[dry-run] would build {cfg.bin_pgdt}")
        return cfg.bin_pgdt
    log(f"building {cfg.bin_pgdt} in the release image")
    build_in_image(cfg, release.Build())
    _PGDT_BUILT = True
    return cfg.bin_pgdt


def build_in_image(
    cfg: Config, build: release.Build, target_dir: Path | None = None, tree: Path = config.REPO
) -> Path:
    """`build` run in the release image, its binary held to the floor there
    (`release.step_build`), and where that binary lands on the host.

    **Every binary a figure runs in `Config.image` is built here**: the
    shipped one, and each one timed beside it — an allocator leg, the
    instrument, the `xz_decode` example — so two binaries a figure compares
    differ by what it names, never by a compiler or a glibc the image supplies
    to one of them alone. `target_dir` is a cargo target directory of the
    build's own (`image_target_dir`), for a build writing the shipped one's
    output path; `None` is the release state's, where that binary lives.
    `tree` is the checkout built, another commit's worktree for
    `cmd_count_pair`'s parent leg."""
    container = cfg.container_argv()
    tag = release.ensure_image(container)
    release.prepare_state(target=target_dir)
    host.run(
        release.run_in_image(
            container, tag, release.build_step([build]), target=target_dir, tree=tree
        ),
        cwd=config.REPO,
    )
    return build.binary(target_dir if target_dir is not None else release.STATE / "target")


def image_target_dir(cfg: Config, name: str) -> Path:
    """The cargo target directory of one build in the image that is not the
    shipped one: under `Config.alloc_build_root`, apart from the host builds
    the printed recipes make there, whose build scripts link the host's glibc
    and must never be found fresh by the image's cargo."""
    return cfg.alloc_build_root / "image" / name

#: Whether this process has already built `peak-rss`, for `_PGDT_BUILT`'s
#: reason: the instrument every resident figure reads through is built from the
#: tree being measured, never found.
_PEAK_RSS_BUILT = False


def peak_rss_target() -> str:
    """The Rust target `peak-rss` is built for: musl, on the host's machine."""
    return f"{platform.machine()}-unknown-linux-musl"


def peak_rss_path() -> Path:
    """Where `cargo build --target` writes `peak-rss`, on the host."""
    return config.REPO / "target" / peak_rss_target() / "release" / "peak-rss"


def ensure_peak_rss_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """`peak-rss`, the resident-set instrument, built static before the first
    reading and mounted at `PEAK_RSS` beside every program.

    **For `<machine>-unknown-linux-musl`**, so it links nothing of the host's
    libc or the image's and a later pin move cannot break it. Refused: the
    gnu target with `+crt-static`, which needs the host's static glibc.
    **Built on the host**, not in the release image as every timed binary is
    (`build_in_image`): it links no glibc at all, so where it is built moves
    nothing a reading stands on.

    **A missing target is refused with the command that adds it**, rather than
    left to `cargo`'s error about a missing `core`. Once per process, for
    `ensure_pgdt_binary`'s reason."""
    global _PEAK_RSS_BUILT
    out = peak_rss_path()
    if _PEAK_RSS_BUILT:
        return out
    _build_static_instrument(cfg, "peak-rss", out, log)
    _PEAK_RSS_BUILT = True
    return out


def _build_static_instrument(
    cfg: Config, package: str, out: Path, log: Callable[[str], None]
) -> None:
    """One static instrument crate built for `peak_rss_target`, or the
    command that installs the target where it is missing."""
    if cfg.dry_run:
        log(f"[dry-run] would build {out}")
        return
    target = peak_rss_target()
    sysroot = Path(host.run(["rustc", "--print", "sysroot"], cwd=config.REPO, capture=True).strip())
    if not (sysroot / "lib" / "rustlib" / target).is_dir():
        raise RuntimeError(
            f"the Rust target {target} is not installed, and `{package}` is built for it: "
            f"`rustup target add {target}`"
        )
    log(f"building {out}")
    host.run(["cargo", "build", "--release", "-p", package, "--target", target], cwd=config.REPO)

#: Whether this process has already built `count-instructions`, for
#: `_PEAK_RSS_BUILT`'s reason.
_COUNTER_BUILT = False


def count_instructions_path() -> Path:
    """Where `cargo build --target` writes `count-instructions`, on the host."""
    return config.REPO / "target" / peak_rss_target() / "release" / "count-instructions"


def ensure_count_instructions_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """`count-instructions`, the instruction counter, built static before a
    counted figure's first reading and mounted at `COUNT_INSTRUCTIONS`, on
    `ensure_peak_rss_binary`'s terms."""
    global _COUNTER_BUILT
    out = count_instructions_path()
    if _COUNTER_BUILT:
        return out
    _build_static_instrument(cfg, "count-instructions", out, log)
    _COUNTER_BUILT = True
    return out

#: Whether this process has already built the `xz_decode` instrument. Per
#: process rather than per file, for the reason the allocator legs are: a
#: binary left in `runs/` by an earlier session was built from whatever the
#: source said then, and this figure's whole content is that decoder's rate.
_XZ_DECODE_BUILT = False


def ensure_xz_decode_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """`pgdt-io`'s `xz_decode` example, built and copied beside the other
    measurement binaries.

    Mechanical, so the harness does it rather than asking for a binary: a
    `cargo build` of a committed target, where a source edit is one no harness
    should perform.

    **An example target, so the shipped binary is untouched.** Every other
    figure in a sweep is timed against that binary, and a build that replaced
    it would re-time all of them against something else — the failure the
    allocator legs' separate target directories exist to prevent, one target
    kind along. Built in the release image (`build_in_image`), as the binary
    it is set beside is.
    """
    global _XZ_DECODE_BUILT
    out = cfg.out_dir / "pgdt-xz-decode"
    if _XZ_DECODE_BUILT:
        return out
    if cfg.dry_run:
        # Announced once, not once per rep: a real run leaves the binary
        # behind, which is the memo; a dry run has to keep its own.
        log(f"  [dry-run] would build the xz_decode instrument into {out}")
        _XZ_DECODE_BUILT = True
        return out
    log(f"  building the xz_decode instrument into {out}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = build_in_image(cfg, release.Build(package="pgdt-io", example="xz_decode"))
    shutil.copyfile(built, out)
    out.chmod(0o755)
    _XZ_DECODE_BUILT = True
    return out

#: Whether this process has already built the `gzip-inflate` instrument, per
#: process for `_XZ_DECODE_BUILT`'s reason.
_GZIP_INFLATE_BUILT = False


def ensure_gzip_inflate_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """The `gzip-inflate` apparatus member, built and copied beside the other
    measurement binaries — another package's binary, so the shipped one is
    untouched, built in the release image for every reason
    `ensure_xz_decode_binary` gives."""
    global _GZIP_INFLATE_BUILT
    out = cfg.out_dir / "pgdt-gzip-inflate"
    if _GZIP_INFLATE_BUILT:
        return out
    if cfg.dry_run:
        log(f"  [dry-run] would build the gzip-inflate instrument into {out}")
        _GZIP_INFLATE_BUILT = True
        return out
    log(f"  building the gzip-inflate instrument into {out}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = build_in_image(cfg, release.Build(package="gzip-inflate"))
    shutil.copyfile(built, out)
    out.chmod(0o755)
    _GZIP_INFLATE_BUILT = True
    return out

#: The `pgdt-gzip-seek` engine legs this process has built, per process for
#: `_XZ_DECODE_BUILT`'s reason.
_GZSEEK_BUILT: set[str] = set()


def gzseek_engine_problem(engine: str, reported: str) -> str | None:
    """Why a `gz_read` build reporting `reported` as its `engine=` is not the
    `engine` leg, or `None` where it is.

    Read off the engine's own `zlibVersion` (`pgdt_gzip_seek::engine`), the one
    thing that names which library the binary links: zlib-rs's carries
    `zlib-rs`, zlib-ng's compatible build `zlib-ng`, and stock zlib's neither.
    A `libz-sys` build handed both of its modes builds stock zlib, so a
    `zlib-ng` leg that silently lost its feature reads as the `zlib` one, and
    the two rows would be one engine timed twice."""
    names = {"zlib-rs": "zlib-rs" in reported, "zlib-ng": "zlib-ng" in reported}
    got = next((name for name, hit in names.items() if hit), "zlib")
    if sum(names.values()) > 1 or got != engine:
        return f"the {engine} leg links `{reported}`, which is {got}'s"
    return None


def ensure_gzseek_binary(cfg: Config, engine: str, log: Callable[[str], None]) -> Path:
    """One engine leg of `pgdt-gzip-seek`'s `gz_read` example, built and then
    interrogated, as an `allocator` leg is (`ensure_allocator_binary`):
    `--no-default-features --features <engine>`, so the leg is the one engine
    it names, the crate refusing any other count; **its own target dir**, so
    no leg's `libz-sys` build is another's; and `--engine` read back and held
    to the leg (`gzseek_engine_problem`). An example of another package than
    `pgdt`, built in the release image for every reason
    `ensure_xz_decode_binary` gives."""
    if engine not in shapes.GZSEEK_ENGINES:
        raise ValueError(f"unknown pgdt-gzip-seek engine {engine!r}")
    out = cfg.out_dir / f"pgdt-gzseek-{engine}"
    if engine in _GZSEEK_BUILT:
        return out
    target = image_target_dir(cfg, f"gzseek-{engine}")
    if cfg.dry_run:
        log(f"  [dry-run] would build the pgdt-gzip-seek {engine} leg into {target}")
        _GZSEEK_BUILT.add(engine)
        return out
    log(f"  building the pgdt-gzip-seek {engine} leg into {target}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    build = release.Build(
        package="pgdt-gzip-seek", example="gz_read", features=(engine,), default_features=False
    )
    built = build_in_image(cfg, build, target_dir=target)
    shutil.copyfile(built, out)
    out.chmod(0o755)
    reported = parsing.parse_reported(host.run([str(out), "--engine"], capture=True)).get("engine", "")
    problem = gzseek_engine_problem(engine, reported)
    if problem is not None:
        out.unlink()
        raise RuntimeError(f"{problem}: the build did not take the feature")
    _GZSEEK_BUILT.add(engine)
    log(f"  {out.name}: {reported}")
    return out
