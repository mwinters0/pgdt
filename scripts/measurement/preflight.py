"""What is refused before the first reading: an apparatus that cannot run, a container limit the
machine cannot hold, a mount the container cannot see, a counter that does not count."""

from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path
from typing import Callable, Sequence
from typing import TYPE_CHECKING

import contained

from measurement import formatting
from measurement import parsing
from measurement.builds import image

if TYPE_CHECKING:
    from measurement.config import Config
    from measurement.figures.declaration import Figure


def counter_preflight(cfg: Config, log: Callable[[str], None]) -> list[str]:
    """`count-instructions` around `/bin/true` in the pinned image, under
    `COUNT_INSTRUCTIONS_CAPS`, before a counted figure's first reading: one
    problem line where it does not count there.

    **Run only where a selected figure counts**, so a machine whose kernel or
    runtime refuses the counter can still take every figure that reads none.
    The count is logged, never published: it is the dynamic loader's and
    `true`'s, the floor no counted leg stands on, since the counter is enabled
    at the command's `exec` and counts nothing of its own."""
    try:
        counter = image.ensure_count_instructions_binary(cfg, log)
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        return [f"`count-instructions` could not be built: {exc}"]
    if cfg.dry_run:
        return []
    argv = [
        *cfg.container_argv(), "run", "--rm",
        *(a for cap in parsing.COUNT_INSTRUCTIONS_CAPS for a in ("--cap-add", cap)),
        "-v", f"{counter}:{parsing.COUNT_INSTRUCTIONS}:ro",
        cfg.image, parsing.COUNT_INSTRUCTIONS, "/bin/true",
    ]
    proc = subprocess.run(argv, text=True, capture_output=True)
    try:
        count = parsing.parse_instructions(proc.stderr)
    except ValueError:
        count = None
    if proc.returncode != 0 or count is None:
        tail = " / ".join(proc.stderr.strip().splitlines()[-PREFLIGHT_STDERR_LINES:])
        return [
            f"`count-instructions` around /bin/true does not count in {cfg.image} "
            f"(exit {proc.returncode}: {tail}): the kernel's `perf_event_paranoid` or the "
            "runtime's seccomp profile refuses `perf_event_open`, so no counted figure can be taken"
        ]
    log(
        f"count-instructions floor: {count} instructions:u around /bin/true in the pinned "
        "image — never published"
    )
    return []

#: What `apparatus_preflight` quotes of a refused start: the tail of its
#: stderr, which is where the loader names the symbol version it lacks.
PREFLIGHT_STDERR_LINES = 3


