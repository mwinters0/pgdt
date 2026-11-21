"""The builds a figure's legs differ by -- allocator, release profile, chunk size, purge delay, the
instrument -- each built once a sitting and asked which it is before it is timed."""

from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path
from typing import Callable, Iterable, Mapping
from typing import TYPE_CHECKING

import contained

import release

from measurement import config
from measurement import gzip_charge
from measurement import host
from measurement import shapes
from measurement.builds import image

if TYPE_CHECKING:
    from measurement.config import Config


#: The three legs of the `allocator` figure, each a `pgdt` feature, **the
#: default build's first** -- `crates/pgdt/Cargo.toml`'s `default`, which a test holds
#: this to -- so a dry run, which asks no binary, names the reference the
#: shipped build would. `system` is the platform allocator, glibc's `malloc`
#: on the recorded apparatus, and is spelled the way the binary spells it
#: rather than as `glibc`, because the crate cannot know which libc it was
#: linked against and the measurement records the image.
ALLOCATOR_LEGS: tuple[str, ...] = ("mimalloc", "system", "jemalloc")

#: What `pgdt --version` appends. The harness *asks the binary* rather than
#: trusting the flags it passed: a leg mislabelled by one word gives a
#: perfectly plausible table of the wrong comparison, which is the same family
#: of failure as profiling the `release` binary and calling it `profiling`.
ALLOCATOR_RE = re.compile(r"\(allocator: ([a-z]+)\)")

#: What an *instrumented* build appends beside its allocator
#: (`crates/pgdt/src/introspect.rs`). Such a build takes atomics on every allocation
#: and prints its own live bytes and its allocators' statistics: it is how an
#: attribution is taken, and it is not what any figure may be timed or measured
#: on (`docs/design/roadmap.md`, "Attribution is introspective; only the gate
#: is blind"). The marker rides in `--version` because that is the one place a
#: *binary* can be asked what it is, which is the same argument the allocator
#: name is read there for.
INSTRUMENT_RE = re.compile(r"\(instrument: ([a-z-]+)\)")


def binary_allocator(binary: Path) -> str:
    """Which allocator a built `pgdt` links against, read out of the binary.

    Not an optional nicety: the day the CLI's default feature set changes,
    the shipped binary becomes a different one and every apparatus line
    that still names the old allocator is wrong with nothing to notice. This is
    what the session stamp reports.

    **An instrumented build is refused here rather than named**, because every
    caller of this function is about to publish something about the binary —
    the session stamp, or an `allocator` leg's label. A counting allocator
    would be timed as `system` and read as the shipped binary, which is the
    same family of failure as profiling the `release` build and calling it
    `profiling`, and the refusal is what makes "not a fourth `ALLOCATOR_LEGS`
    member" mechanical instead of intentional."""
    out = host.run([str(binary), "--version"], capture=True)
    instrument = INSTRUMENT_RE.search(out)
    if instrument is not None:
        raise RuntimeError(
            f"{binary} --version names an instrument ({instrument.group(1)}): an introspection "
            "build reports what it holds and is not what any figure is taken on — build it "
            "into its own target dir and read it, never time it"
        )
    match = ALLOCATOR_RE.search(out)
    if match is None:
        raise RuntimeError(
            f"{binary} --version does not name an allocator ({out.strip()!r}) — "
            "it is too old to be an `allocator` figure leg"
        )
    return match.group(1)

#: What glibc's `getconf GNU_LIBC_VERSION` prints. musl's `getconf` has no such
#: variable and exits non-zero, which is the refusal `glibc_of` wants.
LIBC_RE = re.compile(r"^glibc (\d+(?:\.\d+)+)$", re.MULTILINE)


def glibc_of(cfg: Config, where: str) -> str:
    """The glibc a program run in the image `where` runs under, asked of that
    image rather than assumed from a tag.

    The allocator's argument one layer down (`binary_allocator`): a tag moves
    under the register and says nothing about the libc it holds, so the stamp
    and a figure's marker name what the image answers. **A place answering
    with no glibc is refused**, since the apparatus is a glibc one and the
    figure would otherwise publish under a libc nothing named."""
    argv = [*cfg.container_argv(), "run", "--rm", where, "getconf", "GNU_LIBC_VERSION"]
    try:
        out = host.run(argv, capture=True)
    except subprocess.CalledProcessError:
        out = ""
    match = LIBC_RE.search(out)
    if match is None:
        raise RuntimeError(
            f"{where} answers `getconf GNU_LIBC_VERSION` with {out.strip()!r}, so it is no "
            "glibc — a figure is taken under glibc and names the one it ran under"
        )
    return match.group(1)