def apparatus_preflight(cfg: Config, log: Callable[[str], None]) -> list[str]:
    """The two binaries every figure mounts, started in the pinned image before
    the first reading, one problem line for each that does not start.

    **`pgdt --version` must start there.** The image is the release image's
    base (`register_image`), so a binary the release image built links
    against the glibc it runs under and starts by construction; what fails
    here is a binary built anywhere else — `PGDT_MEASURE_BIN` naming a host
    build — which would fail every leg, so it is refused before the first.

    **`peak-rss` around `/bin/true`** proves it starts there, and its reading
    is the floor every resident reading stands on — the spawning side's share
    it bounds (`crates/peak-rss/src/main.rs`) — logged, never published."""
    if cfg.dry_run:
        image.ensure_peak_rss_binary(cfg, log)
        return []
    try:
        peak = image.ensure_peak_rss_binary(cfg, log)
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        return [f"`peak-rss` could not be built: {exc}"]
    base = [
        *cfg.container_argv(), "run", "--rm",
        "-v", f"{cfg.bin_pgdt}:/pgdt:ro",
        "-v", f"{peak}:{parsing.PEAK_RSS}:ro",
        cfg.image,
    ]

    def start(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run([*base, *argv], text=True, capture_output=True)

    def tail(proc: subprocess.CompletedProcess[str]) -> str:
        return " / ".join(proc.stderr.strip().splitlines()[-PREFLIGHT_STDERR_LINES:])

    problems = []
    pgdt = start(["/pgdt", "--version"])
    if pgdt.returncode != 0:
        problems.append(
            f"{cfg.bin_pgdt} does not start in the pinned image {cfg.image} "
            f"(exit {pgdt.returncode}: {tail(pgdt)}): it was not built in the release image, "
            "whose base this is — unset PGDT_MEASURE_BIN and let the harness build it there, "
            "or point it at a binary `release.py build` made"
        )
    floor = start([parsing.PEAK_RSS, "/bin/true"])
    readings = parsing.MAXRSS_RE.findall(floor.stderr)
    if floor.returncode != 0 or len(readings) != 1:
        problems.append(
            f"`peak-rss` around /bin/true does not report one reading in {cfg.image} "
            f"(exit {floor.returncode}: {tail(floor)})"
        )
    else:
        log(
            f"peak-rss floor: {formatting.fmt_mib(int(readings[0]))} around /bin/true in the pinned "
            "image — what a resident reading stands on, never published"
        )
    return problems


def memory_preflight(cfg: Config, figures: Sequence[Figure], machine: int) -> list[str]:
    """Each container a selected figure's legs run in — its own (`Figure.memory`)
    or the sweep's (`Config.memory`) — held to `contained.MAX_FRACTION` of
    `machine`, the memory of the machine the sweep runs on: one problem line for
    each past it, as `contained.py` refuses its own `--memory`.

    **One ceiling for every container a reading runs in**: a limit near the
    machine's is no limit, whether a figure or a hand run states it, and a
    figure's container sized on a larger machine is refused here rather than
    run past it."""
    problems: list[str] = []
    for what, memory in (
        ("the sweep's container (PGDT_MEASURE_MEMORY)", cfg.memory),
        *((f"{fig.id}'s container", fig.memory) for fig in figures if fig.memory),
    ):
        try:
            contained.check_memory(memory, machine, what)
        except ValueError as exc:
            problems.append(str(exc))
    return problems


def mount_preflight(cfg: Config, dirs: Sequence[Path]) -> list[str]:
    """Each staging directory read back from inside a container before the
    first reading, one problem line for each whose contents it does not see.

    **The harness stages in its own mount namespace and the container runtime
    mounts from the daemon's**, and the two need not agree: a process with a
    private `/tmp` or `/var/tmp` stages, and unlinks, in its own copy, while
    every container reads the runtime's — inputs an earlier sitting left, and
    a cache an earlier build wrote, found twenty minutes in or never. A nonce
    written from here and read there is the one test of that, and it costs one
    container start."""
    if cfg.dry_run:
        return []
    nonce = f"{os.getpid()}-{time.time_ns()}"
    probe = f".mount-probe-{nonce}"
    problems: list[str] = []
    written: list[Path] = []
    for d in dict.fromkeys(dirs):
        try:
            d.mkdir(parents=True, exist_ok=True)
            (d / probe).write_text(nonce)
            written.append(d)
        except OSError as exc:
            problems.append(f"{d} could not take the mount probe: {exc}")
    if not written:
        return problems
    argv = [*cfg.container_argv(), "run", "--rm"]
    for i, d in enumerate(written):
        argv += ["-v", f"{d}:/probe/{i}:ro"]
    script = "; ".join(f"cat /probe/{i}/{probe} 2>/dev/null; echo" for i in range(len(written)))
    try:
        proc = subprocess.run([*argv, cfg.image, "bash", "-c", script], text=True, capture_output=True)
    finally:
        for d in written:
            (d / probe).unlink(missing_ok=True)
    if proc.returncode != 0:
        tail = " / ".join(proc.stderr.strip().splitlines()[-PREFLIGHT_STDERR_LINES:])
        return [*problems, f"the mount probe did not start in {cfg.image} (exit {proc.returncode}: {tail})"]
    seen = proc.stdout.split("\n")
    for i, d in enumerate(written):
        got = seen[i] if i < len(seen) else ""
        if got != nonce:
            problems.append(
                f"{d} is not what a container mounts at that path: a probe written there "
                f"reads back {'nothing' if not got else repr(got)} inside one. This process "
                "and the container runtime see different directories — a private /tmp or "
                "/var/tmp, say — so every run would read the runtime's copy instead of what "
                "is staged. Run the sweep from a shell sharing the runtime's mount namespace"
            )
    return problems