def glibc_named(places: Iterable[str], glibcs: Mapping[str, str]) -> str | None:
    """The glibc a figure ran under, from the places its programs ran in: one
    version, or several joined where they ran in more than one; `None` where no
    place was asked, which is `--dry-run` and a sitting older than the asking."""
    versions = sorted({glibcs[p] for p in places if p in glibcs})
    return " and ".join(versions) or None

#: Legs whose (absent) build a dry run has already reported. Only a dry run
#: needs it: a real build leaves the binary behind, which is the memo.
_ALLOC_ANNOUNCED: set[str] = set()

#: Legs already built *by this process*. The cache is deliberately per-run and
#: not the file on disk: `runs/pgdt-alloc-<leg>` from an earlier session was
#: built from whatever the source said then, and reusing it compares a fresh
#: reference binary against a stale leg -- which is exactly the "plausible table
#: of the wrong comparison" this figure's assertions exist to stop, and it fails
#: silently because a leg still answers `--version` with its own allocator name.
_ALLOC_BUILT: set[str] = set()


def ensure_allocator_binary(cfg: Config, leg: str, log: Callable[[str], None]) -> Path:
    """One leg of the `allocator` figure, built and then interrogated.

    Three details are load-bearing and each fails by producing a table of
    something else:

    * **`--no-default-features --features <leg>`**, so a leg is the one
      allocator it names whatever the CLI's default is: `pgdt` refuses two
      allocator features at compile time, so without the first flag every leg
      but the default's fails to build, and without the second the build
      names none and is refused too.
    * **Its own target dir**, so the shipped binary -- every other figure's
      -- is never overwritten by a `--features` build.
    * **`--version` is read back** and must name this leg.

    Built in the release image (`build_in_image`), as the shipped binary is.

    A fourth is about *when*: the build runs once per leg per **process**, not
    once per leg per machine. `cargo` is incremental, so a leg whose source has
    not moved costs a second; a leg whose source *has* moved is the whole
    reason this figure is being re-taken, and short-circuiting on the file's
    existence would have silently timed the previous session's binary against
    this one's reference.
    """
    if leg not in ALLOCATOR_LEGS:
        raise ValueError(f"unknown allocator leg {leg!r}")
    out = cfg.out_dir / f"pgdt-alloc-{leg}"
    if leg in _ALLOC_BUILT:
        return out
    target = image.image_target_dir(cfg, leg)
    if cfg.dry_run:
        # Announced once per leg, not once per rep: a real run builds on the
        # first call and the file answers every later one, and a dry run that
        # repeated the line thirty times would read as thirty builds.
        if leg not in _ALLOC_ANNOUNCED:
            _ALLOC_ANNOUNCED.add(leg)
            log(f"  [dry-run] would build the {leg} allocator leg into {target}")
        return out
    log(f"  building the {leg} allocator leg into {target}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = image.build_in_image(
        cfg, release.Build(features=(leg,), default_features=False), target_dir=target
    )
    shutil.copyfile(built, out)
    out.chmod(0o755)
    _ALLOC_BUILT.add(leg)
    got = binary_allocator(out)
    if got != leg:
        out.unlink()
        raise RuntimeError(
            f"the {leg} leg reports `{got}`: the build did not take the feature, and timing "
            "it would publish a comparison of two identical binaries"
        )
    log(f"  {out.name}: {leg}")
    return out

#: The `release-profile` figure's legs, each a Cargo profile inheriting
#: `release` in the root `Cargo.toml`, in the table's column order with the
#: caption each carries. **The first is the one every ratio is against**:
#: Cargo's default restated as a profile of its own rather than `release`
#: itself, so the leg stays the default whatever `release` comes to state, and
#: rather than the shipped binary, so all three are built alike from the tree
#: measured and no column is borrowed from another figure.
PROFILE_LEGS: tuple[tuple[str, str], ...] = (
    ("release-default", "Cargo's default: no cross-crate LTO, 16 codegen units"),
    ("release-thin", "thin LTO, 16 codegen units"),
    ("release-fat", "fat LTO, one codegen unit"),
)

#: The leg `[profile.release]` restates (D105), which the default leg is held
#: to by `downstream_verdict`.
SHIPPED_PROFILE_LEG = "release-fat"

#: How far the default leg may trail the shipped one in wall clock: the order's
#: "small", which `roadmap.md`'s "A downstream build is within 3% of ours"
#: states and this mirrors.
DOWNSTREAM_GAP = 0.03

#: What `pgdt --version` names of the profile (`crates/pgdt/src/profile.rs`).
PROFILE_RE = re.compile(r"\(profile: ([a-z0-9-]+)\)")


def binary_profile(binary: Path) -> str:
    """Which Cargo profile a built `pgdt` was made under, read out of the
    binary, for `binary_allocator`'s reason: three legs differing only in a
    flag passed to `cargo` are a table of the wrong comparison the moment one
    flag is dropped, and nothing else would notice. A binary naming none is
    refused, as is one naming `unknown`, which is `crates/pgdt/build.rs` failing to
    read its profile."""
    out = host.run([str(binary), "--version"], capture=True)
    match = PROFILE_RE.search(out)
    if match is None or match.group(1) == "unknown":
        raise RuntimeError(
            f"{binary} --version does not name the profile it was built under "
            f"({out.strip()!r}) — it cannot be a `release-profile` leg"
        )
    return match.group(1)


def profile_leg_binary(cfg: Config, leg: str) -> Path:
    """Where a sitting leaves one `release-profile` leg's binary: in
    `Config.out_dir`, which the next sitting overwrites. Read by the figure
    and by `profile_recipe`'s differential, which profiles these files."""
    return cfg.out_dir / f"pgdt-{leg}"

#: Legs built by this process, and legs a dry run has announced, for
#: `_ALLOC_BUILT`'s and `_ALLOC_ANNOUNCED`'s reasons.
_PROFILE_BUILT: set[str] = set()

_PROFILE_ANNOUNCED: set[str] = set()


def ensure_profile_binary(cfg: Config, leg: str, log: Callable[[str], None]) -> Path:
    """One leg of the `release-profile` figure, built and then interrogated.

    `ensure_allocator_binary`'s details, with the profile in the allocator's
    place: **the default features**, so every leg links the shipped allocator
    and the three differ by the profile alone; **its own target dir**, apart
    from the shipped build's and the other legs'; **`--version` read back**,
    naming this leg's profile and the shipped allocator, or the leg is
    refused; and **once per process**.
    Built in the release image (`build_in_image`), as the shipped binary is.
    """
    if leg not in dict(PROFILE_LEGS):
        raise ValueError(f"unknown release-profile leg {leg!r}")
    out = profile_leg_binary(cfg, leg)
    if leg in _PROFILE_BUILT:
        return out
    target = image.image_target_dir(cfg, leg)
    if cfg.dry_run:
        if leg not in _PROFILE_ANNOUNCED:
            _PROFILE_ANNOUNCED.add(leg)
            log(f"  [dry-run] would build the {leg} profile leg into {target}")
        return out
    log(f"  building the {leg} profile leg into {target}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = image.build_in_image(cfg, release.Build(profile=leg), target_dir=target)
    shutil.copyfile(built, out)
    out.chmod(0o755)
    _PROFILE_BUILT.add(leg)
    got, allocator = binary_profile(out), binary_allocator(out)
    if got != leg or allocator != ALLOCATOR_LEGS[0]:
        out.unlink()
        _PROFILE_BUILT.discard(leg)
        raise RuntimeError(
            f"the {leg} leg reports profile `{got}` and allocator `{allocator}`: a leg is "
            f"its own profile at the shipped features ({ALLOCATOR_LEGS[0]}), or timing it "
            "publishes a comparison of something else"
        )
    log(f"  {out.name}: {leg}, {allocator}")
    return out

#: The `gzip-first-pass` figure's chunk-length leg: `pgdt`'s feature naming
#: it, and the binary a `RunSpec` names it by. The shipped build but for a
#: first pass's chunk, the 1 MiB input chunk in place of rapidgzip's 4 MiB,
#: the speculation limit held: the length D121 rejects, so a re-take can meet
#: its `Reopens` (`docs/design/decisions.md`, "D121").
GZPASS_CHUNK_LEG = "gzip-1mib-chunks"

GZPASS_CHUNK_BINARY = f"chunk:{GZPASS_CHUNK_LEG}"

#: The chunk length that leg's first pass cuts, in bytes, and what its
#: `--version` names (`crates/pgdt/src/version.rs`, `CHUNK_MARKER`); a build
#: naming none cuts the shipped `GZIP_CHUNK`.
GZPASS_CHUNK_BYTES = 1 * config.MIB

CHUNK_RE = re.compile(r"\(gzip chunk: (\d+) MiB\)")


def binary_chunk(binary: Path) -> int:
    """The chunk length a built `pgdt`'s gzip first pass cuts, in bytes, read
    out of the binary for `binary_allocator`'s reason: two legs differing by
    a feature are a comparison of two identical binaries the moment the
    feature is dropped. A binary naming none cuts the shipped length."""
    out = host.run([str(binary), "--version"], capture=True)
    match = CHUNK_RE.search(out)
    return int(match.group(1)) * config.MIB if match else gzip_charge.GZIP_CHUNK

#: Legs built by this process, and legs a dry run has announced, for
#: `_ALLOC_BUILT`'s and `_ALLOC_ANNOUNCED`'s reasons.
_CHUNK_BUILT: set[str] = set()

_CHUNK_ANNOUNCED: set[str] = set()


def ensure_chunk_binary(cfg: Config, leg: str, log: Callable[[str], None]) -> Path:
    """`gzip-first-pass`'s chunk-length leg, built and then interrogated.

    `ensure_allocator_binary`'s details, with the chunk in the allocator's
    place: **the default features plus the leg's**, so it links the shipped
    allocator and differs from the shipped binary by the chunk alone; **its
    own target dir**; **`--version` read back**, naming this leg's chunk, the
    shipped allocator and `release`, or the leg is refused; and **once per
    process**. Built in the release image (`build_in_image`), as the shipped
    binary is. What a separate build cannot remove is its layout, which a
    constant moved may move (`measurements.md`, "Two builds of one source can
    differ by layout"); the figure's table says so."""
    if leg != GZPASS_CHUNK_LEG:
        raise ValueError(f"unknown chunk leg {leg!r}")
    out = cfg.out_dir / f"pgdt-{leg}"
    if leg in _CHUNK_BUILT:
        return out
    target = image.image_target_dir(cfg, leg)
    if cfg.dry_run:
        if leg not in _CHUNK_ANNOUNCED:
            _CHUNK_ANNOUNCED.add(leg)
            log(f"  [dry-run] would build the {leg} chunk leg into {target}")
        return out
    log(f"  building the {leg} chunk leg into {target}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = image.build_in_image(cfg, release.Build(features=(leg,)), target_dir=target)
    shutil.copyfile(built, out)
    out.chmod(0o755)
    _CHUNK_BUILT.add(leg)
    chunk, allocator, profile = binary_chunk(out), binary_allocator(out), binary_profile(out)
    if (chunk, allocator, profile) != (GZPASS_CHUNK_BYTES, ALLOCATOR_LEGS[0], "release"):
        out.unlink()
        _CHUNK_BUILT.discard(leg)
        raise RuntimeError(
            f"the {leg} leg reports a {chunk}-byte chunk, allocator `{allocator}` and profile "
            f"`{profile}`: a leg is the shipped build but for a {GZPASS_CHUNK_BYTES}-byte "
            "chunk, or timing it publishes a comparison of something else"
        )
    log(f"  {out.name}: {chunk >> 20} MiB chunks, {allocator}, {profile}")
    return out

#: Whether the instrument build has already been made by this process, for
#: `_ALLOC_BUILT`'s reason: a binary left in `runs/` by an earlier session was
#: built from whatever the source said then, and reading it beside this
#: session's black-box legs compares two revisions and calls it an attribution.
_INSTRUMENT_BUILT = False


def binary_instrument(binary: Path) -> str:
    """Which instrument a built `pgdt` carries, read out of the binary.

    The mirror of `binary_allocator`'s refusal, pointed the other way: there an
    instrumented build must not be timed, here a leg that declares the
    instrument must actually carry one. Both failures are silent otherwise —
    a default binary produces no report at all, which `_read_instrument` can
    only report as "one of three apparatus faults", and this is the one of the
    three it can name before the sitting starts."""
    out = host.run([str(binary), "--version"], capture=True)
    match = INSTRUMENT_RE.search(out)
    if match is None:
        raise RuntimeError(
            f"{binary} --version names no instrument ({out.strip()!r}): the build did not take "
            "`--features introspect`, and every leg pointed at it would report nothing"
        )
    return match.group(1)


def ensure_instrument_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """The introspection build, built and then interrogated.

    `ensure_allocator_binary`'s three details hold here for the same reasons,
    with one difference: the features are the shipped set **plus** the
    instrument, so there is no `--no-default-features`. The CLI's default set is
    `mimalloc`, the heap the instrument counts in front of, and the instrument
    is meant to run the arrangement the shipped binary runs, so subtracting the
    defaults would measure a third build.

    **Its own target dir**, because a `--features` build in the shipped
    one's overwrites the shipped binary — every other figure's — with a
    binary that takes atomics on every allocation and that
    `binary_allocator` then refuses, which is a sitting lost to a build step.
    Built in the release image (`build_in_image`), as the shipped binary is.

    **`--version` is read back**, and it must name the instrument. A leg
    declaring `RunSpec.instrument` and pointed at a build without the feature
    produces no report, and the run that discovers it is the first rep of the
    hour rather than the second before it.
    """
    global _INSTRUMENT_BUILT
    out = cfg.out_dir / "pgdt-introspect"
    if _INSTRUMENT_BUILT:
        return out
    target = image.image_target_dir(cfg, "introspect")
    if cfg.dry_run:
        log(f"  [dry-run] would build the introspection instrument into {target}")
        return out
    log(f"  building the introspection instrument into {target}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = image.build_in_image(cfg, release.Build(features=("introspect",)), target_dir=target)
    shutil.copyfile(built, out)
    out.chmod(0o755)
    _INSTRUMENT_BUILT = True
    log(f"  {out.name}: {binary_instrument(out)}")
    return out


def count_saves(
    cfg: Config, binary: Path, dump: Path, log: Callable[[str], None]
) -> tuple[int, int]:
    """Cache writes during one `parse`, from `strace`.

    Traced untimed, so strace's overhead reaches no figure, in a container
    under the sweep's limit, as every run of a built binary is
    (`contained.py`).
    **Both** `open` and `openat`: glibc uses one and musl the other, and
    tracing a single call silently reports zero saves against the other libc.
    A save opens once, the file it writes beside the cache and renames over it,
    whose path begins with the cache's, so the filter below counts it; the first
    open is the load's miss.

    Untimed, but the save count it returns is published, and `-f` follows every
    thread — so it states its worker count like everything else here."""
    cache = cfg.warm_dir / "savecount.pgdt"
    cache.unlink(missing_ok=True)
    if cfg.dry_run:
        return (0, 0)
    proc = subprocess.run(
        contained.argv(
            [
                "strace", "-f", "-e", "trace=open,openat",
                str(binary), "parse", "--source", str(dump), "--cache", str(cache),
                "--jobs", str(shapes.SWEEP_JOBS), *shapes.NO_STATISTICS.split(),
            ],
            cfg.memory,
            container=cfg.container_argv(),
        ),
        text=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"strace parse failed:\n{proc.stderr[-2000:]}")
    opens = sum(1 for line in proc.stderr.splitlines() if str(cache) in line)
    size = cache.stat().st_size if cache.exists() else 0
    cache.unlink(missing_ok=True)
    log(f"  {binary.name}: {opens} opens of the cache path, final cache {size} bytes")
    return (opens, size)

# -- mimalloc's purge delay ---------------------------------------------------

#: The variable mimalloc reads its purge delay from, in milliseconds: how long
#: a page it holds of freed blocks stays committed, and resident, before it is
#: decommitted. What the shipped binary keeps past a zero delay is retention of
#: every buffer a scan frees at its rate (`docs/design/decisions.md`, "D165"),
#: which `docs/design/decisions.md`, "D166" bounds by this
#: delay, `pgdt` setting a default the variable overrides
#: (`crates/pgdt/src/alloc.rs`, `PURGE_DELAY_MS`).
PURGE_DELAY_VAR = "MIMALLOC_PURGE_DELAY"

#: The delays `purge-delay`'s and `purge-delay-resident`'s legs state, in
#: milliseconds, beside the shipped leg stating none (`purge_delay_legs`):
#: zero, purging as a page empties, and two between it and mimalloc v3's
#: default of 1000, a decade apart. **The value `pgdt` sets is one of these**
#: (D166: chosen on throughput, what it keeps billed), so this tuple is the
#: choice's resolution, and the shipped leg now runs at its last, so a re-take
#: reads it twice. A value the choice moves to is added here first, so every
#: table names it; pricing mimalloc's default (D166's `Reopens`) adds 1000 as
#: a stating leg.
PURGE_DELAY_LEGS: tuple[int, ...] = (0, 10, 100)

#: Where `pgdt --version` names the purge delay mimalloc runs under once
#: `pgdt` has set its default, the environment's or that default
#: (`crates/pgdt/src/alloc.rs`, `purge_delay_marker`). Not `MIMALLOC_VERBOSE`'s
#: listing, which mimalloc prints as the process loads, before `pgdt` sets it.
PURGE_DELAY_RE = re.compile(r"\(purge delay: (-?\d+) ms\)")

#: The purge delay a dry run names for the shipped leg: `pgdt`'s default, as
#: `crates/pgdt/src/alloc.rs`'s `PURGE_DELAY_MS` states it. Never a reading: a
#: live sitting asks the binary (`binary_purge_delay`).
DRY_RUN_PURGE_DELAY = 100


def purge_delay_legs() -> tuple[int | None, ...]:
    """Every purge-delay leg in its table's column order: the shipped binary
    stating none, the reference every other leg's cost and resident are read
    against, then each `PURGE_DELAY_LEGS` value."""
    return (None, *PURGE_DELAY_LEGS)


def purge_delay_env(delay: int | None) -> str | None:
    """The `RunSpec.env` a leg states: none for the shipped leg, so it runs
    exactly as a release does, and `PURGE_DELAY_VAR` otherwise."""
    return None if delay is None else f"{PURGE_DELAY_VAR}={delay}"


def purge_delay_caption(delay: int | None) -> str:
    """A leg's name, as its column and its per-rep lines print it."""
    return "shipped" if delay is None else f"{delay} ms"


def binary_purge_delay(cfg: Config, binary: Path, delay: int | None) -> int:
    """The purge delay `binary` runs under in the image with `delay`'s leg's
    environment, asked of the binary, which reads it back from mimalloc
    (`PURGE_DELAY_RE`).

    `binary_allocator`'s argument one layer down: a leg labelled by the
    variable it sets is a claim about what mimalloc read, and a variable it
    did not read — renamed, or a binary that sets its own — times the
    shipped delay under another column's name. **A stating leg whose binary
    reports another delay is refused**, and the shipped leg's is what its
    column names. Asked in a container as the legs run, under the sweep's
    limit, never on the host."""
    env = purge_delay_env(delay)
    if cfg.dry_run:
        return DRY_RUN_PURGE_DELAY if delay is None else delay
    argv = [
        *cfg.container_argv(), "run", "--rm", "-m", cfg.memory, "--memory-swap", cfg.memory,
        "-v", f"{binary}:/pgdt:ro",
        *(["-e", env] if env is not None else []),
        cfg.image, "/pgdt", "--version",
    ]  # fmt: skip
    found = set(PURGE_DELAY_RE.findall(host.run(argv, capture=True)))
    if len(found) != 1:
        raise RuntimeError(
            f"{binary} --version names {sorted(found) or 'no'} purge delay"
            + (f" under `{env}`" if env else "")
            + ": no leg is timed under a delay mimalloc does not report"
        )
    got = int(found.pop())
    if delay is not None and got != delay:
        raise RuntimeError(
            f"{binary} runs under a purge delay of {got} ms where its leg states `{env}`: "
            "the variable does not reach mimalloc, and the column would time another delay"
        )
    return got
