#!/usr/bin/env python3
"""The committed measurement harness: it runs the sweep behind
docs/design/measurements.md and emits that doc's tables.

Every re-take before this one was a throwaway script under runs/ that died
with the session, so each re-take re-derived the apparatus from scratch and
ended with a hand-written parser scraping medians out of a shell log --
which is where the transcription errors lived. This owns both halves: it runs
the measurements *and* emits the markdown, so folding a figure into
measurements.md is a paste rather than a transcription.

What it enforces, from that doc's standing rules:

* the timer is inside the container (`bash -c 'time ...'`), never around it,
  because `sudo nerdctl run` costs ~0.75 s and that is larger than several of
  these figures;
* a comparison table is re-taken **whole**, in one interleaved sweep -- each
  rep runs every file-and-mode combination in turn, and the second half of the
  reps runs them in the opposite order, because within a pair the second run is
  the warmer one;
* a warm figure is read from tmpfs and a cold one from the SSD with
  `drop_caches` before *every* run, including before the floor; a `cold-nvme`
  figure is the same discipline on the third device class, which is the only
  one where I/O and parse are within a small factor of each other and
  therefore the only one that can price a readahead or chunk-size default;
* every table carries its per-rep readings, so a wrong median is visible
  against the numbers that produced it (an SE from one sweep is not an error
  bar -- that doc's ninth rule);
* one libc: the default glibc build, in a glibc image pinned by digest, and
  the glibc each figure ran under named in the stamp or in its own marker.

**Selection is per figure, never finer.** One figure is exactly one table, and
a table is atomic -- half of one may not be re-taken. Figures that share a
reading say so and pull the other figure in rather than measuring it twice
(the allocator table's reference `parse` row *is* the warm scan-throughput
table's `COPY` row, and must be the same number).

**Each figure declares the paths that invalidate it**, so `--stale` can say
which figures a diff has made stale. That is the half a harness alone does not
fix: a one-line change to `map::Builder::on_row` invalidated two figures and
nothing announced it. **A section the register does not hold
declares one too** (`Outside.depends`), and its marker in the doc names the
commit its readings were taken at: being outside means the harness cannot
re-take them, not that nothing is told when they go wrong.

**Not every figure times `pgdt`.** `xz-decode-scaling` times the `xz_decode`
example instead, which reaches past the library to the decoder's own bulk entry
point -- the figure is about the decoder rather than about what the library
does with it.
The harness builds it (an *example* target, so `target/release/pgdt` is never
replaced), stages `.xz` inputs beside the plain ones, and gives that figure its
own container memory and its own contention row, both of which its table
declares. The dynamic-filter figures time **`pgdt sql`**, DataFusion's CLI,
with `pgdt parse` building the cache it reads first: it states its worker
count as its session's `target_partitions` — held by a test and said in its
tables.

**No build of another tree is measured here.** A subtraction between two
builds measures everything that differs between the two trees, which grows
every time the tree moves and the pinned side does not; what one mechanism
costs inside a run is an attribution, read off a profile
(`docs/design/roadmap.md`, "Attribution is introspective; only the gate is
blind"), not a second build differenced against the first.

Machine facts stay out of here: every path is an environment variable whose
default suits the machine CLAUDE.local.md describes, and the procedure lives in
CLAUDE.md.

Usage:

    cd scripts
    uv run measure.py --list
    uv run measure.py --figure map-only
    uv run measure.py --stage warm
    uv run measure.py --all
    uv run measure.py --stale --since <rev>
    uv run measure.py --profile-recipe
    uv run measure.py --heaptrack-recipe
    uv run python -m unittest test_measure -v

Three invocations are printed rather than run, for two reasons: koji's because
it is an hour on another medium, and the two instrument recipes' because
neither a sampling profile nor a heap recording is a figure at all. All three
live here because a command kept in prose is a command that stops running.
"""

from __future__ import annotations

import argparse
import atexit
import dataclasses
import functools
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Callable, Iterable, Mapping, Sequence

# The acknowledgement register, in its own module so that adding an entry does
# not touch a path any figure declares. Re-exported: `measure.ACKNOWLEDGED`
# and `measure.Acknowledged` still resolve.
from acknowledged import ACKNOWLEDGED, Acknowledged

# The perf generator's own column lists, imported rather than transcribed: a
# projection names columns, and a name this file spells for itself would
# survive a rename in the generator as a query that fails at run time in the
# middle of a sweep. Import-safe -- the module is stdlib-only and everything
# executable sits behind its `__main__` guard.
import generate_perf_data as perf

# The band the koji leg's compression ratio is gated to, imported for the same
# reason: the figure's table quotes the ratio every sitting, and a band spelled
# again here could drift from the one the generator actually refuses a slice
# against. Import-safe on the same terms.
from generate_xz_input import KOJI_RATIO_MAX, KOJI_RATIO_MIN

REPO = Path(__file__).resolve().parent.parent
SCRIPTS = REPO / "scripts"

GIB = 1024**3
MIB = 1024**2

#: The only path `cargo build --release -p pgdt` writes. It is a constant
#: rather than a literal inside `Config` because `ensure_pgdt_binary` reads it
#: back: the harness may claim to have built `cfg.bin_pgdt` only when the two
#: are the same file.
CARGO_RELEASE_BIN = REPO / "target/release/pgdt"


# --------------------------------------------------------------------------
# Configuration. Defaults describe this machine; every one is overridable so
# the harness survives a move (a harness that has to be rewritten after a
# machine move is the recurrence it exists to end).
# --------------------------------------------------------------------------


def _env(name: str, default: str) -> str:
    return os.environ.get(name, default)


@dataclass
class Config:
    # Where generated inputs live and are kept between sessions. Cold figures
    # read them in place, so this must be the SSD, not tmpfs.
    cache_dir: Path = Path(
        _env("PGDT_MEASURE_CACHE_DIR", "/mnt/ssd/fedora/scratch/pgdump_query/measure")
    )
    # tmpfs, for every warm figure.
    warm_dir: Path = Path(_env("PGDT_MEASURE_WARM_DIR", "/dev/shm/pgdt"))
    # A second *device*, not a second cache: the `cold-nvme` regime exists to
    # read the same bytes off a disk fast enough that the parse is not hidden
    # behind it, so what this path names has to be NVMe and not the SSD
    # `cache_dir` points at. Inputs are copied here from that cache and kept,
    # exactly as they are kept there -- it is disk, not RAM.
    nvme_dir: Path = Path(
        _env("PGDT_MEASURE_NVME_DIR", "/var/tmp/pgdump_query/measure")
    )
    # A cap on how much of the tmpfs the harness may fill, below the bound
    # `warm_bound` sets by design. It only ever lowers that bound: the room
    # is not grown (`docs/design/measurements.md`, "The apparatus").
    warm_budget: float | None = (
        float(os.environ["PGDT_MEASURE_TMPFS_BUDGET_GIB"])
        if "PGDT_MEASURE_TMPFS_BUDGET_GIB" in os.environ
        else None
    )
    out_dir: Path = Path(_env("PGDT_MEASURE_OUT_DIR", str(REPO / "runs")))

    container: str = _env("PGDT_MEASURE_CONTAINER", "sudo nerdctl")
    # **The one image every figure runs in**, of the build host's own
    # distribution, so the host-built `pgdt` links no symbol version it lacks
    # (`measurements.md`, "The apparatus"); `apparatus_preflight` refuses a
    # sweep whose binary does not start in it, and that is the only time the
    # pin moves. **Pinned by digest**, the tag before it being for a reader
    # only: a tag moves under the register, and what a figure names is the
    # glibc the image answers (`glibc_of`) rather than a tag's. A digest the
    # machine lacks is pulled by the first run.
    image: str = _env(
        "PGDT_MEASURE_IMAGE",
        "archlinux:base@sha256:f3691b4dde62ba4c4b6f0ae2c1fbf28e8c0c8c4b9a35c7e06dc1f70e21aa29f6",
    )
    memory: str = _env("PGDT_MEASURE_MEMORY", "512m")
    sudo: str = _env("PGDT_MEASURE_SUDO", "sudo")

    bin_pgdt: Path = Path(_env("PGDT_MEASURE_BIN", str(CARGO_RELEASE_BIN)))
    # The `allocator` figure's three legs. Each is a full cargo target dir, so
    # it goes on scratch rather than under `runs/`, which holds logs and small
    # binaries; the binaries themselves are copied into `runs/`. A separate
    # target dir per leg is not tidiness: a `--features` build writes
    # `target/release/pgdt`, so building a leg in the default dir would
    # silently replace `bin_pgdt` and every other figure in the same sweep
    # would be timed under the wrong allocator.
    alloc_build_root: Path = Path(
        _env(
            "PGDT_MEASURE_ALLOC_BUILD_ROOT",
            "/mnt/ssd/fedora/scratch/pgdump_query/alloc-builds",
        )
    )

    # The size of the seven large inputs. 3.00 GiB is the recorded apparatus;
    # anything else marks the run unpublishable.
    size_gib: float = float(_env("PGDT_MEASURE_SIZE_GIB", "3.0"))
    # Pin every CPU to SWEEP_GOVERNOR for the sweep. Off by default -- see
    # that constant for the measurement that says why.
    pin_governor: bool = _env("PGDT_MEASURE_PIN_GOVERNOR", "") not in ("", "0", "no")
    # Place each leg on one or two L3 groups by the threads it states, and the
    # harness on the other die (`PIN_CHOICES`). Off: `M178`'s sittings refuted
    # it, and anything else marks the run unpublishable.
    pin_cpus: str = _env("PGDT_MEASURE_PIN_CPUS", "off")
    # Mount the timed binaries from tmpfs, read once untimed before each run
    # (`Session.stage_binary`). On: it is the recorded apparatus, in every
    # regime, and anything else marks the run unpublishable.
    stage_binaries: str = _env("PGDT_MEASURE_STAGE_BINARIES", "on")
    # Rep-count override, for smoke runs only. None means each figure's own.
    reps_override: int | None = None
    dry_run: bool = False
    keep_warm: bool = False
    # `--alone`: take exactly the figures named, borrowing nothing. It is a
    # property of the *run* rather than of the selection because of what it
    # makes the tables -- see `unpublishable_reason`.
    alone: bool = False

    @property
    def unpublishable_reason(self) -> str | None:
        """Why this run's tables must not be pasted into the doc, or `None`.

        Two of the three are the apparatus departing from the recorded one.
        The third is not an apparatus change at all: `--alone` takes exactly
        the figures named, so a figure that shares a reading publishes one it
        measured for itself rather than the run its table is set beside — which
        is precisely what the publication refusal declines to let a sitting
        take. Marking the *run* rather than exempting the flag is what keeps
        that refusal one rule: it is already guarded on publishability, so it
        stops firing for a diagnostic sitting by construction instead of by a
        clause naming a flag. What `--alone` is for survives intact — run one
        figure without the hour its borrowed sources cost, to see whether a
        change moved it.

        A sentence rather than a boolean because every reader of it says why:
        the log line, and the banner over the emitted tables."""
        if self.dry_run:
            return (
                "This run measured nothing — `--dry-run` prints what a sitting would do. "
                "It proves the harness runs; it is not a figure."
            )
        if abs(self.size_gib - 3.0) >= 1e-9 or self.reps_override is not None:
            return (
                "This run overrode the recorded apparatus "
                f"(inputs {self.size_gib} GiB, reps {self.reps_override or 'as declared'}). "
                "It proves the harness runs; it is not a figure."
            )
        if self.alone:
            return (
                "`--alone` took exactly the figures named, borrowing nothing, so any table "
                "below that republishes a shared reading measured it here for itself. "
                "It says whether a change moved a figure; it is not a table to fold in."
            )
        if self.arms != (Arm(),):
            return (
                f"This run placed its legs under an apparatus not adopted "
                f"(`--pin-cpus {self.pin_cpus}`, `--stage-binaries {self.stage_binaries}`). "
                "It is an experiment's sitting, read with `--arms` and `--drift`; "
                "it is not a figure."
            )
        return None

    @property
    def arms(self) -> tuple["Arm", ...]:
        """The arrangements every leg of this run is taken under, the first
        being the one `readings` and every table carry (`Arm`)."""
        pins = arm_values(self.pin_cpus, "--pin-cpus", Arm().pinned)
        stages = arm_values(self.stage_binaries, "--stage-binaries", Arm().staged)
        return tuple(Arm(pinned=p, staged=s) for p in pins for s in stages)

    @property
    def publishable(self) -> bool:
        """A run whose apparatus departs from the recorded one, or which took a
        deliberate partial sitting, must not have its tables pasted into the
        doc."""
        return self.unpublishable_reason is None

    def container_argv(self) -> list[str]:
        return shlex.split(self.container)

    def reps(self, declared: int) -> int:
        return self.reps_override if self.reps_override is not None else declared


# --------------------------------------------------------------------------
# Contention telemetry: what the machine was doing while a reading was taken.
#
# A reading is only as good as the machine was quiet, and the harness could
# not previously say how quiet it was. Two sweeps of the same 11 figures, on
# the same inputs and the same binaries, differ by up to **20% on `dd` alone**
# -- which contains none of this project's code. Without telemetry that is
# visible but not diagnosable.
#
# **Counters, not samples.** A warm reading is ~0.5 s, so PSI's `avg10` and a
# 1 Hz sampler each describe a window many times longer than the thing being
# measured. What works is the monotonic `total=` counters, read immediately
# either side of the timed run: their difference is stall *during this rep*.
# Frequency has no such counter under `amd-pstate-epp` (it exposes no
# `cpufreq/stats/time_in_state`), so frequency alone is sampled, and its
# per-reading value says how many samples it averaged -- which for a warm
# reading is one or two.
#
# **Witness, not divisor.** These numbers gate a reading; they never adjust
# one. Dividing by a "contention factor" needs a model of how contention maps
# to *this* workload's slowdown, and the same pair of sweeps shows that model
# cannot be a scalar: `dd` (memory-bandwidth-bound) moved +20.1% while the
# CPU-bound `INSERT` scan moved +0.4%. Any divisor correcting one over-corrects
# the other by 20x, and a mis-calibrated divisor emits a *confidently wrong*
# table -- the exact failure this harness exists to prevent. So contention is
# grounds to discard a reading and take it again: the discipline `drop_caches`
# already applies to a dirty page cache. Control the apparatus, never model it.
#
# **What no counter can see.** On a VM, a neighbour saturating memory
# bandwidth appears as neither steal nor PSI -- the vCPU is scheduled, nothing
# stalls on a runqueue, the instructions are simply slower. The only witness
# for that is a co-measured one, which is what the `dd` floor already is.
# Counters say *why* on bare metal; the floor is what catches a noisy host.
# Neither is a normaliser.
# --------------------------------------------------------------------------

#: The three pressure files, each carrying a `some` and a `full` line.
PSI_RESOURCES = ("cpu", "memory", "io")

#: `/proc/stat`'s aggregate `cpu` line, in order. `steal` is the field that
#: matters on a VM and is always zero on bare metal.
CPU_STAT_FIELDS = (
    "user",
    "nice",
    "system",
    "idle",
    "iowait",
    "irq",
    "softirq",
    "steal",
    "guest",
    "guest_nice",
)

#: How often the background sampler reads the quantities with no counter.
#: 5 Hz puts one or two samples inside a 0.5 s warm reading and costs tens of
#: microseconds a second, well under the noise it is there to measure.
SAMPLE_HZ = 5.0


def read_psi_totals(root: Path = Path("/proc/pressure")) -> dict[str, int]:
    """Monotonic stall counters in microseconds, keyed `"<resource>.<kind>"`
    (`"cpu.some"`, `"memory.full"`, ...).

    A missing file yields missing keys rather than an error: PSI is a kernel
    build option, and a harness that refused to run without it would be dead
    on arrival at the next machine.
    """
    totals: dict[str, int] = {}
    for resource in PSI_RESOURCES:
        try:
            text = (root / resource).read_text()
        except OSError:
            continue
        for line in text.splitlines():
            fields = line.split()
            if not fields:
                continue
            for field_text in fields[1:]:
                name, _, value = field_text.partition("=")
                if name == "total":
                    totals[f"{resource}.{fields[0]}"] = int(value)
    return totals


def read_cpu_jiffies(path: Path = Path("/proc/stat")) -> dict[str, int]:
    """The aggregate `cpu` line by field name, or `{}` where unreadable."""
    try:
        first = path.read_text().split("\n", 1)[0]
    except OSError:
        return {}
    fields = first.split()
    if not fields or fields[0] != "cpu":
        return {}
    return {name: int(v) for name, v in zip(CPU_STAT_FIELDS, fields[1:])}


@dataclass(frozen=True)
class Counters:
    """One instant's reading of every monotonic counter, with the clock it was
    read at. Two of these bracket a timed run."""

    monotonic: float
    psi: dict[str, int]
    cpu: dict[str, int]

    @classmethod
    def read(cls) -> "Counters":
        return cls(time.monotonic(), read_psi_totals(), read_cpu_jiffies())


def counter_delta(before: Counters, after: Counters) -> dict[str, float]:
    """What the machine did between two snapshots.

    Stall is reported absolutely (microseconds) and as a percentage of the
    window, and the percentage is the comparable one: it is what puts a 0.5 s
    warm reading and an 8 s `INSERT` scan on the same scale.

    `cpu_busy_pct` is the **whole machine, this run included**. On a 24-core
    box one scan accounts for a few percent, so a useful threshold sits well
    above what a reading costs on its own.
    """
    elapsed = max(after.monotonic - before.monotonic, 1e-9)
    out: dict[str, float] = {"window_s": round(elapsed, 4)}
    for key in sorted(set(before.psi) & set(after.psi)):
        stalled_us = after.psi[key] - before.psi[key]
        flat = key.replace(".", "_")
        out[f"psi_{flat}_us"] = float(stalled_us)
        out[f"psi_{flat}_pct"] = round(100.0 * stalled_us / 1e6 / elapsed, 3)
    jiffies = {k: after.cpu[k] - before.cpu[k] for k in set(before.cpu) & set(after.cpu)}
    total = sum(jiffies.values())
    if total > 0:
        idle = jiffies.get("idle", 0) + jiffies.get("iowait", 0)
        out["cpu_busy_pct"] = round(100.0 * (total - idle) / total, 2)
        out["cpu_steal_pct"] = round(100.0 * jiffies.get("steal", 0) / total, 3)
    return out


def cpu_freq_paths(root: Path = Path("/sys/devices/system/cpu")) -> list[Path]:
    """One frequency file per CPU, preferring the aperf/mperf-derived *actual*
    average (`cpuinfo_avg_freq`) over `scaling_cur_freq`, which reports what
    was requested rather than what was delivered."""
    paths = []
    for cpu in sorted(root.glob("cpu[0-9]*")):
        for name in ("cpufreq/cpuinfo_avg_freq", "cpufreq/scaling_cur_freq"):
            candidate = cpu / name
            if candidate.exists():
                paths.append(candidate)
                break
    return paths


#: hwmon `name` values worth reading a temperature from, most-wanted first.
#: The CPU package is what matters; a drive's temperature is not why a warm
#: reading moved.
CPU_HWMON_NAMES = ("k10temp", "coretemp", "zenpower")


def thermal_paths(
    thermal_root: Path = Path("/sys/class/thermal"),
    hwmon_root: Path = Path("/sys/class/hwmon"),
) -> list[Path]:
    """CPU temperature inputs, in milli-degrees.

    Two sources because neither is universal: `thermal_zone*` is the portable
    one and this AMD box has none, exposing its package temperature through
    hwmon's `k10temp` instead. Only CPU sensors are collected -- an NVMe or
    chipset reading would dilute the maximum with something that has nothing
    to do with why a scan slowed down.
    """
    zones = sorted(p / "temp" for p in thermal_root.glob("thermal_zone*") if (p / "temp").exists())
    if zones:
        return zones
    out: list[Path] = []
    for hwmon in sorted(hwmon_root.glob("hwmon*")):
        try:
            name = (hwmon / "name").read_text().strip()
        except OSError:
            continue
        if name in CPU_HWMON_NAMES:
            out += sorted(hwmon.glob("temp*_input"))
    return out


class Sampler:
    """Background sampling for the two quantities with no kernel counter.

    Only frequency and temperature are here. Everything else is a counter and
    is read exactly around the run it belongs to, which is strictly better:
    sampling a 0.5 s reading at any affordable rate gives one or two points,
    while a counter difference covers the whole window by construction.
    """

    def __init__(self, hz: float = SAMPLE_HZ) -> None:
        self.interval = 1.0 / hz if hz > 0 else 0.0
        self.freq_paths = cpu_freq_paths()
        self.thermal_paths = thermal_paths()
        #: `(monotonic, mean kHz across cores, busiest core kHz, max milli-°C)`
        self.samples: list[tuple[float, float, float, float]] = []
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def sample_once(self) -> tuple[float, float, float, float]:
        khz: list[float] = []
        for p in self.freq_paths:
            try:
                khz.append(float(p.read_text()))
            except (OSError, ValueError):
                pass
        milli: list[float] = []
        for p in self.thermal_paths:
            try:
                milli.append(float(p.read_text()))
            except (OSError, ValueError):
                pass
        return (
            time.monotonic(),
            statistics.mean(khz) if khz else float("nan"),
            max(khz) if khz else float("nan"),
            max(milli) if milli else float("nan"),
        )

    def _loop(self) -> None:
        while not self._stop.wait(self.interval):
            self.samples.append(self.sample_once())

    def start(self) -> None:
        if self.interval <= 0 or self._thread is not None:
            return
        self._thread = threading.Thread(target=self._loop, name="pgdt-sampler", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2.0)
            self._thread = None

    def window(self, start: float, end: float) -> dict[str, float]:
        """Frequency and temperature over `[start, end]` in monotonic time.

        `freq_samples` is reported beside the mean because it is what says how
        much to trust it: a warm reading yields two samples, and a mean of two
        is a hint rather than a measurement.
        """
        inside = [s for s in self.samples if start <= s[0] <= end]
        if not inside:
            return {"freq_samples": 0}
        mean_khz = [s[1] for s in inside if s[1] == s[1]]
        busiest_khz = [s[2] for s in inside if s[2] == s[2]]
        temps = [s[3] for s in inside if s[3] == s[3]]
        out: dict[str, float] = {"freq_samples": len(inside)}
        if busiest_khz:
            # **The busiest core is the one that matters.** A scan is close to
            # single-threaded, so the mean across 24 cores is dominated by the
            # 23 idle ones and reads far below the frequency the work actually
            # ran at. The mean is kept beside it because the two together say
            # whether anything *else* was running.
            out["freq_busiest_mhz"] = round(statistics.mean(busiest_khz) / 1000.0, 1)
            out["freq_busiest_min_mhz"] = round(min(busiest_khz) / 1000.0, 1)
        if mean_khz:
            out["freq_allcore_mean_mhz"] = round(statistics.mean(mean_khz) / 1000.0, 1)
        if temps:
            out["temp_max_c"] = round(max(temps) / 1000.0, 1)
        return out


# --------------------------------------------------------------------------
# The apparatus: the machine state the recorded figures assume.
# --------------------------------------------------------------------------

#: The governor `--pin-governor` pins every CPU to.
#:
#: **Off by default, because on this machine it measures as a no-op.** The
#: hypothesis was that an 8x scaling range (0.56-4.67 GHz) under `powersave`
#: was moving memory-bandwidth-bound readings. It is not: `amd-pstate-epp` is
#: a hardware-managed P-state driver where the governor name is very nearly
#: cosmetic and the energy-performance preference does the work, so a busy
#: core boosts to 4.55 GHz under `powersave` and 4.55 GHz under `performance`,
#: with an identical `dd` median either way (0.270 s over three runs of a
#: 3 GiB tmpfs read, 2026-08-28).
#:
#: The mechanism is kept and left off rather than deleted, because the
#: reasoning is machine-specific: a box on `acpi-cpufreq` with a genuine
#: `ondemand`/`powersave` governor would show exactly the effect this was
#: written for, and there the flag is a one-word fix rather than a rewrite.
#:
#: Pinning is an **apparatus change**: a figure taken under it is not
#: comparable with one taken without it, so turning it on obliges a full
#: re-sweep.
SWEEP_GOVERNOR = "performance"

#: What `--pin-cpus` and `--stage-binaries` take: the arrangement off every
#: leg (`off`), on every leg (`on`), or both arms, each leg taken under one
#: and then the other, alternating which goes first rep by rep (`alternate`)
#: — the experiment's shape, so its arms share every minute of the machine's
#: own drift rather than a sitting each. Which of `off` and `on` is the
#: recorded apparatus is `Arm`'s defaults.
ARM_CHOICES = ("off", "on", "alternate")


def arm_values(choice: str, flag: str, recorded: bool) -> tuple[bool, ...]:
    """The arms one `ARM_CHOICES` value asks for, the `recorded` one first."""
    if choice == "off":
        return (False,)
    if choice == "on":
        return (True,)
    if choice == "alternate":
        return (recorded, not recorded)
    raise ValueError(f"{flag} takes one of {', '.join(ARM_CHOICES)}, not {choice!r}")


@dataclass(frozen=True)
class Arm:
    """One arrangement a leg is placed under: pinned to L3 groups or not, and
    its binaries on tmpfs or where cargo left them.

    **The defaults are the recorded apparatus**: unpinned, pinning being
    refuted, and staged, which is adopted in every regime
    (`measurements.md`, "The apparatus"). The first arm of `Config.arms` is
    the one every table renders, and a second arm's readings go to
    `raw.json`'s `arms` alone, read by `--arms` within one sitting and
    `--drift` across two."""

    pinned: bool = False
    staged: bool = True

    @property
    def name(self) -> str:
        return f"{'pinned' if self.pinned else 'unpinned'}+{'staged' if self.staged else 'unstaged'}"


#: The L3 groups `--pin-cpus` places on, as indices into `l3_groups`' order
#: (by lowest CPU). **This machine's arrangement, deliberately**: a 3900X is
#: four L3 groups of three cores and their SMT siblings, two to a die, which
#: sysfs does not expose as dies — so the placement is declared here and
#: `pin_topology_problem` refuses any other topology rather than guessing one.
#:
#: A leg stating up to one group's CPUs runs on group 1, one stating up to two
#: groups' on groups 0 and 1 (die 0), and the harness beside a pinned leg — its
#: sampler and every process it launches — on die 1, as the admitting entry
#: settled. A leg stating more, or discovering its count, stays unpinned:
#: its subject is the whole machine, and discovery reads the affinity mask a
#: cpuset would narrow (`runtime-invariants.md`, "RT7").
#:
#: **Its count is its parallelism, not every size a program reads off its
#: CPUs.** A pinned `pgdt sql` leg states one partition and its provider spawns no
#: task, yet `#[tokio::main]`'s worker pool and DataFusion's
#: `planning_concurrency` still follow the cpuset: one worker per CPU given, as
#: unpinned, so fewer idle workers. That comes with the placement, and adoption
#: would install exactly it, so the arms compare what would be adopted and need
#: not attribute. *Rejected:* stating `TOKIO_WORKER_THREADS` on every run —
#: at the machine's count it oversubscribes a pinned leg four to one, at
#: `SWEEP_JOBS` it re-shapes every published leg; and leaving the
#: dynamic-filter legs unpinned, dropping the figures the experiment is for.
PIN_SMALL_GROUPS: tuple[int, ...] = (1,)
PIN_LARGE_GROUPS: tuple[int, ...] = (0, 1)
PIN_HARNESS_GROUPS: tuple[int, ...] = (2, 3)
#: How many L3 groups the placement above is written for.
PIN_GROUP_COUNT = 4

CPU_SYSFS = Path("/sys/devices/system/cpu")


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


def l3_groups(root: Path = CPU_SYSFS) -> list[frozenset[int]]:
    """The machine's L3 groups, each the CPUs sharing one, by lowest CPU."""
    groups = {
        parse_cpu_list(path.read_text())
        for path in root.glob("cpu[0-9]*/cache/index3/shared_cpu_list")
    }
    return sorted(groups, key=min)


def pin_topology_problem(groups: Sequence[frozenset[int]]) -> str | None:
    """Why `--pin-cpus` cannot place on this machine, or `None`.

    The placement names groups by index, so a machine with another count, or
    groups of unequal size, would put a leg somewhere nobody chose."""
    if len(groups) != PIN_GROUP_COUNT or len({len(g) for g in groups}) != 1:
        return (
            f"--pin-cpus is written for {PIN_GROUP_COUNT} equal L3 groups and this machine "
            f"has {len(groups)} ({'; '.join(cpu_list(g) for g in groups) or 'none readable'})"
        )
    return None


def placement(threads: int | None, groups: Sequence[frozenset[int]]) -> frozenset[int] | None:
    """The CPUs a leg stating `threads` is pinned to, or `None` for unpinned."""
    if threads is None:
        return None
    for chosen in (PIN_SMALL_GROUPS, PIN_LARGE_GROUPS):
        cpus = frozenset().union(*(groups[i] for i in chosen))
        if threads <= len(cpus):
            return cpus
    return None


def harness_cpus(groups: Sequence[frozenset[int]]) -> frozenset[int]:
    """Where the harness runs beside a pinned leg."""
    return frozenset().union(*(groups[i] for i in PIN_HARNESS_GROUPS))


#: The staging areas a regime can read from, and the only values a `Regime`
#: may name. Three devices, three areas: the SSD cache, the NVMe copy of it,
#: and the tmpfs staging.
STAGING_AREAS = ("cold", "nvme", "warm")


@dataclass(frozen=True)
class Regime:
    """One regime: which staging area its input is read from, and whether the
    page cache is dropped before every reading.

    **Two declared facts rather than one declared and one read off the name.**
    Both were once inferred from the string: `Session.input_path` matched
    `cold` and `cold-nvme` and returned the *warm* path for everything else,
    while the cache drop fired on any name starting with `cold`. Those two
    guesses disagree for exactly the regime nobody had added yet -- a
    `cold-parallel` row would have dropped the page cache and then read from
    tmpfs, publishing warm readings under a cold heading with no error, since
    each half is separately plausible and neither is checked against the other.
    Declaring them makes the pair the row's business.

    *Rejected: deriving `drops_caches` from `area`.* Today `area != "warm"`
    gives the same four answers, so the field looks redundant. It is not: the
    two are separate questions, and a figure wanting a **second** read off a
    device with the page cache left intact is a regime that declares a device
    area and no drop. Under the derivation that row cannot be written at all
    without first unpicking the rule that forbids it, which is the cost of
    every "these always agree" shortcut -- it holds until the case it was
    meant to describe arrives.
    """

    #: One of `STAGING_AREAS`.
    area: str
    #: Whether `time_run` drops the page cache before the reading. True of
    #: every regime reading a device, because what "cold" means is that the
    #: bytes come off the disk -- a `cold-nvme` reading taken out of page cache
    #: would measure RAM and read as a device with no floor at all.
    drops_caches: bool


#: Every regime the harness knows. **A name with no row here is refused**, not
#: defaulted: `regime_spec` raises, so a regime added to a figure's `stage` and
#: nowhere else fails at the first reading rather than quietly taking one off
#: the wrong device. `test_measure.py` reconciles these names against the
#: regimes the registered figures declare and against `CONTENTION_LIMITS`,
#: both ways, so a new regime cannot land half-declared.
REGIMES: dict[str, Regime] = {
    "cold": Regime(area="cold", drops_caches=True),
    "cold-nvme": Regime(area="nvme", drops_caches=True),
    "warm": Regime(area="warm", drops_caches=False),
    #: The same tmpfs staging as `warm`; what differs is the contention gate
    #: below, the reading deliberately occupying every hardware thread.
    "warm-parallel": Regime(area="warm", drops_caches=False),
}

#: Stage values that name no regime, so the reconciliation does not ask them
#: for a contention row. `criterion` is a `cargo bench` tripwire, which reads
#: no staged input at all; `derived` is a table computed across two sittings
#: rather than measured in one.
NON_REGIME_STAGES = frozenset({"criterion", "derived"})


def regime_spec(name: str) -> Regime:
    """The regime by that name, or a refusal.

    Never a fallback. The defect this exists against is not a crash but a
    plausible table: an unknown regime that resolves to *some* path takes a
    perfectly good reading of the wrong thing, and no column in the emitted
    table says which device it came off.
    """
    try:
        return REGIMES[name]
    except KeyError:
        raise ValueError(
            f"unknown regime {name!r}: declare it in `REGIMES` with the staging area it "
            f"reads from, and give it a `CONTENTION_LIMITS` row saying what contention "
            f"disqualifies one of its readings"
        ) from None


#: A reading whose window shows contention above these limits is not a
#: reading: it is discarded and taken again.
#:
#: **Per regime, because a cold figure's I/O stall is the measurement.** A cold
#: run drops the page cache and then reads 3.00 GiB off the SSD, so it stalls
#: on I/O for a fifth of its window *by construction* -- p95 21.3% against a
#: warm figure's 5.4%. One global I/O limit would either fire on every cold
#: reading or never fire at all, which is why `psi_io_some_pct` is gated in
#: neither regime: in the cold one it is signal, and in the warm one nothing
#: has yet been seen that it would catch and `cpu_busy_pct` would not.
#:
#: Calibrated from 182 readings of the 2026-08-28 `fa186ab` sweep, whose
#: apparatus lines witness a quiet machine throughout. Each limit sits at
#: roughly three times the observed p95, because the gate is here to catch a
#: machine that is *obviously* busy -- a limit tight enough to fire on ordinary
#: variance costs three retakes per reading and buys nothing:
#:
#: | metric | cold p95 | warm p95 | limit |
#: |---|---|---|---|
#: | `cpu_busy_pct` | 4.29 | 4.52 | 15 |
#: | `psi_cpu_some_pct` | 1.16 | 0.20 | 5 |
#: | `cpu_steal_pct` | 0.00 | 0.00 | 2 |
#:
#: `cpu_steal_pct` is zero on this bare-metal box in every reading ever taken,
#: so its limit is untested here and exists for the VM case, where steal is the
#: single most valuable number available.
#:
#: `cold-nvme` is the same regime on a faster device, so it is gated on the
#: same three limits. Naming it rather than falling back to `cold`'s row is
#: deliberate: a regime with no entry gates *nothing*, so a typo'd or newly
#: added regime would silently take every reading it was given. That is held
#: mechanically rather than by discipline -- `test_measure.py` reconciles these
#: keys against `REGIMES` and against the regimes the registered figures'
#: `stage` declarations name, in both directions.
#:
#: **`warm-parallel` is a fourth regime with a gate of its own, and it gates on
#: almost nothing.** It reads from tmpfs exactly as `warm` does; what differs is
#: that the reading deliberately occupies every hardware thread, so "the machine
#: is busy" is the measurement rather than a reason to discard it. Two of the
#: three limits above are therefore not limits here at all: a 24-worker decode
#: drives `cpu_busy_pct` to the nineties by construction, and its own workers
#: are what `psi_cpu_some_pct` sees. `cpu_steal_pct` survives, being about a
#: neighbour rather than about this run.
#:
#: What witnesses such a reading instead is *inside the table*: every row is
#: read against the one-worker row of the same sitting, so a machine that was
#: busy with someone else's work moves both and the ratio survives it. That is
#: the co-measured-floor argument the throughput tables already make, applied to
#: a figure whose floor is a row rather than a `dd`.
#:
#: Falling back to `warm`'s row would have been the alternative and is refused
#: for the reason the table above names: a regime with no row of its own gates
#: nothing, so the choice has to be visible.
CONTENTION_LIMITS: dict[str, dict[str, float]] = {
    "cold": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "cold-nvme": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "warm": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "warm-parallel": {"cpu_steal_pct": 2.0},
}

#: How many times a contended reading is retaken before its figure fails.
GATE_RETRIES = 3


def apparatus_note(records: Sequence[dict]) -> str:
    """One line under a figure's table saying how quiet the machine was while
    it was taken.

    The worst case, not the average: a table's median survives one bad rep,
    but a reader deciding whether to trust the number wants to know the worst
    the apparatus got. An empty string where there is no telemetry at all, so
    a machine without PSI emits the table it always did.
    """
    deltas = [r["telemetry"] for r in records if r.get("telemetry")]
    if not deltas:
        return ""
    def worst(key: str) -> float | None:
        seen = [d[key] for d in deltas if key in d]
        return max(seen) if seen else None
    def least(key: str) -> float | None:
        seen = [d[key] for d in deltas if key in d]
        return min(seen) if seen else None

    bits: list[str] = []
    cpu_stall = worst("psi_cpu_some_pct")
    if cpu_stall is not None:
        bits.append(f"CPU stall ≤{cpu_stall:.2f}%")
    io_stall = worst("psi_io_some_pct")
    if io_stall is not None:
        bits.append(f"I/O stall ≤{io_stall:.2f}%")
    busy = worst("cpu_busy_pct")
    if busy is not None:
        bits.append(f"machine ≤{busy:.0f}% busy")
    steal = worst("cpu_steal_pct")
    if steal is not None:
        bits.append(f"steal ≤{steal:.2f}%")
    freq = least("freq_busiest_min_mhz")
    if freq is not None:
        bits.append(f"busiest core ≥{freq / 1000:.2f} GHz")
    temp = worst("temp_max_c")
    if temp is not None:
        bits.append(f"≤{temp:.0f}°C")
    if not bits:
        return ""
    return "Apparatus over every run in this table: " + ", ".join(bits) + ".\n"


def contention_verdict(
    delta: dict[str, float],
    regime: str = "warm",
    limits: dict[str, dict[str, float]] | None = None,
) -> str | None:
    """The first limit this reading's telemetry breaks, or `None` if it is
    clean. Returns a sentence, because it goes straight into the log.

    A regime with no limits gates nothing, and a limit with no reading behind
    it does not fire -- a machine exposing no PSI must not fail every reading
    for lack of it.
    """
    table = CONTENTION_LIMITS if limits is None else limits
    for key, limit in sorted(table.get(regime, {}).items()):
        if key in delta and delta[key] > limit:
            return f"{key}={delta[key]} over limit {limit}"
    return None


class GovernorPin:
    """Pin every CPU's scaling governor for the duration of a sweep, and put
    it back afterwards.

    Restoration is the part that matters: this changes a machine-wide setting
    that outlives the process, so it is restored on the way out of the context
    **and** from an `atexit` hook, which covers the exits that skip `finally`.
    A failure to restore is logged loudly, with the commands to fix it by
    hand, rather than swallowed -- a machine left pinned is a changed
    apparatus for whatever runs next.
    """

    def __init__(
        self,
        governor: str = SWEEP_GOVERNOR,
        sudo: str = "sudo",
        log: Callable[[str], None] = print,
        root: Path = Path("/sys/devices/system/cpu"),
    ) -> None:
        self.governor = governor
        self.sudo = sudo
        self.log = log
        self.root = root
        self.previous: dict[Path, str] = {}
        self.error: str | None = None

    def files(self) -> list[Path]:
        return sorted(self.root.glob("cpu[0-9]*/cpufreq/scaling_governor"))

    def _write_all(self, values: dict[Path, str]) -> None:
        # One `sudo sh -c` for the whole set: 24 separate elevations is 24
        # chances to be prompted, and the point is an unattended sweep.
        script = "; ".join(
            f"echo {shlex.quote(v)} > {shlex.quote(str(p))}" for p, v in values.items()
        )
        run(shlex.split(self.sudo) + ["sh", "-c", script])

    def __enter__(self) -> "GovernorPin":
        files = self.files()
        if not files:
            self.error = "no scaling_governor files — cpufreq is not exposed here"
            self.log(f"!! governor not pinned: {self.error}")
            return self
        try:
            self.previous = {p: p.read_text().strip() for p in files}
            self._write_all({p: self.governor for p in files})
        except Exception as exc:
            self.error = str(exc)
            self.previous = {}
            self.log(f"!! governor not pinned: {exc}")
            return self
        was = sorted(set(self.previous.values()))
        self.log(f"governor pinned to {self.governor} (was {', '.join(was)})")
        atexit.register(self.restore)
        return self

    def restore(self) -> None:
        if not self.previous:
            return
        previous, self.previous = self.previous, {}
        try:
            self._write_all(previous)
            self.log(f"governor restored to {', '.join(sorted(set(previous.values())))}")
        except Exception as exc:  # loud: the machine has been left changed
            self.log(f"!! GOVERNOR NOT RESTORED ({exc}) — restore it by hand:")
            for p, v in previous.items():
                self.log(f"     echo {v} | sudo tee {p}")

    def __exit__(self, *exc_info: object) -> None:
        self.restore()

    @property
    def ok(self) -> bool:
        return self.error is None


# --------------------------------------------------------------------------
# Numbers and formatting. This is the part a silent error would hurt most, so
# it is the part test_measure.py covers.
# --------------------------------------------------------------------------


def median(values: Sequence[float]) -> float:
    if not values:
        raise ValueError("median of no readings")
    return statistics.median(values)


def spread(values: Sequence[float]) -> tuple[float, float]:
    if not values:
        raise ValueError("spread of no readings")
    return (min(values), max(values))


def fmt_s(value: float) -> str:
    """Seconds, at the precision measurements.md quotes: three decimals below
    two seconds, two above -- the image's timer resolves a microsecond
    (`TIME_FORMAT`), which no table quotes."""
    return f"{value:.3f}" if abs(value) < 2 else f"{value:.2f}"


def fmt_median_spread(values: Sequence[float]) -> str:
    lo, hi = spread(values)
    return f"**{fmt_s(median(values))} s** ({fmt_s(lo)}–{fmt_s(hi)})"


def fmt_rate(nbytes: int, seconds: float) -> str:
    """Decimal MB/s, which is what every rate in measurements.md is."""
    return f"~{nbytes / seconds / 1e6:.0f} MB/s"


def fmt_delta(off: float, on: float) -> str:
    """The Δ column of a two-binary comparison: absolute then percent."""
    d = on - off
    return f"**{d:+.3f} s, {d / off * 100:+.0f}%**"


def fmt_readings(values: Sequence[float]) -> str:
    return ", ".join(fmt_s(v) for v in values)


def fmt_us_per_row(seconds: float, rows: int) -> str:
    return f"{seconds / rows * 1e6:+.2f}"


def fmt_mib(kib: float) -> str:
    """A resident-set reading, in MiB.

    Mebibytes rather than decimal megabytes because the only thing this figure
    is read against is a bound stated in them -- `4 x max(8 MiB, announced)`
    from the buffer pool -- and a claim compared against a bound must not
    change units on the way."""
    return f"{kib / 1024:.2f} MiB"


def fmt_mib_median_spread(values: Sequence[float]) -> str:
    lo, hi = spread(values)
    return f"**{fmt_mib(median(values))}** ({lo / 1024:.2f}–{hi / 1024:.2f})"


def fmt_rss_delta(kib: float) -> str:
    """A *difference* between two resident-set readings.

    Two scales, because this figure's two axes are three orders of magnitude
    apart: the per-byte difference is tens of kibibytes, which MiB would round
    to `0.00`, and the per-block one is tens of mebibytes, which KiB would
    print as a five-digit number nobody can read against the table above it."""
    if abs(kib) >= 1024:
        return f"{kib / 1024:+.2f} MiB"
    return f"{kib:+.0f} KiB"


def md_table(headers: Sequence[str], rows: Sequence[Sequence[str]]) -> str:
    """A GitHub-flavoured table in measurements.md's own style: no padding,
    one `---` per column."""
    out = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for row in rows:
        if len(row) != len(headers):
            raise ValueError(f"row has {len(row)} cells, table has {len(headers)} columns")
        out.append("| " + " | ".join(row) + " |")
    return "\n".join(out)


TIME_RE = re.compile(r"^real\s+(\d+)m([\d.]+)s\s*$", re.MULTILINE)
#: The CPU the timed command spent, beside its wall clock.
CPU_TIME_RE = re.compile(r"^(user|sys)\s+(\d+)m([\d.]+)s\s*$", re.MULTILINE)

#: What every in-container script is prefixed with, outside every command
#: shape as `OOM_ORACLE` is: bash's default report at six decimals rather than
#: three. **Bash 5.3 honours it and 5.2 clamps it to three**, so an image's
#: bash decides whether it reads to the microsecond or the millisecond; the
#: report's shape is the default's either way, which `TIME_RE` reads.
TIME_FORMAT = "TIMEFORMAT=$'\\nreal\\t%6lR\\nuser\\t%6lU\\nsys\\t%6lS'; "


def parse_bash_time(text: str) -> float:
    """Seconds off a `bash -c 'time ...'` report.

    bash writes `real\\t0m0.570s` to stderr. There is exactly one such line per
    timed command; more than one means two commands were timed and the caller
    built the wrong script."""
    matches = TIME_RE.findall(text)
    if not matches:
        raise ValueError(f"no `real` line in timed output:\n{text[-2000:]}")
    if len(matches) > 1:
        raise ValueError(f"{len(matches)} `real` lines in one timed run; the script times more than one command")
    minutes, seconds = matches[0]
    return int(minutes) * 60 + float(seconds)


def parse_bash_cpu(text: str) -> dict[str, float]:
    """The `user` and `sys` seconds beside a `real` line, keyed by name.

    Recorded per run rather than read: a CPU-bound reading's user time moves
    with the placement where its wall clock may not, which is what `M178`'s
    sittings want beside the drift."""
    return {name: int(m) * 60 + float(s) for name, m, s in CPU_TIME_RE.findall(text)}


#: What `peak-rss` prints, on stderr, beside bash's own `real` line.
MAXRSS_RE = re.compile(r"^maxrss_kib=(\d+)$", re.MULTILINE)


def parse_maxrss_kib(text: str) -> int:
    """The peak resident set of the timed process, in KiB.

    Exactly one line per wrapped command, for the same reason `parse_bash_time`
    insists on one `real` line: two would mean the script wrapped two."""
    matches = MAXRSS_RE.findall(text)
    if not matches:
        raise ValueError(f"no `maxrss_kib` line in wrapped output:\n{text[-2000:]}")
    if len(matches) > 1:
        raise ValueError(
            f"{len(matches)} `maxrss_kib` lines in one run; the script wraps more than one command"
        )
    return int(matches[0])


#: What the harness appends to every in-container script, after the timed
#: command and outside every timer, so a run can be asked whether the kernel
#: killed it.
#:
#: **The exit code cannot answer this.** `peak-rss` exits `128 + n` for a
#: death by signal `n` and names it, but a `SIGKILL` does not say who sent it,
#: so a leg the OOM reaper killed reads as one anything else killed; and
#: `nerdctl inspect` cannot answer it either, because `--rm` has destroyed the
#: container by the time there is anything to ask. The container's own
#: `memory.events` counter can, and it is read inside the container while it
#: still exists.
#:
#: **It is not part of any command shape.** `_script` builds what is measured;
#: this is the harness asking the container what happened afterwards, which is
#: why it is appended here rather than written into thirty branches — and why a
#: figure's recorded shape is unchanged by it. Nothing it does is timed: the
#: `time` builtin and `peak-rss` both closed before `$__st` is read.
OOM_ORACLE = "; __st=$?; cat /sys/fs/cgroup/memory.events >&2 || true; exit $__st"

#: The `oom_kill` line of a cgroup v2 `memory.events`.
OOM_KILL_RE = re.compile(r"^oom_kill (\d+)$", re.MULTILINE)


def parse_oom_kills(text: str) -> int | None:
    """How many processes the kernel OOM-killed in this run's cgroup, or `None`
    where the counter was not readable.

    `None` and `0` are different answers and the caller must keep them apart: a
    missing counter means the harness **cannot tell** what killed the run, which
    is the state the whole oracle exists to leave behind, so it can never be
    read as "not killed"."""
    matches = OOM_KILL_RE.findall(text)
    if not matches:
        return None
    return max(int(m) for m in matches)


#: A `key=value` line an instrument writes where the harness can read it. Two
#: write them today: the decode example, on stdout, which has no other output;
#: and `pgdt --features introspect`, into the file `INSTRUMENT_OUT_VAR` names,
#: which is a channel of its own rather than a stream it shares.
REPORTED_RE = re.compile(r"^([a-z_]+)=(\S+)$")

#: The environment variable naming the file `pgdt/src/introspect.rs` writes its
#: report to. **Unset means no report at all**, so a default build and an
#: unmeasured run of the instrument build behave identically.
#:
#: Shared in fact rather than in type: the two constants live in two languages
#: and `test_measure.py` holds them to each other. An environment variable
#: rather than a flag, so the instrumented leg's argv is the argv a figure
#: times — see `RunSpec.instrument`.
INSTRUMENT_OUT_VAR = "PGDT_INTROSPECT_OUT"

#: Where the report is mounted inside the container, and the directory under a
#: sitting's own output that is bind-mounted there. One report per rep lands
#: here and stays, beside `raw.json`: `live_peak_bytes`, the `mimalloc_*` and
#: the `mallinfo_*` fields are per-rep readings, and mimalloc's JSON and the
#: `malloc_info` XML under them are the detail no summary carries.
INSTRUMENT_MOUNT = "/introspect"
INSTRUMENT_DIR = "instrument"


def parse_reported(text: str) -> dict[str, str]:
    """The `key=value` lines a timed binary printed about its own run.

    A binary that reports what it did — how many bytes it decoded, how many
    workers its plan admitted, how many bytes it was holding — closes a gap a
    file size cannot: the harness would otherwise have to compute the plaintext
    volume behind a compressed input by a second mechanism, and the day the two
    disagreed the table would publish a rate over the wrong denominator.

    Lines that are not `key=value` are ignored rather than refused, so a binary
    is free to write whatever else it likes — which is what lets the
    introspection build carry mimalloc's JSON, `malloc_info`'s XML and its own
    scope note in the same file.

    The text is a stream's for the decode example and a **file's** for the
    introspection build (`INSTRUMENT_OUT_VAR`). The parse is the same either
    way; what the file buys is a channel with one writer, where stderr already
    carries `peak-rss`'s own per-rep `maxrss_kib=<n>` by this same grammar.
    """
    out: dict[str, str] = {}
    for line in text.splitlines():
        match = REPORTED_RE.match(line.strip())
        if match:
            out[match.group(1)] = match.group(2)
    return out


#: The worker count and byte budget a scan says it is running under, off its own
#: `scan started` line.
#:
#: **The library's line, not the CLI's provenance one.** Both carry the pair;
#: this one states it bare — `jobs=3 memory_bytes=204537856` — where the CLI's
#: wraps each number in the words that say where it came from, and what a
#: resident reading needs is the arrangement rather than its provenance. Both
#: `scan started` lines of a run print the same pair, so the first match stands.
SCAN_RESOLVED_RE = re.compile(r"\bscan started\b[^\n]*?\bjobs=(\d+) memory_bytes=(\d+)\b")


def parse_resolution(text: str) -> dict[str, str]:
    """What a run resolved for itself, read back off its own log.

    `{}` where the run printed no such line, which is every shape that is not a
    scan. **It is read for every shape rather than for the flagless family
    alone**: the pair is free to read, and a stated shape whose resolved pair is
    not the pair it stated is exactly the apparatus failure the worker-count
    reconciliation cannot see — `--jobs 24` inside a budget affording three
    readers is a row labelled 24 that ran three.

    It is the flagless legs that *need* it. Under discovery the count is
    `Discovered::resolve`'s answer to the allocation, so the reader count a
    resident set belongs to exists nowhere else — the harness cannot compute it
    without reimplementing the rule, which is the second authority
    `QUERY_SUBSTREAM_CAP` refuses by name.
    """
    match = SCAN_RESOLVED_RE.search(text)
    if not match:
        return {}
    return {"resolved_jobs": match.group(1), "resolved_budget": match.group(2)}


#: What `pgdt query` says on stderr about the rows it did not read, and how many
#: it returned: the pruning note (`PlanNoteKind::StatisticsPruned`), the early
#: stop's note, and the closing count.
PRUNING_NOTE_RE = re.compile(
    r"^note: row-group statistics rule out (\d+) of (\d+) group\(s\), so (\d+) of the "
    r"(\d+) byte\(s\) of rows",
    re.MULTILINE,
)
STOP_NOTE_RE = re.compile(
    r"^note: reading stopped early in (\d+) block\(s\) .*? a further (\d+) byte\(s\)",
    re.MULTILINE,
)
ROWS_RETURNED_RE = re.compile(r"^(\d+) row\(s\)$", re.MULTILINE)
#: What the CLI says in place of a count when no row passed.
NO_ROWS_RE = re.compile(r"^no rows found for ", re.MULTILINE)


def parse_query_notes(text: str) -> dict[str, str]:
    """What a `pgdt query` reported of its own reading: the groups and bytes
    its statistics skipped, the bytes an early stop left unread, and the rows
    it returned. `{}` for a run that printed none of them.

    **The process's own count, read beside the timing** — which is what makes
    a pruned leg that silently skipped nothing a refusal rather than a table
    saying pruning buys nothing (`run_statistics_pruning`)."""
    out: dict[str, str] = {}
    if match := PRUNING_NOTE_RE.search(text):
        keys = ("skipped_groups", "groups", "skipped_bytes", "bytes")
        out.update(zip(keys, match.groups()))
    if match := STOP_NOTE_RE.search(text):
        out["stopped_blocks"], out["unread_bytes"] = match.groups()
    if match := ROWS_RETURNED_RE.search(text):
        out["rows_returned"] = match.group(1)
    elif NO_ROWS_RE.search(text):
        out["rows_returned"] = "0"
    return out


#: Where `peak-rss` is mounted in every container: the resident-set
#: instrument, a workspace crate (`peak-rss/src/main.rs` says what it reads and
#: why), built static by `ensure_peak_rss_binary` so it needs nothing of the
#: image. A shape that reads a resident set runs its command under it, with
#: anything the command's environment needs assigned in front of it.
PEAK_RSS = "/peak-rss"


def criterion_median_ns(criterion_root: Path, full_id: str) -> float:
    """The median point estimate, in nanoseconds, out of criterion's own JSON.

    Read from estimates.json rather than scraped off the console: the console
    rounds, and a rounded ratio is how a table acquires a number nobody can
    reproduce. `benchmark.json` carries the `full_id`, so the mangled directory
    name (criterion replaces `/` with `_`) never has to be guessed."""
    for bench in sorted(criterion_root.glob("*/new/benchmark.json")):
        meta = json.loads(bench.read_text())
        if meta.get("full_id") == full_id:
            est = json.loads((bench.parent / "estimates.json").read_text())
            return float(est["median"]["point_estimate"])
    raise FileNotFoundError(f"no criterion benchmark {full_id!r} under {criterion_root}")


# --------------------------------------------------------------------------
# Inputs: generated once onto the SSD, copied into tmpfs for warm figures.
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class InputSpec:
    name: str
    generator: str
    args: tuple[str, ...]
    #: Scale `--size-mb`/`--size-gb` with Config.size_gib. The block-count
    #: inputs do not scale -- their size *is* the block count.
    scales: bool = True
    #: What the staged file is called. `.sql` for a plain dump; `.xz` for a
    #: compressed one, which is a *different kind of input* rather than a plain
    #: one under another name -- `pgdt` recognises a source by content, but the
    #: harness's own staging, eviction and stamping all key on the file name,
    #: and two inputs whose names collided would evict each other silently.
    suffix: str = ".sql"
    #: The input this one is built from, staged first and passed to the
    #: generator as `@SOURCE@`. The `.xz` legs are compressions of inputs the
    #: register already generates, and re-generating the plaintext separately
    #: would put two files with different bytes behind one figure's rows.
    #: Its stamp folds into this one's, so a change to the *source's* generator
    #: regenerates both.
    derives_from: str | None = None
    #: What this input weighs when it has never been generated, where that is
    #: not `size_gib`. A compressed input is a fraction of its plaintext, and a
    #: dry run that guessed 3.00 GiB for it would size the tmpfs budget against
    #: a file five times larger than the one it stages.
    nominal_bytes: int | None = None

    def argv(self, cfg: Config, out: Path) -> list[str]:
        args = list(self.args)
        if self.scales:
            args = [
                f"{cfg.size_gib * 1024:g}" if a == "@SIZE_MB@" else a for a in args
            ]
            args = [f"{cfg.size_gib:g}" if a == "@SIZE_GB@" else a for a in args]
        if self.derives_from:
            source = str(cfg.cache_dir / input_file(self.derives_from))
            args = [source if a == "@SOURCE@" else a for a in args]
        return ["uv", "run", self.generator, *args, str(out)]


def input_file(name: str) -> str:
    """What an input is called wherever it is staged.

    One function rather than an `f"{name}.sql"` at each of the dozen staging,
    eviction and preflight sites: the moment one input stopped being a plain
    dump, every one of those was a place the `.xz` leg could be staged under a
    name nothing else looked for."""
    return name + INPUTS[name].suffix


def _perf(name: str, *flags: str) -> InputSpec:
    return InputSpec(name, "generate_perf_data.py", (*flags, "--size-mb", "@SIZE_MB@"))


INPUTS: dict[str, InputSpec] = {
    # The brace-free control. Its lack of `{`/`[` is a contract, not an
    # accident: the scan-throughput figures are taken on it.
    "control": _perf("control", "--seed", "42"),
    # The instrument's own floor: identical shape, different seed.
    "control43": _perf("control43", "--seed", "43"),
    "composite": _perf("composite", "--composite", "--seed", "42"),
    "arrays": _perf("arrays", "--arrays", "--composite", "--seed", "42"),
    "large_object": InputSpec(
        "large_object",
        "generate_large_object_bench.py",
        ("--size-gb", "@SIZE_GB@", "--seed", "42"),
    ),
    "insert_run": InputSpec(
        "insert_run",
        "generate_insert_run_bench.py",
        ("--size-gb", "@SIZE_GB@", "--seed", "42"),
    ),
    # The quadratic's control: the same byte count in one COPY block.
    "one_block": InputSpec(
        "one_block", "generate_perf_data.py", ("--seed", "42", "--size-mb", "2"), scales=False
    ),
    # `statistics-pruning`'s input: the control's rows, same seed and draws,
    # with a low-cardinality column appended (`generate_pruning_bench.py`).
    "pruning": InputSpec(
        "pruning", "generate_pruning_bench.py", ("--seed", "42", "--size-mb", "@SIZE_MB@")
    ),
    # The dynamic-filter figures' input: the control's rows, same seed and
    # draws, with two join keys appended, and three small build tables
    # (`generate_dynamic_filter_bench.py`).
    "dynfilter": InputSpec(
        "dynfilter",
        "generate_dynamic_filter_bench.py",
        ("--seed", "42", "--size-mb", "@SIZE_MB@"),
    ),
}
for _n in (500, 1000, 2000, 4000):
    INPUTS[f"blocks{_n}"] = InputSpec(
        f"blocks{_n}",
        "generate_block_count_bench.py",
        ("--blocks", str(_n), "--out"),
        scales=False,
    )

# The two `.xz` legs of `xz-decode-scaling`, both at koji's own container
# parameters -- preset 6, so an 8 MiB LZMA2 dictionary, 24 MiB blocks, CRC64 --
# and both about 3.00 GiB of *plaintext*, which is the register's input size and
# the quantity a decode rate is a rate of.
#
# Two of them because the two answer the question differently and neither alone
# is honest. The generated leg is reproducible from committed sources on a
# machine that has never seen koji, which is what makes the figure re-takeable;
# the koji leg is real data at a real compression ratio, which is what the
# per-worker charge's arithmetic is written against.
INPUTS["control_xz"] = InputSpec(
    "control_xz",
    "generate_xz_input.py",
    ("--from-dump", "@SOURCE@"),
    scales=False,
    suffix=".xz",
    derives_from="control",
    # ~5.4x on the control's rows; a generous nominal, since over-reserving
    # tmpfs is recoverable and under-reserving is a figure lost mid-sweep.
    nominal_bytes=700 * MIB,
)
#: The same plaintext as `control_xz`, written in 128 MiB blocks instead of
#: koji's 24 MiB — the second block size `parallel-peak-rss` is taken at.
#:
#: **Two block sizes because a resident set at one of them is a number about the
#: file, not about the library.** A compressed reader's per-worker footprint is
#: one decoded block, so the same stated budget admits a different number of
#: concurrent readers at each size, and a figure taken at one would publish that
#: file's shape as the library's bound.
#:
#: 128 MiB is the size the locally recompressed koji copy has and what
#: `xz --block-size=128MiB` writes (`CLAUDE.local.md`), so it is a shape a user
#: reaches with one flag rather than a number chosen to make a point. It is also
#: the size the shipped 64 MiB budget declines outright: at `--jobs 1` this leg
#: reads through the streaming fallback, and every row above it block-decodes —
#: which is a discontinuity between the first two rows and is what the table
#: says about them.
#:
#: It derives from `control` exactly as `control_xz` does, so both compressed
#: legs are compressions of the same bytes and a change to the perf generator
#: regenerates all three.
INPUTS["control_xz128"] = InputSpec(
    "control_xz128",
    "generate_xz_input.py",
    ("--from-dump", "@SOURCE@", "--block-size", "128MiB"),
    scales=False,
    suffix=".xz",
    derives_from="control",
    # Larger blocks compress a shade better than smaller ones; the same
    # generous nominal `control_xz` carries, for the same reason.
    nominal_bytes=700 * MIB,
)
#: The koji download, and where in it the koji leg's streams are taken from.
#: **Not the head of the file**: koji's first 3 GiB of plaintext compresses
#: 56.19x against the whole file's 19.41x, and a decode rate is per plaintext
#: byte, so a slice cut there would report a rate for bytes unlike the rest of
#: the dump. Seeking past it costs one seek.
#:
#: **This offset is a sample, not a representative, and nothing pretends
#: otherwise.** koji sampled at twelve depths runs from 5.02x to 33.05x, so no
#: single offset stands for the file; 20 GB yields 15.70x, a little denser than
#: the middle. What that costs is confined to the absolute rates -- the scaling
#: curve is a within-leg ratio and barely moves -- and it is why the ratio is
#: gated rather than assumed (`docs/design/decisions.md`, "The compressed source and the cache").
KOJI_XZ = _env(
    "PGDT_KOJI_XZ", "/mnt/wd12t/fedora/koji/koji-2026-07-23.dump.multistream.xz"
)
KOJI_XZ_OFFSET = int(_env("PGDT_KOJI_XZ_OFFSET", str(20_000_000_000)))
#: How many whole streams the koji leg keeps. The download is one 24 MiB block
#: per stream, so 128 of them is 3.00 GiB of plaintext -- the register's input
#: size, and enough blocks that 24 workers are never clamped by the work.
KOJI_XZ_STREAMS = 128
INPUTS["koji_xz"] = InputSpec(
    "koji_xz",
    "generate_xz_input.py",
    (
        "--from-koji",
        KOJI_XZ,
        "--streams",
        str(KOJI_XZ_STREAMS),
        "--from-offset",
        str(KOJI_XZ_OFFSET),
    ),
    scales=False,
    suffix=".xz",
    nominal_bytes=250 * MIB,
)


def input_stamp(spec: InputSpec, cfg: Config) -> str:
    """A hash of the generator's source and the arguments it was given.

    The whole-file bench's lesson, applied to the harness's own inputs: a
    generated file regenerated only when *missing* means a checkout that
    already has one benchmarks pre-change bytes forever, silently -- and the
    population that has one is exactly the population comparing a new number to
    an old one.

    **A derived input folds its source's stamp in.** `control_xz` is a
    compression of `control`, so a change to the *perf* generator moves the
    bytes behind it exactly as it moves the bytes behind every other figure --
    and hashing only this generator would leave the compressed leg measuring
    pre-change rows for as long as the file sat there.
    """
    h = hashlib.sha256()
    h.update((SCRIPTS / spec.generator).read_bytes())
    h.update(repr(spec.argv(cfg, Path("OUT"))).encode())
    if spec.derives_from:
        h.update(input_stamp(INPUTS[spec.derives_from], cfg).encode())
    return h.hexdigest()


#: How many full-size inputs a sweep ever holds on tmpfs at once: the two one
#: paired cross-file difference needs. A figure whose warm set is larger is
#: split into sweeps of its own (`Figure.warm_groups`); the room is never grown
#: to hold it (`docs/design/measurements.md`, "The apparatus").
WARM_FULL_INPUTS = 2

#: Over those inputs' nominal size: the kilobytes a generator overshoots its
#: target by, and nothing more.
WARM_SLACK = 64 * MIB


def warm_bound(cfg: Config) -> int:
    """The tmpfs budget, by design: `WARM_FULL_INPUTS` full-size inputs plus
    `WARM_SLACK`. A constant of the apparatus rather than a sum over the
    figures selected, so a figure that outgrows it fails `test_measure.py`
    instead of growing the room every sweep needs."""
    return int(WARM_FULL_INPUTS * cfg.size_gib * GIB) + WARM_SLACK


#: The file preflight reserves the budget through, in the warm directory.
WARM_RESERVATION = ".budget-reservation"


class StagingError(RuntimeError):
    """An input could not be staged onto tmpfs. It aborts the sweep rather
    than failing one figure: every later figure stages through the same area,
    so the next one would fail the same way or, worse, read a partial file."""


def split_specs(
    figure: str, specs: Sequence[RunSpec], groups: Sequence[tuple[str, ...]]
) -> list[tuple[int, list[RunSpec]]]:
    """A split figure's specs by the group they are swept in, in group order,
    each group's specs in the order given; groups no spec reads are left out.
    A spec is swept in the first group staging its input unless it names a
    later one (`RunSpec.sweep`).

    **A spec a group cannot place is an error**, not a sweep of its own: a
    split figure reads every input warm, and a spec off tmpfs, over an input
    no group names, or naming a group that does not stage its input would be
    measured beside nothing it was declared with. So is a spec naming its
    first group, which would key one reading two ways."""
    parts: list[list[RunSpec]] = [[] for _ in groups]
    for spec in specs:
        homes = [gi for gi, group in enumerate(groups) if spec.input in group]
        where = spec.label or spec.command
        if regime_spec(spec.regime).area != "warm" or not homes:
            raise ValueError(
                f"{figure} is split into warm groups, and {where} reads "
                f"{spec.input!r} in the {spec.regime} regime, which no group stages"
            )
        if spec.sweep is not None and (spec.sweep not in homes or spec.sweep == homes[0]):
            raise ValueError(
                f"{figure}: {where} names sweep {spec.sweep}, but {spec.input!r} is first "
                f"staged in sweep {homes[0]} and in {homes}; a spec names only a later group "
                "staging its input"
            )
        parts[homes[0] if spec.sweep is None else spec.sweep].append(spec)
    return [(gi, part) for gi, part in enumerate(parts) if part]


def in_sweep(figure: str, spec: RunSpec, group: int) -> RunSpec:
    """`spec` as it is swept in `figure`'s warm group `group`: unchanged in the
    first group staging its input, or in an unsplit figure's one sweep, and
    naming the group otherwise (`RunSpec.sweep`)."""
    groups = EVERY_BY_ID[figure].staging_groups
    homes = [gi for gi, g in enumerate(groups) if spec.input in g]
    if len(groups) < 2 or (homes and homes[0] == group):
        return dataclasses.replace(spec, sweep=None)
    if group not in homes:
        raise ValueError(f"{figure}'s sweep {group} does not stage {spec.input!r}")
    return dataclasses.replace(spec, sweep=group)


def nominal_size(cfg: Config, name: str) -> int:
    """What an input would weigh, for a dry run that has not generated it."""
    spec = INPUTS[name]
    if spec.nominal_bytes is not None:
        return spec.nominal_bytes
    if not spec.scales:
        return 2 * MIB
    return int(cfg.size_gib * GIB)


def file_size(cfg: Config, path: Path, name: str) -> int:
    """The input's size on disk, or its nominal size in a dry run."""
    if path.exists():
        return path.stat().st_size
    if cfg.dry_run:
        return nominal_size(cfg, name)
    raise FileNotFoundError(path)


#: The subdirectory of the tmpfs staging area staged binaries go in.
STAGED_BIN_DIR = "bin"


class Stager:
    """Generates inputs onto the SSD once, then copies them into tmpfs as each
    figure needs them, evicting whatever no remaining figure wants.

    Copying beats regenerating: a 3 GiB copy is seconds where the generator is
    minutes, and a `--seed`ed generator is byte-for-byte reproducible so the
    cached file is the same file. Both happen on the **host** -- writing 3 GiB
    of tmpfs from inside a 512 MB container charges those pages to its cgroup
    and kills it."""

    def __init__(self, cfg: Config, log: Callable[[str], None]) -> None:
        self.cfg = cfg
        self.log = log
        #: The steps each input is wanted warm in. A step is one co-measured
        #: set: a figure's whole warm set, or one of its `warm_groups`.
        self.needs: dict[str, list[int]] = {}
        #: Each figure's steps, in its `staging_groups` order.
        self.steps: dict[str, list[int]] = {}
        #: What each step holds on tmpfs at once, by (figure, group index).
        self.group_need: dict[tuple[str, int], int] = {}
        self._profiles: dict[str, dict] = {}
        self._pretended: set[str] = set()
        self._staged: dict[str, int] = {}
        self._adopt_warm_dir()

    # -- generation -------------------------------------------------------

    def cold_path(self, name: str) -> Path:
        self.ensure_generated(name)
        return self.cfg.cache_dir / input_file(name)

    def ensure_generated(self, name: str) -> Path:
        spec = INPUTS[name]
        out = self.cfg.cache_dir / input_file(name)
        stamp = self.cfg.cache_dir / f"{name}.stamp"
        want = input_stamp(spec, self.cfg)
        if out.exists() and stamp.exists() and stamp.read_text().strip() == want:
            return out
        if self.cfg.dry_run:
            if name not in self._pretended:
                self._pretended.add(name)
                self.log(f"  [dry-run] would generate {name} -> {out}")
            return out
        self.cfg.cache_dir.mkdir(parents=True, exist_ok=True)
        # A derived input's source is generated first, and by this same path,
        # so its own stamp is checked rather than assumed present: the whole
        # point of folding the source's stamp into this one is that a moved
        # source generator rebuilds both files, not just the outer one's stamp.
        if spec.derives_from:
            self.ensure_generated(spec.derives_from)
        self.log(f"  generating {name} -> {out}")
        run(spec.argv(self.cfg, out), cwd=SCRIPTS)
        stamp.write_text(want + "\n")
        # A regenerated input invalidates whatever was profiled or staged off
        # the old bytes.
        self._profiles.pop(name, None)
        self._staged.pop(name, None)
        warm = self.cfg.warm_dir / input_file(name)
        if warm.exists():
            warm.unlink()
        return out

    # -- NVMe staging -----------------------------------------------------

    def nvme_path(self, name: str) -> Path:
        """The input on the NVMe, copied from the SSD cache and kept there.

        A copy rather than a second generation, for the reason the tmpfs
        staging copies: a `--seed`ed generator is byte-for-byte reproducible,
        so the two files are the same file, and a 3 GiB copy is seconds where
        the generator is minutes.

        **Stamped like the SSD cache, and kept like it.** The stamp is what
        stops a checkout that already has a copy from measuring pre-change
        bytes forever after the generator moves -- the same failure
        `ensure_generated` exists to prevent, one device along. Kept because
        this is disk rather than RAM: eviction buys nothing here, and a cold
        figure that had to re-copy 3 GiB before every sitting would pay for
        nothing."""
        src = self.ensure_generated(name)
        dst = self.cfg.nvme_dir / input_file(name)
        stamp = self.cfg.nvme_dir / f"{name}.stamp"
        want = input_stamp(INPUTS[name], self.cfg)
        if dst.exists() and stamp.exists() and stamp.read_text().strip() == want:
            return dst
        if self.cfg.dry_run:
            self.log(f"  [dry-run] would stage {name} -> {dst}")
            return dst
        self.cfg.nvme_dir.mkdir(parents=True, exist_ok=True)
        self.log(f"  staging {name} -> {dst}")
        shutil.copyfile(src, dst)
        stamp.write_text(want + "\n")
        return dst

    # -- tmpfs staging ----------------------------------------------------

    def _adopt_warm_dir(self) -> None:
        """Whatever a previous session left on tmpfs counts against the budget
        and is a candidate for eviction like anything else.

        Named per input rather than globbed by suffix: an input's file name is
        `input_file`'s to say, and a glob that knew about `.sql` alone stopped
        seeing half the staging area the day a `.xz` leg was registered."""
        if self.cfg.dry_run or not self.cfg.warm_dir.exists():
            return
        for name in INPUTS:
            path = self.cfg.warm_dir / input_file(name)
            if path.exists():
                self._staged[name] = path.stat().st_size

    def _warm_bytes(self) -> int:
        return sum(self._staged.values())

    def warm_path(self, name: str, step: int = 0) -> Path:
        """`name` on tmpfs, copied there if it is not, evicting for room.

        **Recorded as staged only once its copy has finished**, and a copy
        that fails removes what it wrote: an input marked staged before its
        copy left a partial file that later figures read as the input."""
        src = self.ensure_generated(name)
        dst = self.cfg.warm_dir / input_file(name)
        size = file_size(self.cfg, src, name)
        if self._staged.get(name) == size and (self.cfg.dry_run or dst.exists()):
            return dst
        self._staged.pop(name, None)
        self._make_room(size, step)
        if self.cfg.dry_run:
            self.log(f"  [dry-run] would stage {name} -> {dst}")
            self._staged[name] = size
            return dst
        self.log(f"  staging {name} -> {dst}")
        try:
            self.cfg.warm_dir.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dst)
        except OSError as exc:
            dst.unlink(missing_ok=True)
            raise StagingError(f"staging {name} onto {self.cfg.warm_dir} failed: {exc}") from exc
        self._staged[name] = size
        return dst

    def _make_room(self, need: int, step: int) -> None:
        budget = self.budget()
        while self._warm_bytes() + need > budget:
            if not self._staged:
                raise StagingError(
                    f"{need / GIB:.2f} GiB does not fit in a {budget / GIB:.2f} GiB "
                    "tmpfs budget even when empty"
                )
            # Evict what no remaining step wants; failing that, what is wanted
            # latest. An input the *current* step wants is never a victim --
            # that would be a budget too small for one warm set, which
            # `preflight` refuses before the first run.
            victim = max(self._staged, key=lambda n: self._next_need(n, step))
            if self._next_need(victim, step) <= step:
                raise StagingError(
                    "the tmpfs budget cannot hold one warm set at once: still needs "
                    f"{victim} and {need / GIB:.2f} GiB more"
                )
            self.log(f"  evicting {victim} from tmpfs")
            del self._staged[victim]
            if not self.cfg.dry_run:
                (self.cfg.warm_dir / input_file(victim)).unlink(missing_ok=True)

    def plan(self, figures: Sequence[Figure]) -> None:
        """Number the sitting's steps -- one per figure, or one per group of a
        figure split into `warm_groups` -- and record which steps want each
        input warm, so eviction can pick the one nothing is waiting on and,
        failing that, the one wanted latest. Also size each step, which is
        what `preflight` holds to the budget."""
        self.needs, self.steps, self.group_need = {}, {}, {}
        step = 0
        for fig in figures:
            self.steps[fig.id] = []
            for gi, group in enumerate(fig.staging_groups or ((),)):
                self.steps[fig.id].append(step)
                for name in group:
                    self.needs.setdefault(name, []).append(step)
                self.group_need[(fig.id, gi)] = sum(self.expected_size(n) for n in group)
                step += 1

    def step_of(self, figure: str, group: int) -> int:
        """The step a figure's group is staged at, or 0 for a figure this
        stager was not planned with -- whose inputs nothing else then wants,
        so every staged input is a candidate for eviction."""
        steps = self.steps.get(figure)
        return steps[group] if steps else 0

    def expected_size(self, name: str) -> int:
        """What an input weighs: measured if it has been generated, nominal if
        not. Nominal is the low estimate -- every generator overshoots its
        target by a few KB -- which is what `WARM_SLACK` is for."""
        path = self.cfg.cache_dir / input_file(name)
        if path.exists():
            return path.stat().st_size
        return nominal_size(self.cfg, name)

    def budget(self) -> int:
        """The tmpfs ceiling: `warm_bound`, or the explicit
        PGDT_MEASURE_TMPFS_BUDGET_GIB where that is lower."""
        bound = warm_bound(self.cfg)
        if self.cfg.warm_budget is not None:
            return min(bound, int(self.cfg.warm_budget * GIB))
        return bound

    def reserve(self, budget: int) -> str | None:
        """Allocate what the budget may still add to the staging area, then
        give it back: why it cannot be had, or `None`.

        **An allocation, not a reading of free space**: `statvfs` cannot see a
        per-user tmpfs quota, so a free-space check passed a budget the quota
        refused mid-sweep. `fallocate` on tmpfs is charged exactly as a write
        is, and gives back everything it took when it fails, so any limit
        refuses it here, before the first measurement."""
        want = budget - self._warm_bytes()
        if want <= 0:
            return None
        probe = self.cfg.warm_dir / WARM_RESERVATION
        try:
            self.cfg.warm_dir.mkdir(parents=True, exist_ok=True)
            fd = os.open(probe, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            try:
                os.posix_fallocate(fd, 0, want)
            finally:
                os.close(fd)
        except OSError as exc:
            return (
                f"{self.cfg.warm_dir} could not reserve {want / GIB:.2f} GiB, what the "
                f"{budget / GIB:.2f} GiB tmpfs budget may still stage there: {exc}. A "
                "per-user quota is invisible to `df`; point PGDT_MEASURE_WARM_DIR at a "
                "memory-backed filesystem that can hold the budget"
            )
        finally:
            probe.unlink(missing_ok=True)
        return None

    def preflight(self, figures: Sequence[Figure]) -> list[str]:
        """Everything knowable before the first run: does each warm set fit
        the budget, can the staging area actually hold the budget, do the
        inputs fit the disk they are generated onto.

        This exists because the alternative is finding out twenty minutes in,
        with a figure already lost and its dependants failing behind it."""
        problems: list[str] = []
        budget = self.budget()
        for fig in figures:
            groups = fig.staging_groups
            for gi, group in enumerate(groups):
                need = self.group_need.get((fig.id, gi), 0)
                if need > budget:
                    which = f"{fig.id}'s sweep over {', '.join(group)}" if len(groups) > 1 else fig.id
                    problems.append(
                        f"{which} needs {need / GIB:.2f} GiB of tmpfs at once, over the "
                        f"{budget / GIB:.2f} GiB budget"
                    )
        if not self.cfg.dry_run:
            problem = self.reserve(budget)
            if problem:
                problems.append(problem)
        wanted = {
            n
            for fig in figures
            for n in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs)
        }
        # A derived input's source is generated onto the same disk, so it is
        # part of what the space check is about even where no figure names it.
        for name in list(wanted):
            source = INPUTS[name].derives_from
            while source:
                wanted.add(source)
                source = INPUTS[source].derives_from
        missing = sum(
            self.expected_size(n)
            for n in wanted
            if not (self.cfg.cache_dir / input_file(n)).exists()
        )
        try:
            self.cfg.cache_dir.mkdir(parents=True, exist_ok=True)
            cache_free = shutil.disk_usage(self.cfg.cache_dir).free
        except OSError as exc:
            problems.append(f"{self.cfg.cache_dir} is not usable as an input cache: {exc}")
            return problems
        if missing > cache_free:
            problems.append(
                f"{self.cfg.cache_dir} has {cache_free / GIB:.2f} GiB free, under the "
                f"{missing / GIB:.2f} GiB of inputs still to generate"
            )
        problems += self._nvme_preflight(figures)
        return problems

    def _nvme_preflight(self, figures: Sequence[Figure]) -> list[str]:
        """Does the NVMe hold the copies the `cold-nvme` regime reads from.

        Its own check rather than a third branch of the one above, because the
        NVMe area is neither of the other two: it is not a budget to evict
        against like tmpfs, and it is not where the generators write. What it
        can fail on is a path that is not there and a device that is full, and
        both are knowable in the first second."""
        wanted = {n for fig in figures for n in fig.nvme_inputs}
        if not wanted:
            return []
        try:
            self.cfg.nvme_dir.mkdir(parents=True, exist_ok=True)
            free = shutil.disk_usage(self.cfg.nvme_dir).free
        except OSError as exc:
            return [f"{self.cfg.nvme_dir} is not usable as an NVMe staging area: {exc}"]
        want = sum(
            self.expected_size(n)
            for n in wanted
            if not (self.cfg.nvme_dir / input_file(n)).exists()
        )
        if want > free:
            return [
                f"{self.cfg.nvme_dir} has {free / GIB:.2f} GiB free, under the "
                f"{want / GIB:.2f} GiB of inputs the cold-NVMe figures still need copied "
                "there. Point PGDT_MEASURE_NVME_DIR at an NVMe volume with room"
            ]
        return []

    def _next_need(self, name: str, step: int) -> float:
        return min((i for i in self.needs.get(name, []) if i >= step), default=float("inf"))

    def cleanup(self) -> None:
        if self.cfg.keep_warm or self.cfg.dry_run:
            return
        for name in list(self._staged):
            path = self.cfg.warm_dir / input_file(name)
            if path.exists():
                self.log(f"  removing {path}")
                path.unlink()
            del self._staged[name]
        # The caches a run writes into the staging directory go with it, and
        # so do the binaries a staged leg ran (`Session.stage_binary`).
        for leftover in self.cfg.warm_dir.glob("*.dtcache"):
            leftover.unlink(missing_ok=True)
        shutil.rmtree(self.cfg.warm_dir / STAGED_BIN_DIR, ignore_errors=True)

    # -- profiling --------------------------------------------------------

    def profile(self, name: str) -> dict:
        """Row and column counts for an input, off `pgdt info --json`.

        Not a figure: it runs on the host, untimed, once per input, and only
        the tables that quote a per-row cost need it."""
        if name in self._profiles:
            return self._profiles[name]
        cache = self.cfg.cache_dir / f"{name}.profile.json"
        stamp = input_stamp(INPUTS[name], self.cfg)
        if cache.exists():
            got = json.loads(cache.read_text())
            if got.get("stamp") == stamp:
                self._profiles[name] = got
                return got
        # Resolved only on a miss: asking for a profile must not drag an input
        # back onto tmpfs that eviction has already taken off it.
        if self.cfg.dry_run:
            # Plausible shape, so a dry run exercises every division the emit
            # path performs rather than stopping at the first one.
            nominal = nominal_size(self.cfg, name)
            return {
                "stamp": stamp,
                "rows": nominal // 4000,
                "columns": 16,
                "bytes": nominal,
                "row_bytes": 4000,
            }
        self.log(f"  profiling {name} (host, untimed)")
        path = self.warm_path(name) if name in self._staged else self.cold_path(name)
        tmp = self.cfg.cache_dir / f"{name}.profile.dtcache"
        tmp.unlink(missing_ok=True)
        # `--jobs` even though nothing here is timed: the row counts this
        # returns are the divisor under every per-row number in the document,
        # and no invocation this harness makes inherits a CLI default that
        # moves underneath it (`SWEEP_JOBS`).
        run(
            [
                str(self.cfg.bin_pgdt), "parse",
                "--source", str(path),
                "--dtcache", str(tmp),
                "--jobs", str(SWEEP_JOBS),
                *NO_STATISTICS.split(),
            ],
            quiet=True,
        )
        out = run(
            [str(self.cfg.bin_pgdt), "info", "--dtcache", str(tmp), "--json"], capture=True
        )
        tmp.unlink(missing_ok=True)
        index = json.loads(out)
        rows, columns, data_bytes = 0, 0, 0
        for span in index["spans"]:
            copy = span.get("body", {}).get("Data", {}).get("Copy")
            if not copy:
                continue
            rows += copy["row_count"]
            columns = max(columns, len(copy["header"]["columns"]))
            data_bytes += copy["terminator_offset"] - copy["data_offset"]
        got = {
            "stamp": stamp,
            "rows": rows,
            "columns": columns,
            "bytes": path.stat().st_size,
            "row_bytes": round(data_bytes / rows) if rows else 0,
        }
        cache.write_text(json.dumps(got, indent=1) + "\n")
        self._profiles[name] = got
        return got


# --------------------------------------------------------------------------
# Running things.
# --------------------------------------------------------------------------


def run(
    argv: Sequence[str], cwd: Path | None = None, capture: bool = False, quiet: bool = False
) -> str:
    stdout = subprocess.PIPE if capture else (subprocess.DEVNULL if quiet else None)
    proc = subprocess.run(
        list(argv), cwd=str(cwd) if cwd else None, text=True, stdout=stdout, check=True
    )
    return proc.stdout or ""


@dataclass(frozen=True)
class RunSpec:
    """One timed command: a binary, an input, a command shape, a regime."""

    binary: str  # "pgdt" | "dfcli" (`pgdt sql`) | … | "none" (dd); `Session.binary_path`
    input: str
    command: str
    regime: str  # "cold" | "cold-nvme" | "warm"
    label: str
    #: The container memory limit this one run is given, overriding the figure's
    #: own. `None` — every spec but the reserve figure's flagless legs — takes
    #: the figure's `memory`, or the recorded 512 MB.
    #:
    #: **A per-spec limit exists because one figure's axis *is* the limit.** A
    #: flagless run resolves its whole arrangement from the allocation it was
    #: given, so two legs differing only in `-m` are two different readings, not
    #: two reps of one; every other figure states its flags and wants one
    #: container for the table.
    memory: str | None = None
    #: Whether this leg runs an **instrument build** and must therefore produce
    #: a report (`INSTRUMENT_OUT_VAR`).
    #:
    #: **It is a declaration, not a consequence of the binary.** A missing
    #: report otherwise reads as `{}`, which is right for every default build
    #: and indistinguishable from three apparatus bugs: a leg built without the
    #: feature, a leg pointed at the default binary, and a report that never
    #: reached the file. Declared here, the absence is an error at the rep that
    #: produced it rather than an empty column an hour later. A leg the kernel
    #: OOM-killed is the legitimate absence and stays censored, as `KILL_TOLERANT`
    #: leaves it.
    #:
    #: Not part of `key`: it says what the harness must find, not what
    #: arrangement was measured, and two legs cannot differ by it alone — the
    #: build does not change the argv.
    instrument: bool = False
    #: Which of a split figure's `warm_groups` this run is swept in, where its
    #: input is staged in more than one: `None` is the first group staging it,
    #: so only a later group's run states one (`in_sweep`). **Part of `key`**:
    #: the same input in two sweeps is two readings, each paired only with its
    #: own sweep's (`nested-end-to-end`'s control).
    sweep: int | None = None

    def key(self, figure: str) -> str:
        """This run's identity, which is what a reading is filed under.

        **The limit is part of it whenever there is one**, since two flagless
        legs differ by nothing else and would otherwise share a key — the
        failure `_attribution_specs` names, where two legs differing only in the
        words a table prints silently become one reading. It is appended rather
        than always present so that a past sitting's `raw.json` still renders:
        every spec that states no limit keys exactly as it did before. A later
        sweep's run is appended the same way."""
        base = f"{figure}/{self.binary}/{self.input}/{self.command}/{self.regime}"
        if self.memory is not None:
            base += f"/m={self.memory}"
        return base if self.sweep is None else f"{base}/sweep={self.sweep}"


#: The perf table's columns, split the way the generator writes them: the 16
#: scalars, then the two arrays under `--arrays`, then the composite under
#: `--composite`. Taken from the generator so a rename there reaches the
#: projections below.
_PERF_SCALARS = tuple(name for name, _ in perf.COLUMNS)
_PERF_ARRAYS = tuple(name for name, _ in perf.ARRAY_COLUMNS)
_PERF_COMPOSITE = tuple(name for name, _ in perf.COMPOSITE_COLUMNS)

#: The five projection widths `projection-widths` is taken at, over the
#: `--arrays --composite` file's 19 columns. Every list is a *subsequence* of
#: the file's own column order, so no row introduces a reordering as a second
#: variable, and each is a superset of the row above it -- which is what makes
#: the difference between two adjacent rows the cost of exactly the columns
#: they differ by.
#:
#: Each key is that width's column count, and a test is what holds it so.
#: Writing `len(_PERF_SCALARS)` as the key instead would keep the mapping
#: consistent with itself while quietly renaming the rows the doc's table
#: carries.
PROJECTION_WIDTHS: dict[int, tuple[str, ...]] = {
    0: (),
    1: ("v_smallint",),
    16: _PERF_SCALARS,
    17: _PERF_SCALARS + _PERF_COMPOSITE,
    19: _PERF_SCALARS + _PERF_ARRAYS + _PERF_COMPOSITE,
}


def projection_flags(width: int) -> str:
    """The `pgdt query` flags that ask for one width.

    Zero columns is `--no-columns` rather than an empty repetition of
    `--column`, which is the CLI this phase specifies; every other width
    repeats `--column`, because a comma-separated list cannot express a column
    name containing a comma."""
    if width not in PROJECTION_WIDTHS:
        raise ValueError(f"no projection registered at width {width}")
    names = PROJECTION_WIDTHS[width]
    if not names:
        return "--no-columns"
    return " ".join(f"--column {name}" for name in names)


#: The predicate shapes `predicate-terms` is taken at: a column of the control
#: file, and how many terms are asked against it.
#:
#: **Every term is false for every row, and the terms are OR'd.** `Or`
#: evaluates its children until one is `True`, so an all-false disjunction
#: evaluates every one of them on every row -- and no row survives, so the
#: decode, the Arrow build and the render are identically absent from every
#: row of the table. That is what makes the difference between two rows here
#: the predicate and nothing else. A conjunction would do the opposite: `And`
#: stops at the first non-`True` conjunct, so an N-term one costs what a
#: 1-term one does.
#:
#: **Two depths, because what a shared split would remove is the walk.**
#: `ResolvedTerm::eval` takes its field with `field_ranges(..).nth(i)`, from
#: the front of the row, once per term -- so a term against the control's
#: thirteenth column walks thirteen fields and one against its first walks
#: one. `v_bool` is the deep column rather than the last one because the two
#: depths must differ by the walk and not by what decoding the field costs,
#: and its values are one byte; `v_escaped` and `v_long_text` are the two the
#: row order otherwise recommends and both are the wrong shape for that.
PREDICATE_SHAPES: dict[str, tuple[str, int]] = {
    "deep-1": ("v_bool", 1),
    "deep-2": ("v_bool", 2),
    "deep-3": ("v_bool", 3),
    "deep-5": ("v_bool", 5),
    "shallow-5": ("id", 5),
    "shallow-1": ("id", 1),
}


def predicate_expr(shape: str) -> str:
    """The `--where` expression one shape asks for.

    Distinct literals rather than one repeated, so the tree really is N terms:
    nothing folds two terms (`decisions.md`, "Predicates"), but a table
    whose rows differ only in how often one term is repeated invites the
    reader to wonder."""
    if shape not in PREDICATE_SHAPES:
        raise ValueError(f"no predicate registered as {shape!r}")
    column, terms = PREDICATE_SHAPES[shape]
    if terms < 1:
        raise ValueError(f"predicate {shape!r} asks for no terms")
    return " OR ".join(f"{column}=zzz{i}" for i in range(1, terms + 1))


#: The read chunk sizes `chunk-size` is taken at, in bytes, smallest first.
#: `1 << 20` is the shipped default (`scan::SCAN_CHUNK_DEFAULT_SIZE_BYTES`) and every
#: other row is read against it.
#:
#: **The range brackets the read path's own buffer-pool ceiling.** 8 MiB is
#: the largest buffer `io::BufferPool::give` keeps for a length nobody
#: announced; a read loop announces its chunk size, so a 16 MiB chunk is
#: pooled like the rest. The 16 MiB row is in the table on purpose either way:
#: a sweep that stopped at the ceiling would leave a reader to assume the curve
#: continues, and this row is what says whether it does.
CHUNK_SIZES: tuple[int, ...] = (64 << 10, 256 << 10, 1 << 20, 4 << 20, 8 << 20, 16 << 20)

#: The row every other chunk-size row is a ratio against.
CHUNK_DEFAULT = 1 << 20

#: The worker counts `xz-decode-scaling` is taken at, and the row order of its
#: table. One is the probe this figure replaces; 24 is every hardware thread on
#: the recorded apparatus; 12 is its physical cores, which is where SMT stops
#: adding a core and starts sharing one.
DECODE_WORKERS: tuple[int, ...] = (1, 2, 4, 8, 12, 16, 24)

#: The row every other decode row is a ratio against — one worker, which is
#: what the serial path this project ships has today.
DECODE_BASELINE = 1

#: The `--jobs` values the two `parallel-*` figures are taken at, and the row
#: order of their tables.
#:
#: **The same counts as `DECODE_WORKERS`, deliberately.** The decode figure is
#: the floor these two are read against — it says what the decoder alone does
#: with N workers — so a row of one that has no counterpart in the other would
#: be a comparison nobody can make. The endpoints carry the same meaning here:
#: 1 is the serial path this project ships, 12 is the machine's physical cores,
#: 24 is every hardware thread.
#:
#: **The range deliberately runs past the count a plain source delivers.**
#: `POOL_DEPTH` clamps `BufferPool::slots()` to four and the interior split
#: grants `MayWait`, so a fifth fused worker on a plain file waits; the rows
#: above four keep that arrangement in the table. What the wait costs them is
#: not separated from anything else they pay (`docs/design/decisions.md`,
#: "D25").
PARALLEL_JOBS: tuple[int, ...] = (1, 2, 4, 8, 12, 16, 24)

#: The row every other parallel row is a ratio against: `--jobs 1`, which the
#: CLI turns into `Parallelism::Serial` — the serial code path this project
#: ships, not a pool of one.
PARALLEL_BASELINE = 1

#: The command-shape families whose worker count is a figure's **axis** rather
#: than the apparatus's constant, spelled as the prefixes `_script` dispatches
#: on. Each is an existing shape's name with `-jobs-<n>` appended, so the shape
#: a parallel row measures is nameable as "the warm throughput table's row,
#: under N workers".
#:
#: Declared as a constant because three things read it and a fourth would
#: otherwise have to guess: `_script` dispatches on it, `command_shapes`
#: enumerates it, and `pinned_count_problems` exempts it from `SWEEP_JOBS` —
#: beside the decode instrument's `decode-` shapes, the reserve figure's two
#: stated families (`RESERVE_JOBS`) and its flagless one, each by its own
#: prefix. A family added to `_script` and not here states a count nothing
#: reconciles.
#:
#: **The provider family's count is `target_partitions`, not `--jobs`**
#: (`PARALLEL_SCAN`): its rows run `pgdt sql`, and the `--jobs`
#: its untimed builder states is `SWEEP_JOBS` on every row.
JOBS_AXIS: tuple[str, ...] = ("parse-jobs-", "parse-rss-jobs-", "dfcli-query-typed-jobs-")

#: The provider family's shape, `JOBS_AXIS`' third prefix less its `-jobs-`.
PARALLEL_SCAN = "dfcli-query-typed"

#: The **read-buffer budget** every row of both `parallel-*` figures runs at,
#: stated as the `--memory` allowance that leaves it (`stated_allowance`).
#:
#: **One value for every row, because the axis is the worker count.** A budget
#: that grew with `--jobs` would make each row a different apparatus, and the
#: table's ratios would be over two variables at once.
#:
#: **2 GiB, because it must admit the widest row's partitions on the coarsest
#: input.** A block-decoding `XzSource` charges each reader its block unit, the
#: chunk buffer and the decoder's own retention
#: (`xz_seek::Reader::decode_footprint`, 9,471,776 B on this shape's 8 MiB
#: dictionary), and the readers together the block pool's retention list,
#: `(POOL_DEPTH.max(workers) − 1)` units (`WorkerMemory`) — so 24 workers over
#: 24 MiB blocks want `24 x 34.03 + 23 x 24 MiB` = 1.34 GiB, and 24 over 128 MiB
#: blocks want more than any budget this machine would state; the 128 MiB leg
#: is bound by its own block size and says so, which is the whole point of
#: taking `parallel-peak-rss` at two of them. Below this the widest rows would be silently clamped by
#: `worker_count`, and a clamped row is a lower count wearing a higher label.
#:
#: **`--jobs 1` states it too**: `Parallelism::workers(1, _)` is `Serial`
#: carrying the budget, so the baseline row runs one block-decoding reader at
#: this budget rather than the streaming fallback, and a speedup is a speedup
#: over that. Each table says so.
#:
#: **A budget clamp and a `POOL_DEPTH` clamp are not the same kind of thing,
#: which is why this table annotates one and sizes around the other.** Both
#: deliver fewer workers than a row's label asks for, and the table already
#: carries a note for the second. The difference is whose ceiling it is.
#: `POOL_DEPTH` is the shipped library's own, so a reader of the published
#: figure meets it too and the annotated row states a real property of the
#: thing measured. This constant is the *harness's* choice; a row clamped by it
#: publishes an apparatus decision wearing a library ceiling's clothes, and
#: nothing in the table distinguishes the two. So the annotation that suffices
#: for `POOL_DEPTH` does not discharge a budget clamp — the budget is sized so
#: that the clamp does not happen, and where it cannot be, what the figure
#: measures has to be restated rather than footnoted.
PARALLEL_BUDGET = 2 << 30

#: The fixed room the two `parallel-*` figures' containers hold *above* the
#: budget they state: the decoder dictionaries, the batches in flight and the
#: allocator's retention, none of which the stated budget covers.
#:
#: **It is a constant, and that is the whole principle.** What is refused is
#: sizing this off the `--jobs` axis, because what sits above a stated budget is
#: nothing the library states: the terms no charge bills and the allocator's
#: retention, which on the platform allocator is glibc seeding an arena for
#: every thread that allocates (`docs/status/history/2026-09-09.md`, "`M76`: the arena cap is not the
#: runtime's") — a number picked from the allocator on the machine that took
#: the figure rather than from anything the library asks for, and
#: `measurements.md`'s contract is that a figure carries the command that
#: re-takes it. Sizing off the *budget* is the opposite case: the budget is a
#: number the library is handed and promises to bound its pools by, so
#: `budget + headroom` states the library's own contract and travels to any
#: machine.
PARALLEL_HEADROOM = 2 << 30

#: What the two `parallel-*` figures' containers are given, against the
#: register's 512 MB — an apparatus departure, and each figure's own table says
#: so. **Derived, never typed**: it is the stated budget plus
#: `PARALLEL_HEADROOM`, so raising the budget cannot leave the container behind.
#:
#: **Never a literal.** A literal `3g` is this rule against a 1 GiB budget —
#: budget plus 2 GiB — and nothing recomputes it when the budget moves: raising
#: the budget to 2 GiB under it halved the headroom instead of moving the
#: container. The block pool's slot ceiling is
#: `clamp((budget - chunk_held) / unit, 1, POOL_DEPTH.max(jobs))`, a function of
#: the *budget* and the announced count rather than of the readers delivered
#: (`KD21`), so at `control_xz128`'s 128 MiB unit it went from seven slots to
#: fifteen: `parallel-peak-rss`'s widest row measured 2110 MiB at 1 GiB and
#: **3067 MiB at 2 GiB**, five megabytes under a 3072 MiB limit ([`../docs/status/history/2026-09-10.md`](../docs/status/history/2026-09-10.md),
#: "The container was sized off a number that moved"). The 24 MiB leg rose
#: 89 MiB over the same step, being depth-bound rather than budget-bound, which
#: is why the shared constant hid it.
PARALLEL_MEMORY = f"{(PARALLEL_BUDGET + PARALLEL_HEADROOM) // GIB}g"

#: The sub-stream count a provider leg of `parallel-scan-throughput` actually
#: gets from `PARALLEL_BUDGET`, keyed by input — `worker_count`'s
#: `WorkerMemory::affords`, reached through the scan budget's draw
#: (`datafusion-pgdump/src/table.rs`, `scan`) — **for the legs
#: a budget clamp reaches at all**. A leg absent from this dict is one the
#: budget never clamps inside `PARALLEL_JOBS`, and it carries no per-cell
#: annotation, there being nothing to say.
#:
#: **The divisor is per source, not universal.** `plan_partitions` adds the
#: held batch's `max_source_span` only where the source retains by the read
#: chunk (`crate::io::RetainedUnit`); a block-decoding `XzSource` retains by the
#: partition, whose decoded blocks `partition_bytes` has already charged, so its
#: per-reader term is what one reader of it holds and nothing more.
#:
#: **A span is spent before a count is cut.** `plan_partitions` narrows the
#: held batch's span to what the budget leaves once the readers asked for are
#: paid for, stopping at one *announced* read chunk — `ScanOptions::chunk_size_bytes`
#: (`docs/design/decisions.md`, "D84"), which no leg here states, so the shipped
#: default — so a plain leg's count is solved against that narrowed span and
#: not against the shipped 64 MiB ceiling.
#:
#: **Hand-computed, not derived from a mirrored formula.** A Python
#: reimplementation of `worker_count`/`plan_partitions` would be a second
#: authority on the library's own arithmetic and go stale silently the moment
#: either constant moves; a hardcoded value, like `PARALLEL_JOBS`'s literal 4
#: for `POOL_DEPTH`, is checked by hand against the source once and is exactly
#: as good until the constants it was checked against move, at which point the
#: figure is stale on the paths already in its `depends`.
#:  `.xz`:   `24 MiB` block (`control_xz`'s block size, the one being decoded
#:           and then retained) + `1 MiB` chunk buffer + `9,471,776 B` of
#:           decoder (an 8 MiB dictionary, the 1 MiB input chunk and
#:           `liblzma`'s own 34,592 B of state) = `34.03 MiB` a reader, the
#:           span not charged, plus a `24 MiB` unit of the shared retention
#:           list for every reader past `POOL_DEPTH`. The count is solved
#:           against `margin_allowance`, which binds here — `1,689 MiB` against
#:           a `2,048 MiB` cap — so `floor((1689 + 24) / 58.03) = 29`, still
#:           past the top of `PARALLEL_JOBS`, so no entry.
#:  plain:   the source recommends nothing, so `fit` hands it
#:           `DEFAULT_MEMORY_BUDGET` whatever is stated
#:           (`docs/design/decisions.md`, "D83") — `64 MiB`, not the stated
#:           `PARALLEL_BUDGET`. A reader costs `8 MiB` (`POOL_MAX_BYTES`, which
#:           is also what `LocalFileSource::partitions` applies its multiple to
#:           and is capped straight back to), so `n` readers leave
#:           `(64 - 8n) / n MiB` of span apiece: `24 MiB` at two and `8 MiB` at
#:           four, both of which seat every worker asked for. At eight there is
#:           nothing left, the span stops at the `1 MiB` floor, and `9 MiB` a
#:           sub-stream affords **seven** — which is the cap, every higher row
#:           of `PARALLEL_JOBS` landing on the same floor and the same seven.
QUERY_SUBSTREAM_CAP: dict[str, int] = {"control": 7}

#: The worker count every `pgdt` invocation this harness makes states, and the
#: one every registered figure is taken at **except the three whose axis it is**
#: (`parallel-scan-throughput`, `parallel-peak-rss`, `xz-decode-scaling`) **and
#: `reserve`**, whose stated legs hold `RESERVE_JOBS` and whose flagless legs
#: state nothing.
#:
#: **The exemption is by axis, and it is what `JOBS_AXIS` names.** A figure
#: measuring what the second worker buys cannot state one count for every row —
#: its rows *are* the counts — so it states `PARALLEL_JOBS` instead, exactly as
#: the decode instrument states `DECODE_WORKERS` in its own vocabulary. What the
#: exemption is not is a licence to inherit: every one of those shapes still
#: states a count, `worker_count_problems` still refuses a shape that pins none,
#: and `pinned_count_problems` refuses a shape that pins something *other* than
#: this constant without declaring itself an axis. So the failure this whole
#: reconciliation exists against — a shape whose count moved because a default
#: did — is closed on both sides rather than opened by the exemption.
#:
#: **A worker count is apparatus, on the same argument the allocator is.** A
#: shape that states nothing measures whatever the CLI's `--jobs` defaults to
#: that day — and that default has moved underneath the published figures
#: more than once and now depends on the source, one worker on a plain file and
#: `available_parallelism()` on an `.xz` one, without a single shape changing,
#: which is a figure whose apparatus nothing in the document can name. Nothing
#: about this constant follows the flag. So no invocation
#: here inherits it: `_script` states it, `profile_argv` states it, the koji
#: recipe takes it as a parameter, and `--check` refuses a shape that pins no
#: count (`measurements.md`, "The apparatus").
#:
#: **It is 1 because that is the arrangement the published sitting measured**,
#: not because serial is preferred: every published shape outside the declared
#: exemptions — the `JOBS_AXIS` families, the decode instrument's `decode-`
#: shapes, the reserve's two stated families at `RESERVE_JOBS` and its flagless
#: legs — states `--jobs 1`, so a re-take at this value reproduces that
#: apparatus rather than replacing it. Raising it is an apparatus change
#: and obliges a re-sweep, exactly as changing the allocator would.
SWEEP_JOBS = 1

#: **Every `pgdt parse` this harness runs gathers no statistics**, outside the
#: figures whose subject is the gathering or what it buys (`GATHER_STATISTICS`). `parse`
#: records at the data level by default, reading every value of every column,
#: so a shape that inherited the default would re-time what its figure
#: measures the day the default moved — the failure `SWEEP_JOBS` is stated
#: against. `statistics_flag_problems` refuses a shape that runs `parse`
#: without it; `--preamble-only` reads no row and takes no statistics flag, so
#: it states none. The metadata level records no census either, so a query
#: over a cache it wrote reads its table once more for one, and no figure
#: queries such a cache.
NO_STATISTICS = "--statistics-level metadata"

#: `pgdump_query::statistics::ROW_GROUP_DEFAULT_SIZE_BYTES`, mirrored, and
#: held to the library's by a test.
ROW_GROUP_SIZE = 1 << 20

#: **What the statistics figures state instead, where they gather**, and the
#: untimed builders of the dynamic-filter figures and the query figures
#: (`DATA_LEVEL_QUERIES`) with them. Their subject is the
#: data level the rule above keeps out of every other figure, what its
#: statistics buy a query, or a query over the cache it writes, so they are its one exemption, and they state the request rather
#: than inherit it for the rule's own reason: the selection and the group size
#: are both defaults that can move, and a shape inheriting either would re-time
#: its figure the day one did. The size stated is the default's base size, and a
#: stated size is gathered exactly, so what is priced is gathering at a
#: mebibyte, which a flagless `parse` coarsens where rows are wide.
GATHER_STATISTICS = f"--statistics-level data --row-group-size {ROW_GROUP_SIZE}"

#: **The query figures query a data-level cache one untimed `parse` wrote in
#: the same container**, as `statistics-pruning` does, stating `--statistics
#: none` so no pruning enters the reading. A query over no cache
#: (`--dtcache none`) maps inside the timer, timing the census
#: and the count with the rows, and one over a metadata-level cache reads its
#: table once more for a census, inside the timer (`NO_STATISTICS`). These are the
#: prefixes `_script` dispatches on. `query-nomatch*` is not here: it times
#: the map and its saves, which a built cache would leave nothing of.
#:
#: **Every row of a figure reads one cache**: the builder states `SWEEP_JOBS`
#: whatever its query states, as `PARALLEL_SCAN`'s does on every row of its
#: axis. **Builder and query are joined by `&&`**, as every shape's untimed
#: builder joins its timed command (`test_measure.py`, `Scripts`): a builder
#: that failed would leave the query to map the table cold and save it inside
#: the timer, published as a read over a cache. What the
#: query reads carries decoding the whole cache, statistics included
#: (`PRUNING_LEGS`), which a query after a default `parse` pays too, and the
#: builder discovers its statistics allowance from the figure's container
#: (D85), so the container sizes the cache the query decodes.
DATA_LEVEL_QUERIES: tuple[str, ...] = (
    "query-typed",
    "query-strings",
    "query-project-",
    "query-where-",
)
#: That builder, ahead of the timer.
DATA_LEVEL_BUILDER = (
    f"/pgdt parse --source /dump.sql --dtcache /tmp/x.dtcache --jobs {SWEEP_JOBS} "
    f"{GATHER_STATISTICS} >/dev/null && "
)
#: What the timed query states: the builder's cache, and no use of its
#: statistics.
DATA_LEVEL_QUERY = "--dtcache /tmp/x.dtcache --statistics none"

#: `statistics-gathering`'s shapes: one whole-file `parse` under the resident
#: wrapper, at the metadata level and at the data level, `<family><leg>-rss`.
#: **The figure prices the data level whole**: the census, the unrepresentable
#: count and the statistics together, the only two levels a `parse` offers
#: (`docs/design/decisions.md`, "D77"), and which of the three costs what is
#: an attribution, read off a profile of the `data` leg (`PROFILE_SHAPES`).
#: That is the shipped default `parse`, which no scan figure times. **No leg
#: states a column override** (`metadata,<table>.<column>=data`): it censuses
#: and counts the whole table with one column's statistics, so subtracted from
#: these it would difference the census back out, which `D35` retired for
#: the profile. The id stays
#: `statistics-gathering`, `data` being a value of `--statistics-level`.
#: **Both legs are the family's own**, the `metadata` leg included, though
#: its argv is `parse-rss`'s: the figure's container is not the register's, so
#: it is not that run, and a leg borrowed from `peak-rss` would stand this
#: figure in a sharing edge that confines it to a stamped sweep.
STATISTICS_FAMILY = "parse-statistics-"
#: The two legs, in the table's column order, each with the flags it states.
STATISTICS_LEGS: tuple[tuple[str, str], ...] = (
    ("metadata", NO_STATISTICS),
    ("data", GATHER_STATISTICS),
)

#: `statistics-gathering`'s container limit, against the register's 512 MB.
#: **Chosen generously, so that no kill interrupts the figure**: statistics are
#: held per group per tracked column, and the arithmetic's worst case — every
#: one of a 3.00 GiB input's 3,072 groups holding a full dictionary of 256-byte
#: entries and two capped bounds on each of the control's 16 columns — is about
#: 800 MiB above the scan. The figure records resident beside the time, so the
#: headroom this leaves is read rather than assumed. What statistics may hold is
#: bounded (`docs/design/decisions.md`, "D85"); this limit sits above that bound
#: rather than standing in for it.
STATISTICS_MEMORY = "2g"

#: `statistics-pruning`'s shapes, `<family><filter>-<leg>`: a `pgdt query`
#: timed against a cache one untimed gathering `parse` wrote in the same
#: container, as `info-cache-rss` builds its own — so the cache is by
#: construction this input's, and the two legs differ by `--statistics` alone.
PRUNING_FAMILY = "query-pruning-"
#: The filters, in the table's row order: the `--where` each states and what
#: the row calls it. **The range is the one the spec names**, a selective range
#: on a sorted column, which statistics answer twice over — groups wholly
#: outside it are skipped by their bounds, and the block's ascending order stops
#: the read at the first row past its upper bound. **The equality is answered by
#: a dictionary alone**, `v_category` being text under no stated collation, so
#: `pgdt`'s semantics reads no bounds for it (`generate_pruning_bench.py`). Both select 3,000 rows of the
#: 3.00 GiB input: one run of `id`, and three runs of one label spread across
#: the file — which is what the table's rows-returned column reads back.
#: **The third is one its statistics cannot narrow** (`PRUNING_UNNARROWED`):
#: `v_smallint` is drawn uniformly per row, so each group's few hundred values
#: have bounds spanning the column's range, holding the midrange `0`, and more
#: distinct values than a dictionary keeps. Every group's bounds are read and
#: none is skipped, so its legs differ by what consulting them costs. A
#: `smallint` rather than a wider integer so the filter still returns rows:
#: 13 of the 3.00 GiB input's, where one value of a `bigint` would match none.
PRUNING_FILTERS: dict[str, tuple[str, str]] = {
    "range": ("id > 400000 AND id <= 403000", "Range on the sorted `id`"),
    "dictionary": ("v_category=category-200", "Equality on the low-cardinality `v_category`"),
    "unnarrowed": ("v_smallint=0", "Equality on the uniformly drawn `v_smallint`"),
}
#: The filter whose pruned leg must consult statistics and skip nothing, where
#: the others' must skip.
PRUNING_UNNARROWED = "unnarrowed"
#: The legs, in the table's column order: what `query --statistics` states.
PRUNING_LEGS = ("none", "all")
#: **No leg prices carrying statistics.** The cache is decoded whole whatever
#: the query states (`cache::read_cache_file`), so every leg pays the
#: statistics' decode, and no `parse` writes a cache that holds its table's
#: census without its statistics: a metadata-level one leaves the query to
#: re-read the table for its census inside the timer. A column override's
#: cache, carrying one column's statistics, would time nearly the subtraction
#: the retired leg took, whose reading the spreads left unresolved; what would
#: resolve it is the query timing its own cache load, which no decision asks
#: for. The pruned legs bound the decode from above, each decoding the whole
#: cache inside its wall, and the figure's prose states that bound.

#: DataFusion's CLI, as the binary the figures time carries it: `pgdt`'s
#: `sql`, run after a `pgdt parse` that builds the cache it reads.
SQL_SHELL = "/pgdt sql"
#: The catalog `--dump` registers the input under: a generated dump names no
#: database, so one is given.
DFCLI_CATALOG = "bench"
#: **The worker count, stated as `datafusion-cli` takes it**: the session's
#: `target_partitions`, which is what the provider plans a scan's partitions
#: against, set from the environment `ConfigOptions::from_env` reads. Stated at
#: `SWEEP_JOBS` on every run of the binary, for that constant's reason, and
#: held there by `worker_count_problems` and `pinned_count_problems`.
DFCLI_PARTITIONS = "DATAFUSION_EXECUTION_TARGET_PARTITIONS"
#: `dynamic-filter-join` and `dynamic-filter-topk`'s shapes,
#: `<family><figure>-<query>-<leg>`: one untimed `pgdt parse` stating
#: `GATHER_STATISTICS` writes the cache where `--dump` looks for it, beside
#: the dump, and the timed `pgdt sql -c` runs one query under
#: one of `DYNFILTER_LEGS`. What the query returned is hashed outside the
#: timer, so the three legs are held to one answer.
#:
#: **One binary, two levers, three legs**: DataFusion's own producer flag
#: (`optimizer.enable_{join,topk}_dynamic_filter_pushdown`) and the
#: provider's `pgdump.dynamic_filter_rows`, stated by a `SET` run ahead of the
#: query in the same process. So each table prices what ships — the filter on
#: against off — and what the setting buys and costs — rows evaluated against
#: the filter on, which `decisions.md`, "D93" reads.
DYNFILTER_FAMILY = "dfcli-dynamic-filter-"
#: Each figure's queries, in its table's row order: the SQL and what the row
#: calls it. **Every join counts one payload column over the probe's rows it
#: matched**, so a row the filter would drop is one whose `v_text` need not be
#: decoded, and the output is one line however many rows matched. The build
#: side is the small table, which is what the planner collects from the
#: provider's row counts (`generate_dynamic_filter_bench.py`):
#:
#: - **`clustered`** joins the ascending `id` against a run of consecutive ids:
#:   the bounds rule out every group but the run's, where group pruning pays.
#: - **`unclustered`** joins `u_key`, a bijection of `id`, against as many
#:   scattered values: no group's bounds narrow anything, and only a row-level
#:   check drops a row.
#: - **`costing`** joins `bucket` against every value it holds, the largest
#:   `IN` list a join publishes: the filter rejects no row, and what checking
#:   it costs is pure overhead.
#:
#: **A join answers `count(*)` first, and `count(p.v_text)` beside it**: the
#: first is the rows it matched, which its table reports; the second keeps the
#: payload decoded, what row-level evaluation exists to skip, and is not that
#: count, `v_text` being nullable.
#:
#: **The TopK orders by the unsorted `u_key`**, projecting a payload column
#: beside it; `u_key` has no ties, so the answer is one answer.
DYNFILTER_QUERIES: dict[str, dict[str, tuple[str, str]]] = {
    "join": {
        "clustered": (
            f"SELECT count(*), count(p.v_text) FROM {DFCLI_CATALOG}.public.perf p "
            f"JOIN {DFCLI_CATALOG}.public.near b ON p.id = b.k",
            "A selective join on the clustered `id`",
        ),
        "unclustered": (
            f"SELECT count(*), count(p.v_text) FROM {DFCLI_CATALOG}.public.perf p "
            f"JOIN {DFCLI_CATALOG}.public.scattered b ON p.u_key = b.k",
            "A selective join on the unclustered `u_key`",
        ),
        "costing": (
            f"SELECT count(*), count(p.v_text) FROM {DFCLI_CATALOG}.public.perf p "
            f"JOIN {DFCLI_CATALOG}.public.every b ON p.bucket = b.k",
            "A join on `bucket` rejecting no row",
        ),
    },
    "topk": {
        "unsorted": (
            f"SELECT p.u_key, p.v_text FROM {DFCLI_CATALOG}.public.perf p "
            "ORDER BY p.u_key LIMIT 10",
            "`ORDER BY` the unsorted `u_key`, `LIMIT 10`",
        ),
    },
}
#: The producer flag each figure's legs state, as `ConfigOptions::from_env`
#: reads `optimizer.enable_{join,topk}_dynamic_filter_pushdown`.
DYNFILTER_FLAGS = {
    "join": "DATAFUSION_OPTIMIZER_ENABLE_JOIN_DYNAMIC_FILTER_PUSHDOWN",
    "topk": "DATAFUSION_OPTIMIZER_ENABLE_TOPK_DYNAMIC_FILTER_PUSHDOWN",
}
#: The legs, in the table's column order, and the producer flag's value in
#: each: `off`, the filter off; `on`, the filter on at the provider's default;
#: `rows`, on with `DYNFILTER_ROWS_SQL` run first.
DYNFILTER_LEGS = {"off": "false", "on": "true", "rows": "true"}
#: The leg stating the setting, and the statement it runs ahead of its query:
#: its own `-c`, which prints nothing under `--format csv`, so the answer
#: read back is the query's alone.
DYNFILTER_ROWS_LEG = "rows"
DYNFILTER_ROWS_SQL = "SET pgdump.dynamic_filter_rows = true"
#: **The startup leg**: the builder as every leg runs it, then
#: `pgdt sql` registering the dump and answering `STARTUP_SQL`
#: under the timer, so what loading the program, its runtime and the
#: registration cost a leg — and how much that varies — sits in each table
#: beside the legs it is inside. Taken in each figure's own interleave rather
#: than borrowed between the two.
DYNFILTER_STARTUP = f"{DYNFILTER_FAMILY}startup"
STARTUP_SQL = "SELECT 1"


def dfcli_invocation(figure: str, name: str, leg: str, dump: str) -> tuple[list[str], list[str]]:
    """The environment and the arguments one run of `SQL_SHELL` over a
    dynamic-filter shape states, over the dump at `dump`, its SQL last.

    One function for the figure (`_script`) and for the account's
    instruments (`profile_recipe`), so a profiled or introspected leg is the
    timed one by construction: a flag moved in one and not the other would
    profile something no figure measures."""
    if name not in DYNFILTER_QUERIES.get(figure, {}) or leg not in DYNFILTER_LEGS:
        raise ValueError(f"unknown dynamic-filter shape {figure}-{name}-{leg}")
    env = [
        f"{DFCLI_PARTITIONS}={SWEEP_JOBS}",
        f"{DYNFILTER_FLAGS[figure]}={DYNFILTER_LEGS[leg]}",
    ]
    argv = ["--dump", f"{DFCLI_CATALOG}={dump}", "--format", "csv", "-q"]
    if leg == DYNFILTER_ROWS_LEG:
        argv += ["-c", DYNFILTER_ROWS_SQL]
    return env, [*argv, "-c", DYNFILTER_QUERIES[figure][name][0]]


def dfcli_shell(env: list[str], program: str, argv: list[str]) -> str:
    """`dfcli_invocation`'s run as one shell line, each statement
    single-quoted."""
    words = (f"'{a}'" if " " in a else a for a in argv)
    return f"{' '.join(env)} {program} {' '.join(words)}"


#: What a timed run of `SQL_SHELL` answered, read back off `/tmp/result.csv`
#: outside the timer as `key=value` lines `parse_reported` takes: its rows
#: under the header, its first row's first field, and a digest of the whole.
DFCLI_ANSWER = (
    "echo result_rows=$(($(wc -l </tmp/result.csv) - 1)) && "
    "echo result_first=$(sed -n 2p /tmp/result.csv | cut -d, -f1) && "
    "echo result_digest=$(sha256sum </tmp/result.csv | cut -d' ' -f1)"
)

#: `parallel-scan-throughput`'s provider legs' query: every column of the
#: control's table counted, under a filter every row passes.
#:
#: **Every column, so every column is decoded typed**, as the `pgdt query`
#: legs it replaced decoded them (`KD57` is why it replaced them); **one row
#: out**, so no printing serializes what the partitions decode in parallel.
#: **The filter is what keeps the counts from being answered**: unfiltered,
#: the plan node's exact NULL counts answer every `count(<column>)` with no
#: row read, where a pushed filter hands them over as estimates
#: (`docs/design/decisions.md`, "D89"). `id IS NOT NULL` keeps every row,
#: since the generator writes an `id` on each, so its statistics prune
#: nothing.
PARALLEL_SCAN_SQL = (
    f"SELECT {', '.join(f'count({name})' for name, _ in perf.COLUMNS)} "
    f"FROM {DFCLI_CATALOG}.{perf.TABLE} WHERE id IS NOT NULL"
)


def parallel_scan_invocation(partitions: int, dump: str) -> tuple[list[str], list[str]]:
    """The environment and the arguments one provider leg of
    `parallel-scan-throughput` states over the dump at `dump`, at
    `partitions` as its session's `target_partitions`.

    **The allowance is the `pgdt` legs' own**, stated by a `SET` ahead of the
    query in the same process, so each column of the table runs under one
    stated allowance (`PARALLEL_BUDGET`)."""
    if partitions not in PARALLEL_JOBS:
        raise ValueError(f"{partitions} is a partition count the figure does not carry")
    env = [f"{DFCLI_PARTITIONS}={partitions}"]
    argv = [
        "--dump", f"{DFCLI_CATALOG}={dump}", "--format", "csv", "-q",
        "-c", f"SET pgdump.memory = {stated_allowance(PARALLEL_BUDGET)}",
        "-c", PARALLEL_SCAN_SQL,
    ]
    return env, argv

#: What the decode figure's container is given, against the register's 512 MB.
#: At 24 workers over 24 MiB blocks the decoder holds 24 decoded slots, 26
#: compressed windows and 24 LZMA2 dictionaries — around 950 MB on the
#: generated leg, whose compressed windows are the larger of the two. It is an
#: apparatus departure and the figure's own table says so.
DECODE_MEMORY = "2g"

#: `pgdump_query::io::DEFAULT_MEMORY_BUDGET`, mirrored.
#:
#: Hardcoded rather than read out of the source, on `QUERY_SUBSTREAM_CAP`'s
#: argument: a second authority on the library's own constants goes stale
#: silently, and a literal checked by hand once is exactly as good until the
#: constant moves — at which point the figure is stale on paths its `depends`
#: already names. It is what a run that states no budget gets, which is the one
#: reading `reserve` takes off the budget axis.
LIBRARY_DEFAULT_BUDGET = 64 << 20

#: The stated budgets `reserve` reads a resident set across: its one axis.
#:
#: **Four, spanning an order of magnitude either side of the library's own
#: `DEFAULT_MEMORY_BUDGET`**, because the quantity this axis publishes is a
#: *difference* from the number stated and one reading of it cannot say whether
#: the difference is a constant or a fraction
#: (`docs/design/decisions.md`, "I/O, memory and parallelism").
#:
#: **What the axis shows needs all four.** A plain source reads flat across
#: them, the budget binding almost nothing there; a block-decoding `.xz` rises
#: with the reader count each budget affords at `RESERVE_JOBS`, since what a
#: stated budget buys there is readers. One reading could have said neither.
RESERVE_BUDGETS: tuple[int, ...] = (64 << 20, 128 << 20, 256 << 20, 512 << 20)

#: The worker count every reserve reading states.
#:
#: The top of `PARALLEL_JOBS`, which is this machine's `available_parallelism()`
#: and so the count `Parallelism::discover()` recommends here for an `.xz`
#: source: the stated legs pin that count and vary only the budget.
#:
#: **This is a declared axis for `pinned_count_problems`, not an inherited
#: count.** The family states it on every shape; what makes it exempt from
#: `SWEEP_JOBS` is that the figure holds the count fixed and varies the budget,
#: which is the same exemption `JOBS_AXIS` takes with the two swapped.
RESERVE_JOBS = PARALLEL_JOBS[-1]

#: The two arena settings each budget is read at: token, `MALLOC_ARENA_MAX`
#: value (empty = set nothing), and what the table calls the leg.
#:
#: **Two.** `unset` is the operator who capped nothing, which is the case that
#: kills the process and the arrangement `MEMORY_RESERVE` was chosen under.
#: `two` is the tightest value an operator would plausibly set — the floor the
#: manual publishes and what the koji probes used — so it bounds how much of the
#: resident set is arena retention at all, which the uncapped leg cannot say on
#: its own. **Neither can be dropped**: a one-leg figure cannot report a null,
#: and an arena claim here has more than once outlived the build it was
#: measured on.
#:
#: **No leg caps at the worker count plus one.** A cap at or above the arena
#: count cannot bind, but the arena count is not the worker count: the
#: instrument reads `readers + 2` arenas at every limit that resolved two readers
#: or more, so a `readers + 1` cap sits below it there. At one reader it reads 2
#: arenas, where a cap of 2 is already inert. No reading prices such a cap; the leg is left out as one not
#: worth a sitting, not as one inert by construction.
RESERVE_ARENAS: tuple[tuple[str, str, str], ...] = (
    ("unset", "", "arenas uncapped"),
    ("two", "2", "`MALLOC_ARENA_MAX=2`"),
)

#: The two sources each leg is read over, and what the table calls each.
#:
#: **Both, because the rule states one reserve for every source.** The budget
#: binds the pools on a block-decoding `.xz` and barely binds anything on a
#: plain source, so that one number has to be read against the shape where it
#: is loosest as well as the one where it is tightest.
RESERVE_INPUTS: tuple[tuple[str, str], ...] = (
    ("control_xz", "`.xz`"),
    ("control", "plain"),
)

#: The command-shape prefix the reserve family answers to. It carries `rss`
#: because `Session.time_run` reads the wrapper's report only for a shape whose
#: name says it has one.
RESERVE_FAMILY = "parse-rss-reserve-"

#: The command-shape prefix of the legs that state **nothing** — no `--jobs`,
#: no `--memory` — so that the arrangement under test is the one a
#: flagless invocation resolves for itself.
#:
#: **This is the one family in the register that states no worker count, and it
#: is declared at both ends rather than slipping past the reconciliation.**
#: `worker_count_problems` and `pinned_count_problems` skip it by this prefix,
#: and a test asserts that what it states is *neither* flag — the exemption is
#: from stating a count, never a licence to pin one quietly. What earns it is
#: that the count is the **reading**: under discovery `Discovered::resolve`
#: lowers the source's recommendation to what the allocation affords, so the
#: `jobs=` a run reports is the reader count its resident set is a resident set
#: *for*, and a shape that pinned one would measure an arrangement the shipped
#: default never produces.
#:
#: **It does not reopen the figure the register refuses** (`measurements.md`,
#: "The apparatus"): a *throughput* table off unpinned shapes, the throughput of
#: the default being already measured at a stated count by
#: `parallel-scan-throughput`. This is a resident reading of an arrangement no
#: stated shape can express, and the count it runs at is recorded per leg from
#: the run's own report rather than assumed.
RESERVE_FLAGLESS = "parse-rss-discover-"

#: The command-shape prefix of the **path step**: the same `parse` at a stated
#: budget either side of what one block-decoding reader costs, so that block
#: decode is afforded on one leg and declined on the other
#: (`BlockCache::affordable`).
#:
#: A family of its own rather than two more entries in `RESERVE_BUDGETS`, whose
#: four values are crossed with both arena settings and both sources: the step's
#: two budgets are neither whole mebibytes nor an axis anything else is read
#: across, and adding them there would multiply eight legs out of a pair.
RESERVE_STEP_FAMILY = "parse-rss-step-"

#: `pgdump_query::SCAN_CHUNK_DEFAULT_SIZE_BYTES`, mirrored — the read chunk a scan settles
#: at when no `--chunk-size` is stated, and one of the three terms in what a
#: block-decoding reader holds.
#:
#: Named apart from `CHUNK_DEFAULT`, which is the same number wearing the
#: chunk-size figure's hat — that one is *the row every other row is a ratio
#: against* and this one is *what a reader holds a buffer of*. A test holds the
#: two equal, which is the cheap reconciliation: the day they diverge, one of
#: the two meanings has moved and the figure that reads it is wrong.
LIBRARY_CHUNK_BYTES = 1 << 20

#: `xz_seek::Reader::decode_footprint()` on an 8 MiB-dictionary file: the
#: dictionary, the 1 MiB input chunk and `liblzma`'s own 34,592 B of state.
#: Every `.xz` input here is written at preset 6, so every one of them has that
#: dictionary (`INPUTS["control_xz"]`).
#:
#: **The chunk term is a constant across every registered input, not a
#: coincidence of one of them.** `SeekTable::input_chunk()` is the largest
#: block's *compressed* extent **capped at `xz_seek`'s 1 MiB `INPUT_CHUNK`**, and
#: both compressed inputs here compress a block to far more than that — 4.6 MB
#: at 24 MiB blocks, 24.5 MB at 128 MiB — so both saturate the cap. A file whose
#: every block compresses below 1 MiB would charge less and this constant would
#: over-state, which is the direction that costs nothing.
XZ_DECODE_FOOTPRINT = 9_471_776

#: The dictionary term inside `XZ_DECODE_FOOTPRINT`: 8 MiB for an
#: 8 MiB-dictionary file, allocated by `liblzma` through C `malloc` and
#: therefore **invisible to the counting allocator**, which sees only what
#: passes through Rust's `GlobalAlloc`.
#:
#: Named here because it is the one term an attribution must add back by hand:
#: the instrument's `live_*` family cannot see it and its `mallinfo_*`/`malloc_*`
#: family cannot separate it, so a decomposition that subtracted the two would
#: charge it to retention. Read off a stack rather than modelled —
#: `heaptrack_print` attributes exactly 8,388,608 bytes for one decoder through
#: `lzma_lz_decoder_init` ← `lzma_raw_decoder` ← `PayloadDecoder::new`
#: (`measurements.md`, "What an instrument can see").
XZ_DICT_BYTES = 8_388_608


def reader_bytes(unit: int) -> int:
    """What **one** concurrent block-decoding reader of a file with `unit`-sized
    blocks holds: one block slot — the block it is decoding, which is the block
    it then retains — the chunk buffer a straddling read is assembled into, and
    the decoder's own retention.

    `BlockCache::reader_bytes`, mirrored — the per-worker term of the charge
    `BlockCache::affordable` compares a budget against, and what
    `XzSource::partition_advice` charges a sub-stream. It is not the whole
    charge: `charge_bytes` adds the retention list the pool shares, which is
    `POOL_DEPTH - 1` units at one reader and `jobs - 1` above the depth.

    **It is one number and not two, at the default chunk every leg here runs
    at.** `BlockCache::reader_bytes` takes its chunk term from
    `XzSource::charged_chunk_bytes`, which answers `SCAN_CHUNK_DEFAULT_SIZE_BYTES` whenever
    no read loop has announced a length — so the charge
    `XzSource::default_worker_memory` recommends against *before* the file is
    open for reading and the charge `BlockCache::affordable` then compares a
    budget to are the same charge. *Rejected:* charging the unannounced pool's
    `POOL_MAX_BYTES` ceiling — it runs the recommendation 7 MiB a reader high
    and costs a reader at every allocation (`io.rs`, `charged_chunk_bytes`).
    A flagless run's resolved budget is **never** this number times its count:
    the shared retention list is billed at every count,
    so a budget read off a run's own report carries `pool_bytes` besides, which
    is what `charge_bytes` states. It splits again only for a caller that states
    a `--chunk-size` other than the default, which no leg of this figure does.

    **Hand-computed, on `QUERY_SUBSTREAM_CAP`'s argument.** A Python
    reimplementation of the library's arithmetic is a second authority that goes
    stale silently; a mirror checked by hand against the source once is exactly
    as good until one of its terms moves, at which point the figure is already
    stale on the paths its `depends` names. At 24 MiB blocks this is
    34.03 MiB. *Rejected:* two units a reader — the decode buffer and the
    retained block are one buffer, and the second unit is already the pool's
    own list (`pool_bytes`).
    """
    return unit + LIBRARY_CHUNK_BYTES + XZ_DECODE_FOOTPRINT


#: `pgdump_query::io::POOL_DEPTH`, mirrored: the floor `BufferPool::slots`
#: clamps a pool's slot count to, whatever worker count was announced to it.
#:
#: It lives with the charge rather than with the other library mirrors because
#: it is the second term of that charge: the block pool is sized
#: `POOL_DEPTH.max(jobs)` and `BlockCache::slot` drains to one below it before
#: obtaining the buffer `retain` then pushes back, so the pool holds
#: `(POOL_DEPTH.max(jobs) - 1) x unit` on top of the one unit each reader has in
#: flight. That is `pool_bytes`, which the library bills as what the pool
#: holds (`io.rs`, `WorkerMemory`); naming it as a
#: column of its own is what keeps it out of the residual, where it would read
#: as a term nothing accounts for.
LIBRARY_POOL_DEPTH = 4

#: `pgdump_query::io::MEMORY_RESERVE`, mirrored: what `Parallelism::discover`
#: holds back from a discovered limit before it divides.
#:
#: It is the model's upper bound rather than a term in it. The rule's promise is
#: that everything a scan holds above what it billed fits inside this number, so
#: a leg whose unnamed remainder exceeds it is a leg the rule cannot keep inside
#: its allocation — which is the criterion `charge_model_problem` applies, and
#: the reason the bound is a library constant rather than a tolerance the
#: harness chose.
LIBRARY_MEMORY_RESERVE = 384 << 20


def stated_allowance(budget: int) -> int:
    """The `--memory` value that leaves `budget` bytes for the read buffers.

    **`--memory` states a *resident* allowance, not a buffer budget**
    (`docs/design/decisions.md`, "D83"): `Parallelism::within` takes
    `MEMORY_RESERVE` off the top before a reader is counted, so a figure
    registered against a buffer budget states that budget plus the reserve and
    goes on measuring what it was registered to measure.

    **It reproduces the old arrangement exactly only up to a 256 MiB budget.**
    The carve also holds the resolved *count* under `margin_allowance`, which a
    typed number never used to answer to, and that ceiling falls below the cap
    once the budget passes `4 x MEMORY_RESERVE - 5 x MEMORY_UNPOOLED_BOUND`.
    Above that a leg resolves fewer readers than it did — `reserve`'s stated
    axis is that arrangement, and `KD34` names what it reads; the number stated
    here is still the one the figure's text names.

    **Not the inverse of the carve**, which has none: nothing recovers the
    allowance a budget came from, because the budget is a `min` of two terms.
    This is the one direction that is a function.
    """
    return budget + LIBRARY_MEMORY_RESERVE


#: `pgdump_query::io::MEMORY_UNPOOLED_BOUND`, mirrored: this crate's bound on
#: what a scan holds resident outside the pools its charge bills, and what
#: `margin_allowance` predicts a count's resident with.
#:
#: **The inner of the model's two fault lines**, where `LIBRARY_MEMORY_RESERVE`
#: is the outer one. A remainder above this is a finding about the *bound* — the
#: library predicts counts with a number the readings have overrun, so the
#: margin is not leaving what it claims — while a remainder above the reserve is
#: the *rule* failing, an arrangement the discovery cannot keep inside its
#: allocation. One threshold cannot tell those apart, which is why there are
#: two, and both are registered before the sitting.
#:
#: Read off the reserve constant's five-build grid under today's charge rather
#: than fitted: the worst surviving block-path remainder there is 238.6 MiB at
#: 24 MiB blocks and 142.0 MiB at 128, and this is the worst rounded up to a
#: 64 MiB step (`rederived_unpooled_bound`), about 17 MiB above it. The
#: published `reserve` sitting's worst remainder rounds by the same arithmetic
#: to 192 MiB (`docs/design/decisions.md`, "I/O, memory and parallelism").
LIBRARY_MEMORY_UNPOOLED_BOUND = 256 << 20


def pool_bytes(unit: int, jobs: int) -> int:
    """What the block pool retains at `jobs` readers on top of the per-reader
    term: `(POOL_DEPTH.max(jobs) - 1) x unit`, and never zero.

    `WorkerMemory::pool_bytes`, mirrored — the second term of the library's
    charge, stated as what the pool holds rather than as a floor that decays.
    Kept as a column
    of its own rather than folded into `charge_bytes` because it is **unbounded
    in the block size** where every other term is not: at one reader 72 MiB at
    koji's 24 MiB blocks, 384 at 128, 1.5 GiB at 512, and growing by a unit a
    reader past `POOL_DEPTH`. A model that hid it inside a flat remainder would
    read as a term nothing accounts for.

    **Billed at every cell.** *Rejected:* `max(0, POOL_DEPTH - jobs) x unit`
    beside a per-reader term of two units, which clamps off at four readers and
    bills exactly one unit more than the pool holds at every count — a
    one-reader cell reads 130.0 MiB billed against 111.1 held at 24 MiB blocks.

    **It has two consumers and they read it for opposite purposes.**
    `charge_bytes` adds it, because the budget rule bills it; `_depooled`
    subtracts it, because a term known before the sitting has no business in a
    fitted intercept — and the shape that makes it worth billing apart is the
    same shape that makes a straight line across it wrong, being constant below
    `POOL_DEPTH` and per-reader above (`_depooled`). A change to this function moves
    both a charge and a published line.
    """
    return max(LIBRARY_POOL_DEPTH, jobs, 1) * unit - unit


def charge_bytes(unit: int, jobs: int) -> int:
    """What the budget rule charges `jobs` concurrent block-decoding readers of
    a file with `unit`-sized blocks, in bytes: the per-reader term times the
    count, plus the retention list those readers share.

    `WorkerMemory::at`, mirrored — what `Parallelism::fit` solves a cap against
    and what `stream::worker_count` solves a budget against, so under discovery
    it is also the budget a run reports for itself.
    """
    return jobs * reader_bytes(unit) + pool_bytes(unit, jobs)


def discovered_budget(limit: int) -> int:
    """What a flagless run inside a container of `limit` bytes resolves as its
    budget: `limit − MEMORY_RESERVE`, floored at zero.

    `Parallelism::within`, mirrored — `allowance.saturating_sub(
    MEMORY_RESERVE)` (`io.rs`), which a stated `--memory` reaches by the same
    call (`stated_allowance`).

    **The margin is deliberately left out**, as the recommendation is. The
    resolved *count* also answers to `MEMORY_MARGIN_PERCENT` — its predicted
    resident, `charge + MEMORY_UNPOOLED_BOUND`, must leave a fifth of the
    limit — so what a run reports is at most this and
    often less. That makes this an upper bound on the
    budget rather than a prediction of it, which is what the use below wants:
    a count comparison that stays an upper bound.

    **Used to reason about the registered axis, never to report a reading.**
    Every number this figure publishes reads the budget back off the run's own
    `scan started` line, because a harness predicting it would be a second
    authority on the rule under test. What this answers instead is a question
    about `RESERVE_LIMITS` itself — whether the limits registered there can put
    three distinct reader counts on a line — which is settled before any run
    exists and cannot be read off one (`reserve_axis_problems`).
    """
    return max(0, limit - LIBRARY_MEMORY_RESERVE)


def block_path_afforded(unit: int, budget: int) -> bool:
    """Whether `budget` admits **one** block-decoding reader of a file with
    `unit`-sized blocks — so whether a leg ran the block path or the streaming
    fallback.

    `BlockCache::affordable`, mirrored: `worker_memory(..).at(1) <= budget`,
    which is `charge_bytes(unit, 1)` — the per-reader charge **plus the
    retention list that one reader leaves standing**, so the line is
    106.03 MiB at 24 MiB blocks and 522.03 at 128 — against 34.03 and 138.03 for
    the per-reader term alone. So `reader_bytes <= budget` is not this
    comparison (`io.rs`, `BlockCache::affordable`).

    **One function because the renderer asks the question at four places** — the
    flagless cell's own label, the fit's window, the model check's exclusion and
    the path step's two budgets — and four copies of it can disagree, which is
    a table whose cells silently change mechanism while reading as one series.

    **Asked of a reported budget, never of a container limit.** A flagless run
    resolves `limit − MEMORY_RESERVE` and says so on its own `scan started`
    line, so the budget is a reading and the path follows from it by the
    library's own arithmetic; deriving it from the `-m` token instead would
    re-implement the reserve rule under test.
    """
    return budget >= charge_bytes(unit, 1)


def afforded_readers(unit: int, budget: int) -> int:
    """How many concurrent block-decoding readers of a file with `unit`-sized
    blocks `budget` affords — the largest `k` with `charge_bytes(unit, k) <=
    budget`, and zero where not even one fits.

    `Parallelism::fit`, mirrored, with the recommendation **and the margin**
    left out: the count a run actually resolves is `min(recommendation,
    fit-under-margin)`, the recommendation is the machine's and the margin
    (`MEMORY_MARGIN_PERCENT`) lowers the fit further at every limit,
    so this is an upper bound on the resolved count rather than a prediction of
    it. Both omissions push the same way, which is what keeps the fit-ability
    check below a necessary condition.

    **Used to reason about the registered axis, never to report a reading**, on
    the same terms as `discovered_budget`. What it answers is whether
    `RESERVE_LIMITS` can put three distinct reader counts on a flagless
    family's line at all — which `reserve_axis_problems` asks —
    which is knowable before any run and, because the cap can only *collapse*
    two limits onto one count, is a necessary condition rather than a
    sufficient one. A sitting on a host with fewer cores than there are
    distinct fits publishes a secant, which is the per-sitting half the static
    check cannot reach.
    """
    if budget < charge_bytes(unit, 1):
        return 0
    k = 1
    while charge_bytes(unit, k + 1) <= budget:
        k += 1
    return k


def charge_model(unit: int, jobs: int, held: float) -> tuple[int, int, float]:
    """One leg's resident set, split into the two terms the model names and the
    one it does not: `(billed, floor, unnamed)`, all in bytes.

    - **billed** is what the budget rule charged — `charge_bytes(unit, jobs)`,
      which under discovery is also the budget the run reports for itself.
    - **pool** is `pool_bytes`, the part of that bill the block pool's
      retention list accounts for — inside `billed`, reported beside it because
      it is the one term unbounded in the block size.
    - **unnamed** is `held - billed`, which is glibc's arena retention as far
      as any reading here goes (`measurements.md`, "What a scan holds above the
      budget it was given").

    **This is an account and not a fit** — every term is arithmetic from the
    source, evaluated at the cell, where the alternative is doing it by hand
    over a sitting's readings after the runs have been spent searching for a
    constant (`.claude/skills/evidence/SKILL.md`, rule 1).
    """
    billed = charge_bytes(unit, jobs)
    pool = pool_bytes(unit, jobs)
    return billed, pool, held - billed


#: The three bands `charge_model_problem` reports. Every one of the module's
#: `BAND_*` names carries a stance in `BAND_STANCE` below, which is what
#: `charge_band_problems` holds it to.
BAND_RULE = "rule"
BAND_BOUND = "bound"
BAND_OVER_BILL = "over-bill"

#: The check's own rule, as the enumeration it is: which bands refute the model.
#: **`bound` alone is released**, and the rule is written as a list rather than
#: as a threshold because a threshold silently releases the band nobody has
#: argued about — "no evaluated cell above `MEMORY_RESERVE`" counts the check's
#: *upper* lines and lets the non-negativity side out with them.
#:
#: **Released means the allocation is intact *and* the sitting discharges the
#: finding**, which is the test any further band answers. A bound fault meets
#: both: the remainder is still under `MEMORY_RESERVE`, and the sitting
#: re-derives `MEMORY_UNPOOLED_BOUND` from its own cells
#: (`rederived_unpooled_bound`). An over-bill meets neither — it has to exceed
#: the whole of the rest of the process's footprint before the arithmetic can
#: report it at all, so it is never apparatus scatter the way a 238.6 MiB
#: remainder against a 256 MiB bound is, and it leaves nothing for the sitting
#: to repair.
#:
#: A band is released by mapping to `None`; every other band refutes the model,
#: and its value is *why* — the verdict prints that clause beside the cells, so
#: a band cannot be given a refuting stance without the reason being written
#: down in the same place. A band absent from the mapping refutes too, so a
#: fourth fault line added later reads as a refutation until somebody argues it
#: out rather than defaulting into the released half; `charge_band_problems` is
#: what makes that argument visible instead of silent.
#:
#: **A refutation does not bar publication** — see the verdict in
#: `run_reserve`: every cell is a real reading, and only a killed leg
#: (`KILL_TOLERANT`) keeps a figure out of the document.
BAND_STANCE: dict[str, str | None] = {
    BAND_OVER_BILL: (
        "an over-bill is bytes the rule charged that nothing holds, so it admitted fewer "
        "readers than the allocation afforded"
    ),
    BAND_BOUND: None,
    BAND_RULE: (
        "a remainder above `MEMORY_RESERVE` is an arrangement the discovery could not keep "
        "inside its allocation"
    ),
}

#: Why an unlisted band refutes. It is a reason and not an error because the
#: verdict must still render: `--check` is where an unargued band is reported
#: (`charge_band_problems`), and a sitting that runs before that is read should
#: say plainly why it refuted rather than raise.
UNARGUED_BAND_REFUTES = (
    "no stance is recorded for it, so it refutes until somebody argues it out"
)


def band_refutes(band: str) -> str | None:
    """Why this band refutes the model, or `None` where the rule releases it.

    The default is to refute: a fault line added later does not inherit the
    released half by being unmentioned.
    """
    return BAND_STANCE.get(band, UNARGUED_BAND_REFUTES)


#: The step `MEMORY_UNPOOLED_BOUND` is read off in: the reserve constant's
#: candidate builds were 64 MiB apart, so the bound is the worst observed
#: remainder rounded up to a step of it and `rederived_unpooled_bound` re-does
#: that arithmetic over a sitting's own cells.
UNPOOLED_BOUND_STEP = 64 << 20


@dataclass(frozen=True)
class ChargeFault:
    """One cell's fault: which of the model's three lines it crossed, and the
    sentence that says so.

    **The band is the half the verdict consumes.** A verdict that collapses
    every fault into one refutation calls a cell the inner line reads as *a
    finding about the bound, the allocation intact* a failure of the model
    exactly as a breach of the rule is. A fault carries its band so that the
    verdict can say which, and so that `BAND_STANCE` can be read by name.
    """

    band: str
    text: str

    @property
    def refutes(self) -> bool:
        """Whether this cell refutes the model.

        Read off `BAND_STANCE`, which is the rule written as an enumeration:
        `bound` is released, because the remainder is still inside
        `MEMORY_RESERVE` *and* the sitting re-derives the constant it overran.
        Every other band refutes, an unlisted one included — a band with no
        stance recorded is one nobody has argued out, not one the check lets
        through.
        """
        return band_refutes(self.band) is not None


def rederived_unpooled_bound(worst_unnamed: float) -> int:
    """What `MEMORY_UNPOOLED_BOUND` would be if it were read off these cells:
    the smallest 64 MiB step covering the worst unnamed remainder among them.

    This is the arithmetic the shipped constant was read off — 238.6 MiB worst
    over the reserve constant's five-build grid, the next step up being 256 —
    re-done over whatever sitting is in hand, which is why a cell above the
    bound is a finding rather than a refutation: the re-derivation is arithmetic
    over readings already taken, not a re-take (`docs/design/decisions.md`,
    "I/O, memory and parallelism").
    """
    steps = (max(0, int(worst_unnamed)) + UNPOOLED_BOUND_STEP - 1) // UNPOOLED_BOUND_STEP
    return max(1, steps) * UNPOOLED_BOUND_STEP


def charge_model_problem(unit: int, jobs: int, held: float) -> ChargeFault | None:
    """Why this leg refutes the charge model, or `None` where it does not.

    **Three fault lines, and none of them is a tolerance somebody picked.**

    - **Non-negative.** A negative remainder is an *over-bill*: the rule charged
      bytes nothing holds, so it admitted fewer readers than the allocation
      afforded. It is the failure a grid search over reserve constants cannot
      report at all, because a too-large charge shows up there as headroom.
    - **No larger than `MEMORY_UNPOOLED_BOUND`**, the inner line: that constant
      is what `margin_allowance` predicts a count's resident with, so a cell
      above it is a leg whose headroom is smaller than the library promised —
      a finding about the *bound*, which was read off the reserve constant's
      five-build grid.
    - **No larger than `MEMORY_RESERVE`**, the outer line: the reserve is by
      construction what covers everything the charge does not bill, so a
      remainder above it is a leg the *rule* cannot keep inside its allocation
      — the rule failing, stated per cell instead of per sitting.

    **Two lines rather than one, because they are different findings.** Between
    them the allocation still holds and the number the count is predicted
    against is wrong; above the outer one the allocation does not. Moving the
    single threshold inward would have made a slightly low bound read as a
    failed rule, and leaving it at the reserve would have made a bound the
    readings overran invisible until a kill.

    **The band travels with the sentence**, because the verdict that reads this
    names the band, and a distinction spent before it reaches its only consumer
    is not a distinction.

    Asked only of a leg that took the block path and survived: the streaming
    fallback holds none of these terms, and a censored leg's reading is a bound
    rather than a number.
    """
    billed, pool, unnamed = charge_model(unit, jobs, held)
    if unnamed < 0:
        return ChargeFault(
            BAND_OVER_BILL,
            f"**over-billed by {_fmt_budget_bytes(-unnamed)}** — {jobs} reader(s) were charged "
            f"{_fmt_budget_bytes(billed)}"
            + (f", of which {_fmt_budget_bytes(pool)} is the pool's retention list" if pool else "")
            + f", and the whole process held {_fmt_budget_bytes(held)}. The rule admitted "
            "fewer readers than the allocation affords.",
        )
    if unnamed > LIBRARY_MEMORY_RESERVE:
        return ChargeFault(
            BAND_RULE,
            f"**{_fmt_budget_bytes(unnamed)} unnamed**, above the "
            f"{_fmt_budget_bytes(LIBRARY_MEMORY_RESERVE)} `MEMORY_RESERVE` that is meant to "
            f"cover it — {jobs} reader(s) billed {_fmt_budget_bytes(billed)}"
            + (f", of which {_fmt_budget_bytes(pool)} is the pool's list," if pool else "")
            + f" against {_fmt_budget_bytes(held)} held. **The rule does not hold here**: the "
            "discovery cannot keep this arrangement inside its allocation.",
        )
    if unnamed > LIBRARY_MEMORY_UNPOOLED_BOUND:
        return ChargeFault(
            BAND_BOUND,
            f"**{_fmt_budget_bytes(unnamed)} unnamed**, above the "
            f"{_fmt_budget_bytes(LIBRARY_MEMORY_UNPOOLED_BOUND)} `MEMORY_UNPOOLED_BOUND` the "
            f"margin predicts with — {jobs} reader(s) billed {_fmt_budget_bytes(billed)}"
            + (f", of which {_fmt_budget_bytes(pool)} is the pool's list," if pool else "")
            + f" against {_fmt_budget_bytes(held)} held. **A finding about the bound, not the "
            "rule**: it is still inside `MEMORY_RESERVE`, so the allocation holds and what is "
            "wrong is the number the count is predicted against.",
        )
    return None


#: The two compressed inputs the flagless axis is read over: the registered
#: input, what the table calls it, and its block size.
#:
#: **Both block sizes, because the charge is a multiple of the unit.** A
#: unit-shaped error in that charge is multiplied by the reader count, and a
#: fit taken at one block size cannot tell a term that scales with the unit
#: from one that does not.
#:
#: The block size is declared here rather than read off the file because it is
#: what `reader_bytes` and `pool_bytes` are functions of, and the renderer
#: needs it to say which legs took the block path at all — `parse` emits no
#: decline note, that being a `PlanNote` on a query's `TableStream`, so
#: `block_path_afforded` over the run's own reported budget is what answers it.
RESERVE_FLAGLESS_INPUTS: tuple[tuple[str, str, int], ...] = (
    ("control_xz", "24 MiB blocks", 24 << 20),
    ("control_xz128", "128 MiB blocks", 128 << 20),
)

#: The container limits the flagless axis is read at: the `-m` value and the
#: bytes it states.
#:
#: **The limit is the axis, because under discovery it is the only input.** A
#: flagless run reads `limit − MEMORY_RESERVE`, fits the source's recommended
#: count inside it and spends exactly what that many readers cost, so one
#: number decides both terms of the arrangement — which is why these legs carry
#: a **per-spec** container limit where every other figure takes its own.
#:
#: **512 MiB is the bottom of the range**, where the rule's own worst headroom
#: is. The 128 MiB leg reads the streaming fallback there — 128 MiB granted
#: against a 522.03 MiB line — and the 24 MiB leg takes the block path at one
#: reader, which each cell says for itself (`block_path_afforded`). A fallback
#: leg is still a leg the allocation has to hold. 1 GiB and 1.5 GiB put the
#: middle of the curve in the fit rather than only its worst end; and 2 GiB is
#: where the 24 MiB leg's count saturates at the source's own recommendation,
#: which is what separates "the allowance ran out" from "the recommendation
#: did".
#:
#: **544 MiB and 1088 MiB are there for the side of the charge below
#: `POOL_DEPTH`**, where the retention list is a constant `(POOL_DEPTH − 1)`
#: units rather than growing with the count. `544m` resolves two readers of
#: 24 MiB blocks, a below-depth count no other limit gives that family.
#: `1088m` resolves one reader of 128 MiB blocks — the count `1g` resolves too,
#: so it adds a second cell of that arrangement rather than a count — and
#: eleven of 24 MiB blocks. The 128 MiB family's below-depth regime is
#: therefore one distinct count, and a line through it alone would be an
#: intercept asserted as a measurement; the family's fit is over all its
#: counts, which is what `RESERVE_FIT_MIN_COUNTS` holds.
#:
#: **Additive, so nothing already read moves**: `512m` keeps the axis's worst
#: headroom and the other three keep theirs. `544m` is also close to the
#: smallest arrangement the mechanism has, which is where a fit over the 24 MiB
#: family is checked rather than extrapolated
#: (`.claude/skills/evidence/SKILL.md`, rule 2).
#:
#: **A leg may be OOM-killed, and that is a reading rather than an apparatus
#: failure** — see `KILL_TOLERANT`, which is where that licence is granted and
#: bounded. The rule aims resident at the limit by construction, so every
#: flagless leg that takes the block path sits close to its own ceiling.
RESERVE_LIMITS: tuple[tuple[str, int], ...] = (
    ("512m", 512 << 20),
    ("544m", 544 << 20),
    ("1g", 1 << 30),
    ("1088m", 1088 << 20),
    ("1536m", 1536 << 20),
    ("2g", 2 << 30),
)

#: The command-shape prefixes whose legs may be OOM-killed without the sitting
#: dying: a kill there is a **censored reading**, recorded and reported, and the
#: figure carrying it is barred from publication.
#:
#: **Why a licence at all.** The flagless family's whole subject is a rule that
#: aims resident *at* the allocation, so a leg sitting against its own ceiling
#: is the arrangement under test rather than a mis-set apparatus. Failing fast
#: on the first one loses every leg behind it — which is what happened: an hour's
#: `--alone` sitting died on its fifth leg of eighteen and published no account
#: at all, so the mechanism legs, the path step and the whole stated-budget axis
#: were paid for and thrown away.
#:
#: **Why it is per family and never harness-wide.** Everywhere else a kill *is*
#: the apparatus failing, and being loud about it is what caught the one that
#: mattered: `parallel-peak-rss` once measured 3067 MiB inside a 3072 MiB
#: container, and a licence written across the harness would have swallowed it
#: into a footnote. A shape outside this tuple that is killed still raises, and
#: says so in those words, which the exit code cannot.
#:
#: **A censored cell is not a number.** The reading is a lower bound on a peak
#: the process never reached, so it enters neither a fit nor a headroom column;
#: the renderer states the kill instead. What it is *evidence of* is the thing
#: the constant is being chosen against, which is why it is recorded rather than
#: discarded.
KILL_TOLERANT: tuple[str, ...] = (RESERVE_FLAGLESS,)


def kill_tolerant(command: str) -> bool:
    """Whether an OOM kill of this command shape is a reading rather than an
    apparatus failure (`KILL_TOLERANT`)."""
    return command.startswith(KILL_TOLERANT)


#: The limit the mechanism leg is read at, and the input it is read over. The
#: capped instrument leg takes the same pair, so the black-box delta and the
#: introspective one describe the same arrangement.
#:
#: **One limit and one block size, deliberately.** This figure is gated
#: `warm-parallel` and crossing the mechanism leg with the limits and the
#: block sizes buys a second cross of the expensive axis for no question
#: anybody asked. The limit is the smallest on the axis. **At it the 24 MiB
#: leg resolves one reader on the block path, where the uncapped process
#: already runs two arenas**, so `MALLOC_ARENA_MAX=2` is inert at this cell —
#: the published leg reads it at −112 KiB — and says nothing about the arenas
#: more readers open. The path step beside it is the one pair that states a
#: budget, so that the block path and the streaming fallback are compared
#: inside this one allocation.
RESERVE_MECHANISM_LIMIT = "512m"
RESERVE_MECHANISM_INPUT = "control_xz"

#: How far the instrument build's resident set may sit from the shipped
#: build's before the check calls the two different runs, as a percentage.
#:
#: **Stated rather than read off the shipped leg's spread.** This is a different
#: binary — its `.text` is its own and every allocation goes through a counter —
#: so three reps of the shipped build say nothing about how far it may
#: legitimately be. Ten percent is the same order `measurements.md` records for
#: two builds of one source differing by code layout alone; what the check is
#: for is catching an instrument leg that ran a *different arrangement*, which
#: the resolved-count half answers exactly, and this half is the coarse bound
#: beside it.
INSTRUMENT_TOLERANCE_PCT = 10

#: The limits the **instrument** legs are read at: the flagless arrangement,
#: run on the introspection build, so the process states its own terms instead
#: of being differenced.
#:
#: **The whole flagless axis at one block size**, so the instrument's
#: decomposition can be read against the black-box fit leg for leg rather than
#: at a single cell. One block size, for `RESERVE_MECHANISM_LIMIT`'s reason:
#: whether a term scales with the unit is the black-box axis's question, and it
#: already crosses both.
RESERVE_INSTRUMENT_LIMITS: tuple[str, ...] = tuple(token for token, _ in RESERVE_LIMITS)

#: The block size of the input the mechanism legs run over, read out of
#: `RESERVE_FLAGLESS_INPUTS` rather than written again, so the unit the step's
#: budgets are computed from and the unit the flagless table prints cannot
#: disagree.
RESERVE_MECHANISM_UNIT = next(
    unit for name, _, unit in RESERVE_FLAGLESS_INPUTS if name == RESERVE_MECHANISM_INPUT
)

#: The path step's two stated budgets: exactly what one block-decoding reader of
#: that file costs the budget rule, and **one byte less**.
#:
#: `block_path_afforded` is the comparison, so the pair straddles it and nothing
#: else differs between the two runs — which is what makes the difference
#: between them the whole block path against the streaming fallback rather than
#: a budget change with a path change inside it.
#:
#: **It is `charge_bytes(unit, 1)` and not `reader_bytes(unit)`**:
#: `BlockCache::affordable` charges that one reader's share of the retention
#: list, so a pair straddling the per-reader term alone would run the streaming
#: decoder on *both* legs and publish their difference as the cost of a path
#: neither took.
#:
#: It earns its place whatever the attribution finds: it is what an operator
#: needs in order to decide whether `--memory` is worth setting, and no
#: figure states it.
RESERVE_STEP_BUDGETS: tuple[int, ...] = (
    charge_bytes(RESERVE_MECHANISM_UNIT, 1),
    charge_bytes(RESERVE_MECHANISM_UNIT, 1) - 1,
)

#: How many distinct reader counts a two-term fit must cover before its
#: **intercept** is published.
#:
#: **Three, because the model has two terms.** `resident = fixed + readers ×
#: per_reader` passes exactly through two points, so at two distinct counts the
#: residual the table prints is **`±0 MiB` by construction** and cannot be told
#: from a two-term model that happens to describe the mechanism. Three is the
#: smallest number of points at which the residual is a reading.
#:
#: **It is a property of the model, not of one family.** Every reader of a
#: fitted line here is reading `_least_squares`' two terms, so the guard lives
#: at that boundary — `_fit_or_secant`, which all three call sites cross — and
#: not in whichever renderer happened to notice it first.
#:
#: **Below it the slope survives and the intercept does not.** Both traps the
#: rule exists for (`.claude/skills/evidence/SKILL.md`, rule 2) are statements
#: about the intercept: curvature outside the window folds into it, and a
#: clamped term reads as a constant of the process. A difference between two
#: distinct reader counts is a measured **secant** and carries no model claim,
#: so it is published — named endpoints, no intercept, no residual — where
#: refusing the whole line would withhold a number that is sound and silently
#: withdraw the instrument's comparison against `reader_bytes` in a censored
#: sitting.
#:
#: `_least_squares` keeps its own floor of two, which is where the arithmetic
#: stops being defined; this is the *publication* rule above it, and it is the
#: one a reader of the table is relying on.
RESERVE_FIT_MIN_COUNTS = 3


def fmt_chunk(size: int) -> str:
    """A chunk size as the table spells it — KiB below a mebibyte, else MiB.

    Every registered size is a whole number of either, which a test holds:
    a table row reading `1.5 MiB` would be a size nobody chose."""
    if size % (1 << 20) == 0:
        return f"{size >> 20} MiB"
    if size % (1 << 10) == 0:
        return f"{size >> 10} KiB"
    raise ValueError(f"chunk size {size} is not a whole number of KiB")


def _script(command: str) -> str:
    """The in-container shell for one command shape.

    The timer is a bash builtin inside the container. Nothing redirects stderr
    inside a timed command -- some shells route `time`'s own report through the
    timed command's redirection, which deletes the figure and leaves a labelled
    run with no number under it.

    **Every `pgdt` shape states its worker count**, because a shape that
    inherits the CLI's default measures whatever that default is on the day --
    see `SWEEP_JOBS`. `--check` refuses a shape that pins none."""
    q = "time /pgdt"
    j = f"--jobs {SWEEP_JOBS}"
    ns = NO_STATISTICS
    if command == "parse":
        return f"{q} parse --source /dump.sql --dtcache /tmp/x.dtcache {j} {ns} >/dev/null"
    if command == "parse-rss":
        # The same `parse` as above, wrapped so the run reports its own peak
        # resident set as well as its wall clock. The redirection is outside
        # the wrapper and takes pgdt's stdout with it; the reading goes to
        # stderr, where bash's `time` report already goes.
        return (
            f"time {PEAK_RSS} /pgdt parse --source /dump.sql "
            f"--dtcache /tmp/x.dtcache {j} {ns} >/dev/null"
        )
    if command == "parse-preamble":
        return (
            f"{q} parse --preamble-only --source /dump.sql --dtcache /tmp/x.dtcache "
            f"{j} >/dev/null"
        )
    if command == "parse-preamble-rss":
        # `rss-attribution`'s per-*table* leg: the same prepass as above,
        # wrapped so what it holds resident is the reading. It stops at the end
        # of the schema section, before a data block is read, which is what
        # separates a cost paid per table from one paid per `COPY` block --
        # `peak-rss`'s own inputs cannot, since `blocks4000` gives every table
        # exactly one block and the two coincide in it.
        return (
            f"time {PEAK_RSS} /pgdt parse --preamble-only "
            f"--source /dump.sql --dtcache /tmp/x.dtcache {j} >/dev/null"
        )
    if command == "info-cache-rss":
        # An index *deserialized* rather than built: the second route to the
        # per-table structures, with no scanner, no census and no splice
        # anywhere in the timed process.
        #
        # **The cache is built in the same container, outside the timer and
        # outside the wrapper.** A prebuilt one mounted from the host would
        # have to be staged per input and kept in step with the input's own
        # stamp; built here it is by construction this input's cache. `time`
        # and the wrapper both sit on the `info` alone, so `parse_bash_time`
        # and `parse_maxrss_kib` each still see exactly one report -- and the
        # wrapper is a fresh process, so its `RUSAGE_CHILDREN` cannot carry the
        # builder's peak.
        #
        # **The worker count is stated on the builder, and `info` states
        # none**: `info` takes no `--jobs` because it starts no workers, so
        # there is no default for it to inherit.
        return (
            f"/pgdt parse --source /dump.sql --dtcache /tmp/x.dtcache {j} {ns} >/dev/null && "
            f"time {PEAK_RSS} /pgdt info --dtcache /tmp/x.dtcache "
            ">/dev/null"
        )
    if command in ("query-nomatch-cached-rss", "query-nomatch-rss"):
        # The pair that isolates the un-throttled splice, one flag apart. A
        # table that never matches maps to EOF and renders no row, so what
        # differs between them is the save throttle and nothing else: with a
        # cache path `parse`'s throttle governs, with `--dtcache none` the
        # whole-list rebuild is paid per block (`KD5`).
        #
        # `query` rather than `parse` because `parse` refuses `--dtcache none`
        # outright -- "cache is disabled, but `parse` requires a cache file" --
        # so the un-throttled shape is reachable only through `query`.
        cache = "/tmp/x.dtcache" if command.endswith("cached-rss") else "none"
        return (
            f"time {PEAK_RSS} /pgdt query --source /dump.sql "
            f"--table public.nosuchtable --dtcache {cache} {j} >/dev/null"
        )
    if command == "parse-cache-out":
        # The cache goes to the mounted tmpfs, not the container's own layer,
        # and the removal is outside the timer.
        return (
            "rm -f /out/measure.dtcache; "
            f"{q} parse --source /dump.sql --dtcache /out/measure.dtcache {j} {ns} >/dev/null"
        )
    if command in ("query-typed", "query-strings"):
        # Over a data-level cache built ahead of the timer (`DATA_LEVEL_QUERIES`).
        mode = command.split("-")[1]
        return (
            f"{DATA_LEVEL_BUILDER}{q} query --source /dump.sql --table public.perf "
            f"{DATA_LEVEL_QUERY} --schema-mode {mode} {j} >/dev/null"
        )
    if command.startswith("query-project-"):
        # Typed, always: the figure is about what building a column costs, and
        # `strings` builds every column the same cheap way.
        width = command.rpartition("-")[2]
        if not width.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        return (
            f"{DATA_LEVEL_BUILDER}{q} query --source /dump.sql --table public.perf "
            f"{DATA_LEVEL_QUERY} --schema-mode typed {projection_flags(int(width))} {j} "
            ">/dev/null"
        )
    if command.startswith("query-where-"):
        # `strings`, always, for two reasons that agree. The typed `=` decodes
        # the literal once against the column's own type, so `zzz1` on an
        # `integer` column is `Error::PredicateValueDecode` before the first
        # row; and the zero-copy path is the one this lever is read against,
        # since `strings` is where the field split and the walk are most of
        # what the library does (`decisions.md`, "D29", whose split-and-walk and decode rows do not move with the
        # mode).
        expr = predicate_expr(command.removeprefix("query-where-"))
        return (
            f"{DATA_LEVEL_BUILDER}{q} query --source /dump.sql --table public.perf "
            f"{DATA_LEVEL_QUERY} --schema-mode strings --where '{expr}' {j} >/dev/null"
        )
    if command == "query-nomatch":
        # Maps to EOF (the table never matches) and never saves.
        return (
            f"{q} query --source /dump.sql --table public.nosuchtable --dtcache none "
            f"{j} >/dev/null"
        )
    if command.startswith(STATISTICS_FAMILY):
        # `statistics-gathering`: the same wrapped `parse` as `parse-rss`, its
        # level stated by the leg.
        leg, _, suffix = command.removeprefix(STATISTICS_FAMILY).partition("-")
        flags = dict(STATISTICS_LEGS)
        if leg not in flags or suffix != "rss":
            raise ValueError(f"unknown command shape {command!r}")
        return (
            f"time {PEAK_RSS} /pgdt parse --source /dump.sql "
            f"--dtcache /tmp/x.dtcache {j} {flags[leg]} >/dev/null"
        )
    if command.startswith(PRUNING_FAMILY):
        # `statistics-pruning`. The builder is outside the timer and states the
        # gathering request; the timed `query` states which use it makes of it.
        # `typed`, stated: the range compares `id` as an integer.
        name, _, leg = command.removeprefix(PRUNING_FAMILY).rpartition("-")
        if name not in PRUNING_FILTERS or leg not in PRUNING_LEGS:
            raise ValueError(f"unknown command shape {command!r}")
        expr = PRUNING_FILTERS[name][0]
        return (
            f"/pgdt parse --source /dump.sql --dtcache /tmp/x.dtcache {j} "
            f"{GATHER_STATISTICS} >/dev/null && "
            f"{q} query --source /dump.sql --table public.perf --dtcache /tmp/x.dtcache "
            f"--schema-mode typed --where '{expr}' --statistics {leg} {j} >/dev/null"
        )
    if command == DYNFILTER_STARTUP:
        # The builder and the registration exactly as a leg has them, and a
        # query whose answer is read back outside the timer, so a startup that
        # did not answer is refused rather than timed.
        env = [f"{DFCLI_PARTITIONS}={SWEEP_JOBS}"]
        argv = ["--dump", f"{DFCLI_CATALOG}=/dump.sql", "--format", "csv", "-q", "-c", STARTUP_SQL]
        return (
            f"/pgdt parse --source /dump.sql --dtcache /dump.sql.dtcache {j} "
            f"{GATHER_STATISTICS} >/dev/null && "
            f"time {dfcli_shell(env, SQL_SHELL, argv)} "
            ">/tmp/result.csv && "
            "echo startup_answer=$(sed -n 2p /tmp/result.csv)"
        )
    if command.startswith(DYNFILTER_FAMILY):
        # `dynamic-filter-join` and `-topk`. The builder is outside the timer,
        # as `statistics-pruning`'s is, and writes the cache beside the dump,
        # where `--dump` looks — the root of the container's own layer, since
        # the dump is mounted read-only. The answer is read back outside the
        # timer too, as `key=value` lines `parse_reported` takes.
        figure, _, rest = command.removeprefix(DYNFILTER_FAMILY).partition("-")
        name, _, leg = rest.rpartition("-")
        if name not in DYNFILTER_QUERIES.get(figure, {}) or leg not in DYNFILTER_LEGS:
            raise ValueError(f"unknown command shape {command!r}")
        env, argv = dfcli_invocation(figure, name, leg, "/dump.sql")
        return (
            f"/pgdt parse --source /dump.sql --dtcache /dump.sql.dtcache {j} "
            f"{GATHER_STATISTICS} >/dev/null && "
            f"time {dfcli_shell(env, SQL_SHELL, argv)} "
            f">/tmp/result.csv && {DFCLI_ANSWER}"
        )
    if command.startswith("parse-chunk-"):
        # The read chunk, the one lever of the three I/O defaults that is a
        # value rather than a scheme. `parse` rather than `query`: this is
        # about the bytes arriving, and the row machinery above it is what the
        # rest of the register measures.
        size = command.rpartition("-")[2]
        if not size.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(size) not in CHUNK_SIZES:
            raise ValueError(f"{command!r} names a chunk size the figure does not carry")
        return (
            f"{q} parse --source /dump.sql --dtcache /tmp/x.dtcache "
            f"--chunk-size {size} {j} {ns} >/dev/null"
        )
    if command.startswith(RESERVE_FAMILY):
        # The reserve family: one stated budget, one stated worker count, and
        # an arena setting in front of the wrapper rather than on `nerdctl
        # run`, so the whole leg is visible in the recorded argv the way every
        # other apparatus choice is.
        #
        # **The assignment goes before `peak-rss`, not before `/pgdt`.** The
        # wrapper starts its arguments with its own environment, so the child
        # inherits it; an assignment written on the inner command would be a
        # further argument to `peak-rss` and would set nothing.
        token, _, budget = command.removeprefix(RESERVE_FAMILY).rpartition("-")
        arenas = {name: value for name, value, _ in RESERVE_ARENAS}
        if token not in arenas:
            raise ValueError(f"{command!r} names an arena setting the figure does not carry")
        if not budget.isdigit() or int(budget) not in RESERVE_BUDGETS:
            raise ValueError(f"{command!r} names a budget the figure does not carry")
        arena = f"MALLOC_ARENA_MAX={arenas[token]} " if arenas[token] else ""
        return (
            f"time {arena}{PEAK_RSS} /pgdt parse "
            f"--source /dump.sql --dtcache /tmp/x.dtcache "
            f"--jobs {RESERVE_JOBS} --memory {stated_allowance(int(budget))} {ns} >/dev/null"
        )
    if command.startswith(RESERVE_FLAGLESS):
        # The flagless family: the same `parse` under the same wrapper, with
        # **neither** flag, so what runs is what `Discovered::resolve`
        # resolves from the container's own limit. The arena setting is still a
        # token, because the arena-cap mechanism leg is this shape with
        # `MALLOC_ARENA_MAX` set and nothing else changed.
        #
        # **The container limit is not here**, and it cannot be: it is a
        # `nerdctl run` argument rather than an argv one, so it rides on the
        # `RunSpec` instead (`RunSpec.memory`) and is what two legs of this
        # family differ by.
        token = command.removeprefix(RESERVE_FLAGLESS)
        arenas = {name: value for name, value, _ in RESERVE_ARENAS}
        if token not in arenas:
            raise ValueError(f"{command!r} names an arena setting the figure does not carry")
        arena = f"MALLOC_ARENA_MAX={arenas[token]} " if arenas[token] else ""
        return (
            f"time {arena}{PEAK_RSS} /pgdt parse "
            f"--source /dump.sql --dtcache /tmp/x.dtcache {ns} >/dev/null"
        )
    if command.startswith(RESERVE_STEP_FAMILY):
        # The path step: a stated budget either side of `charge_bytes(unit, 1)`
        # — what `BlockCache::affordable` charges one reader, pool list
        # included, and not `reader_bytes` alone — one byte
        # apart, so the two runs differ by whether `BlockCache::affordable`
        # admits a block-decoding reader and by nothing else.
        budget = command.removeprefix(RESERVE_STEP_FAMILY)
        if not budget.isdigit() or int(budget) not in RESERVE_STEP_BUDGETS:
            raise ValueError(f"{command!r} names a budget the figure does not carry")
        return (
            f"time {PEAK_RSS} /pgdt parse "
            f"--source /dump.sql --dtcache /tmp/x.dtcache "
            f"--jobs {RESERVE_JOBS} --memory {stated_allowance(int(budget))} {ns} >/dev/null"
        )
    if command.startswith(JOBS_AXIS):
        # The three shapes whose worker count is a figure's axis rather than the
        # apparatus's constant. Each `pgdt` one is otherwise the shape it is
        # named after, so a `parse` row of `parallel-scan-throughput` and the
        # corresponding row of `scan-throughput-warm` differ in `--jobs` and
        # `--memory` and nothing else.
        #
        # **The budget is stated on every row, including the first, where it is
        # inert**: `--jobs 1` is `Parallelism::Serial` and `Serial` carries no
        # budget, so the baseline runs at the library's own default. Writing the
        # flag anyway keeps the argv one shape rather than two, and the table is
        # what says the baseline is the serial path.
        shape, _, jobs = command.rpartition("-jobs-")
        if not jobs.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(jobs) not in PARALLEL_JOBS:
            raise ValueError(f"{command!r} names a job count the figure does not carry")
        p = f"--jobs {jobs} --memory {stated_allowance(PARALLEL_BUDGET)}"
        if shape == "parse":
            return f"{q} parse --source /dump.sql --dtcache /tmp/x.dtcache {p} {ns} >/dev/null"
        if shape == "parse-rss":
            return (
                f"time {PEAK_RSS} /pgdt parse --source /dump.sql "
                f"--dtcache /tmp/x.dtcache {p} {ns} >/dev/null"
            )
        if shape == PARALLEL_SCAN:
            # The dynamic-filter figures' builder and read-back around
            # `PARALLEL_SCAN_SQL`: the builder states `SWEEP_JOBS`, not the
            # row's count, so every row reads one cache, written beside the
            # dump where `--dump` looks; the count is the provider's
            # `target_partitions`, and `--memory` its `SET pgdump.memory`.
            env, argv = parallel_scan_invocation(int(jobs), "/dump.sql")
            return (
                f"/pgdt parse --source /dump.sql --dtcache /dump.sql.dtcache {j} "
                f"{GATHER_STATISTICS} >/dev/null && "
                f"time {dfcli_shell(env, SQL_SHELL, argv)} "
                f">/tmp/result.csv && {DFCLI_ANSWER}"
            )
        raise ValueError(f"unknown command shape {command!r}")
    if command.startswith("decode-"):
        # The `xz_decode` example, not `pgdt`: nothing in the library decodes
        # concurrently yet, so this figure reaches the decoder's own bulk entry
        # point directly. `/pgdt` is the harness's fixed mount point for
        # whichever binary a spec names, and `/dump.sql` its fixed mount point
        # for the input, whatever that input actually is.
        #
        # **Nothing is redirected.** The instrument's stdout is where the
        # decoded byte count and the admitted worker count come back, which is
        # what the table's rate is computed from and what says the worker count
        # was not clamped.
        workers = command.rpartition("-")[2]
        if not workers.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(workers) not in DECODE_WORKERS:
            raise ValueError(f"{command!r} names a worker count the figure does not carry")
        return f"time /pgdt --source /dump.sql --workers {workers}"
    if command == "dd":
        return "time dd if=/dump.sql of=/dev/null bs=4M"
    raise ValueError(f"unknown command shape {command!r}")


def dynfilter_shapes(figure: str | None = None) -> tuple[str, ...]:
    """The dynamic-filter figures' command shapes, or one figure's, in its
    table's order: each query, in `DYNFILTER_LEGS`' order."""
    return tuple(
        f"{DYNFILTER_FAMILY}{fig}-{name}-{leg}"
        for fig, queries in DYNFILTER_QUERIES.items()
        if figure in (None, fig)
        for name in queries
        for leg in DYNFILTER_LEGS
    )


def command_shapes() -> tuple[str, ...]:
    """Every command shape `_script` builds, enumerated.

    `_script` parses its argument rather than matching it, so the
    parameterized families are expanded from the registries that define them
    — one list, not two. What this exists for is the worker-count
    reconciliation below, which has to iterate the shapes to check them;
    `test_measure.py` holds it against `_script`'s own branches, so a shape
    added there and not here is an error rather than a silent exemption."""
    return (
        "parse",
        "parse-rss",
        "parse-preamble",
        "parse-preamble-rss",
        "info-cache-rss",
        "parse-cache-out",
        "query-typed",
        "query-strings",
        "query-nomatch",
        "query-nomatch-cached-rss",
        "query-nomatch-rss",
        *(f"query-project-{w}" for w in PROJECTION_WIDTHS),
        *(f"query-where-{s}" for s in PREDICATE_SHAPES),
        *(f"parse-chunk-{n}" for n in CHUNK_SIZES),
        *(f"{STATISTICS_FAMILY}{leg}-rss" for leg, _ in STATISTICS_LEGS),
        *(f"{PRUNING_FAMILY}{name}-{leg}" for name in PRUNING_FILTERS for leg in PRUNING_LEGS),
        *dynfilter_shapes(),
        DYNFILTER_STARTUP,
        *(f"{family}{n}" for family in JOBS_AXIS for n in PARALLEL_JOBS),
        *(
            f"{RESERVE_FAMILY}{token}-{budget}"
            for token, _, _ in RESERVE_ARENAS
            for budget in RESERVE_BUDGETS
        ),
        *(f"{RESERVE_FLAGLESS}{token}" for token, _, _ in RESERVE_ARENAS),
        *(f"{RESERVE_STEP_FAMILY}{budget}" for budget in RESERVE_STEP_BUDGETS),
        *(f"decode-{w}" for w in DECODE_WORKERS),
        "dd",
    )


#: A worker count stated on a command line: `pgdt`'s `--jobs`, or the decode
#: instrument's own `--workers`. Either spelling pins the count; what fails is
#: a shape carrying neither.
_WORKER_COUNT = re.compile(r"--(?:jobs|workers) \d+")

#: The one shape that states no worker count and is right not to: `dd` is the
#: device floor, not a run of ours. Named rather than inferred, so a second
#: non-`pgdt` shape has to be admitted here on purpose.
_NO_WORKERS = ("dd",)

#: The one `pgdt` family that states no worker count and is right not to: the
#: reserve figure's flagless legs, whose reading *is* the count a flagless run
#: resolves. Declared as its own prefix rather than appended to `_NO_WORKERS`
#: above, because that tuple's rule is "not a run of ours" and this is the
#: opposite — it is exactly a run of ours, stating nothing on purpose
#: (`RESERVE_FLAGLESS`).
_NO_FLAGS = (RESERVE_FLAGLESS,)


def worker_count_problems() -> list[str]:
    """Command shapes that inherit a worker count instead of stating one.

    The mechanical half of "a worker count is apparatus" (`SWEEP_JOBS`): a
    shape that pins nothing measures whatever the CLI's `--jobs` defaults to
    that day, and no table can say which arrangement it read. That is how the
    default has moved underneath the published figures with no shape changing
    and nothing noticing.

    **The flagless family is exempt and declares itself** (`_NO_FLAGS`): its
    reading is the count a flagless run resolves, so a shape that pinned one
    would measure an arrangement the shipped default never produces.
    `flagless_flag_problems` is the other side of that exemption — it holds
    those shapes to stating *neither* flag, so the exemption cannot become a
    quiet pin."""
    return [
        command
        for command in command_shapes()
        if command not in _NO_WORKERS
        and not command.startswith(_NO_FLAGS)
        and (
            not _WORKER_COUNT.search(_script(command))
            or _dfcli_partitions(_script(command)) == set()
        )
    ]


#: The partition count a run of `SQL_SHELL` states: its session's
#: `target_partitions`, from the environment in front of it.
_DFCLI_PARTITIONS = re.compile(rf"\b{DFCLI_PARTITIONS}=(\d+) (?:\S+=\S+ )*{re.escape(SQL_SHELL)} ")


def _dfcli_partitions(script: str) -> set[str] | None:
    """The partition counts every run of `SQL_SHELL` in `script` states, `set()`
    where one of them states none, or `None` for a script that runs none.

    **A run of the SQL shell is held to its own count**, because the
    shapes that run it run `pgdt parse` too: the untimed builder's `--jobs` would
    otherwise satisfy `_WORKER_COUNT` for a timed run that inherits
    DataFusion's `target_partitions`, which defaults to the core count."""
    runs = script.count(f"{SQL_SHELL} ")
    if not runs:
        return None
    stated = _DFCLI_PARTITIONS.findall(script)
    return set(stated) if len(stated) == runs else set()


def stated_threads(command: str) -> int | None:
    """The most workers any program in a command shape states, which is what
    `--pin-cpus` places it by, or `None` where one of them discovers its own.

    Read off the script rather than declared beside the shape, so the count a
    leg is placed by is the count it runs at. `dd` states none and is one
    thread; the flagless family states none on purpose and stays unpinned,
    its reading being what discovery resolves."""
    if command in _NO_WORKERS:
        return 1
    if command.startswith(_NO_FLAGS):
        return None
    script = _script(command)
    counts = [int(n) for n in re.findall(r"--(?:jobs|workers) (\d+)", script)]
    partitions = _dfcli_partitions(script)
    if partitions == set():
        return None
    counts += [int(n) for n in partitions or ()]
    return max(counts) if counts else None


#: One `pgdt parse` invocation inside a command shape's script, up to the next
#: command separator: a `;`, or the `&&` every builder is joined by.
_PARSE_RUN = re.compile(r"/pgdt parse (?:(?!&&)[^;])*")


#: The command-shape prefixes `statistics_flag_problems` lets state
#: `GATHER_STATISTICS`.
GATHERING_FAMILIES: tuple[str, ...] = (
    STATISTICS_FAMILY,
    PRUNING_FAMILY,
    DYNFILTER_FAMILY,
    f"{PARALLEL_SCAN}-jobs-",
    *DATA_LEVEL_QUERIES,
)


def statistics_flag_problems() -> list[str]:
    """Command shapes running a `pgdt parse` that does not state
    `NO_STATISTICS`.

    `parse` gathers statistics unless told not to, so such a shape times the
    gathering default rather than the scan its figure names. `--preamble-only`
    stops before any row and refuses the flag, so it is exempt.

    **The families whose subject is the gathering or what it buys may state
    `GATHER_STATISTICS` instead**, and only that — the two statistics
    figures', the dynamic-filter figures', whose scans prune by what the
    untimed builder gathered, and the query figures', which read the cache it
    wrote (`DATA_LEVEL_QUERIES`, `PARALLEL_SCAN`) — and a `parse` of theirs that inherits the
    request is reported exactly as anyone else's is."""
    return [
        command
        for command in command_shapes()
        if any(
            NO_STATISTICS not in run
            and "--preamble-only" not in run
            and not (command.startswith(GATHERING_FAMILIES) and GATHER_STATISTICS in run)
            for run in _PARSE_RUN.findall(_script(command))
        )
    ]


#: A byte budget stated on a command line. The flagless family must carry
#: neither this nor a worker count.
_BUDGET_STATED = re.compile(r"--memory \d+")


def flagless_flag_problems() -> list[str]:
    """Shapes in the flagless family that state a flag after all.

    The exemption `_NO_FLAGS` opens is from *stating a count*, and its whole
    premise is that the arrangement under test is the one a run with no flags
    resolves for itself. A shape that quietly acquired `--jobs` or
    `--memory` would still pass `worker_count_problems` — it is exempt
    — and would publish a stated arrangement under a heading that says
    discovered, which is the failure the count reconciliation exists against
    seen from the far side."""
    bad = []
    for command in command_shapes():
        if not command.startswith(_NO_FLAGS):
            continue
        script = _script(command)
        stated = [*_WORKER_COUNT.findall(script), *_BUDGET_STATED.findall(script)]
        if stated:
            bad.append(f"{command} states {', '.join(sorted(stated))}")
    return bad


def pinned_count_problems() -> list[str]:
    """Command shapes stating a worker count that is neither `SWEEP_JOBS` nor a
    declared axis.

    The other half of the exemption `JOBS_AXIS` opens. `worker_count_problems`
    catches a shape that pins *nothing*; this catches one that pins something
    else — a shape edited to `--jobs 4` because a sitting wanted it that day, or
    a shape moved into a parallel family without being declared one. Either
    produces a table whose apparatus line is wrong about it, which is the same
    defect from the other side and is the one no `--stale` can see.

    The decode instrument states `--workers`, not `--jobs`, and its counts are
    its own figure's axis; it is exempt for the same reason, by prefix. So are
    the reserve figure's two stated families, whose count is the figure's own
    constant (`RESERVE_JOBS`), and its flagless one, which states nothing and is
    held to that by `flagless_flag_problems`.
    """
    bad = []
    for command in command_shapes():
        if command in _NO_WORKERS or command.startswith(
            ("decode-", RESERVE_FAMILY, RESERVE_STEP_FAMILY, *_NO_FLAGS, *JOBS_AXIS)
        ):
            continue
        stated = set(_WORKER_COUNT.findall(_script(command)))
        if stated != {f"--jobs {SWEEP_JOBS}"}:
            bad.append(f"{command} states {', '.join(sorted(stated)) or 'nothing'}")
        partitions = _dfcli_partitions(_script(command))
        if partitions is not None and partitions != {str(SWEEP_JOBS)}:
            bad.append(
                f"{command} states {DFCLI_PARTITIONS}="
                f"{', '.join(sorted(partitions)) or 'nothing'}"
            )
    return bad


def charge_band_problems() -> list[str]:
    """Fault bands the code defines that `BAND_STANCE` says nothing about.

    **A rule written as a threshold releases a band nobody argued about.** "No
    evaluated cell above `MEMORY_RESERVE`" is a true sentence about two of
    `charge_model_problem`'s three lines and silently lets the third — the
    over-bill side — through. The rule is an enumeration (`BAND_STANCE`), and
    this is what holds the enumeration to the bands that exist: a fourth line
    added later refutes by default and is reported here until its stance is
    recorded, rather than inheriting whichever half its author had in mind.

    **Asked of the constants rather than of a sitting**, so it fails at
    `--check` time: two things that must agree, with nothing else reading
    both.

    The definition site it reads is the naming: every `BAND_*` string constant
    in this module is one of `charge_model_problem`'s bands, which is why the
    unlisted band's reason is spelled `UNARGUED_BAND_REFUTES` rather than with
    the prefix.
    """
    bad = []
    for name, value in sorted(globals().items()):
        if not name.startswith("BAND_") or not isinstance(value, str):
            continue
        if value not in BAND_STANCE:
            bad.append(
                f"{name} ({value!r}) has no stance in `BAND_STANCE`, so it refutes the model "
                "by default — record it as released or refuting"
            )
    return bad


def reserve_axis_problems() -> list[str]:
    """What is wrong with the registered flagless axis: a block size at which
    the registered limits cannot reach `RESERVE_FIT_MIN_COUNTS` distinct reader
    counts.

    **It does not ask whether the pool term is billed anywhere**, because
    `(POOL_DEPTH.max(jobs) − 1) × unit` is billed at **every** cell and is the
    larger half of the charge above four readers: there is no window for a limit
    to be registered inside and no cell that evades it.

    **Nor does it ask that the axis straddle `POOL_DEPTH`.** Below-depth
    coverage is real and is why `544m` and `1088m` are registered
    (`RESERVE_LIMITS`), but it is a property of what the *fit* needs, and the
    fit is given the charge's own two-regime model to subtract rather than a
    straight line to find across the kink (`_depooled`). A check that the axis
    straddles the depth would be guarding a requirement nothing has; the term it
    would protect is already held to the library's own constants by
    `test_the_mirrored_pool_depth_and_constants_are_the_librarys_own`.

    **Asked of the registered limits rather than of a sitting**, which is what
    makes it a `--check` and not a verdict: an axis whose block-path limits
    afford fewer than `RESERVE_FIT_MIN_COUNTS` distinct reader counts can only
    ever publish a secant, and a later axis edit that quietly makes that true
    should fail before a sitting is spent rather than after. It is a
    **necessary** condition and not a sufficient one: the resolved count is
    `min(recommendation, fit)`, so a host with fewer cores than there are
    distinct fits collapses two of them onto one count. That residue is a
    per-sitting property and is what the secant covers; the static check
    is not where the guarantee comes from.
    """
    bad = []
    for _name, label, unit in RESERVE_FLAGLESS_INPUTS:
        counts = sorted(
            {afforded_readers(unit, discovered_budget(limit)) for _, limit in RESERVE_LIMITS}
            - {0}
        )
        if len(counts) < RESERVE_FIT_MIN_COUNTS:
            bad.append(
                f"{label}: the registered limits afford {len(counts)} distinct reader "
                f"count(s) — {counts or 'none'} — under the {RESERVE_FIT_MIN_COUNTS} a "
                "two-term fit needs, so this family publishes a secant however the sitting "
                "goes; register a limit affording a count none of the others does"
            )
    return bad


class Session:
    def __init__(
        self,
        cfg: Config,
        stager: Stager,
        log: Callable[[str], None],
        out_root: Path | None = None,
    ) -> None:
        self.cfg = cfg
        self.stager = stager
        self.log = log
        #: This sitting's output directory, which is where an instrument leg's
        #: reports are kept — beside `raw.json`, because they are that
        #: sitting's readings and not a scratch artifact. `None` falls back to
        #: `runs/`, which is what a test constructing a bare `Session` gets.
        self.out_root = out_root or cfg.out_dir
        self.readings: dict[str, list[float]] = {}
        #: Peak resident set, in KiB, under the same keys as `readings`. A
        #: second dict rather than a second number per reading: only the runs
        #: whose command shape carries the RSS wrapper have one, and a figure
        #: that wants it wants it *instead of* the wall clock, not beside it.
        self.rss: dict[str, list[float]] = {}
        self.records: list[dict] = []
        #: The stager's step this sitting is at (`Stager.plan`): the figure in
        #: hand, or the group of a split figure being swept.
        self.step = 0
        self.figure_id = ""
        #: The container memory limit in force, which `emit` sets per figure.
        #: `None` means the recorded 512 MB.
        self.memory: str | None = None
        #: The key-value lines the last run's own stdout carried. Only the
        #: decode instrument writes any; a figure that wants one wants it
        #: *beside* the wall clock rather than instead of it, which is why it
        #: is neither a second reading nor part of the RSS dict.
        self._last_stdout: dict[str, str] = {}
        #: Those lines, kept per reading key, so a figure can compute a rate
        #: from what the binary said it decoded rather than from a file size a
        #: second mechanism would have to agree with.
        self.reported: dict[str, dict[str, str]] = {}
        #: The instrument build's own report, **one dict per rep**, under the
        #: same keys as `rss`. A second dict rather than more entries in
        #: `reported`: that one is keyed per spec and holds facts identical
        #: across reps, of which only the last is kept, and `live_peak_bytes`
        #: and the `mallinfo_*` fields are not that — they are readings, and a
        #: spread of them is the thing an attribution reads.
        self.instrument: dict[str, list[dict[str, str]]] = {}
        #: How many reports this sitting has written, which names the next
        #: file. A plain counter rather than the reading key: a rep discarded
        #: by the contention gate is re-taken, and two runs must not write the
        #: same file.
        self._instrument_reports = 0
        #: The report the last run wrote, or `{}` where the leg declared none.
        self._last_instrument: dict[str, str] = {}
        #: How many of each key's reps the kernel OOM-killed. A censored
        #: reading: it is in neither `readings` nor `rss`, because the peak it
        #: reports is a bound the process never got past rather than the peak
        #: the arrangement holds — see `KILL_TOLERANT`. The key is present with
        #: a count where a kill happened and absent everywhere else, so a
        #: renderer asks one question rather than comparing rep counts.
        self.killed: dict[str, int] = {}
        self._dry_reps: dict[str, int] = {}
        self._last_telemetry: dict[str, float] = {}
        self._last_rss: float | None = None
        #: Whether the run just taken was OOM-killed under a family that
        #: tolerates it. Read by `sweep`, which files the rep as censored
        #: instead of as a reading.
        self._last_killed = False
        self.sampler = Sampler()
        #: Every reading's telemetry, in the order taken, so a sweep can be
        #: audited after the fact even where the gate let a reading through.
        self.telemetry: list[dict] = []
        #: The places each figure's programs ran in — an image, or `HOST` —
        #: and the glibc each place answered, which is what a figure's marker
        #: names where the stamp does not speak for it (`marker_glibc`).
        self.places: dict[str, set[str]] = {}
        self.glibcs: dict[str, str] = {}
        #: The arrangements every leg is taken under, and the one in force.
        #: The first is the one `readings` and every table carry.
        self.arms = cfg.arms
        self.arm = self.arms[0]
        #: Every arm's readings under the same keys as `readings`, kept only
        #: where there is more than one arm (`raw.json`'s `arms`).
        self.arm_readings: dict[str, dict[str, list[float]]] = {}
        #: The machine's L3 groups, read only where a leg may be pinned.
        self.groups: list[frozenset[int]] = (
            l3_groups() if any(a.pinned for a in self.arms) else []
        )
        #: Where the harness was allowed to run when it started, which is
        #: where it goes back to beside an unpinned leg.
        self._harness_home = frozenset(os.sched_getaffinity(0))
        self._harness_at: frozenset[int] | None = None
        #: Each binary's tmpfs copy, made once a process (`stage_binary`).
        self._staged_bins: dict[Path, Path] = {}

    # -- placement ------------------------------------------------------------

    def leg_cpus(self, spec: RunSpec) -> frozenset[int] | None:
        """The CPUs this leg is pinned to under the arm in force, or `None`."""
        if not self.arm.pinned:
            return None
        return placement(stated_threads(spec.command), self.groups)

    def place_harness(self, cpus: frozenset[int]) -> None:
        """Move every thread of this process — the sampler's included — and so
        whatever it launches next, onto `cpus`. `sched_setaffinity` moves one
        thread, so each is moved by its own id."""
        if cpus == self._harness_at or self.cfg.dry_run:
            return
        for tid in os.listdir("/proc/self/task"):
            try:
                os.sched_setaffinity(int(tid), cpus)
            except (ProcessLookupError, PermissionError):
                continue
        self._harness_at = cpus

    def stage_binary(self, src: Path) -> Path:
        """`src`'s copy on tmpfs, which a staged leg mounts in its place.

        Copied once a process, after the harness's own build: a `drop_caches`
        evicts a binary read off disk, so an unstaged cold leg loads it inside
        the timer, and tmpfs is shmem, which it does not evict. Outside the
        warm budget — a few hundred MiB against the tmpfs's slack — and removed
        with the staged inputs (`Stager.cleanup`)."""
        if src in self._staged_bins:
            return self._staged_bins[src]
        # Named by the path it came from as well as its own name: the
        # instrument build and the register's are both called `pgdt`.
        tag = hashlib.sha256(str(src.resolve()).encode()).hexdigest()[:12]
        dst = self.cfg.warm_dir / STAGED_BIN_DIR / f"{tag}-{src.name}"
        if self.cfg.dry_run:
            self.log(f"  [dry-run] would stage {src} -> {dst}")
        else:
            dst.parent.mkdir(parents=True, exist_ok=True)
            self.log(f"  staging {src} -> {dst}")
            shutil.copy2(src, dst)
        self._staged_bins[src] = dst
        return dst

    def ran_in(self, figure: str, where: str) -> None:
        """Record that `figure`'s program runs in `where`, asking that place
        for its glibc the first time — before the run, so a place that is no
        glibc refuses the figure rather than a sitting's last table."""
        self.places.setdefault(figure, set()).add(where)
        if not self.cfg.dry_run and where not in self.glibcs:
            self.glibcs[where] = glibc_of(self.cfg, where)

    # -- one timed run ----------------------------------------------------

    def binary_path(self, which: str) -> Path:
        if which in ("pgdt", "dfcli"):
            return self.cfg.bin_pgdt
        if which == "xzdecode":
            return ensure_xz_decode_binary(self.cfg, self.log)
        if which.startswith("alloc:"):
            return ensure_allocator_binary(self.cfg, which.removeprefix("alloc:"), self.log)
        if which == "introspect":
            return ensure_instrument_binary(self.cfg, self.log)
        raise ValueError(f"unknown binary {which!r}")

    def input_path(self, name: str, regime: str) -> Path:
        """Where this regime's copy of that input lives.

        Resolved through `REGIMES`, which raises on a name it does not carry.
        The branch below has no fallback for the same reason: an unrecognised
        regime must not resolve to a path, because a reading off the wrong
        device is a table nobody can tell from a right one.
        """
        area = regime_spec(regime).area
        if area == "cold":
            return self.stager.cold_path(name)
        if area == "nvme":
            return self.stager.nvme_path(name)
        if area == "warm":
            return self.stager.warm_path(name, self.step)
        raise ValueError(f"regime {regime!r} names an unknown staging area {area!r}")

    def input_size(self, name: str, regime: str) -> int:
        """That input's size, which is what a rate is per.

        A warm input is sized off the SSD copy it is staged from -- the same
        bytes -- so a renderer asking after a split figure's sweeps does not
        stage back an input eviction has taken off tmpfs."""
        if regime_spec(regime).area == "warm":
            return file_size(self.cfg, self.cfg.cache_dir / input_file(name), name)
        return file_size(self.cfg, self.input_path(name, regime), name)

    def stage(self, figure: str, group: int) -> None:
        """Move to that figure's group's step and stage the group, evicting
        what no later step wants first."""
        self.step = self.stager.step_of(figure, group)
        fig = EVERY_BY_ID.get(figure)
        groups = fig.staging_groups if fig else ()
        for name in groups[group] if group < len(groups) else ():
            self.stager.warm_path(name, self.step)

    def drop_caches(self) -> None:
        argv = shlex.split(self.cfg.sudo) + ["sh", "-c", "sync; echo 3 > /proc/sys/vm/drop_caches"]
        if self.cfg.dry_run:
            self.log("  [dry-run] " + " ".join(argv))
            return
        run(argv)

    def _read_instrument(self, spec: RunSpec, path: Path) -> dict[str, str]:
        """The instrument build's report, off the file it wrote.

        **A missing or empty file is an error, not an empty report.** The leg
        declared `instrument`, so the report is part of what this rep was
        taken for, and there are exactly three ways it can be absent from a run
        that did not die: the binary was built without the feature, the leg was
        pointed at the default binary, or the write failed and said so on
        stderr. All three are apparatus faults, all three used to read as `{}`,
        and an empty column an hour later is what that looks like.

        A leg the kernel killed never reaches this — the report is written at
        exit, so its absence there is the kill (`KILL_TOLERANT`)."""
        text = path.read_text() if path.exists() else ""
        report = parse_reported(text)
        if not report:
            raise RuntimeError(
                f"{spec.label}: this leg declares the instrument, and no report reached "
                f"{path}. Either the binary was built without `--features introspect`, "
                f"or the leg is pointed at the default binary, or the write failed — "
                f"`pgdt --version` names the instrument when it is there, and a failed "
                f"write says so on stderr."
            )
        return report

    def instrument_report_path(self, spec: RunSpec) -> Path:
        """Where this run's instrument report will land, on the host.

        One file per rep, numbered in the order the sitting took them, under
        the sitting's own output directory. The name carries the reading key so
        a report can be read back by eye without `raw.json`, and the number is
        what keeps a re-take from overwriting the rep it replaced."""
        self._instrument_reports += 1
        slug = re.sub(r"[^A-Za-z0-9]+", "-", spec.key(self.figure_id)).strip("-")
        return (
            self.out_root
            / INSTRUMENT_DIR
            / f"{self._instrument_reports:04d}-{slug}.txt"
        )

    def time_run(self, spec: RunSpec) -> float:
        self._last_killed = False
        self._last_instrument = {}
        dump = self.input_path(spec.input, spec.regime)
        script = _script(spec.command)
        bins: list[tuple[Path, str]] = []
        if spec.binary != "none":
            bins = [(self.binary_path(spec.binary), "/pgdt")]
        # The instrument beside the program where the shape runs under it,
        # staged with it, so neither loads inside the timer.
        if PEAK_RSS in script.split():
            bins.append((ensure_peak_rss_binary(self.cfg, self.log), PEAK_RSS))
        mounts = [
            f"{self.stage_binary(path) if self.arm.staged else path}:{at}:ro"
            for path, at in bins
        ]
        mounts.append(f"{dump}:/dump.sql:ro")
        # A staged leg reads its binaries once before the timer, inside the
        # container and off the path it will run them from.
        preread = (
            f"cat {' '.join(at for _, at in bins)} >/dev/null; "
            if self.arm.staged and bins
            else ""
        )
        cpus = self.leg_cpus(spec)
        if spec.command == "parse-cache-out":
            mounts.append(f"{self.cfg.warm_dir}:/out")
        # The instrument writes to a file rather than to a stream, so the leg
        # needs somewhere to put it and a variable saying where. Neither
        # touches the argv: the command shape a figure records is the shape
        # that ran (`RunSpec.instrument`).
        report_path = None
        env_argv: list[str] = []
        if spec.instrument:
            report_path = self.instrument_report_path(spec)
            if not self.cfg.dry_run:
                report_path.parent.mkdir(parents=True, exist_ok=True)
            mounts.append(f"{report_path.parent}:{INSTRUMENT_MOUNT}")
            env_argv = [
                "-e",
                f"{INSTRUMENT_OUT_VAR}={INSTRUMENT_MOUNT}/{report_path.name}",
            ]
        # The spec's own limit first: a flagless leg's container limit is the
        # axis it is read across, so it overrides the figure's one container.
        memory = spec.memory or self.memory or self.cfg.memory
        argv = [
            *self.cfg.container_argv(),
            "run",
            "--rm",
            "-m",
            memory,
            "--memory-swap",
            memory,
        ]
        if cpus is not None:
            argv += ["--cpuset-cpus", cpu_list(cpus)]
        for m in mounts:
            argv += ["-v", m]
        argv += env_argv
        # The command shape, then the harness's own question about how it
        # ended. `OOM_ORACLE` is appended here rather than written into
        # `_script` so that what a figure records as its shape is exactly what
        # was measured, and so the oracle covers every shape by construction
        # rather than by thirty branches remembering to carry it.
        # The report's format and a staged leg's untimed read go in front of
        # it on the same terms: outside the shape, so neither is recorded as
        # part of what was measured.
        argv += [
            self.cfg.image, "bash", "-c", TIME_FORMAT + preread + script + OOM_ORACLE
        ]
        # The harness beside a pinned leg goes to the other die, and back
        # beside an unpinned one, before anything it launches for this run.
        if self.groups:
            self.place_harness(harness_cpus(self.groups) if cpus else self._harness_home)

        # Whether the cache is dropped is the regime's own declaration, not a
        # prefix on its name: the name and the staging area are two facts, and
        # a regime that read the second off the first would drop caches for
        # anything called `cold-*` however it was staged.
        if regime_spec(spec.regime).drops_caches:
            self.drop_caches()
        if self.cfg.dry_run:
            self.log("  [dry-run] " + " ".join(shlex.quote(a) for a in argv))
            # A deterministic pseudo-reading, so a dry run exercises the median,
            # the spread and every table this figure formats. Never a figure:
            # `--dry-run` marks its output unpublishable like any other
            # apparatus override.
            key = spec.key("dry")
            rep = self._dry_reps.get(key, 0)
            self._dry_reps[key] = rep + 1
            digest = hashlib.sha256(f"{key}/{rep}".encode()).digest()
            self._last_rss = 6000 + digest[1] / 255 * 500 if "rss" in spec.command else None
            # A plausible instrument report, for the same reason the reading is
            # plausible: a dry run must exercise every division a table performs
            # rather than stopping at the first missing key.
            self._last_stdout = (
                {
                    "plaintext": str(3 * GIB),
                    "delivered": str(3 * GIB),
                    "workers": spec.command.rpartition("-")[2],
                    "blocks": str(DECODE_WORKERS[-1] * 8),
                }
                if spec.command.startswith("decode-")
                else {}
            )
            if spec.command.startswith(DYNFILTER_FAMILY):
                # A stand-in answer, one per query and alike in both legs,
                # which the table refuses to render otherwise.
                query = spec.command.rpartition("-")[0]
                self._last_stdout = {
                    "result_rows": "1",
                    "result_first": "100",
                    "result_digest": hashlib.sha256(query.encode()).hexdigest(),
                }
                if spec.command == DYNFILTER_STARTUP:
                    self._last_stdout = {"startup_answer": "1"}
            if spec.command.startswith(f"{PARALLEL_SCAN}-jobs-"):
                # One answer for every provider leg of the figure, which it
                # refuses to render otherwise.
                self._last_stdout = {
                    "result_rows": "1",
                    "result_first": "100",
                    "result_digest": hashlib.sha256(PARALLEL_SCAN_SQL.encode()).hexdigest(),
                }
            if spec.command.startswith(PRUNING_FAMILY):
                # A stand-in of what the query's own notes say, for the same
                # reason: the pruning table divides by them and refuses a
                # pruned leg that skipped nothing — or, for the filter its
                # statistics cannot narrow, one that skipped anything, and a
                # leg over a cache without statistics that consulted any.
                pruned = spec.command.endswith("-all")
                skipped = (
                    0
                    if spec.command.startswith(f"{PRUNING_FAMILY}{PRUNING_UNNARROWED}-")
                    else 3060
                )
                self._last_stdout = {
                    "rows_returned": "3000",
                    **(
                        {
                            "skipped_groups": str(skipped),
                            "groups": "3072",
                            "skipped_bytes": str(skipped * MIB),
                            "bytes": str(3072 * MIB),
                        }
                        if pruned
                        else {}
                    ),
                }
            if spec.memory is not None:
                # A stand-in resolution, for the same reason the reading above
                # is a stand-in: a dry run must exercise every division the
                # flagless table performs, and the count it fits over exists
                # only in what a real run reports. Monotone in the limit so the
                # fit is well conditioned, and deliberately **not** the
                # library's own arithmetic — nothing here may grow into a second
                # authority on what an allocation resolves to.
                rank = 1 + [token for token, _ in RESERVE_LIMITS].index(spec.memory)
                self._last_stdout = {
                    **self._last_stdout,
                    "resolved_jobs": str(3 * rank),
                    "resolved_budget": str(3 * rank * (65 << 20)),
                }
            if spec.instrument:
                # A stand-in report, so a dry run reaches whatever an
                # attribution computes from these rather than stopping at the
                # first missing key. Varied by rep, because they are per-rep
                # readings and a renderer that collapsed a spread would look
                # correct against a constant.
                self._last_instrument = {
                    "instrument": "counting-allocator",
                    "live_scope": "rust-global-alloc",
                    "live_bytes": str(64 << 10),
                    "live_peak_bytes": str(200 * MIB + digest[2] * MIB // 255),
                    "mimalloc_scope": "rust-heap",
                    "mimalloc_committed_bytes": str(96 * MIB),
                    "mimalloc_committed_peak_bytes": str(320 * MIB),
                    "mimalloc_reserved_bytes": str(1024 * MIB),
                    "mimalloc_reserved_peak_bytes": str(1024 * MIB),
                    "glibc_scope": "c-malloc",
                    "mallinfo_arena": str(130 * MIB),
                    "mallinfo_hblkhd": "0",
                    "mallinfo_uordblks": str(66 * MIB),
                    "mallinfo_fordblks": str(64 * MIB),
                    "malloc_heaps": "6",
                    "malloc_system_current": str(130 * MIB),
                    "malloc_system_max": str(350 * MIB),
                }
            return 0.4 + digest[0] / 255 * 5.0
        # The counters bracket the run as tightly as possible: two procfile
        # reads, outside the timer, either side of the subprocess. Their
        # difference is what the machine did *during this rep* -- which is the
        # only window that means anything for a reading half a second long.
        started = time.time()
        before, mono_start = Counters.read(), time.monotonic()
        proc = subprocess.run(argv, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        mono_end, after = time.monotonic(), Counters.read()
        oom = parse_oom_kills(proc.stderr)
        if proc.returncode != 0:
            # What killed it, said in those words: an exit status of 137 is a
            # `SIGKILL`, and only the oracle says whether the reaper sent it.
            if oom is None:
                why = (
                    " — and the container's `memory.events` was unreadable, so whether the "
                    "kernel killed it cannot be told from here"
                )
            elif oom > 0:
                why = f" — OOM-killed ({oom} process(es), by the container's own `memory.events`)"
            else:
                why = " — not an OOM kill: the container's `memory.events` counted none"
            if oom and kill_tolerant(spec.command):
                # A censored reading. Recorded with everything that could still
                # be read off it, so `raw.json` carries the kill and `--render`
                # reproduces the cell; the sitting goes on to the next leg.
                self._last_killed = True
                self._last_rss = None
                self._last_stdout = parse_resolution(proc.stderr)
                # The instrument reports at exit and this process never got
                # there, so its absence is the kill rather than an apparatus
                # fault — the one legitimate absence `RunSpec.instrument`
                # exempts.
                self._last_instrument = {}
                # What the run still said about itself, under names that cannot
                # be read as readings. Both are **lower bounds**: the wrapper
                # reports the peak the process had reached when the kernel
                # reaped it, and the timer the seconds it had run for. Kept
                # because they are evidence of the very thing the constant is
                # being chosen against; named apart because a fit or a headroom
                # column that picked them up would describe a run that stopped.
                def _bound(read: Callable[[str], float]) -> float | None:
                    try:
                        return read(proc.stderr)
                    except ValueError:
                        return None

                self.records.append(
                    {
                        "figure": self.figure_id,
                        "spec": dataclasses.asdict(spec),
                        "seconds": None,
                        "maxrss_kib": None,
                        "killed": True,
                        "maxrss_bound_kib": _bound(parse_maxrss_kib),
                        "seconds_to_kill": _bound(parse_bash_time),
                        "oom_kill": oom,
                        "exit": proc.returncode,
                        **self._arm_record(cpus),
                        "wall_including_container": round(time.time() - started, 3),
                        "telemetry": {},
                        "reported": self._last_stdout,
                        "argv": argv,
                    }
                )
                return 0.0
            raise RuntimeError(
                f"{spec.label} exited {proc.returncode}{why}"
                f"\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}"
            )
        seconds = parse_bash_time(proc.stderr)
        self._last_rss = parse_maxrss_kib(proc.stderr) if "rss" in spec.command else None
        # What the run said about itself on its own streams: the decode
        # example's counts on stdout, and, for a scan, the arrangement its
        # `scan started` line names. One dict, all of it identical across reps.
        # The introspection build's report is **not** here — it is per-rep, and
        # it arrives in a file of its own (`_read_instrument`).
        self._last_stdout = {
            **parse_reported(proc.stdout),
            **parse_resolution(proc.stderr),
            **parse_query_notes(proc.stderr),
        }
        if report_path is not None:
            self._last_instrument = self._read_instrument(spec, report_path)
        telemetry = counter_delta(before, after)
        telemetry.update(self.sampler.window(mono_start, mono_end))
        self.records.append(
            {
                # The figure this reading was taken for. Recorded rather than
                # re-derived, because `--render` splits the records per figure
                # to rebuild each apparatus line, and a spec that two figures
                # share would otherwise be attributed by guess.
                "figure": self.figure_id,
                "spec": dataclasses.asdict(spec),
                "seconds": seconds,
                "maxrss_kib": self._last_rss,
                # Zero where the container answered and nothing was killed;
                # `None` where `memory.events` could not be read, which is a
                # different answer and stays one (`parse_oom_kills`).
                "oom_kill": oom,
                # The CPU the timed command spent, beside its wall clock.
                "cpu_seconds": parse_bash_cpu(proc.stderr),
                **self._arm_record(cpus),
                "wall_including_container": round(time.time() - started, 3),
                "telemetry": telemetry,
                "reported": self._last_stdout,
                # The instrument's own readings, per rep, and the file they
                # came out of — kept in the record as well as in `instrument`
                # so `--render` can rebuild a per-rep column and a reader can
                # find the `malloc_info` XML the summary does not carry.
                "instrument": self._last_instrument,
                "instrument_report": (
                    str(report_path.relative_to(self.out_root)) if report_path else None
                ),
                "argv": argv,
            }
        )
        self.telemetry.append({"key": spec.key(self.figure_id), **telemetry})
        self._last_telemetry = telemetry
        return seconds

    def _arm_record(self, cpus: frozenset[int] | None) -> dict:
        """What a run's record says about the arrangement it was taken under.

        `secondary_arm` marks a reading no table renders, so a reader of
        `runs` — `censored_bounds` — takes only the first arm's."""
        return {
            "arm": self.arm.name,
            "secondary_arm": self.arm != self.arms[0],
            "cpuset": cpu_list(cpus) if cpus is not None else None,
        }

    # -- sweeps -----------------------------------------------------------

    def sweep(self, figure: str, specs: Sequence[RunSpec], reps: int) -> None:
        """`_sweep_one` over `specs` -- or, for a figure split into
        `warm_groups`, over each group's specs in turn, that group staged
        first, so a group's inputs are measured only beside each other."""
        fig = EVERY_BY_ID.get(figure)
        groups = fig.staging_groups if fig else ()
        if len(groups) < 2:
            if named := [s.label or s.command for s in specs if s.sweep is not None]:
                raise ValueError(f"{figure} is one sweep, and {named} name a warm group")
            self._sweep_one(figure, specs, reps)
            return
        for gi, part in split_specs(figure, specs, groups):
            self.stage(figure, gi)
            self._sweep_one(figure, part, reps)

    def _sweep_one(self, figure: str, specs: Sequence[RunSpec], reps: int) -> None:
        """One interleaved sweep: every rep runs every spec in turn, and the
        second half of the reps runs them in the opposite order.

        Both halves matter. Interleaving keeps a session's slow upward drift
        off whichever file went first; reversing keeps the pair's own warming
        off whichever binary went first."""
        keys = [s.key(figure) for s in specs]
        for k in keys:
            self.readings.setdefault(k, [])
        # Where the figure's programs run, and so which glibc it names. A `dd`
        # floor is a reading of `dd` rather than of the program, so it names
        # nothing.
        for spec in specs:
            if spec.binary != "none":
                self.ran_in(figure, self.cfg.image)
        for rep in range(reps):
            order = list(specs) if rep < (reps + 1) // 2 else list(reversed(specs))
            # Each leg under every arm in turn, the arm going first alternating
            # rep by rep, so neither arm is always the warmer one.
            arms = self.arms if rep % 2 == 0 else tuple(reversed(self.arms))
            for spec, arm in ((spec, arm) for spec in order for arm in arms):
                self.arm = arm
                seconds = self.take(spec, rep)
                if len(self.arms) > 1 and not self._last_killed:
                    self.arm_readings.setdefault(arm.name, {}).setdefault(
                        spec.key(figure), []
                    ).append(seconds)
                if arm != self.arms[0]:
                    # A second arm's reading is `arm_readings`' alone: every
                    # table renders the first arm.
                    continue
                if self._last_killed:
                    # Censored: recorded as a kill, never as a number. The RSS
                    # key is opened here so a leg whose every rep was killed
                    # still resolves — an absent key means "this shape carries
                    # no RSS wrapper", which is a different fact.
                    key = spec.key(figure)
                    self.killed[key] = self.killed.get(key, 0) + 1
                    if "rss" in spec.command:
                        self.rss.setdefault(key, [])
                    if spec.instrument:
                        # Opened for the reason the RSS key is: an absent key
                        # means "this leg carries no instrument", which is a
                        # different fact from "every rep of it was killed".
                        self.instrument.setdefault(key, [])
                    if self._last_stdout:
                        self.reported[key] = self._last_stdout
                    continue
                self.readings[spec.key(figure)].append(seconds)
                # Only the accepted reading's RSS is kept: a rep the gate
                # discarded is discarded whole, never half-kept because the
                # quantity it carries is one the machine's load cannot move.
                if self._last_rss is not None:
                    self.rss.setdefault(spec.key(figure), []).append(self._last_rss)
                # The instrument's own report is a property of the *input and
                # the flags*, identical across reps, so the last one stands for
                # all of them rather than accumulating a list of one repeated
                # answer.
                if self._last_stdout:
                    self.reported[spec.key(figure)] = self._last_stdout
                # The introspection build's numbers are the other shape: a
                # reading per rep, kept as a list beside `rss` for the same
                # reason that one is a list. Collapsing them into `reported`
                # would publish one rep's high-water as the leg's.
                if self._last_instrument:
                    self.instrument.setdefault(spec.key(figure), []).append(
                        self._last_instrument
                    )

    def take(self, spec: RunSpec, rep: int) -> float:
        """One reading, retaken while the machine says it was contended.

        A contended reading is **discarded, not corrected** -- see the
        telemetry section's "witness, not divisor". Retaking is the same
        remedy `drop_caches` applies to a dirty page cache, and it is bounded:
        a machine that cannot produce a quiet reading in `GATE_RETRIES`
        attempts is not a machine this figure can be taken on, and saying so
        beats emitting a number nobody can defend.

        Limits are per regime, because a cold reading stalls on I/O by
        construction and a warm one does not -- see `CONTENTION_LIMITS`.
        """
        for attempt in range(1, GATE_RETRIES + 1):
            seconds = self.time_run(spec)
            label = spec.label if len(self.arms) == 1 else f"{spec.label} [{self.arm.name}]"
            if self._last_killed:
                # A killed run is not a timing reading at all, so the
                # contention gate has nothing to judge and retaking it would
                # spend `GATE_RETRIES` runs reproducing the kill.
                self.log(f"  rep{rep + 1} {label}: **OOM-killed** — recorded, censored")
                return seconds
            verdict = contention_verdict(self._last_telemetry, spec.regime)
            if verdict is None:
                self.log(f"  rep{rep + 1} {label}: {seconds:.6f} s")
                return seconds
            self.log(
                f"  rep{rep + 1} {label}: {seconds:.6f} s — DISCARDED, "
                f"machine contended ({verdict}); attempt {attempt}/{GATE_RETRIES}"
            )
        raise RuntimeError(
            f"{spec.label}: {GATE_RETRIES} consecutive readings were taken under contention "
            f"({contention_verdict(self._last_telemetry, spec.regime)}) — "
            "the machine is too busy to measure on"
        )

    def get(self, figure: str, spec: RunSpec) -> list[float]:
        return self.readings[spec.key(figure)]

    def get_rss(self, figure: str, spec: RunSpec) -> list[float]:
        """The peak resident sets, in KiB, of one spec's accepted reps.

        A leg every rep of which was OOM-killed answers `[]`, not a KeyError:
        the key exists because the shape carries the wrapper, and it is empty
        because every reading it took was censored (`kills`)."""
        return self.rss[spec.key(figure)]

    def instrument_reports(self, figure: str, spec: RunSpec) -> list[dict[str, str]]:
        """This spec's instrument reports, one per surviving rep.

        `[]` for a leg every rep of which was OOM-killed, and a `KeyError` for
        a leg that never declared the instrument — the same split `get_rss`
        makes, and for the same reason: a renderer must be able to tell "this
        shape carries no instrument" from "every reading it took was
        censored"."""
        return self.instrument[spec.key(figure)]

    def kills(self, figure: str, spec: RunSpec) -> int:
        """How many of this spec's reps the kernel OOM-killed.

        Zero for every spec outside `KILL_TOLERANT`, which cannot reach this
        state — a kill there raises and the figure is lost, which is the whole
        of what the licence is narrow for."""
        return self.killed.get(spec.key(figure), 0)

    def figure_kills(self, figure: str) -> dict[str, int]:
        """Every killed leg of one figure, by reading key."""
        prefix = f"{figure}/"
        return {k: n for k, n in self.killed.items() if k.startswith(prefix)}

    def censored_bounds(self, figure: str, spec: RunSpec) -> list[tuple[float | None, float | None]]:
        """What each OOM-killed rep of this spec still reported: the peak
        resident set the wrapper had reached when the kernel reaped it, in KiB,
        and the seconds it had run for.

        **Both are lower bounds, and the pair is one rep's** — a renderer that
        maxed the two columns separately would report a peak from one rep and a
        duration from another. They are what `time_run`'s kill branch kept
        (`maxrss_bound_kib`, `seconds_to_kill`), named apart from `rss` and
        `readings` precisely so that no fit or headroom column picks them up.

        Read off `records` rather than off a fourth dict, because that is where
        the kill branch already puts them and `ReplaySession` already loads
        them — so `--render` reproduces a constraint line without the sitting
        having recorded anything new. A leg with no killed rep answers `[]`, and
        so does a sitting taken before the kill branch existed: an empty list
        means *nothing is known*, which is what the caller prints."""
        want = dataclasses.asdict(spec)
        return [
            (r.get("maxrss_bound_kib"), r.get("seconds_to_kill"))
            for r in self.records
            if r.get("killed")
            and not r.get("secondary_arm")
            and r.get("figure") == figure
            and r.get("spec") == want
        ]

    def has(self, figure: str, spec: RunSpec) -> bool:
        return spec.key(figure) in self.readings

    def borrow(self, figure: str, shared: Shared) -> list[RunSpec]:
        """Copy the readings one declared `Shared` names into `figure`'s keys.

        A reading another figure already took. The allocator table's reference
        `parse` row *is* the warm scan-throughput table's `COPY` row --
        re-measuring it would put two different numbers in the doc for one
        measurement.

        Returns the specs that were satisfied, which is empty when the source
        figure was not in this sitting. What is *not* satisfied is left absent
        rather than faked, so the figure's own sweep measures it and the note
        says so.

        **A run's resident set crosses the share with its wall clock.** The
        borrow copied `readings` alone while every figure standing in one
        published a duration; the two resident figures that will share
        `peak-rss`'s runs publish a *peak RSS* off exactly the same reps, and a
        borrow that left `rss` behind would satisfy the spec — `has` reads
        `readings` — and then fail in the renderer with a `KeyError` on a key
        the sitting believes it holds. The key is copied whenever the source has
        one, **empty list included**: an *absent* `rss` key means "this shape
        carries no RSS wrapper" and an empty one means "every rep of it was
        killed", which is the distinction `sweep` opens the key to preserve.

        The other two channels are deliberately not copied. `reported` and
        `instrument` are read by the run function of the figure that *declared*
        them — `RunSpec.instrument` is a declaration of what the harness must
        find, not a property of the run — and no republished spec is such a
        leg, which a test holds. Copying them would hand a borrower a report it
        never asked the harness to look for."""
        got = []
        for spec in shared.republished:
            source_key = spec.key(shared.source)
            readings = self.readings.get(source_key)
            if readings:
                self.readings[spec.key(figure)] = list(readings)
                if (rss := self.rss.get(source_key)) is not None:
                    self.rss[spec.key(figure)] = list(rss)
                got.append(spec)
        return got


def resolve_commit(rev: str) -> str | None:
    """`rev` as a full commit sha, or `None` where this repository has no such
    commit."""
    proc = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}"],
        cwd=str(REPO),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    return proc.stdout.strip() or None


#: Whether this process has already built the shipped binary. Per process for
#: `_ALLOC_BUILT`'s reason, one level up: this is the binary every `pgdt`
#: figure but the allocator's other legs is timed against, and a stale one is
#: what a sitting cannot see.
_PGDT_BUILT = False


def ensure_pgdt_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """The shipped binary, built before the first reading rather than found.

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
    that build writes is `CARGO_RELEASE_BIN`, so a harness that ran it and then
    timed a different file would be asserting a provenance it does not have.
    There the binary is the caller's, and its absence is `main`'s error rather
    than a build.
    """
    global _PGDT_BUILT
    if _PGDT_BUILT or cfg.bin_pgdt != CARGO_RELEASE_BIN:
        return cfg.bin_pgdt
    if cfg.dry_run:
        # A dry run measures nothing and must work where no binary exists, so
        # it announces instead — the same arm `ensure_allocator_binary` has,
        # for the same reason.
        log(f"[dry-run] would build {cfg.bin_pgdt}")
        return cfg.bin_pgdt
    log(f"building {cfg.bin_pgdt}")
    run(["cargo", "build", "--release", "-p", "pgdt"], cwd=REPO)
    _PGDT_BUILT = True
    return cfg.bin_pgdt


#: Whether this process has already built `peak-rss`, for `_PGDT_BUILT`'s
#: reason: the instrument every resident figure reads through is built from the
#: tree being measured, never found.
_PEAK_RSS_BUILT = False


def peak_rss_target() -> str:
    """The Rust target `peak-rss` is built for: musl, on the host's machine."""
    return f"{platform.machine()}-unknown-linux-musl"


def peak_rss_path() -> Path:
    """Where `cargo build --target` writes `peak-rss`: under `target/<target>/`,
    a directory `target/release/pgdt` does not share."""
    return REPO / "target" / peak_rss_target() / "release" / "peak-rss"


def ensure_peak_rss_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """`peak-rss`, the resident-set instrument, built static before the first
    reading and mounted at `PEAK_RSS` beside every program.

    **For `<machine>-unknown-linux-musl`**, so it links nothing of the host's
    libc or the image's and a later pin move cannot break it. Refused: the
    gnu target with `+crt-static`, which needs the host's static glibc. A
    `--target` build of this package alone writes under its own directory and
    unifies no feature into the shipped binary, so `target/release/pgdt` is
    untouched.

    **A missing target is refused with the command that adds it**, rather than
    left to `cargo`'s error about a missing `core`. Once per process, for
    `ensure_pgdt_binary`'s reason."""
    global _PEAK_RSS_BUILT
    out = peak_rss_path()
    if _PEAK_RSS_BUILT:
        return out
    if cfg.dry_run:
        log(f"[dry-run] would build {out}")
        _PEAK_RSS_BUILT = True
        return out
    target = peak_rss_target()
    sysroot = Path(run(["rustc", "--print", "sysroot"], cwd=REPO, capture=True).strip())
    if not (sysroot / "lib" / "rustlib" / target).is_dir():
        raise RuntimeError(
            f"the Rust target {target} is not installed, and `peak-rss` is built for it: "
            f"`rustup target add {target}`"
        )
    log(f"building {out}")
    run(["cargo", "build", "--release", "-p", "peak-rss", "--target", target], cwd=REPO)
    _PEAK_RSS_BUILT = True
    return out


#: What `apparatus_preflight` quotes of a refused start: the tail of its
#: stderr, which is where the loader names the symbol version it lacks.
PREFLIGHT_STDERR_LINES = 3


def apparatus_preflight(cfg: Config, log: Callable[[str], None]) -> list[str]:
    """The two binaries every figure mounts, started in the pinned image before
    the first reading, one problem line for each that does not start.

    **`pgdt --version` must start there.** The image is of the build host's
    distribution so that a host-built `pgdt` links no symbol version it lacks,
    but the host's glibc moves at every upgrade and the pin does not. A binary
    the image cannot load fails every leg, so it is refused here, naming the
    pin, and the pin moves then and only then, the next stamp naming its glibc.
    Refused: refusing on any difference between the host's glibc and the
    image's, which would make every host upgrade an apparatus change.

    **`peak-rss` around `/bin/true`** proves it starts there, and its reading
    is the floor every resident reading stands on — the spawning side's share
    it bounds (`peak-rss/src/main.rs`) — logged, never published."""
    if cfg.dry_run:
        ensure_peak_rss_binary(cfg, log)
        return []
    try:
        peak = ensure_peak_rss_binary(cfg, log)
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        return [f"`peak-rss` could not be built: {exc}"]
    base = [
        *cfg.container_argv(), "run", "--rm",
        "-v", f"{cfg.bin_pgdt}:/pgdt:ro",
        "-v", f"{peak}:{PEAK_RSS}:ro",
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
            f"(exit {pgdt.returncode}: {tail(pgdt)}): the host has outrun the pin, so move "
            "`Config.image` (PGDT_MEASURE_IMAGE) to a digest of the build host's distribution "
            "that loads it, and the next stamp names its glibc"
        )
    floor = start([PEAK_RSS, "/bin/true"])
    readings = MAXRSS_RE.findall(floor.stderr)
    if floor.returncode != 0 or len(readings) != 1:
        problems.append(
            f"`peak-rss` around /bin/true does not report one reading in {cfg.image} "
            f"(exit {floor.returncode}: {tail(floor)})"
        )
    else:
        log(
            f"peak-rss floor: {fmt_mib(int(readings[0]))} around /bin/true in the pinned "
            "image — what a resident reading stands on, never published"
        )
    return problems


#: Whether this process has already built the `xz_decode` instrument. Per
#: process rather than per file, for the reason the allocator legs are: a
#: binary left in `runs/` by an earlier session was built from whatever the
#: source said then, and this figure's whole content is that decoder's rate.
_XZ_DECODE_BUILT = False


def ensure_xz_decode_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """`pgdump_query`'s `xz_decode` example, built and copied beside the other
    measurement binaries.

    Mechanical, so the harness does it rather than asking for a binary: a
    `cargo build` of a committed target, where a source edit is one no harness
    should perform.

    **An example target, so `target/release/pgdt` is untouched.** Every other
    figure in a sweep is timed against that binary, and a build that replaced
    it would re-time all of them against something else — the failure the
    allocator legs' separate target directories exist to prevent, one target
    kind along.
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
    run(
        ["cargo", "build", "--release", "-p", "pgdump_query", "--example", "xz_decode"],
        cwd=REPO,
    )
    shutil.copyfile(REPO / "target/release/examples/xz_decode", out)
    out.chmod(0o755)
    _XZ_DECODE_BUILT = True
    return out


#: The three legs of the `allocator` figure, each a `pgdt` feature, **the
#: default build's first** -- `pgdt/Cargo.toml`'s `default`, which a test holds
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
#: (`pgdt/src/introspect.rs`). Such a build takes an atomic on every allocation
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
    `target/release/pgdt` becomes a different binary and every apparatus line
    that still names the old allocator is wrong with nothing to notice. This is
    what the session stamp reports.

    **An instrumented build is refused here rather than named**, because every
    caller of this function is about to publish something about the binary —
    the session stamp, or an `allocator` leg's label. A counting allocator
    would be timed as `system` and read as the shipped binary, which is the
    same family of failure as profiling the `release` build and calling it
    `profiling`, and the refusal is what makes "not a fourth `ALLOCATOR_LEGS`
    member" mechanical instead of intentional."""
    out = run([str(binary), "--version"], capture=True)
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


#: Where a figure's program runs when it runs in no container: `cargo bench`,
#: which is `nested-decode-micro`. Every other place is an image reference.
HOST = "host"

#: What glibc's `getconf GNU_LIBC_VERSION` prints. musl's `getconf` has no such
#: variable and exits non-zero, which is the refusal `glibc_of` wants.
LIBC_RE = re.compile(r"^glibc (\d+(?:\.\d+)+)$", re.MULTILINE)


def glibc_of(cfg: Config, where: str) -> str:
    """The glibc a program run in `where` — an image, or `HOST` — runs under,
    asked of that place rather than assumed from a tag.

    The allocator's argument one layer down (`binary_allocator`): a tag moves
    under the register and says nothing about the libc it holds, so the stamp
    and a figure's marker name what the image answers. **A place answering
    with no glibc is refused**, since the apparatus is a glibc one and the
    figure would otherwise publish under a libc nothing named."""
    argv = ["getconf", "GNU_LIBC_VERSION"]
    if where != HOST:
        argv = [*cfg.container_argv(), "run", "--rm", where, *argv]
    try:
        out = run(argv, capture=True)
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
    * **Its own target dir**, so `target/release/pgdt` -- every other figure's
      binary -- is never overwritten by a `--features` build.
    * **`--version` is read back** and must name this leg.

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
    target = cfg.alloc_build_root / leg
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
    target.mkdir(parents=True, exist_ok=True)
    run(
        [
            "cargo", "build", "--release", "-p", "pgdt",
            "--no-default-features", "--features", leg,
            "--target-dir", str(target),
        ],
        cwd=REPO,
    )
    shutil.copyfile(target / "release/pgdt", out)
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
    out = run([str(binary), "--version"], capture=True)
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

    **Its own target dir**, because a `--features` build in the default one
    overwrites `target/release/pgdt` — every other figure's binary — with a
    binary that takes an atomic on every allocation and that
    `binary_allocator` then refuses, which is a sitting lost to a build step.

    **`--version` is read back**, and it must name the instrument. A leg
    declaring `RunSpec.instrument` and pointed at a build without the feature
    produces no report, and the run that discovers it is the first rep of the
    hour rather than the second before it.
    """
    global _INSTRUMENT_BUILT
    out = cfg.out_dir / "pgdt-introspect"
    if _INSTRUMENT_BUILT:
        return out
    target = cfg.alloc_build_root / "introspect"
    if cfg.dry_run:
        log(f"  [dry-run] would build the introspection instrument into {target}")
        return out
    log(f"  building the introspection instrument into {target}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    target.mkdir(parents=True, exist_ok=True)
    run(
        [
            "cargo", "build", "--release", "-p", "pgdt",
            "--features", "introspect",
            "--target-dir", str(target),
        ],
        cwd=REPO,
    )
    shutil.copyfile(target / "release/pgdt", out)
    out.chmod(0o755)
    _INSTRUMENT_BUILT = True
    log(f"  {out.name}: {binary_instrument(out)}")
    return out


def count_saves(
    cfg: Config, binary: Path, dump: Path, log: Callable[[str], None]
) -> tuple[int, int]:
    """Cache writes during one `parse`, from `strace`.

    Traced on the host and untimed, so strace's overhead reaches no figure.
    **Both** `open` and `openat`: glibc uses one and musl the other, and
    tracing a single call silently reports zero saves against the other libc.
    A save opens once, the file it writes beside the cache and renames over it,
    whose path begins with the cache's, so the filter below counts it; the first
    open is the load's miss.

    Untimed, but the save count it returns is published, and `-f` follows every
    thread — so it states its worker count like everything else here."""
    cache = cfg.warm_dir / "savecount.dtcache"
    cache.unlink(missing_ok=True)
    if cfg.dry_run:
        return (0, 0)
    proc = subprocess.run(
        [
            "strace", "-f", "-e", "trace=open,openat",
            str(binary), "parse", "--source", str(dump), "--dtcache", str(cache),
            "--jobs", str(SWEEP_JOBS), *NO_STATISTICS.split(),
        ],
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


# --------------------------------------------------------------------------
# Figures. One figure is exactly one table.
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Shared:
    """A reading this figure takes from another figure rather than measuring.

    Declared here rather than left in a `session.borrow` call site, because a
    call site is not a graph: the harness could not compute what re-taking a
    figure drags, so `--figure` could not name it and every run function
    assembled its provenance paragraph by hand -- naming its own direct
    sources, which is short of the honest set whenever a source is itself
    borrowed from.

    `republished` are the runs whose *number* appears in this figure's table as
    well as in the source's. That is the relation the closure is computed over,
    because it is the one that puts two numbers in the doc for one measurement
    when only half the pair is re-taken. It is empty where the readings are
    consumed without being republished -- `cross-file-floor` differences
    `nested-end-to-end`'s reps into a per-row cost, which is a derived quantity
    and not that figure's number a second time -- and such an entry still
    orders the run and still pulls its source into a selection.

    *Rejected: making derivation an edge too.* It would close the one hazard
    this line leaves -- re-taking `nested-end-to-end` alone leaves
    `cross-file-floor`'s row 1 a difference over reps that no longer exist
    anywhere -- but at the cost of dragging a fifth figure into every
    `allocator` sitting and falsifying the four-figure closure
    `measurements.md` publishes. The hazard is closed by *naming* the reverse
    direction rather than taking it, which is the distinction `--figure`
    already draws: `derivation_gaps` says it before the first reading and in
    the emitted header, and `--check` reports it beside the partial sittings.
    Selection is directional and reads every share, so the forward direction --
    `--figure cross-file-floor` pulling its source in -- was never at risk.
    """

    source: str
    #: What this figure's table calls the borrowed reading, in its own terms.
    #: The generated note is only as legible as this phrase, since it is what a
    #: reader deciding whether to fold the table in actually reads.
    what: str
    republished: tuple[RunSpec, ...] = ()


@dataclass(frozen=True)
class Subtraction:
    """A reading this figure takes across two inputs of `source`'s reps.

    Declared because which inputs share a sweep is decided by it: a difference
    or a ratio between two inputs' readings, rep by rep or median against
    median, holds only where both were taken in one sweep, a session's own
    level shift being as large as what such a difference resolves
    (`measurements.md`, "Re-take a comparison table whole"). Where it is read
    is no matter -- a table row, a sentence or heading of the figure's
    section, or another figure consuming `source`'s reps -- and the reader
    declares it. `test_measure.py` holds each to one of `source`'s warm
    groups, and `_per_row_diffs` refuses a pair nothing declares. A reading
    normalised within its own sweep -- a shape against its own floor, a leg
    against its own one-worker row -- is set beside another sweep's, never
    subtracted from it, and is not one."""

    source: str
    #: The pair, unordered: which side is subtracted is the reader's.
    inputs: tuple[str, str]
    #: What reads it, in the section's own terms.
    what: str


@dataclass
class Figure:
    id: str
    #: A human label for the measurements.md section this table belongs under.
    #: **Not an address**: the doc addresses a figure by its `<!-- figure: id -->`
    #: marker, so a heading may quote a number and may be rewritten when the
    #: number moves without losing its table. The label is what a sitting
    #: renders as that heading, so `--check` holds it to begin with the heading
    #: its marker sits under (`section_label_problems`): a heading rewritten
    #: takes its label with it. Not read from the heading instead: `--list`,
    #: the sweep log and a sitting's grouping print it without the doc.
    section: str
    #: The regimes this figure is taken in, `+`-separated — every token a key
    #: of `REGIMES`, or one of `NON_REGIME_STAGES` for a figure that reads no
    #: staged input. `--stage` selects on it, and `registered_regimes` is what
    #: the harness's regime vocabulary is reconciled against, so a token here
    #: that nothing declares is an error rather than a silent fourth regime.
    stage: str
    #: Repo-relative paths whose change invalidates this figure. `--stale`
    #: intersects these with a diff. A figure that cannot say what invalidates
    #: it is one nobody has thought about.
    depends: tuple[str, ...]
    #: Caption for this figure's table, when its section holds more than one.
    #: The cold and warm throughput tables share a section on purpose: the
    #: `INSERT` path's per-byte CPU is then a division within one place rather
    #: than across two regimes, which is the defect that allocated the warm
    #: table in the first place.
    table_label: str = ""
    cold_inputs: tuple[str, ...] = ()
    warm_inputs: tuple[str, ...] = ()
    #: A figure whose warm set outgrows `warm_bound`, split: each group is
    #: staged and swept on its own, in this order, so `Session.sweep` runs a
    #: spec only beside the specs over its own group. Inputs share a group
    #: wherever a declared `Subtraction` pairs them, and an input two groups'
    #: subtractions both need is measured in each (`RunSpec.sweep`). Empty is
    #: one sweep over `warm_inputs`.
    warm_groups: tuple[tuple[str, ...], ...] = ()
    #: Inputs this figure reads cold off the NVMe. A third list rather than a
    #: flag on `cold_inputs`, because the two name different *devices* and a
    #: figure is free to want both -- which is what the throughput section
    #: does, one table per regime over the same three files.
    nvme_inputs: tuple[str, ...] = ()
    #: Readings this figure takes from another rather than measuring them.
    #: The borrow graph, declared: `requires` and `sharing_closure` are both
    #: read off it, and `share_readings` writes the table's provenance
    #: paragraph from it.
    shares: tuple[Shared, ...] = ()
    #: The cross-input readings this figure reads, its own reps' or another
    #: figure's.
    subtracts: tuple[Subtraction, ...] = ()
    #: The container memory limit this figure's runs are given, where the
    #: recorded 512 MB is not what it needs. It is an **apparatus** departure,
    #: so a figure that sets it says so in its own table: the register's one
    #: line is "3.00 GiB inputs read by a `glibc` binary in a 512 MB
    #: container", and a figure holding N decoded 24 MiB blocks
    #: at once cannot be one of them at 24 workers.
    memory: str | None = None
    #: Consumers that repeat this figure's numbers **without naming it** — the
    #: residue the scan cannot find. `depends` is the edge into a figure; the
    #: edge out is `consumers()`, computed from who spells the figure's id, and
    #: declaring it beside each figure is what left every tuple pointing at a
    #: document a retarget had already replaced. What stays here is the manual
    #: and the README: they state the claim to a reader who will never see a
    #: figure id, so nothing in their text can be matched against one.
    also_quoted_by: tuple[str, ...] = ()
    run: Callable[[Session], str] = field(default=lambda s: "")

    @property
    def requires(self) -> tuple[str, ...]:
        """The figures whose readings this one uses, pulled in automatically.

        Derived from `shares` rather than declared beside it: two lists of the
        same fact drift, and the one that drifts is the one no run function
        reads."""
        return tuple(dict.fromkeys(s.source for s in self.shares))

    @property
    def staging_groups(self) -> tuple[tuple[str, ...], ...]:
        """The warm sets this figure stages one at a time: its `warm_groups`,
        or its whole warm set as one, or none."""
        if self.warm_groups:
            return self.warm_groups
        return (self.warm_inputs,) if self.warm_inputs else ()

    @property
    def sweeps(self) -> tuple[tuple[str, ...], ...]:
        """The input sets this figure measures together: its `warm_groups`, or
        every input it reads as one sweep."""
        if self.warm_groups:
            return self.warm_groups
        return (tuple(dict.fromkeys((*self.warm_inputs, *self.cold_inputs, *self.nvme_inputs))),)


def subtraction_sweep(source: str, a: str, b: str) -> int:
    """The sweep of `source` in which a reading across `a` and `b` is taken:
    the one group holding both, for a pair some figure declares.

    **An undeclared pair is refused**, and so is a declared one no single
    group holds: either is a difference whose two sides may come from two
    sweeps, which is a session's drift published as a between-file one."""
    pair = {a, b}
    if not any(
        s.source == source and set(s.inputs) == pair
        for fig in EVERY_FIGURE
        for s in fig.subtracts
    ):
        raise ValueError(
            f"no figure declares a subtraction of {a!r} and {b!r} over {source}'s reps "
            "(`Figure.subtracts`)"
        )
    homes = [gi for gi, group in enumerate(EVERY_BY_ID[source].sweeps) if pair <= set(group)]
    if len(homes) != 1:
        raise ValueError(
            f"{source} measures {a!r} and {b!r} together in {len(homes)} sweeps, not one"
        )
    return homes[0]


#: The paths behind each mechanism a figure can depend on. Declared narrowly
#: on purpose: a blanket `pgdump_query/src/` would mark every figure stale on
#: every commit, which is a `--stale` nobody reads. The cost of narrowness is
#: that a change *outside* these paths that moves a figure goes unannounced —
#: which is why the doc carries a session stamp as well, so "are these figures
#: from before or after my change" has a second answer.
SCAN = (
    "pgdump_query/src/scan.rs",
    "pgdump_query/src/copy.rs",
    "pgdump_query/src/lex.rs",
    "pgdump_query/src/stream.rs",
)
#: Every figure that times a `pgdt` run over a file reads its bytes through
#: this one module, whatever else the figure is about, so it is its own
#: mechanism rather than part of `SCAN`: `nested-end-to-end` and
#: `cross-file-floor` declare no scanner path and are still moved by it.
#: The read path was undeclared until the buffer pool landed, which made a
#: change to the largest single term in a warm `parse`'s user time read green
#: against every table it moved.
READ = ("pgdump_query/src/io.rs",)
MAP = ("pgdump_query/src/map.rs",)
#: The map's own per-block cost is two files, not one: `map::Builder::snapshot`
#: clones the span list and `stream::splice` rebuilds the index from it, under
#: the save throttle's gate. A figure that prices the map declares both, or a
#: change to the gate reads green against a table it just moved.
MAP_BUILD = (*MAP, "pgdump_query/src/stream.rs")
#: Where a filter term is resolved and evaluated. Its own mechanism because
#: until `predicate-terms` no registered figure passed a filter at all, so
#: nothing in the register was moved by this file and nothing declared it --
#: which is the same shape of blindness the read path had before `READ`.
PREDICATE = ("pgdump_query/src/predicate.rs",)
#: Per-type scalar decode and render-back. Its own mechanism rather than part
#: of `NESTED`, because the figures it moves are not the ones `NESTED` moves:
#: every figure that runs a *typed* query pays it on every scalar column,
#: including `projection-widths`, whose narrow rows have no nested column in
#: them at all. Undeclared until the scalar decoders lost their per-field
#: allocations, which is the third instance of the blindness `READ` and
#: `PREDICATE` record: a file nothing declared because no slice had yet
#: touched it.
DECODE = ("pgdump_query/src/decode.rs",)
CACHE = ("pgdump_query/src/cache.rs",)
#: The unrepresentable count, which every data-level mapping pass takes beside
#: the census (`docs/design/decisions.md`, "D96").
UNREPRESENTABLE = ("pgdump_query/src/unrepresentable.rs",)
NESTED = ("pgdump_query/src/nested.rs", "pgdump_query/src/batch.rs")
PREAMBLE = ("pgdump_query/src/index.rs", "pgdump_query/src/preamble.rs")
#: A query figure also reads through the CLI's own row rendering. The whole
#: `src/` directory rather than `main.rs`: a declared path is matched by
#: prefix, so naming the one file leaves every other module in that crate as a
#: staleness edge nobody declared -- and the crate now has three
#: (`main.rs`, `where_expr.rs`, `alloc.rs`).
QUERY_CLI = ("pgdt/src/",)

#: The resident-set instrument every resident figure reads through
#: (`PEAK_RSS`): a figure running a shape under it declares it, since a change
#: to what it reads moves every such reading.
RSS_INSTRUMENT = ("peak-rss/src/",)

GEN_PERF = ("scripts/generate_perf_data.py",)
GEN_BLOCKS = ("scripts/generate_block_count_bench.py",)
GEN_SHAPES = (
    "scripts/generate_perf_data.py",
    "scripts/generate_large_object_bench.py",
    "scripts/generate_insert_run_bench.py",
)
#: The pruning input's generator, and the perf generator whose rows it writes.
GEN_PRUNING = ("scripts/generate_pruning_bench.py", *GEN_PERF)
#: The dynamic-filter input's generator, and the perf generator likewise.
GEN_DYNFILTER = ("scripts/generate_dynamic_filter_bench.py", *GEN_PERF)
#: `pgdt sql`: the provider it reads the dump through, and DataFusion's CLI
#: around it, the library `pgdt` calls.
DATAFUSION = ("datafusion-pgdump/src/", "datafusion-cli-pgdump/src/")
#: Where statistics are gathered, stored and read back. `gather.rs` and
#: `statistics.rs` are the gathering; `pgtype.rs`, `resolve.rs` and
#: `predicate.rs` are the comparison a bound is keyed and ordered under.
STATISTICS = (
    "pgdump_query/src/gather.rs",
    "pgdump_query/src/statistics.rs",
    "pgdump_query/src/pgtype.rs",
    "pgdump_query/src/resolve.rs",
    *PREDICATE,
)
#: What a query figure's reading carries beyond its rows, its query timed over
#: a data-level cache (`DATA_LEVEL_QUERIES`): the cache, decoded whole inside
#: the timer, and the census and statistics the untimed builder left in it.
#: Merged into a figure's own paths with `_declare`, which drops a repeat.
CACHED_QUERY = (*MAP, *CACHE, *STATISTICS)


def _declare(*paths: str) -> tuple[str, ...]:
    """`paths` in order, each once: a path declared twice is printed twice by
    `--list`."""
    return tuple(dict.fromkeys(paths))


# -- scan throughput --------------------------------------------------------

#: The three shapes, then **each one's own `dd` floor**: a shape over its own
#: sweep's floor of its own bytes cancels that sweep's level shift, which is
#: what lets the warm table's rows -- a sweep per input (`Figure.warm_groups`)
#: -- and the three regimes' tables be set beside each other as ratios, and on
#: a cold device it takes the file's own layout out of the ratio. *Rejected:*
#: control's floor alone for the unsplit cold and NVMe tables, which makes
#: "Against the floor" mean a different thing per regime to save two rows.
_THROUGHPUT_SHAPES = (
    ("control", "`COPY` block"),
    ("large_object", "Large-object region"),
    ("insert_run", "`INSERT` run"),
)
_THROUGHPUT_ROWS = (
    *((inp, "parse", label) for inp, label in _THROUGHPUT_SHAPES),
    *((inp, "dd", f"`dd` → `/dev/null`, {label}") for inp, label in _THROUGHPUT_SHAPES),
)


def _throughput_specs(regime: str) -> list[RunSpec]:
    return [
        RunSpec("none" if cmd == "dd" else "pgdt", inp, cmd, regime, f"{label} ({regime})")
        for inp, cmd, label in _THROUGHPUT_ROWS
    ]


def _throughput_table(session: Session, figure: str, specs: Sequence[RunSpec]) -> str:
    floors = {
        spec.input: median(session.get(figure, spec)) for spec in specs if spec.command == "dd"
    }
    rows = []
    for spec, (_, _, label) in zip(specs, _THROUGHPUT_ROWS):
        values = session.get(figure, spec)
        size = session.input_size(spec.input, spec.regime)
        against = (
            "—"
            if spec.command == "dd"
            else f"{median(values) / floors[spec.input]:.2f}× its floor's time"
        )
        rows.append(
            [label, fmt_median_spread(values), fmt_rate(size, median(values)), against]
        )
    return md_table(["Input", "Wall", "Rate", "Against the floor"], rows)


def _throughput_figure(session: Session, figure: str, regime: str, reps: int) -> str:
    """The throughput table, in one regime: three shapes and their floors.

    Its warm `COPY` row is also the allocator table's reference `parse` row --
    same binary, same command, same input -- which borrows it rather than
    measuring it twice, since two numbers in the doc for one measurement is the
    defect the whole sweep exists to remove."""
    specs = _throughput_specs(regime)
    note = share_readings(session, figure)
    to_run = [s for s in specs if s.key(figure) not in session.readings]
    session.sweep(figure, to_run, reps)
    return (
        _throughput_table(session, figure, specs)
        + "\n"
        + note
        + "\n"
        + _per_rep(figure, session, specs)
    )


def run_scan_throughput_cold(session: Session) -> str:
    return _throughput_figure(session, "scan-throughput-cold", "cold", session.cfg.reps(3))


def run_scan_throughput_warm(session: Session) -> str:
    return _throughput_figure(session, "scan-throughput-warm", "warm", session.cfg.reps(3))


def run_scan_throughput_nvme(session: Session) -> str:
    """The same four rows, cold on the NVMe.

    **Five reps rather than the other two tables' three.** What this table is
    read for is a *ratio* against its own floor, at a device fast enough that
    the ratio is near 1 and a few percent of it decides three levers; and each
    reading here is a third of a cold SSD one, so the reps are affordable where
    they would not be there. The comparison the doc makes across the three
    tables is the ratio, never one table's absolute against another's, so the
    differing rep count costs nothing it relies on."""
    return _throughput_figure(session, "scan-throughput-nvme", "cold-nvme", session.cfg.reps(5))


# -- the read chunk size ----------------------------------------------------

#: The three regimes the chunk size is swept in, in the table's column order,
#: with the caption each column carries.
#:
#: **All three, not the NVMe alone.** The NVMe is the only class on which a
#: chunk size *can* win — the other two hide everything behind the device — but
#: the default that ships is the one that is worst-case-best across the
#: classes, so the two that cannot win are exactly the ones that say what
#: changing it would cost. Warm is here for the third question the other two
#: cannot answer: what a chunk size costs in CPU, where no device hides it.
CHUNK_REGIMES: tuple[tuple[str, str], ...] = (
    ("warm", "Warm, tmpfs"),
    ("cold", "Cold, SATA SSD"),
    ("cold-nvme", "Cold, NVMe"),
)


def _chunk_specs() -> list[RunSpec]:
    return [
        RunSpec(
            "pgdt",
            "control",
            f"parse-chunk-{size}",
            regime,
            f"{fmt_chunk(size)} ({regime})",
        )
        for regime, _ in CHUNK_REGIMES
        for size in CHUNK_SIZES
    ]


def run_chunk_size(session: Session) -> str:
    """One `parse` of the control file at six chunk sizes, in three regimes.

    **Nine reps.** The cold-NVMe throughput table's own `COPY` row spreads
    1.285–1.499 s over five — about 15% of its median, which is nearly twice
    the whole 8.9% envelope this figure's decision lives inside
    (`measurements.md`, "Scan throughput by input shape", whose NVMe table
    carries that arithmetic). Five reps cannot resolve a lever that small; nine is what
    makes a flat table mean *flat* rather than *unresolved*, and the per-rep
    listing beneath is what lets a reader check that for themselves.

    **Every row is one file, one command and one binary**, differing only in
    the number the flag carries — which is the within-file attribution this
    campaign's rules ask for, and the reason this is a sweep over a flag
    rather than over six builds."""
    specs = _chunk_specs()
    session.sweep("chunk-size", specs, session.cfg.reps(9))

    by_regime = {regime: {} for regime, _ in CHUNK_REGIMES}
    for spec in specs:
        size = int(spec.command.rpartition("-")[2])
        by_regime[spec.regime][size] = median(session.get("chunk-size", spec))

    rows = []
    for size in CHUNK_SIZES:
        cells = [fmt_chunk(size) + (" *(default)*" if size == CHUNK_DEFAULT else "")]
        for regime, _ in CHUNK_REGIMES:
            got = by_regime[regime][size]
            ratio = got / by_regime[regime][CHUNK_DEFAULT]
            spec = next(
                s for s in specs if s.regime == regime and s.command.endswith(f"-{size}")
            )
            cells.append(f"{fmt_median_spread(session.get('chunk-size', spec))} · {ratio:.2f}×")
        rows.append(cells)
    table = md_table(["Chunk", *(label for _, label in CHUNK_REGIMES)], rows)
    return table + "\n" + _per_rep("chunk-size", session, specs)


# -- nested end to end ------------------------------------------------------


def _cached_query_note(subject: str) -> str:
    """What a query figure's notes say of the cache `subject` reads, generated
    from the shape so the sentence cannot outlive it (`DATA_LEVEL_QUERIES`)."""
    return (
        f"{subject} reads a cache one untimed `pgdt parse` stating `{GATHER_STATISTICS}` "
        "wrote in the same container, and states `--statistics none`, so no row group is "
        "skipped: its reading carries decoding that cache whole, statistics included, and "
        "no mapping pass.\n"
    )


_NESTED_FILES = {
    "control": "control — 16 scalar columns",
    "composite": "`--composite` — the same 16 plus one composite",
    "arrays": "`--arrays --composite` — the same 16 plus three nested",
}

#: The figure's two sweeps: each nested file with a control of its own, since
#: two of them and a control do not fit `warm_bound` and every reading across
#: files is against control (`Figure.subtracts`). The first sweep's control is
#: the one `cross-file-floor` and the allocator table read.
_NESTED_SWEEPS: tuple[tuple[str, str], ...] = (("control", "composite"), ("control", "arrays"))


def _nested_specs() -> list[RunSpec]:
    specs = []
    for gi, (_, nested) in enumerate(_NESTED_SWEEPS):
        for name in _NESTED_SWEEPS[gi]:
            beside = f" beside {nested}" if name == "control" else ""
            for mode in ("strings", "typed"):
                spec = RunSpec("pgdt", name, f"query-{mode}", "warm", f"{name} {mode}{beside}")
                specs.append(in_sweep("nested-end-to-end", spec, gi))
    return specs


def run_nested_end_to_end(session: Session) -> str:
    figure = "nested-end-to-end"
    session.sweep(figure, _nested_specs(), session.cfg.reps(5))
    rows, per_rep = [], []
    for gi, (_, nested) in enumerate(_NESTED_SWEEPS):
        for name in _NESTED_SWEEPS[gi]:
            label = _NESTED_FILES[name]
            flag = _NESTED_FILES[nested].partition(" — ")[0]
            beside = f", in {flag}'s sweep" if name == "control" else ""
            sv, tv = (
                session.get(figure, in_sweep(figure, RunSpec("pgdt", name, cmd, "warm", ""), gi))
                for cmd in ("query-strings", "query-typed")
            )
            profile = session.stager.profile(name)
            diff = median(tv) - median(sv)
            rows.append(
                [
                    label + beside,
                    f"{profile['rows']:,}",
                    f"{fmt_s(median(sv))} s",
                    f"{fmt_s(median(tv))} s",
                    f"**{diff / profile['rows'] * 1e6:.2f} µs/row**",
                    f"{median(tv) / median(sv):.2f}×",
                ]
            )
            per_rep.append(
                f"- {name}{beside} — `strings`: {fmt_readings(sv)}; `typed`: {fmt_readings(tv)}"
            )
    table = md_table(
        ["File", "Rows", "`strings`", "`typed`", "`typed` − `strings`", "Ratio"], rows
    )
    headline = _per_row_diffs(session, figure, "control", "arrays")
    baselines = []
    for _, nested in _NESTED_SWEEPS:
        control, other = (
            session.get(figure, spec)
            for spec in _across(figure, "control", nested, "query-strings")
        )
        flag = _NESTED_FILES[nested].partition(" — ")[0]
        baselines.append(f"{flag} {median(other) / median(control):.3f}×")
    return (
        table
        + "\n\n**The three nested columns' cost**, the arrays file's `typed` − `strings` less "
        f"its own sweep's control's, paired rep by rep: **{median(headline):+.2f} µs/row** "
        f"({', '.join(f'{v:+.2f}' for v in sorted(headline))}).\n\n"
        "Each nested file's `strings` leg against its own sweep's control's: "
        + ", ".join(baselines)
        + ".\n\n"
        + _cached_query_note("Each `query`")
        + "\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- the cross-file floor ---------------------------------------------------


def _across(figure: str, a: str, b: str, command: str) -> tuple[RunSpec, RunSpec]:
    """`a`'s and `b`'s runs of `command` from the one sweep of `figure` a
    declared subtraction pairs them in (`subtraction_sweep`), which refuses a
    pair nothing declares."""
    gi = subtraction_sweep(figure, a, b)
    spec_a, spec_b = (
        in_sweep(figure, RunSpec("pgdt", name, command, "warm", ""), gi) for name in (a, b)
    )
    return spec_a, spec_b


def _per_row_diffs(session: Session, figure: str, a: str, b: str) -> list[float]:
    """Per-rep µs/row differences between two files' own typed−strings costs,
    from the one sweep that declares them a pair.

    Each file's typed leg is differenced against *its own* strings leg first,
    which is what makes the subtraction legitimate: whatever the untyped
    baseline is worth on a given file cancels out of that file's own
    difference, and would not cancel out of a cross-file ratio."""
    strings, typed = (_across(figure, a, b, cmd) for cmd in ("query-strings", "query-typed"))
    out = []
    for s_spec, t_spec in zip(strings, typed):
        profile = session.stager.profile(s_spec.input)
        s, t = session.get(figure, s_spec), session.get(figure, t_spec)
        out.append([(tv - sv) / profile["rows"] * 1e6 for sv, tv in zip(s, t)])
    reps = min(len(out[0]), len(out[1]))
    return [out[1][i] - out[0][i] for i in range(reps)]


def run_cross_file_floor(session: Session) -> str:
    figure = "cross-file-floor"
    # Row 1 is read off the nested figure's first sweep's own reps -- the same
    # five runs, not a second pass over the same two files.
    share = _per_row_diffs(session, "nested-end-to-end", "control", "composite")
    # Row 2 is this figure's own: two files that differ only in their seed.
    specs = []
    for name in ("control", "control43"):
        for mode in ("strings", "typed"):
            specs.append(RunSpec("pgdt", name, f"query-{mode}", "warm", f"{name} {mode}"))
    session.sweep(figure, specs, session.cfg.reps(6))
    floor = _per_row_diffs(session, figure, "control", "control43")
    rows = [
        [
            "composite column's share — control against `--composite`",
            str(len(share)),
            f"**{median(share):+.2f} µs**",
            ", ".join(f"{v:+.2f}" for v in sorted(share)),
        ],
        [
            "**the instrument's own floor** — control against a second control (`--seed 43`, same 16 columns)",
            str(len(floor)),
            f"**{median(floor):+.2f} µs**",
            ", ".join(f"{v:+.2f}" for v in sorted(floor)),
        ],
    ]
    table = md_table(["Reading", "Reps", "Paired median", "Per-rep readings"], rows)
    return table + "\n\n" + _cached_query_note("Each `query`")


# -- projection widths ------------------------------------------------------

#: The rows, in width order, each labelled by what its difference against the
#: row above buys. Ascending because every row's column list contains the one
#: above it: the difference is then the columns they differ by and nothing
#: else, over identical rows of one file.
_PROJECTION_ROWS: tuple[tuple[int, str, str], ...] = (
    (0, "0 — `--no-columns`", "the replay floor: no decode, no build, no render"),
    (1, "1 — `v_smallint`", "one cheap scalar, above the floor"),
    (16, "16 — every scalar", "the other 15 scalars"),
    (17, "17 — the scalars and `v_comp`", "**the composite column alone**"),
    (19, "19 — every column", "**the two array columns alone**"),
)


def run_projection_widths(session: Session) -> str:
    """One file at five projection widths.

    Every attribution the cross-file apparatus makes is a subtraction between
    two adjacent rows here, over identical rows of an identical file — so
    neither `cross-file-floor`'s subtraction floor nor a file-dependent untyped
    baseline enters. That is what makes this a
    supersession of that apparatus rather than one more figure beside it.
    """
    figure = "projection-widths"
    specs = [
        RunSpec("pgdt", "arrays", f"query-project-{width}", "warm", f"{width}-column")
        for width, _, _ in _PROJECTION_ROWS
    ]
    # Six, like the two differencing figures this replaces: the reading that
    # matters is a paired difference between adjacent rows, and pairing is
    # per rep.
    session.sweep(figure, specs, session.cfg.reps(6))
    profile = session.stager.profile("arrays")
    rows, per_rep, previous = [], [], None
    for width, label, buys in _PROJECTION_ROWS:
        spec = RunSpec("pgdt", "arrays", f"query-project-{width}", "warm", "")
        values = session.get(figure, spec)
        if previous is None:
            delta = "—"
        else:
            paired = [(a - b) / profile["rows"] * 1e6 for a, b in zip(values, previous)]
            delta = f"**{median(paired):+.2f} µs**"
        rows.append(
            [
                label,
                f"{fmt_s(median(values))} s",
                f"{median(values) / profile['rows'] * 1e6:.2f} µs",
                delta,
                buys,
            ]
        )
        per_rep.append(f"- {label}: {fmt_readings(values)}")
        previous = values
    table = md_table(
        ["Projection", "Median", "Per row", "Δ per row against the row above", "What that buys"],
        rows,
    )
    return (
        table
        + f"\n\nOne file — `--arrays --composite`, {profile['rows']:,} rows of "
        f"{profile['columns']} columns — read {len(_PROJECTION_ROWS)} ways, warm and typed, "
        "through the CLI. Per-row differences are paired rep by rep and then taken as a "
        "median.\n\n"
        + _cached_query_note("Each `query`")
        + "\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- what a filter term costs ------------------------------------------------

#: The rows of `predicate-terms`, in the order the table carries them. The
#: first four are one depth at four term counts, so their differences are what
#: one more term costs; the fifth is the fourth's terms moved to the front of
#: the row, so its difference is the walk those five terms pay; the sixth
#: closes the shallow ladder, so the reader has a slope at both depths.
_PREDICATE_ROWS: tuple[tuple[str, str, str], ...] = (
    ("deep-1", "1 term, 13th column", "the base: one term, thirteen fields in"),
    ("deep-2", "2 terms, 13th column", "one more term at that depth"),
    ("deep-3", "3 terms, 13th column", "one more"),
    ("deep-5", "5 terms, 13th column", "two more — the five-way disjunction"),
    ("shallow-5", "5 terms, 1st column", "**the walk those five terms pay**"),
    ("shallow-1", "1 term, 1st column", "four of those five terms, walk-free"),
)


def run_predicate_terms(session: Session) -> str:
    """One file, six predicates, no row surviving any of them.

    The shape the split-sharing lever pays off on, which no other figure in
    the register runs: every one of these queries evaluates every term of an
    all-false disjunction on every row and emits nothing, so what separates
    two rows is the predicate and nothing downstream of it.
    """
    figure = "predicate-terms"
    specs = [
        RunSpec("pgdt", "control", f"query-where-{shape}", "warm", label)
        for shape, label, _ in _PREDICATE_ROWS
    ]
    # Six, as `projection-widths` takes: the reading that matters is a paired
    # difference between two rows, and pairing is per rep.
    session.sweep(figure, specs, session.cfg.reps(6))
    profile = session.stager.profile("control")
    rows, per_rep, previous = [], [], None
    for shape, label, buys in _PREDICATE_ROWS:
        spec = RunSpec("pgdt", "control", f"query-where-{shape}", "warm", "")
        values = session.get(figure, spec)
        if previous is None:
            delta = "—"
        else:
            paired = [(a - b) / profile["rows"] * 1e6 for a, b in zip(values, previous)]
            delta = f"**{median(paired):+.2f} µs**"
        rows.append(
            [
                label,
                f"{fmt_s(median(values))} s",
                f"{median(values) / profile['rows'] * 1e6:.2f} µs",
                delta,
                buys,
            ]
        )
        per_rep.append(f"- {label}: {fmt_readings(values)}")
        previous = values
    table = md_table(
        ["Predicate", "Median", "Per row", "Δ per row against the row above", "What that buys"],
        rows,
    )
    written = "\n".join(
        f"- {label}: `--where '{predicate_expr(shape)}'`"
        for shape, label, _ in _PREDICATE_ROWS
    )
    return (
        table
        + f"\n\nOne file — the brace-free control, {profile['rows']:,} rows of "
        f"{profile['columns']} columns — read {len(_PREDICATE_ROWS)} ways, warm and "
        "`--schema-mode strings`, through the CLI. Every term is an equality against a "
        "literal no value of the column can equal, so every row is walked, every term is "
        "evaluated, and no row is decoded, built or rendered. Per-row differences are "
        "paired rep by rep and then taken as a median.\n\n"
        + _cached_query_note("Each `query`")
        + "\nAs written:\n"
        + written
        + "\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- the per-block quadratic ------------------------------------------------

_BLOCK_COUNTS = (500, 1000, 2000, 4000)

#: The series, and the control that makes it readable as one: the same byte
#: count in **one** `COPY` block. Without it the table shows a cost rising with
#: block count and cannot say how much of the cost *is* block count — the
#: one-block run is what the 4000-block figure is a multiple of. A control the
#: harness does not run is a control that goes stale silently.
_QUADRATIC_ROWS: tuple[tuple[str, str], ...] = (
    ("one_block", "1 (control)"),
    *tuple((f"blocks{n}", f"{n}") for n in _BLOCK_COUNTS),
)


def run_per_block_quadratic(session: Session) -> str:
    """The series and its control, on the shipped binary alone.

    **There was a "before" column here, built at the commit preceding
    `SaveThrottle`, and it is retired rather than repaired.** A pinned
    historical build prices everything that differs between the two trees, and
    that set only grows: by the time anything ran the column the gap was 453
    commits, so it charged 453 commits of unrelated work to the throttle while
    the section conceded only that it was "a whole-commit comparison". What the
    throttle and its gate bought is a settled historical fact and is recorded as
    one beside the mechanism (`decisions.md`, "D63"), which costs a sentence rather than a build with no expiry
    condition on it."""
    figure = "per-block-quadratic"
    specs = [
        RunSpec("pgdt", name, "parse-cache-out", "warm", name) for name, _ in _QUADRATIC_ROWS
    ]
    session.sweep(figure, specs, session.cfg.reps(2))

    counts = [input_block_count(name) for name, _ in _QUADRATIC_ROWS]
    rows, per_rep, save_counts = [], [], []
    for name, label in _QUADRATIC_ROWS:
        dump = session.input_path(name, "warm")
        readings = session.get(figure, RunSpec("pgdt", name, "parse-cache-out", "warm", ""))
        saves, cache_size = count_saves(
            session.cfg, session.binary_path("pgdt"), dump, session.log
        )
        save_counts.append(saves)
        rows.append(
            [
                label,
                _fmt_bytes(file_size(session.cfg, dump, name)),
                _fmt_bytes(cache_size),
                f"{fmt_s(median(readings))} s",
                str(saves),
            ]
        )
        per_rep.append(f"- {label}: {fmt_readings(readings)}")
    table = md_table(["blocks", "dump", "final cache", "parse", "saves"], rows)
    # Read off the column rather than asserted beside it: the sentence is the
    # throttle's whole visible signature here, and a claim the renderer states
    # without computing is one that goes false the sitting the shape changes.
    span = (
        f"stays at {save_counts[0]}"
        if len(set(save_counts)) == 1
        else f"runs {min(save_counts)}–{max(save_counts)}"
    )
    note = (
        "\n\nSave counts are `strace -f -e trace=open,openat` on the host, untimed. Across a "
        f"block count multiplying by {counts[-1] // counts[0]:,}× from the control, "
        f"the save count {span} — the throttle is a ratio against elapsed time, not a count of "
        "watermarks.\n"
    )
    return table + note + "\nPer-rep readings (s):\n" + "\n".join(per_rep) + "\n"


def _fmt_bytes(n: int) -> str:
    # Three scales, because `peak-rss` puts a 2 MB input and a 3.00 GiB one in
    # adjacent rows and "3072.0 MB" is not the size anybody asked for.
    if n >= GIB:
        return f"{n / GIB:.2f} GiB"
    if n >= MIB:
        return f"{n / MIB:.1f} MB"
    return f"{n / 1024:.0f} KB"


# -- what a scan holds resident ---------------------------------------------

#: The four rows of `peak-rss`, in table order: an input and what it varies.
#:
#: **Three readings, two controlled comparisons, one pivot.** `one_block` is
#: the pivot; `control` is the *same generator, same seed, same shape* at 1536x
#: the bytes, so the first two rows differ in bytes and nothing else; and the
#: block-count rows hold the byte count near the pivot's while multiplying the
#: block count by 500 and 4000. That is what lets one table answer both halves
#: of the claim it carries -- nothing accumulates per byte, nothing accumulates
#: per block -- which is precisely what koji, at 74 blocks over 784 GB, cannot:
#: a cost that scales with block count cannot express itself in it at all.
_RSS_ROWS: tuple[str, ...] = ("one_block", "control", "blocks500", "blocks4000")

#: The pivot every subtraction in this table is taken against.
_RSS_PIVOT = "one_block"


def input_block_count(name: str) -> int:
    """How many `COPY` blocks one registered input holds.

    Read off the generator and its arguments rather than written beside the
    row, so a block count that changes in the generator cannot leave a stale
    number in the table's own column."""
    spec = INPUTS[name]
    if spec.generator == "generate_perf_data.py":
        return 1
    if spec.generator == "generate_block_count_bench.py":
        return int(spec.args[spec.args.index("--blocks") + 1])
    raise ValueError(f"input {name!r} does not say how many blocks it holds")


def run_peak_rss(session: Session) -> str:
    figure = "peak-rss"
    specs = [
        RunSpec("pgdt", name, "parse-rss", "warm", f"peak RSS {name}") for name in _RSS_ROWS
    ]
    session.sweep(figure, specs, session.cfg.reps(3))

    by_name = dict(zip(_RSS_ROWS, specs))
    rows, per_rep = [], []
    for name, spec in zip(_RSS_ROWS, specs):
        readings = session.get_rss(figure, spec)
        size = file_size(session.cfg, session.input_path(name, "warm"), name)
        rows.append(
            [
                f"`{name}`",
                _fmt_bytes(size),
                f"{input_block_count(name):,}",
                fmt_mib_median_spread(readings),
            ]
        )
        per_rep.append(f"- `{name}`: " + ", ".join(fmt_mib(v) for v in readings))
    table = md_table(["Input", "Bytes", "`COPY` blocks", "Peak RSS"], rows)

    pivot = median(session.get_rss(figure, by_name[_RSS_PIVOT]))
    pivot_size = file_size(
        session.cfg, session.input_path(_RSS_PIVOT, "warm"), _RSS_PIVOT
    )
    byte_size = file_size(session.cfg, session.input_path("control", "warm"), "control")
    per_byte = median(session.get_rss(figure, by_name["control"])) - pivot
    steps = []
    for name in _RSS_ROWS:
        count = input_block_count(name)
        if count == input_block_count(_RSS_PIVOT):
            continue
        delta = median(session.get_rss(figure, by_name[name])) - pivot
        extra = count - input_block_count(_RSS_PIVOT)
        steps.append(
            f"{count:,} blocks cost {fmt_rss_delta(delta)} "
            f"({delta * 1024 / extra:+,.0f} bytes a block)"
        )
    note = (
        f"\n\nEvery subtraction is against `{_RSS_PIVOT}`, the 1-block "
        f"{_fmt_bytes(pivot_size)} pivot. **Per byte:** "
        f"{byte_size / pivot_size:.0f}× the bytes costs **{fmt_rss_delta(per_byte)}**. "
        f"**Per block**, at byte counts within an order of magnitude of the pivot's: "
        + ", and ".join(steps)
        + ".\n"
    )
    return table + note + "\nPer-rep readings:\n" + "\n".join(per_rep) + "\n"


# -- what that growth is made of --------------------------------------------

#: The two block counts every leg of `rss-attribution` is taken at.
#:
#: **Two, not one.** A single absolute at 4,000 blocks folds in a baseline that
#: differs by 110 MiB between allocators, and jemalloc would read as
#: catastrophic on absolutes when its *slope* is within 35% of glibc's. Both
#: are `generate_block_count_bench.py` outputs, so the pair differs in block
#: count and in nothing else, and each reading the table publishes is a slope
#: in blocks rather than one number with a fixed baseline inside it.
_ATTRIBUTION_INPUTS: tuple[str, str] = ("blocks500", "blocks4000")

#: The nine legs, in table order: what the table calls the leg, which binary
#: runs it, and which command shape it is.
#:
#: **The two allocators besides the shipped one are named, never
#: re-specified.** What a leg's
#: binary *is* -- `--no-default-features`, its own target dir, `--version` read
#: back -- is the `allocator` figure's apparatus rule, so this figure calls
#: `ensure_allocator_binary` rather than carrying a second recipe for the same
#: builds, which is how two recipes for one binary come to differ. What is
#: genuinely this figure's own is the two shapes no other figure runs -- the
#: preamble prepass and `info` over a finished cache -- which are what separate
#: a cost paid per *table* from one paid per `COPY` block.
_ATTRIBUTION_LEGS: tuple[tuple[str, str, str], ...] = (
    ("`parse` — the `peak-rss` row", "pgdt", "parse-rss"),
    ("`parse`, system", "alloc:system", "parse-rss"),
    ("`parse`, jemalloc", "alloc:jemalloc", "parse-rss"),
    ("`parse --preamble-only`", "pgdt", "parse-preamble-rss"),
    ("`info --dtcache` over the finished cache", "pgdt", "info-cache-rss"),
    ("`query` (no match), cached", "pgdt", "query-nomatch-cached-rss"),
    ("`query` (no match), `--dtcache none`", "pgdt", "query-nomatch-rss"),
    ("`query` (no match), `--dtcache none`, system", "alloc:system", "query-nomatch-rss"),
    ("`query` (no match), `--dtcache none`, jemalloc", "alloc:jemalloc", "query-nomatch-rss"),
)


def _attribution_specs() -> list[tuple[str, RunSpec, RunSpec]]:
    """Each leg's label and its two specs, small block count first.

    A leg is identified by binary *and* shape, never by its label: `RunSpec.key`
    carries neither the label nor anything else, so two legs differing only in
    the words the table prints would silently share one reading. A test holds
    the nine apart."""
    return [
        (
            label,
            *(
                RunSpec(binary, name, command, "warm", f"{label} — {name}")
                for name in _ATTRIBUTION_INPUTS
            ),
        )
        for label, binary, command in _ATTRIBUTION_LEGS
    ]


def run_rss_attribution(session: Session) -> str:
    """What the per-block resident growth `peak-rss` measures is made of.

    `peak-rss` measures the whole; this attributes it, by holding the block
    count as the only axis and varying one mechanism at a time. Every reading
    is a resident set, and the published quantity is the **slope** -- bytes a
    block -- so an allocator's baseline cannot masquerade as growth.
    """
    figure = "rss-attribution"
    legs = _attribution_specs()
    # Before the first reading, as the `allocator` figure builds its own: a leg
    # discovered missing at rep two has already spent the session's first rep
    # under a different machine state.
    for _, binary, _ in _ATTRIBUTION_LEGS:
        if binary.startswith("alloc:"):
            ensure_allocator_binary(session.cfg, binary.removeprefix("alloc:"), session.log)
    # The `parse` reference row, from `peak-rss` where this sitting took it.
    # Borrowed rather than re-measured so the doc carries one number per
    # measurement, and *dropped from the interleave* rather than left in it: a
    # spec the sitting already holds would spend a reading to overwrite one.
    note = share_readings(session, figure)
    specs = [spec for _, small, big in legs for spec in (small, big)]
    session.sweep(
        figure, [s for s in specs if not session.has(figure, s)], session.cfg.reps(3)
    )

    small_n, big_n = (input_block_count(name) for name in _ATTRIBUTION_INPUTS)
    rows, per_rep = [], []
    for label, small_spec, big_spec in legs:
        small_reps = session.get_rss(figure, small_spec)
        big_reps = session.get_rss(figure, big_spec)
        small, big = median(small_reps), median(big_reps)
        rows.append(
            [
                label,
                fmt_mib(small),
                fmt_mib(big),
                f"{(big - small) * 1024 / (big_n - small_n):+,.0f} B",
            ]
        )
        per_rep.append(
            f"- {label}: "
            + " · ".join(
                ", ".join(f"{v / 1024:.2f}" for v in reps) for reps in (small_reps, big_reps)
            )
        )
    table = md_table(
        ["Leg", f"{small_n:,} blocks", f"{big_n:,} blocks", "Per block"], rows
    )
    return (
        table
        + (f"\n{note}" if note else "")
        + f"\n\nPer-rep readings (MiB, {small_n:,} then {big_n:,}):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- the map's own quadratic ------------------------------------------------


def run_map_only(session: Session) -> str:
    figure = "map-only"
    counts = (1000, 2000, 4000)
    specs = [
        RunSpec("pgdt", f"blocks{n}", "query-nomatch", "warm", f"map only blocks{n}")
        for n in counts
    ]
    session.sweep(figure, specs, session.cfg.reps(3))
    medians = [f"{fmt_s(median(session.get(figure, s)))} s" for s in specs]
    table = md_table(
        ["blocks", *[str(n) for n in counts]], [["map only, no saving", *medians]]
    )
    per_rep = "\n".join(
        f"- {n} blocks: {fmt_readings(session.get(figure, s))}" for n, s in zip(counts, specs)
    )
    return table + "\n\nPer-rep readings (s):\n" + per_rep + "\n"


# -- the preamble prepass ---------------------------------------------------


def run_preamble_prepass(session: Session) -> str:
    figure = "preamble-prepass"
    preamble = RunSpec("pgdt", "blocks4000", "parse-preamble", "warm", "preamble-only blocks4000")
    session.sweep(figure, [preamble], session.cfg.reps(5))
    note = share_readings(session, figure)
    full_spec = RunSpec("pgdt", "blocks4000", "parse-cache-out", "warm", "full parse blocks4000")
    if full_spec.key(figure) not in session.readings:
        session.sweep(figure, [full_spec], session.cfg.reps(2))
    full = session.get(figure, full_spec)
    pv = session.get(figure, preamble)
    dump = session.input_path("blocks4000", "warm")
    first_copy = 0
    if dump.exists():
        header = subprocess.run(
            ["grep", "-m1", "-b", "-a", "-E", "^COPY .* FROM stdin;", str(dump)],
            text=True, stdout=subprocess.PIPE, env={**os.environ, "LC_ALL": "C"},
        ).stdout
        first_copy = int(header.split(":", 1)[0]) if ":" in header else 0
    total = file_size(session.cfg, dump, "blocks4000")
    table = md_table(
        ["", "Wall"],
        [
            ["`parse --preamble-only`, 4000-table dump", f"**{fmt_s(median(pv))} s**"],
            ["full `parse` of the same file", f"{fmt_s(median(full))} s"],
        ],
    )
    share = (
        f"\n\nThe first `COPY` header sits at byte {first_copy:,} of {total:,} — "
        f"{first_copy / total:.0%} of the file is preamble, which is the most "
        "preamble-heavy shape available here.\n"
    )
    return table + share + note + f"\nPer-rep readings (s): {fmt_readings(pv)}\n"


# -- the nested decode micro ------------------------------------------------

# The literals the bench builds, reproduced so the byte counts in the table are
# computed rather than remembered: `int_array_literal(n)` is `{` plus n copies
# of a constant-width 11-byte int joined by commas plus `}`.
_RECORD_LITERAL = '(-1234567890,"lorem ipsum dolor sit amet")'


def array_literal_bytes(n: int) -> int:
    return 12 * n + 1


_MICRO_ROWS = (
    ("`integer[]`, 4 elements", array_literal_bytes(4), "array_4", "text_copy/array_4_len"),
    ("`integer[]`, 50 elements", array_literal_bytes(50), "array_50", "text_copy/array_50_len"),
    ("two-field composite", len(_RECORD_LITERAL), "record_2", "text_copy/record_2_len"),
)

VIEW_BATCH = 1024


def run_nested_decode_micro(session: Session) -> str:
    cfg = session.cfg
    session.ran_in("nested-decode-micro", HOST)
    if not cfg.dry_run:
        run(
            ["cargo", "bench", "-p", "pgdump_query", "--bench", "decoders", "--", "nested"],
            cwd=REPO,
        )
    root = REPO / "target/criterion/nested"
    if cfg.dry_run:
        return "(dry run: criterion not executed)"
    view_ns = criterion_median_ns(root, "nested/text_view_x1024") / VIEW_BATCH
    rows = []
    for label, nbytes, bench, control in _MICRO_ROWS:
        decode = criterion_median_ns(root, f"nested/{bench}/decode")
        render = criterion_median_ns(root, f"nested/{bench}/render")
        copy = criterion_median_ns(root, f"nested/{control}")
        rows.append(
            [
                label,
                str(nbytes),
                _fmt_ns(decode),
                _fmt_ns(render),
                f"**{(decode + render) / copy:.1f}×**",
                f"**{(decode + render) / view_ns:.0f}×**",
            ]
        )
    table = md_table(
        ["Literal", "Bytes", "`decode`", "`render`", "÷ copy", "÷ view"], rows
    )
    controls = "\n".join(
        f"- copy control at {nbytes} bytes: {_fmt_ns(criterion_median_ns(root, f'nested/{control}'))}"
        for _, nbytes, _, control in _MICRO_ROWS
    )
    slope_d = (
        criterion_median_ns(root, "nested/array_50/decode")
        - criterion_median_ns(root, "nested/array_4/decode")
    ) / 46
    slope_r = (
        criterion_median_ns(root, "nested/array_50/render")
        - criterion_median_ns(root, "nested/array_4/render")
    ) / 46
    notes = (
        f"\nThe view control is `text_view_x1024`'s median ÷ {VIEW_BATCH} = **{view_ns:.2f} ns**, "
        "a floor on the borrowed arm rather than the borrowed arm itself, so the `÷ view` column "
        "bounds the real ratio from above.\n"
        f"Per-element slope between the two array lengths: **{slope_d:.0f} ns** decoding and "
        f"**{slope_r:.0f} ns** rendering.\n\nCriterion medians (ns):\n" + controls + "\n"
    )
    return table + notes


# -- the allocator ----------------------------------------------------------

#: The three headline shapes, over the control file, warm, each paired with
#: the figure whose reading of it is shared rather than retaken.
#:
#: These are the shapes the phase's baseline table quotes and the shapes the
#: profile was taken on: `parse` is discovery, and the two `query` modes are
#: row extraction with and without typing. Nothing narrower would answer the
#: question, because the allocator's share of each is different -- the
#: allocator is ~8% of a `typed` query's user time and does not appear in
#: `parse`'s profile at all, and it is `parse` where the answer turned out to
#: be decided.
_ALLOCATOR_SHAPES: tuple[tuple[str, str, str], ...] = (
    ("parse", "`pgdt parse` — structure discovery", "scan-throughput-warm"),
    (
        "query-strings",
        "`query --schema-mode strings` — zero-copy extraction",
        "nested-end-to-end",
    ),
    ("query-typed", "`query --schema-mode typed`", "nested-end-to-end"),
)


#: The same three shapes as a borrow graph the register can read. Built from
#: `_ALLOCATOR_SHAPES` rather than written out again, so a shape added there
#: cannot be one the harness forgets to share.
ALLOCATOR_SHARES: tuple[Shared, ...] = tuple(
    Shared(
        source,
        f"the reference column's `{command}` row",
        (RunSpec("pgdt", "control", command, "warm", ""),),
    )
    for command, _, source in _ALLOCATOR_SHAPES
)


def _allocator_reference(cfg: Config) -> str:
    """The allocator the *shipped* binary links against, read out of it.

    This figure's reference column is `target/release/pgdt` itself rather than
    a fourth build of the same source, for two reasons. It is the binary every
    other figure in the doc was taken with, so the ratios are ratios against
    the published numbers instead of against a build nothing else uses -- and
    two builds of one source can differ by ~10% from code layout alone
    (`measurements.md`, "Two builds of one source can differ by layout"), which
    is larger than the effect being measured. And it keeps the doc carrying
    **one** number per measurement: the reference readings are borrowed from
    the figures that already take them, the warm throughput table's `COPY` row
    and the nested table's control rows.

    Reading the name off the binary rather than assuming the default is what
    makes the figure survive its own answer: adopt a leg and this becomes the
    reference, with the other two measured against it and no code change."""
    if cfg.dry_run:
        return ALLOCATOR_LEGS[0]
    return binary_allocator(cfg.bin_pgdt)


def allocator_columns(reference: str) -> list[str]:
    """The table's columns: the shipped allocator first, then the rest in
    register order. The reference is the column the ratios are against, so it
    is the one a reader needs first."""
    return [reference] + [leg for leg in ALLOCATOR_LEGS if leg != reference]


def _allocator_specs(reference: str) -> list[RunSpec]:
    """Every leg of every shape, plus the co-measured floor.

    The reference leg runs as the plain `pgdt` binary; the others are built
    per leg. The floor is one row rather than three: `dd` links no allocator,
    so a per-leg floor would be three readings of one thing. It is here for
    the same reason every other warm table co-measures one -- a session's own
    drift is what a warm absolute is read against."""
    if reference not in ALLOCATOR_LEGS:
        raise ValueError(f"unknown allocator leg {reference!r}")
    specs = []
    for command, label, _ in _ALLOCATOR_SHAPES:
        for leg in allocator_columns(reference):
            binary = "pgdt" if leg == reference else f"alloc:{leg}"
            specs.append(RunSpec(binary, "control", command, "warm", f"{label} ({leg})"))
    specs.append(RunSpec("none", "control", "dd", "warm", "`dd` → `/dev/null` (warm)"))
    return specs


def run_allocator(session: Session) -> str:
    figure = "allocator"
    reference = _allocator_reference(session.cfg)
    columns = allocator_columns(reference)
    specs = _allocator_specs(reference)
    # Every leg built before the first reading, not lazily at the rep that
    # first wants one: a `cargo` build across this machine's cores moves the
    # very number the next rep takes, which is the same rule that keeps a
    # sweep off a busy machine.
    for leg in columns:
        if leg != reference:
            ensure_allocator_binary(session.cfg, leg, session.log)
    # The reference column, shape by shape, from whichever figure already took
    # it. Borrowed rather than retaken so the doc carries one number per
    # measurement; measured here, with a note, when this figure runs alone.
    note = share_readings(session, figure)
    to_run = [s for s in specs if s.key(figure) not in session.readings]
    session.sweep(figure, to_run, session.cfg.reps(5))

    rows = []
    for command, label, _ in _ALLOCATOR_SHAPES:
        cells = [label]
        reference_median = median(
            session.get(figure, RunSpec("pgdt", "control", command, "warm", ""))
        )
        for leg in columns:
            binary = "pgdt" if leg == reference else f"alloc:{leg}"
            values = session.get(figure, RunSpec(binary, "control", command, "warm", ""))
            cell = fmt_median_spread(values)
            if leg != reference:
                cell += f" — {median(values) / reference_median:.2f}×"
            cells.append(cell)
        rows.append(cells)
    rows.append(
        [
            "`dd` → `/dev/null` — the co-measured floor",
            fmt_median_spread(session.get(figure, specs[-1])),
            *(["—"] * (len(columns) - 1)),
        ]
    )
    header = [
        f"`{leg}`" + (" — the shipped binary" if leg == reference else "")
        for leg in columns
    ]
    table = md_table(["Warm, on tmpfs", *header], rows)

    if session.cfg.dry_run:
        provenance = "Legs are not built or interrogated under `--dry-run`.\n"
    else:
        legs = ", ".join(
            f"`{leg}`"
            if leg == reference
            else f"`{binary_allocator(ensure_allocator_binary(session.cfg, leg, session.log))}`"
            for leg in columns
        )
        provenance = (
            "Every leg was asked what it links against before it was timed — "
            f"{legs} — so a leg whose build silently dropped its feature cannot be "
            "published as a comparison of two identical binaries.\n"
        )
    return (
        table
        + "\n\n"
        + provenance
        + "\n"
        + _cached_query_note("Each `query` row")
        + note
        + "\n"
        + _per_rep(figure, session, specs)
    )


# -- xz decode scaling ------------------------------------------------------

#: The two legs, in the table's column order, with the caption each carries.
#: The generated one first, because it is the one anybody can re-take.
DECODE_LEGS: tuple[tuple[str, str], ...] = (
    ("control_xz", "Generated control"),
    ("koji_xz", "koji, 128 streams"),
)


def _decode_specs() -> list[RunSpec]:
    return [
        RunSpec("xzdecode", leg, f"decode-{workers}", "warm-parallel", f"{label}, {workers}w")
        for leg, label in DECODE_LEGS
        for workers in DECODE_WORKERS
    ]


def run_xz_decode_scaling(session: Session) -> str:
    """Plaintext decode rate against worker count, over two `.xz` files.

    **The rate's denominator comes from the instrument, not from a file size.**
    A compressed input's plaintext volume is in its seek table and nowhere the
    harness can `stat`, so the binary prints what it decoded and the table
    divides that by the wall clock. It also prints the worker count its plan
    admitted and refuses to run when that is short of what was asked for, so no
    row here can be a second reading of a lower count wearing a higher label.

    **Every row is read against the one-worker row of its own leg**, which is
    both the figure's content — what the second core through the twenty-fourth
    buy — and its witness, this being a regime where the machine's own busyness
    cannot gate a reading (`CONTENTION_LIMITS`).

    **Five reps.** The spread that matters here is between adjacent worker
    counts near the top of the curve, where the increments are small; three
    reps resolved the bottom of the curve and left the top ambiguous, and the
    whole sitting is minutes rather than the hours a sweep costs.
    """
    ensure_xz_decode_binary(session.cfg, session.log)
    specs = _decode_specs()
    session.sweep("xz-decode-scaling", specs, session.cfg.reps(5))

    by_leg: dict[str, dict[int, list[float]]] = {leg: {} for leg, _ in DECODE_LEGS}
    plaintext: dict[str, int] = {}
    for spec in specs:
        workers = int(spec.command.rpartition("-")[2])
        by_leg[spec.input][workers] = session.get("xz-decode-scaling", spec)
        reported = session.reported.get(spec.key("xz-decode-scaling"), {})
        if "delivered" in reported:
            plaintext[spec.input] = int(reported["delivered"])

    rows = []
    for workers in DECODE_WORKERS:
        cells = [str(workers) + (" *(serial)*" if workers == DECODE_BASELINE else "")]
        for leg, _ in DECODE_LEGS:
            values = by_leg[leg][workers]
            got, base = median(values), median(by_leg[leg][DECODE_BASELINE])
            rate = fmt_rate(plaintext.get(leg, 0), got)
            cells.append(f"{fmt_median_spread(values)} · {rate} · {base / got:.2f}×")
        rows.append(cells)
    table = md_table(["Workers", *(label for _, label in DECODE_LEGS)], rows)

    sizes = []
    for leg, label in DECODE_LEGS:
        compressed = file_size(
            session.cfg, session.input_path(leg, "warm-parallel"), leg
        )
        plain = plaintext.get(leg, 0)
        ratio = f"{plain / compressed:.2f}×" if compressed else "—"
        # The koji leg's density is the one that can move between sittings, an
        # offset or a refreshed download landing in a region of quite different
        # bytes, so it is gated where the slice is cut and named here — a rate
        # per plaintext byte is a property of the bytes, and a table that
        # quoted one without its density would be quoting it of koji.
        # Stated as what the apparatus refuses, not as a verdict on the number
        # beside it: the ratio is printed from the readings and the band is the
        # generator's, so a sentence claiming the one is inside the other would
        # be the table vouching for itself.
        gate = (
            f" (`scripts/generate_xz_input.py` refuses a slice outside "
            f"{KOJI_RATIO_MIN:g}–{KOJI_RATIO_MAX:g}×)"
            if leg == "koji_xz"
            else ""
        )
        sizes.append(
            f"- {label}: {_fmt_bytes(compressed)} compressed, {_fmt_bytes(plain)} of plaintext, "
            f"{ratio}{gate}"
        )
    notes = (
        "\n\nEach cell is wall clock, the plaintext rate it implies, and the speedup over that "
        f"leg's own one-worker row. The instrument is `pgdump_query/examples/xz_decode.rs` in a "
        f"{DECODE_MEMORY} container — **not** the register's 512 MB, which cannot hold "
        f"{DECODE_WORKERS[-1]} decoded 24 MiB blocks — and it refuses a run whose plan admits "
        "fewer workers than were asked for.\n\n"
        + "\n".join(sizes)
        + "\n"
    )
    return table + notes + "\n" + _per_rep("xz-decode-scaling", session, specs)


# -- what a second scan worker buys -----------------------------------------

#: The four legs of `parallel-scan-throughput`, in the table's column order:
#: an input, the command family whose `-jobs-<n>` shapes it is run under, and
#: the caption the column carries.
#:
#: **Two axes crossed, both of which the phase argues about separately.**
#: Plain against `.xz` is whether there is a decoder in front of the scan;
#: `parse` against a typed scan is discovery against extraction. The rule
#: the design argues from is one line over those two — *parallelize what is CPU-bound*
#: (`docs/design/decisions.md`, "D25").
#: A table missing a column cannot check that rule; it would confirm whichever
#: half it kept.
#:
#: **The extraction legs run the provider, not `pgdt query`**
#: (`PARALLEL_SCAN`): `pgdt query`'s in-order merge reads one sub-stream at a
#: time past its first round (`KD57`), so a column of it times the merge
#: rather than the library's sub-streams, which DataFusion polls together.
PARALLEL_LEGS: tuple[tuple[str, str, str], ...] = (
    ("control", "parse", "Plain, `parse`"),
    ("control", PARALLEL_SCAN, "Plain, typed provider scan"),
    ("control_xz", "parse", "`.xz`, `parse`"),
    ("control_xz", PARALLEL_SCAN, "`.xz`, typed provider scan"),
)

#: The input whose byte count a leg's rate is per.
#:
#: **A compressed leg's rate is per *plaintext* byte, and the plaintext is a
#: registered input.** `control_xz` is a compression of `control`, so the
#: plaintext volume is `control`'s own size exactly — no instrument has to
#: report it and no seek table has to be read, which is the one thing
#: `xz-decode-scaling` could not do (its koji leg is a prefix of a file nothing
#: here generates). Dividing by the *compressed* size instead would state a rate
#: five times too low under a heading that reads like the plain one's.
PARALLEL_PLAINTEXT: dict[str, str] = {"control": "control", "control_xz": "control"}


def _parallel_specs() -> list[RunSpec]:
    return [
        RunSpec(
            "dfcli" if family == PARALLEL_SCAN else "pgdt",
            inp,
            f"{family}-jobs-{jobs}",
            "warm-parallel",
            f"{label}, {jobs}{'p' if family == PARALLEL_SCAN else 'j'}",
        )
        for inp, family, label in PARALLEL_LEGS
        for jobs in PARALLEL_JOBS
    ]


def parallel_answer_problems(reported: Mapping[str, Mapping[str, str]]) -> list[str]:
    """Why `parallel-scan-throughput`'s provider legs do not price one
    answer, keyed by each leg's `RunSpec.label`.

    **Every provider cell must return one answer**, byte for byte, at every
    partition count over both files: the two files are one plaintext and the
    count changes no row, so a difference is a scan that lost or repeated rows
    under the timer. An answer of no row, or a first count of none, is a
    query that read nothing."""
    digests = {answer.get("result_digest") for answer in reported.values()}
    if not reported or None in digests or "" in digests or len(digests) != 1:
        return ["the provider legs answered differently, or not at all"]
    bad = []
    for label, answer in reported.items():
        if answer.get("result_rows") != "1" or answer.get("result_first", "0") == "0":
            bad.append(f"{label}: the query counted no row")
    return bad


def run_parallel_scan_throughput(session: Session) -> str:
    """Wall clock against the worker count, over four legs of one 3.00 GiB
    plaintext: `pgdt parse` at each `--jobs`, and the provider's typed scan at
    each `target_partitions`.

    **Every cell is read against the one-job cell of its own leg**, which is
    both the figure's content — what the second worker through the twenty-fourth
    buy — and its witness: `warm-parallel` gates on almost nothing, a reading
    that occupies every hardware thread being busy by construction, so what
    stands in for the gate is that a machine busy with someone else's work moves
    a leg's whole column and leaves the ratio (`CONTENTION_LIMITS`). It is also
    why two programs may share a table: no cell is read against another
    leg's.

    **The baseline row is the serial path, not a pool of one.** `--jobs 1` is
    `Parallelism::Serial` carrying the same stated allowance as every other row,
    so a compressed leg's first row is one block-decoding reader rather than the
    streaming fallback. That serial path is the arrangement this project ships,
    which is what makes it the right denominator.

    **Five reps**, for `xz-decode-scaling`'s reason: the increments that matter
    are between adjacent counts near the top of the curve, where three reps
    leave the ordering ambiguous.
    """
    figure = "parallel-scan-throughput"
    specs = _parallel_specs()
    session.sweep(figure, specs, session.cfg.reps(5))
    problems = parallel_answer_problems(
        {
            spec.label: session.reported.get(spec.key(figure), {})
            for spec in specs
            if spec.binary == "dfcli"
        }
    )
    if problems:
        raise RuntimeError(f"{figure} does not price one answer: " + "; ".join(problems))

    by_leg: dict[tuple[str, str], dict[int, list[float]]] = {}
    for spec in specs:
        family, _, jobs = spec.command.rpartition("-jobs-")
        by_leg.setdefault((spec.input, family), {})[int(jobs)] = session.get(figure, spec)

    rows = []
    for jobs in PARALLEL_JOBS:
        cells = [str(jobs) + (" *(serial)*" if jobs == PARALLEL_BASELINE else "")]
        for inp, family, _ in PARALLEL_LEGS:
            values = by_leg[(inp, family)][jobs]
            got, base = median(values), median(by_leg[(inp, family)][PARALLEL_BASELINE])
            nbytes = file_size(
                session.cfg,
                session.input_path(PARALLEL_PLAINTEXT[inp], "warm-parallel"),
                PARALLEL_PLAINTEXT[inp],
            )
            cell = f"{fmt_median_spread(values)} · {fmt_rate(nbytes, got)} · {base / got:.2f}×"
            # A budget clamp is not `POOL_DEPTH`'s to footnote once — see
            # `QUERY_SUBSTREAM_CAP`. A provider leg the budget clamps inside
            # the axis states, on every row above four, the count it actually
            # planned, so a reader never has to ask whether a given cell is
            # the label or the ceiling; a leg absent from that dict is never
            # clamped and carries nothing.
            if family == PARALLEL_SCAN and jobs > 4 and inp in QUERY_SUBSTREAM_CAP:
                achieved = min(jobs, QUERY_SUBSTREAM_CAP[inp])
                cell += f" · {achieved} sub-stream{'s' if achieved != 1 else ''}"
            cells.append(cell)
        rows.append(cells)
    table = md_table(
        ["`--jobs` · `target_partitions`", *(label for _, _, label in PARALLEL_LEGS)], rows
    )

    plain = file_size(session.cfg, session.input_path("control", "warm-parallel"), "control")
    compressed = file_size(
        session.cfg, session.input_path("control_xz", "warm-parallel"), "control_xz"
    )
    allowance = stated_allowance(PARALLEL_BUDGET)
    notes = (
        "\n\nEach cell is wall clock, the plaintext rate it implies, and the speedup over that "
        "leg's own one-worker row. Both `.xz` legs decode the same "
        f"{_fmt_bytes(plain)} of plaintext the plain legs read directly "
        f"({_fmt_bytes(compressed)} on disk, {plain / compressed:.2f}×), so every rate is per "
        "the same bytes.\n\n"
        "**The `parse` legs run `pgdt parse` at `--jobs` N; the provider legs run "
        f"`pgdt sql -c` at `{DFCLI_PARTITIONS}=N`**, the session's "
        "`target_partitions`, which the provider plans a scan's sub-streams against and "
        "DataFusion polls together. `pgdt query` is not timed: its in-order merge reads one "
        "sub-stream at a time past its first round "
        "([`../status/deficiencies.md`](../status/deficiencies.md), `KD57`). The query is "
        f"`SELECT count({perf.COLUMNS[0][0]}), …, count({perf.COLUMNS[-1][0]}) FROM "
        f"{perf.TABLE} WHERE id IS NOT NULL`, a `count` of each of the table's "
        f"{len(perf.COLUMNS)} columns: every column decoded typed "
        "and one row out, the filter keeping every row and leaving the scan's exact NULL "
        'counts estimates, so no count is answered without the rows (`docs/design/decisions.md`, '
        '"D89"). Every provider cell answered alike, byte for byte, at every count over both '
        "files. A provider cell is read against its own leg and never against a `parse` "
        "cell: each carries the program's startup and the dump's registration, which "
        "`dynamic-filter-join`'s startup leg reads.\n\n"
        f"Every row states the allowance `{allowance}` — `--memory {allowance}` on a `parse` "
        f"leg, `SET pgdump.memory = {allowance}` run ahead of the query in the same process "
        f"on a provider leg — which leaves {_fmt_bytes(PARALLEL_BUDGET)} for read buffers, in "
        f"a {PARALLEL_MEMORY} container — **not** the "
        "register's 512 MB, which cannot hold twenty-four decoded 24 MiB blocks. The "
        "one-worker row states the same allowance: `--jobs 1` is `Parallelism::Serial` "
        "carrying it, as is a provider scan planned at one partition, so an `.xz` leg's "
        "one-worker row is one block-decoding reader rather than the streaming fallback, and "
        "that serial path is what a speedup is a speedup over.\n\n"
        "**A plain leg's count is what is asked for, not what is delivered.** "
        "`POOL_DEPTH` clamps the chunk pool to four slots and the interior split lets a "
        "worker wait for one, so a fifth fused worker on a plain source waits. What that "
        "wait costs the rows above four is not separated from anything else they pay "
        '(`docs/design/decisions.md`, "D25").\n\n'
        + _substream_note()
        + "Each provider leg, at every count, reads a cache one untimed `pgdt parse` stating "
        f"`{GATHER_STATISTICS}` wrote beside the dump in the same container, where `--dump` "
        "looks for it: its reading carries decoding that cache whole, statistics included, "
        "and no mapping pass, and its filter rules out no row group.\n\n"
        f"**`PARALLEL_BUDGET` is {_fmt_bytes(PARALLEL_BUDGET)} so that no `.xz` row is "
        "budget-clamped;** a plain source stays on the library's default budget whatever is "
        'stated (`docs/design/decisions.md`, "D83"), which is the clamp the counts above '
        "state. A compressed reader is charged its block, the chunk buffer and the "
        "decoder's own retention, and the readers together the block pool's retention list "
        '(`docs/design/decisions.md`, "I/O, memory and parallelism"), so a smaller '
        "budget would hold the widest `.xz` rows below the twenty-four they are labelled.\n"
    )
    return table + notes + "\n" + _per_rep(figure, session, specs)


def _substream_note() -> str:
    """What the provider columns say about the count they actually planned.

    Read off `QUERY_SUBSTREAM_CAP` rather than stated, because the dict is what
    decides whether a cell carries the annotation: a paragraph asserting a
    clamp the renderer did not print, or silence over one it did, is the
    divergence the harness owns its own prose to avoid.
    """
    head = (
        "**A provider leg's count can be clamped a second way, and that one the "
        "table states per cell rather than footnotes once.** `plan_partitions` solves a "
        "scan's sub-stream count against the read-buffer budget, each sub-stream costing "
        "its read plus a batch span narrowed toward the chunk size before the count is cut "
        '(`docs/design/decisions.md`, "D4", "D84") — a budget '
        "the *harness* chose, not a ceiling the library ships. "
    )
    if not QUERY_SUBSTREAM_CAP:
        return (
            head + "**At this budget neither provider leg reaches it**, so no cell "
            "carries the annotation: what each leg is charged and what it affords is "
            "`QUERY_SUBSTREAM_CAP`'s own argument, computed once there.\n\n"
        )
    clamped = ", ".join(
        f"`{count}` on {'`.xz`' if inp.endswith('_xz') else 'plain'}"
        for inp, count in sorted(QUERY_SUBSTREAM_CAP.items())
    )
    return (
        head + f"The rows at or above that count on such a leg state the count they actually "
        f"planned: {clamped}. Below that count a cell's sub-stream figure equals its "
        "row label; at or above it, every further worker asked for buys nothing more "
        "to plan.\n\n"
    )


# -- what a parallel scan holds resident ------------------------------------

#: The two legs of `parallel-peak-rss`: the same plaintext at two block sizes.
#: See `INPUTS["control_xz128"]` for why one of them would be a figure about the
#: file rather than about the library.
PARALLEL_RSS_LEGS: tuple[tuple[str, str], ...] = (
    ("control_xz", "24 MiB blocks"),
    ("control_xz128", "128 MiB blocks"),
)


def _parallel_rss_specs() -> list[RunSpec]:
    return [
        RunSpec("pgdt", leg, f"parse-rss-jobs-{jobs}", "warm-parallel", f"{label}, {jobs}j")
        for leg, label in PARALLEL_RSS_LEGS
        for jobs in PARALLEL_JOBS
    ]


def run_parallel_peak_rss(session: Session) -> str:
    """Peak resident set against `--jobs`, at two `.xz` block sizes.

    **The claim under test is that one stated number bounds the read path**, so
    the table's own witness is the column that stops rising: a leg whose peak
    keeps climbing with `--jobs` is a budget that is not a bound, and a leg that
    stops *above* the read-buffer budget its allowance leaves is the block
    pool's slot ceiling following the announced count rather than the delivered
    one (`KD21`). Two block
    sizes because a compressed reader's per-worker footprint is one decoded
    block, so the count the budget admits is a property of the *file* — at one
    size the table would publish that file's shape as the library's ceiling.

    **Every row block-decodes, the one-job row included**: `--jobs 1` is
    `Parallelism::Serial` carrying the stated budget, which affords one reader
    of either block size (`BlockCache::affordable`). So the baseline each row is
    read against is a one-reader block path, and on the 128 MiB leg it already
    holds the pool's four slots.

    **Three reps**, as `peak-rss` takes: a peak is a maximum rather than a mean,
    so it is far steadier across reps than a wall clock, and the reps are here
    to catch an outlier rather than to resolve a small difference.
    """
    figure = "parallel-peak-rss"
    specs = _parallel_rss_specs()
    session.sweep(figure, specs, session.cfg.reps(3))

    by_leg: dict[str, dict[int, list[float]]] = {}
    for spec in specs:
        jobs = int(spec.command.rpartition("-jobs-")[2])
        by_leg.setdefault(spec.input, {})[jobs] = session.get_rss(figure, spec)

    rows = []
    for jobs in PARALLEL_JOBS:
        cells = [str(jobs) + (" *(serial)*" if jobs == PARALLEL_BASELINE else "")]
        for leg, _ in PARALLEL_RSS_LEGS:
            values = by_leg[leg][jobs]
            delta = median(values) - median(by_leg[leg][PARALLEL_BASELINE])
            cells.append(
                fmt_mib_median_spread(values)
                + ("" if jobs == PARALLEL_BASELINE else f" · {fmt_rss_delta(delta)}")
            )
        rows.append(cells)
    table = md_table(["`--jobs`", *(label for _, label in PARALLEL_RSS_LEGS)], rows)

    sizes = []
    for leg, label in PARALLEL_RSS_LEGS:
        sizes.append(
            f"- {label}: `{leg}`, "
            f"{_fmt_bytes(file_size(session.cfg, session.input_path(leg, 'warm-parallel'), leg))}"
            " compressed"
        )
    notes = (
        "\n\nEach cell is peak resident set, and the change from that leg's own one-job row. "
        f"Every row states `--memory {stated_allowance(PARALLEL_BUDGET)}`, the allowance "
        f"that leaves {_fmt_bytes(PARALLEL_BUDGET)} for read buffers, in a "
        f"{PARALLEL_MEMORY} container — an apparatus "
        "departure from the register's 512 MB, which is smaller than the budget under "
        "test. The one-job row states the same allowance — `--jobs 1` is "
        "`Parallelism::Serial` carrying it — so every row on both legs block-decodes, "
        "the one-job row with one reader.\n\n"
        + "\n".join(sizes)
        + "\n\nPer-rep readings (peak RSS):\n"
        + "\n".join(
            f"- {spec.label}: "
            + ", ".join(fmt_mib(v) for v in session.get_rss(figure, spec))
            for spec in specs
        )
        + "\n\n**Where a leg goes flat, it is the block pool's slot ceiling that stopped "
        "growing.** The block pool's slots are `clamp((budget - held) / unit, 1, "
        "max(POOL_DEPTH, jobs))`, where `held` is the chunk pool's own retention — "
        "`clamp(budget / chunk, 1, POOL_DEPTH) * chunk`, which is `POOL_DEPTH * chunk` "
        "only once the budget affords four chunk slots. It holds one unit below that beside the block "
        "each reader has in flight. Above four workers the depth term is the stated "
        "`--jobs`, so which term binds is set by the *file's* block size: on the fine "
        "leg the budget term is far above the axis, the depth term binds, and the curve "
        "is still climbing at the right-hand end; on the coarse leg the budget term "
        "binds and the leg levels off. That ceiling follows the `--jobs` announced "
        "rather than the readers the budget affords, so the coarse leg's flat value "
        "sits above the read-buffer budget the allowance leaves, though within the stated "
        "`--memory` — `KD21`, not a bound the library keeps.\n"
    )
    return table + notes


# -- what a scan holds above the budget it was given ------------------------


def _fmt_budget(n: int) -> str:
    """A stated budget, in MiB.

    Not `_fmt_bytes`, which labels a mebibyte-scale value `MB`: that reads
    correctly for a file whose size nobody chose and wrongly for a number
    someone typed on a command line as `--memory 67108864`. Every
    registered budget is a whole mebibyte, which a test holds."""
    if n % MIB:
        raise ValueError(f"budget {n} is not a whole number of MiB")
    return f"{n >> 20} MiB"


def _fmt_budget_bytes(n: int) -> str:
    """A budget a *run* resolved for itself, in MiB to one place.

    `_fmt_budget` is for a number somebody typed and refuses anything but a
    whole mebibyte, which is the right refusal there. A discovered budget is
    `readers × what one reader holds` and is almost never whole — 195.1 MiB is
    three readers of a 24 MiB-block file — so it is formatted rather than
    checked."""
    return f"{n / MIB:.1f} MiB"


#: The one reading this figure does not take on the budget axis: the shipped
#: serial arrangement, at the library's own `DEFAULT_MEMORY_BUDGET`.
#:
#: **It is `peak-rss`'s `control` row, spec for spec**, which is why this
#: figure declares it as its one `Shared` edge — the same binary, command,
#: input and regime, so measuring it twice would put two numbers in the doc for
#: one measurement. A sitting that took `peak-rss` borrows it; one that did not
#: measures it here and says so in the table's own provenance paragraph.
_RESERVE_BASELINE = RunSpec("pgdt", "control", "parse-rss", "warm", "plain, serial default")


def _reserve_specs() -> list[RunSpec]:
    """One spec per source, arena setting and stated budget.

    A leg is identified by its command shape, which carries both the arena
    token and the budget, so two legs differing only in the words the table
    prints cannot share a reading -- the failure `_attribution_specs` names."""
    return [
        RunSpec(
            "pgdt",
            name,
            f"{RESERVE_FAMILY}{token}-{budget}",
            "warm-parallel",
            f"{source} {arena}, {_fmt_budget(budget)} stated",
        )
        for name, source in RESERVE_INPUTS
        for token, _, arena in RESERVE_ARENAS
        for budget in RESERVE_BUDGETS
    ]


#: The arena token that sets nothing. Every reading outside the arena legs
#: themselves is taken under it, because the shipped constant has to survive the
#: operator who capped nothing.
RESERVE_UNCAPPED = RESERVE_ARENAS[0][0]

#: The arena token that caps, read off the registry rather than written again.
RESERVE_CAPPED = next(token for token, value, _ in RESERVE_ARENAS if value)


def _flagless_shape(arena: str = "") -> str:
    """The flagless command shape, under one arena setting."""
    return f"{RESERVE_FLAGLESS}{arena or RESERVE_UNCAPPED}"


def _reserve_flagless_specs() -> list[RunSpec]:
    """One spec per compressed input and container limit, stating no flags.

    **The limit is on the spec, not in the shape**, because it is a `nerdctl
    run` argument: these legs share one command shape and are told apart by
    `RunSpec.memory`, which `RunSpec.key` carries for exactly this reason."""
    return [
        RunSpec(
            "pgdt",
            name,
            _flagless_shape(),
            "warm-parallel",
            f"{label}, flagless in {token}",
            memory=token,
        )
        for name, label, _ in RESERVE_FLAGLESS_INPUTS
        for token, _ in RESERVE_LIMITS
    ]


def _reserve_mechanism_specs() -> list[tuple[str, RunSpec]]:
    """The mechanism leg, and what the table calls it.

    **The reference is not here.** It is the flagless leg at the same input and
    the same limit, which the axis above already measures — so this is one
    reading rather than a pair, and the comparison is against a number no leg of
    this block had to re-take.

    **One leg, where three were registered.** The arena cap is the *same
    flagless arrangement* with one mechanism changed, and it is the only one of
    the three that bears on glibc's dynamic mmap threshold, which retains a
    block-sized buffer in the arena of every thread that ever decoded one:
    capping the arenas bounds how many can hold one. At `RESERVE_MECHANISM_LIMIT`
    the uncapped process already runs no more arenas than the cap allows, so the
    leg is inert there (`RESERVE_MECHANISM_LIMIT`). The two allocator legs are
    **dropped rather than re-aimed** — jemalloc and mimalloc do not have that
    threshold, so swapping them removes the mechanism instead of measuring it.
    On the default build, whose Rust heap is mimalloc's, the threshold reaches
    only what C allocates, and the rendered paragraph says so wherever the
    instrument reports two heaps.
    What replaced them is `_reserve_instrument_specs`, which asks the process
    rather than subtracting two of them.

    The path step is the other exception and states a budget, because the thing
    under test is a comparison the environment cannot express:
    `BlockCache::affordable` is read off the budget, so one byte either side of
    the one-reader charge it compares against (`charge_bytes(unit, 1)`, which is
    `reader_bytes` plus the pool's retention list) is the only way to change the
    path and nothing else.
    """
    arena = next(label for token, _, label in RESERVE_ARENAS if token == RESERVE_CAPPED)
    return [
        (
            arena,
            RunSpec(
                "pgdt",
                RESERVE_MECHANISM_INPUT,
                _flagless_shape(RESERVE_CAPPED),
                "warm-parallel",
                f"flagless in {RESERVE_MECHANISM_LIMIT}, {arena}",
                memory=RESERVE_MECHANISM_LIMIT,
            ),
        )
    ]


def _reserve_instrument_specs() -> list[RunSpec]:
    """The flagless arrangement on the introspection build, which is where the
    attribution comes from.

    **The same command shape as the black-box flagless legs**, so the two are
    the same arrangement measured two ways and the `getrusage` axis is the check
    on this one: a term the process names has to show up in the sum the wrapper
    measures. They differ by `RunSpec.binary` alone, which `key` carries, so
    neither can be read as a rep of the other.

    **The uncapped axis, plus the capped leg the mechanism row also takes.** The
    axis is what decomposes — every limit resolves its own reader count, so the
    program's own high-water can be read against that count — and the capped leg
    is what says *where* an arena cap's megabytes go, which the black-box delta
    beside it can only say *whether*; its `Arenas` column is also what says
    whether the cap had any arena to remove.

    These legs are readings of this figure, declared here; the build is never
    the timed binary: it takes an atomic on every allocation and
    `binary_allocator` refuses it."""
    return [
        RunSpec(
            "introspect",
            RESERVE_MECHANISM_INPUT,
            _flagless_shape(),
            "warm-parallel",
            f"instrument, flagless in {token}",
            memory=token,
            instrument=True,
        )
        for token in RESERVE_INSTRUMENT_LIMITS
    ] + [
        RunSpec(
            "introspect",
            RESERVE_MECHANISM_INPUT,
            _flagless_shape(RESERVE_CAPPED),
            "warm-parallel",
            f"instrument, flagless in {RESERVE_MECHANISM_LIMIT}, "
            + next(label for token, _, label in RESERVE_ARENAS if token == RESERVE_CAPPED),
            memory=RESERVE_MECHANISM_LIMIT,
            instrument=True,
        )
    ]


def _reserve_step_specs() -> list[RunSpec]:
    """The path step's pair, in table order: the budget that affords one
    block-decoding reader, then the one a byte below it."""
    return [
        RunSpec(
            "pgdt",
            RESERVE_MECHANISM_INPUT,
            f"{RESERVE_STEP_FAMILY}{budget}",
            "warm-parallel",
            f"`--memory {stated_allowance(budget)}`, block decode "
            + (
                "afforded"
                if block_path_afforded(RESERVE_MECHANISM_UNIT, budget)
                else "declined"
            ),
            memory=RESERVE_MECHANISM_LIMIT,
        )
        for budget in RESERVE_STEP_BUDGETS
    ]


def _least_squares(points: Sequence[tuple[float, float]]) -> tuple[float, float]:
    """The intercept and slope of the line best fitting `points`.

    `resident = fixed + readers × per_reader`, which is the pair this figure
    publishes. Written out rather than taken from a library because the harness
    depends on nothing but the standard library, and because three lines of
    arithmetic are easier to check than an import is to justify.

    **It is never given a resident set.** The mechanism is
    piecewise linear where this model is straight: the pool holds `workers`
    blocks in flight plus `max(POOL_DEPTH, workers) − 1` retained
    (`WorkerMemory::pool_bytes`), so held memory rises by one unit a reader
    below `POOL_DEPTH` and by two above it, with a real kink at four readers
    that is in the measured resident and not only in the charge. Every caller
    therefore hands this function a **remainder** — `_depooled` has already
    taken that term off each ordinate — and what comes back are the two terms
    the sitting does not know in advance.

    Raises on fewer than two distinct abscissae: a "fit" through one point is an
    intercept asserted as a measurement, which is the extrapolation mistake
    `.claude/skills/evidence/SKILL.md`'s second rule names.
    """
    xs = [x for x, _ in points]
    if len(set(xs)) < 2:
        raise ValueError(f"a fit needs two distinct reader counts, got {sorted(set(xs))}")
    n = len(points)
    mean_x, mean_y = sum(xs) / n, sum(y for _, y in points) / n
    cov = sum((x - mean_x) * (y - mean_y) for x, y in points)
    var = sum((x - mean_x) ** 2 for x in xs)
    slope = cov / var
    return mean_y - slope * mean_x, slope


def _depooled(
    points: Sequence[tuple[float, float]], unit: int
) -> list[tuple[float, float]]:
    """`points` — `(readers, MiB)` — with the block pool's retention list taken
    off each ordinate, leaving the remainder a line may honestly be fitted to.

    **The charge is piecewise linear and a straight line across its kink is a
    biased line**. `pool_bytes` is `(POOL_DEPTH.max(workers) − 1) ×
    unit`: constant below `POOL_DEPTH` and growing by a unit a reader above it,
    so what a leg holds rises by one unit a reader at the bottom of the axis and
    by two at the top, and both registered families straddle the bend
    (`RESERVE_LIMITS`). Fitting one line across it puts the bend into the
    intercept — over the charge's own held-unit values ≈49 MiB of intercept bias
    at 24 MiB blocks and ≈421 MiB at 128, where the slope reads 1.37 units
    against a true 2.0.

    **Subtracting rather than fitting a second regime is `charge_model`'s
    principle one table over**: every quantity in the term is known before the sitting and
    already mirror-checked against the library's own constants, so taking it off
    is arithmetic and not a degree of freedom. What is left — a fixed cost and a
    per-reader cost outside the pools the charge bills — is regime-free and is
    the part the sitting genuinely measures, which is why
    `RESERVE_FIT_MIN_COUNTS` is unchanged: the remainder still has two terms.

    **Only a block-path leg may be handed here.** A leg on the streaming
    fallback holds no block pool at all, so subtracting a retention list from it
    would invent a negative term; callers drop those legs from the line by name
    (`block_path_afforded`), the way the flagless family always has.
    """
    return [(x, y - pool_bytes(unit, int(x)) / MIB) for x, y in points]


def _fit_or_secant(
    points: Sequence[tuple[float, float]], unit: int
) -> tuple[float | None, float]:
    """`(fixed, per_reader)` where the points cover enough of the axis to
    publish an intercept, and `(None, per_reader)` where they do not.

    **The publication guard, at the boundary every fitted line crosses.**
    `RESERVE_FIT_MIN_COUNTS` says why three; what this function adds is that the
    rule is enforced once, in front of `_least_squares`, rather than in whichever
    renderer remembered it — the harness has three call sites across two
    families, and a guard carried by one of them lets the instrument account
    publish an intercept in the same sitting the flagless axis refuses one.

    **The slope is the same number either way, which is why it survives.** With
    exactly two distinct abscissae the least-squares slope *is* the secant
    between the two groups' means — `(ȳ₂ − ȳ₁) / (x₂ − x₁)`, whatever the group
    sizes — so the caller printing a secant is printing a reading of the axis,
    not a second arithmetic path that has to be kept in step with the first.
    What is withheld is the intercept and, with it, the residual, both of which
    are the two-term model's claims rather than the data's.

    **It is also where the pool term comes off**, for the same reason: the
    de-pooling is a property of the mechanism rather than of one table, and a
    renderer that forgot it would publish an intercept carrying the charge's own
    bend (`_depooled`). So both terms this returns are the remainder's —
    the cost *outside* the block pool's retention list — and a caller computing
    residuals against them measures its ordinates through `_depooled` too.

    Raises below two distinct abscissae, where `_least_squares` does: a secant
    needs two points as much as a fit does.
    """
    remainder = _depooled(points, unit)
    fixed, slope = _least_squares(remainder)
    if len({x for x, _ in remainder}) < RESERVE_FIT_MIN_COUNTS:
        return None, slope
    return fixed, slope


def _censored_constraint(
    session: Session, figure: str, spec: RunSpec, label: str, token: str, limit: int
) -> str:
    """One OOM-killed leg, written as the constraint it still proves.

    **It is stated as "did not fit", not as `peak > limit`.** What the kill
    reads off the apparatus is that the kernel reaped a process in that cgroup
    ([`../docs/design/runtime-invariants.md`](../docs/design/runtime-invariants.md),
    `RT9`), which says the arrangement did not run inside the allocation — a
    fact about the *arrangement*, needing no argument about what the cgroup
    charged to whom. The register's fourth scope limit does license the step to
    "something reclaim could not free hit the ceiling", clean page cache being
    reclaimed rather than killed for; what it does not license is the last step
    to `peak > limit`, because the charge is the **cgroup's** and the fit is
    over one process's `ru_maxrss`. "The rule's own arrangement did not fit its
    allocation" is already the thing a reserve constant is chosen against, so
    the stronger sentence would buy precision in a quantity the line is not
    about.

    **It bounds the arrangement, not the fitted terms.** It prints under the
    fits because that is where a reader looks for what happened at the limits,
    and the window sentence above — each fit naming the legs it covers — is
    only legible with the legs it does *not* cover adjacent to it. That
    placement is not a claim that this is a point the fit should pass near:
    the reserve constant is chosen from a headroom criterion over surviving
    reps, and a killed leg has no headroom to report.

    **The number beside it is a floor, and says so.** `maxrss_bound_kib` is
    where the wrapper's reading had got to when the process was reaped, so it
    is a lower bound on a peak that was never reached; `seconds_to_kill` is how
    far in, which separates a leg that died opening its first block from one
    that ran most of a scan. The worst rep's pair is printed, since a censored
    leg's reps are not samples of one distribution and their median means
    nothing.

    **Not fitted, deliberately.** Interval censoring is the statistically right
    treatment of a bound like this and the wrong size for a family of
    `RESERVE_LIMITS`' size — see `run_reserve`, which is where that refusal is
    argued. A constraint line is what carries the reading without a second
    fitting technique in the harness.
    """
    killed = session.kills(figure, spec)
    bounds = session.censored_bounds(figure, spec)
    report = session.reported.get(spec.key(figure), {})
    where = (
        f"{report['resolved_jobs']} reader(s) inside "
        f"{_fmt_budget_bytes(int(report['resolved_budget']))}"
        if "resolved_jobs" in report and "resolved_budget" in report
        else "an arrangement it never lived to report"
    )
    # The *worst* rep, as one rep: a peak from one and a duration from another
    # would describe a run that did not happen.
    worst = max((b for b in bounds if b[0] is not None), key=lambda b: b[0], default=None)
    floor = (
        f" The wrapper had reached **{fmt_mib(worst[0])}** when it was reaped"
        + (f", {worst[1]:.1f} s in" if worst[1] is not None else "")
        + " — a floor on the peak, not the peak."
        if worst
        else " The run reported no bound before it died, so the kill is all there is."
    )
    return (
        f"- **{label}** at `-m {token}`: **did not fit {limit / MIB:,.0f} MiB** with {where} "
        f"({killed} rep(s) killed)." + floor
    )


def run_reserve(session: Session) -> str:
    """What a scan holds resident **above** the budget it was told it could have.

    The budget rule is `limit - reserve`, and this is the one number in it.
    Neither other resident figure answers it: `peak-rss` measures the whole
    against nothing, and `rss-attribution` decomposes growth per `COPY` block.

    **It checks a model rather than searching for a constant.**
    `MEMORY_RESERVE` was chosen off five builds' flagless legs against a
    headroom criterion and `MEMORY_UNPOOLED_BOUND` by arithmetic over the same
    readings, so what a sitting can do is fault (`charge_model_problem`). The
    fitted **fixed and per-reader terms** are published beside that check, each
    with its spread, because a table reporting only resident leaves the
    decomposition unstated; neither constant is read off them. This is the
    opposite end of `rss-attribution`, which holds block count as its axis and
    publishes a *slope* against an intercept that is the allocator's baseline.

    **The attribution is introspective, and the black-box legs are its check.**
    A subtraction between whole runs cannot name a term that no leg removes. So
    the account comes from the process reporting its own live bytes and its
    allocator's retention (`_reserve_instrument_specs`), and the `getrusage` axis
    stays as the independent reading it has to agree with — two instruments
    sharing no mechanism, which is the independence
    `.claude/skills/evidence/SKILL.md`'s third rule asks for. The instrument
    build is never the timed binary: it takes an atomic on every allocation, and
    `binary_allocator` refuses it (`docs/design/roadmap.md`, "Attribution is
    introspective; only the gate is blind").

    **Two families, because one arrangement cannot answer both questions.** The
    flagless legs run what a person who states nothing gets, which is the
    arrangement the constant is for and the only one that can say what the
    shipped default holds; the stated legs pin the count and vary the budget,
    which is the only arrangement that can tell a reader's cost from a budget
    byte's -- under discovery `budget = jobs x per_worker` exactly, so there the
    two axes are one axis. Dropping either leaves a figure that cannot answer
    one of the two questions asked of it.

    **The uncapped legs are the ones the shipped default answers to**, because
    the default has to survive the operator who did not set `MALLOC_ARENA_MAX` --
    that being the case that kills the process.

    **A leg the kernel killed is a third cell state, not a missing number**
    (`KILL_TOLERANT`). Its reading is a bound on a peak the process never
    reached, so the cell says so and carries no headroom; and the leg leaves the
    fit **whether or not a rep survived**. `emit` is what then bars the figure
    from publication.

    **What that exclusion is for is enforcement, not the survivorship it looks
    like.** Survivorship is real — the reps that survived are the ones that
    stayed under the ceiling, so a line through them reads low — but it is not
    what earns the rule its place, because two other rules already close the
    path it protects: the constant is not read off a censored table at all, and
    it is chosen from a measured headroom rather than off this fit. Both of
    those are prose. This exclusion is the only *mechanical* thing between a
    censored sitting and a fitted number, which is why it stays. Stating it the
    other way round is how the rule gets reversed by a later session that
    correctly observes the bias argument is not load-bearing.

    *Rejected:* fitting the censored point as an interval-censored observation.
    It is the statistically right answer and the wrong size for a family of
    `RESERVE_LIMITS`' size — it buys precision this table cannot support and puts
    a second fitting technique in the harness.

    **Each fit states the window it covers, in the legs that are in it.** Every
    other clause names what *left* — declined, censored — and a reader holding
    only those has to subtract them from a tuple the table never prints. It
    matters here more than it would elsewhere, because a leg is censored exactly
    when its resident ran closest to its ceiling: dropping every such leg leaves
    a line through the legs that had room, which is
    [`.claude/skills/evidence/SKILL.md`](../.claude/skills/evidence/SKILL.md)
    rule 2's window trap with the sign flipped. Both branches say it — the
    no-fit one too, since "no fit" is a claim about a window as much as a fit
    is.

    **A killed leg is then printed as a constraint, under the fits**
    (`_censored_constraint`). It is the one reading in the table that says
    resident is *above* a number rather than at one, which is worth more to a
    reserve than another interior point; leaving it in `raw.json` unread was
    `KILL_TOLERANT` recording a reading and the renderer discarding it, two
    policies for one number.
    """
    figure = "reserve"
    stated = _reserve_specs()
    flagless = _reserve_flagless_specs()
    mechanism = _reserve_mechanism_specs()
    instrument = _reserve_instrument_specs()
    steps = _reserve_step_specs()
    # Before the first reading, as `rss-attribution` and the `allocator` figure
    # build theirs: a leg discovered missing at rep two has already spent the
    # session's first rep under a different machine state. The instrument build
    # is the one whose absence is silent — a default binary writes no report and
    # the leg fails at the rep, not at the build — so it is made and
    # interrogated here.
    if instrument:
        ensure_instrument_binary(session.cfg, session.log)
    # The serial baseline, from `peak-rss` where this sitting took it, and
    # dropped from the interleave below rather than left in it — a spec the
    # sitting already holds would spend a reading to overwrite one.
    note = share_readings(session, figure)
    specs = [
        *flagless,
        *instrument,
        *(spec for _, spec in mechanism),
        *steps,
        *stated,
        _RESERVE_BASELINE,
    ]
    session.sweep(
        figure, [s for s in specs if not session.has(figure, s)], session.cfg.reps(3)
    )

    per_rep: list[str] = []

    def rss(spec: RunSpec) -> list[float]:
        readings = session.get_rss(figure, spec)
        killed = session.kills(figure, spec)
        per_rep.append(
            f"- {spec.label}: "
            + (", ".join(fmt_mib(v) for v in readings) or "no surviving rep")
            + (f" · **{killed} rep(s) OOM-killed**" if killed else "")
        )
        return readings

    def arrangement(spec: RunSpec) -> tuple[int, int] | None:
        """The worker count and budget this leg's own run reported, or `None`
        where no rep of it got as far as reporting one."""
        report = session.reported.get(spec.key(figure), {})
        if "resolved_jobs" not in report or "resolved_budget" not in report:
            return None
        return int(report["resolved_jobs"]), int(report["resolved_budget"])

    def resolved(spec: RunSpec) -> tuple[int, int]:
        """The worker count and budget this leg's own run reported.

        Read back rather than computed: the count a flagless run resolves is
        `Discovered::resolve`'s answer to the allocation, and a harness that
        predicted it would be a second authority on the rule under test.

        A leg with a surviving rep that reported nothing is an error; a leg with
        none at all is a censored cell and its caller asks `arrangement`
        instead, because the mode report is printed before the scan opens and a
        run killed early enough may never have reached it."""
        got = arrangement(spec)
        if got is None:
            raise RuntimeError(
                f"{spec.label}: the run reported no resolved arrangement, so the fit has no "
                "reader count — `scan started` is where it comes from (`parse_resolution`)"
            )
        return got

    # -- the flagless axis: what the shipped default resolves and holds -----
    limits = dict(RESERVE_LIMITS)
    by_flagless = {(spec.input, spec.memory): spec for spec in flagless}
    flagless_rows, fits = [], []
    for token, limit in RESERVE_LIMITS:
        cells = [f"`-m {token}`"]
        for name, _, unit in RESERVE_FLAGLESS_INPUTS:
            spec = by_flagless[(name, token)]
            readings = rss(spec)
            killed = session.kills(figure, spec)
            if not readings:
                # The third cell state: every rep censored, so there is no
                # number to print. What the leg still says is the arrangement
                # it resolved before it died, which is the half of the reading
                # the kill did not destroy.
                got = arrangement(spec)
                cells.append(
                    f"**OOM-killed**, {killed} rep(s) · "
                    + (
                        f"{got[0]}r, {_fmt_budget_bytes(got[1])}"
                        if got
                        else "arrangement never reported"
                    )
                    + " · head —"
                )
                continue
            jobs, budget = resolved(spec)
            worst = max(readings) * 1024
            head = (limit - worst) / limit * 100
            # Whether this leg took the block path at all is the budget
            # against what one reader of *this file* costs, which is
            # `block_path_afforded` — `BlockCache::affordable` exactly. A
            # declined leg is not on the line the fit is over, and the fit
            # leaves it out by name.
            #
            # **Every cell names its path, not only the declined ones.** The two
            # arrangements hold different things — the streaming fallback keeps
            # no block slots at all — so a column mixing them is two series
            # printed as one, and an unmarked cell cannot be told from a cell
            # nobody checked.
            block_path = block_path_afforded(unit, budget)
            cells.append(
                f"{fmt_mib_median_spread(readings)} · {jobs}r, {_fmt_budget_bytes(budget)} · "
                f"head {head:.1f}%"
                + (" · *block path*" if block_path else " · *streaming*")
                # A partly-censored leg's surviving reps are the ones that did
                # not reach the ceiling, so its median understates and its worst
                # is not the worst. Said in the cell, and kept out of the fit.
                + (f" · **{killed} rep(s) OOM-killed**" if killed else "")
            )
        flagless_rows.append(cells)
    flagless_table = md_table(
        ["Allocation", *(label for _, label, _ in RESERVE_FLAGLESS_INPUTS)], flagless_rows
    )

    constraints: list[str] = []
    for name, label, unit in RESERVE_FLAGLESS_INPUTS:
        points, declined, censored_legs = [], [], []
        for token, limit in RESERVE_LIMITS:
            spec = by_flagless[(name, token)]
            readings = session.get_rss(figure, spec)
            # A censored leg is out of the fit whether or not a rep survived:
            # that exclusion is the one mechanical thing between a censored
            # sitting and a fitted number (`run_reserve`). It is not out of the
            # *figure* — `_censored_constraint` is what it still says, printed
            # under the fits.
            if session.kills(figure, spec):
                censored_legs.append(token)
                constraints.append(
                    _censored_constraint(session, figure, spec, label, token, limit)
                )
                continue
            jobs, budget = resolved(spec)
            if not block_path_afforded(unit, budget):
                declined.append(token)
                continue
            points.append((token, jobs, readings))
        censored_tail = (
            f". `{'`, `'.join(censored_legs)}` was OOM-killed and is out of the fit — its "
            "reading is a bound on a peak the process never reached, not a point on the line; "
            "what it still proves is below"
            if censored_legs
            else ""
        )
        # The window, said as the legs that are *in*. Every other clause here
        # names what left — declined, censored — and a reader who has only
        # those has to subtract them from a tuple they cannot see.
        window = ", ".join(f"`{t}` at {j}r" for t, j, _ in points) or "no leg at all"
        declined_tail = (
            f". `{'`, `'.join(declined)}` declined the block path and is not in the line"
            if declined
            else ""
        )
        counts = {jobs for _, jobs, _ in points}
        if len(counts) < 2:
            # Not even a secant: one abscissa is a point, and a line through a
            # point is an intercept asserted as a measurement.
            fits.append(
                f"- **{label}**: no line — "
                + (
                    f"the block path is declined at {', '.join(f'`{t}`' for t in declined)} and "
                    if declined
                    else ""
                )
                + f"what is left is {window}, which is {len(counts)} distinct reader count(s), "
                "under the 2 any line through them needs"
                + censored_tail
                + "."
            )
            continue
        # Every line here is over the **remainder**: `_fit_or_secant` takes the
        # block pool's retention list off each ordinate first, that term being
        # known before the sitting and the reason a single straight line across
        # the axis reads its intercept ≈49 MiB high at 24 MiB blocks and
        # ≈421 MiB high at 128 (`_depooled`). Only block-path legs reach
        # here, which is what makes the subtraction well defined.
        fixed, per_reader = _fit_or_secant(
            [(j, median(r) / 1024) for _, j, r in points], unit
        )
        # The band is the same line taken over the per-rep extremes rather than
        # the medians: a term's spread is what the reps permit it to be, and a
        # single residual says nothing about which of the two terms moved.
        band = [
            _fit_or_secant([(j, pick(r) / 1024) for _, j, r in points], unit)
            for pick in (min, max)
        ]
        if fixed is None:
            # The secant. `RESERVE_FIT_MIN_COUNTS` is what withholds the
            # intercept, and the residual goes with it: a two-term model has
            # none at two points, so printing one would be printing zero as
            # though it were a reading.
            ends = " → ".join(
                ", ".join(f"`{t}`" for t, j, _ in points if j == count) + f" at {count}r"
                for count in (min(counts), max(counts))
            )
            fits.append(
                f"- **{label}**: no intercept — a **secant**, not a fit: a reader "
                f"**{per_reader:,.1f} MiB** outside the pool "
                f"({band[0][1]:,.1f}–{band[1][1]:,.1f}) over "
                f"{ends}. The {len(points)} leg(s) left cover {len(counts)} distinct reader "
                f"count(s), under the {RESERVE_FIT_MIN_COUNTS} a two-term model needs before "
                "its intercept is a reading, so none is published and no residual with it"
                + declined_tail
                + censored_tail
            )
            continue
        # Measured through `_depooled` as well, so the residual is a residual of
        # the line that was actually fitted rather than of a line over a
        # quantity nobody fitted.
        residual = max(
            abs(y - (fixed + per_reader * j))
            for (j, y) in _depooled([(j, median(r) / 1024) for _, j, r in points], unit)
        )
        fits.append(
            f"- **{label}**: outside the pool — fixed **{fixed:,.0f} MiB** "
            f"({band[0][0]:,.0f}–{band[1][0]:,.0f}), a reader **{per_reader:,.1f} MiB** "
            f"({band[0][1]:,.1f}–{band[1][1]:,.1f}), over "
            f"the {len(points)} leg(s) it covers — {window}; residuals "
            f"reach ±{residual:,.0f} MiB"
            + declined_tail
            + censored_tail
        )

    # -- the charge against what was held -----------------------------------
    #
    # The model check, per cell. Every other reading in this figure is a number
    # somebody then has to reason about; this is the arithmetic done in the
    # renderer, at the cell, against a criterion registered before the sitting
    # (`charge_model_problem`). Done by hand over a sitting's readings, it dies
    # with the session that does it, which is why it is here.
    #
    # **It reports; it neither raises nor bars publication — two levers, not
    # one.** `KILL_TOLERANT` pulls both: the sitting survives a kill *and*
    # `emit` refuses to publish the figure that lost the leg. So the precedent
    # settles the first half here and says nothing about the second, which
    # turns on what a cell is. A censored cell is a bound on a peak the process
    # never reached, so the table above it describes an arrangement nobody
    # measured; every cell here is a real reading of a real run, and what a
    # refutation falsifies is the library's claim about those readings rather
    # than the readings. Barring publication on it would leave
    # `measurements.md` able to carry only tables that agree with the library,
    # which is the opposite of what it is for. So the verdict is written in
    # bands (`BAND_STANCE`): an over-bill or a cell above the reserve refutes
    # the model and says why, and a cell between the bound and the reserve is a
    # finding that re-derives the bound from the sitting's own remainders.
    model_rows, model_faults, model_unnamed = [], [], []
    for name, label, unit in RESERVE_FLAGLESS_INPUTS:
        for token, _limit in RESERVE_LIMITS:
            spec = by_flagless[(name, token)]
            readings = session.get_rss(figure, spec)
            # A censored leg's reading is a bound on a peak never reached, so it
            # is not a `held` this model may be evaluated at — the same
            # exclusion the fit above makes, for the same reason.
            if session.kills(figure, spec) or not readings:
                continue
            jobs, budget = resolved(spec)
            # A declined leg ran the streaming fallback, which holds none of
            # these terms. Excluded by what the leg *did*, read off its own
            # reported budget, rather than by which token it carries.
            if not block_path_afforded(unit, budget):
                continue
            held = max(readings) * 1024
            billed, floor, unnamed = charge_model(unit, jobs, held)
            fault = charge_model_problem(unit, jobs, held)
            model_unnamed.append(unnamed)
            if fault:
                model_faults.append((fault, f"- **{label}** at `-m {token}`: {fault.text}"))
            model_rows.append(
                [
                    f"{label}, `-m {token}`",
                    f"{jobs}r",
                    _fmt_budget_bytes(billed),
                    _fmt_budget_bytes(floor) if floor else "—",
                    fmt_mib(held / 1024),
                    _fmt_budget_bytes(unnamed),
                    # The band, not a bare "refuted": which line a cell crossed
                    # is what says whether it refutes the model.
                    "met" if fault is None else f"**{fault.band}**",
                ]
            )
    model_table = md_table(
        [
            "Leg",
            "Readers",
            "Billed",
            "of which pool list",
            "Worst rep held",
            "Unnamed",
            "Criterion",
        ],
        model_rows,
    )
    if model_faults:
        refuting = [line for fault, line in model_faults if fault.refutes]
        inside = [line for fault, line in model_faults if not fault.refutes]
        bands = []
        if refuting:
            # One clause per band actually hit, rather than one sentence over
            # the whole stanza: the bands refute for different reasons, and one
            # sentence asserts a single band's reason over every cell in it.
            hit = sorted({fault.band for fault, _ in model_faults if fault.refutes})
            bands.append(
                "**The model is refuted, and by these cells:**\n\n"
                + "\n".join(refuting)
                + f"\n\nThe `{BAND_BOUND}` band is the one a sitting discharges itself, and each "
                "cell above is in a band it is not: "
                + "; ".join(f"`{band}`, where {band_refutes(band)}" for band in hit)
                + ". The table publishes all the same: a refutation is a reading of the library, "
                "and only a killed leg keeps a figure out of the document."
            )
        if inside:
            # The re-derivation runs over every evaluated cell rather than over
            # the faulting ones — the bound is what covers the worst remainder
            # the sitting saw, and the cells under it are as much evidence of
            # that as the cells over it — but only where nothing bars.
            #
            # Withheld from the whole sitting wherever the model is refuted,
            # rather than from the refuting family alone: a refuted cell
            # describes an arrangement the rule did not keep inside its
            # allocation, or a charge nothing holds, and a bound is a term of
            # the model that sitting has just contradicted.
            tail = ""
            if any(fault.band == BAND_BOUND for fault, _ in model_faults):
                if refuting:
                    tail = (
                        "\n\nNo bound is re-derived from this sitting: the model is refuted "
                        "in it, so its worst remainder is not a reading a term of that model "
                        "may be sized to."
                    )
                else:
                    worst = max(model_unnamed)
                    tail = (
                        "\n\nThe worst remainder over the "
                        f"{len(model_rows)} evaluated cell(s) is {_fmt_budget_bytes(worst)}, so "
                        "these readings re-derive `MEMORY_UNPOOLED_BOUND` at "
                        f"**{_fmt_budget_bytes(rederived_unpooled_bound(worst))}** — the "
                        f"smallest {_fmt_budget_bytes(UNPOOLED_BOUND_STEP)} step that covers "
                        "it, which is the shipped bound's own arithmetic re-done over this "
                        "sitting and not a re-take."
                    )
            bands.append(
                "**Inside the rule, and so a finding rather than a refutation** — the "
                f"`{BAND_BOUND}` band is the one a sitting discharges itself, and every cell "
                "here is in it:\n\n"
                + "\n".join(inside)
                + tail
            )
        model_verdict = "\n\n".join(bands)
    elif model_rows:
        model_verdict = (
            "**The model holds at every cell above**: nothing here crosses any of its three "
            "lines."
        )
    else:
        # Not the same claim as the one above, and the difference is the whole
        # value of the check: every leg declined the block path or was censored,
        # so the model was evaluated nowhere and says nothing about the charge.
        model_verdict = (
            "**The model was evaluated at no cell**: every flagless leg either declined the "
            "block path or was censored, so nothing here bears on the charge."
        )

    # -- the attribution: what the process says it held ---------------------
    #
    # This is the half no subtraction between whole runs produces. Every column
    # below is a number the process reported about itself, and the two families
    # do not cover the same memory: `live_*` is what passed through Rust's
    # `GlobalAlloc`, `mallinfo_*`/`malloc_*` are glibc's view of the whole
    # process, C included. The renderer keeps them apart and adds the one term
    # that sits between them — `liblzma`'s per-reader dictionary — by hand.
    def reported_median(spec: RunSpec, key: str) -> float | None:
        """One instrument field's median over this leg's surviving reps, or
        `None` where the leg reported none — which is the kill, since the report
        is written at exit."""
        values = [float(r[key]) for r in session.instrument_reports(figure, spec) if key in r]
        return median(values) if values else None

    instrument_rows, account_rows, account_points, checks = [], [], [], []
    # The same legs' readings for a two-heap report, one column a reading, each
    # headed with the heap it covers and none subtracted from another: readings
    # are not a model, so the reason the account is withheld does not reach
    # them (`docs/status/history/2026-10-05.md`).
    two_heap_rows = []
    for spec in instrument:
        readings = rss(spec)
        killed = session.kills(figure, spec)
        got = arrangement(spec)
        heap_max = reported_median(spec, "malloc_system_max")
        live_peak = reported_median(spec, "live_peak_bytes")
        if heap_max is None or live_peak is None or not readings:
            # A censored leg, or one whose reps all died before exit. It carries
            # the same third cell state the flagless axis uses, and enters
            # neither table's arithmetic.
            instrument_rows.append(
                [spec.label, "—", f"**OOM-killed**, {killed} rep(s)", "—", "—", "—", "—", "—"]
            )
            two_heap_rows.append(
                [spec.label, "—", f"**OOM-killed**, {killed} rep(s)", *(["—"] * 7)]
            )
            continue
        readers = got[0] if got else 0
        resident = median(readings) * 1024
        fordblks = reported_median(spec, "mallinfo_fordblks") or 0.0
        hblkhd = reported_median(spec, "mallinfo_hblkhd") or 0.0
        heaps = reported_median(spec, "malloc_heaps") or 0.0
        instrument_rows.append(
            [
                spec.label,
                f"{readers}r" if got else "—",
                fmt_mib_median_spread(readings),
                _fmt_budget_bytes(heap_max),
                _fmt_budget_bytes(live_peak),
                f"{heaps:.0f}",
                _fmt_budget_bytes(fordblks),
                _fmt_budget_bytes(hblkhd),
            ]
        )

        def reported_bytes(key: str) -> str:
            """A reading the report may lack, as bytes or a dash: a report
            from before mimalloc's statistics carries none of its keys."""
            value = reported_median(spec, key)
            return "—" if value is None else _fmt_budget_bytes(value)

        two_heap_rows.append(
            [
                spec.label,
                f"{readers}r" if got else "—",
                fmt_mib_median_spread(readings),
                _fmt_budget_bytes(live_peak),
                reported_bytes("mimalloc_committed_peak_bytes"),
                reported_bytes("mimalloc_reserved_peak_bytes"),
                _fmt_budget_bytes(heap_max),
                f"{heaps:.0f}",
                _fmt_budget_bytes(fordblks),
                _fmt_budget_bytes(hblkhd),
            ]
        )
        # The account, term by term. Each term is a high-water **of its own**,
        # so the sum bounds any single instant rather than describing one, and
        # the remainder is a residual of maxima. Said in the prose below, and
        # the reason no line here claims an identity.
        dictionaries = readers * XZ_DICT_BYTES
        unattributed = heap_max - live_peak - dictionaries
        account_rows.append(
            [
                spec.label,
                fmt_mib(resident / 1024),
                _fmt_budget_bytes(resident - heap_max),
                _fmt_budget_bytes(live_peak),
                _fmt_budget_bytes(dictionaries),
                _fmt_budget_bytes(unattributed),
                (
                    f"{fordblks / unattributed * 100:.0f}%"
                    if unattributed > 0
                    else "—"
                ),
            ]
        )
        if not killed and got:
            account_points.append(
                (readers, live_peak, unattributed, fordblks, spec.label, got[1])
            )
        # The check: the same arrangement measured black-box. A term the
        # instrument names has to show up in the sum the wrapper measures, and
        # the two instruments share no mechanism — which is the independence
        # two black-box sittings agreeing with each other never have.
        #
        # **The arrangement is the exact half and resident is the approximate
        # one.** Whether the instrument build resolved the same reader count and
        # budget is a yes or no, and a no means the attribution describes a run
        # the shipped build does not make. Resident cannot be held to the
        # shipped leg's own spread — this is a different binary, so its text and
        # its allocator bookkeeping are its own — so the tolerance is stated
        # rather than read off three reps of something else.
        black_box = by_flagless.get((spec.input, spec.memory))
        if black_box is not None and spec.command == black_box.command:
            theirs = session.get_rss(figure, black_box)
            if theirs:
                delta = median(readings) - median(theirs)
                off = abs(delta) / median(theirs) * 100
                same = arrangement(black_box) == got
                checks.append(
                    f"- `-m {spec.memory}`: the instrument build resolved "
                    + (
                        f"the shipped build's arrangement, {readers} reader(s)"
                        if same
                        else "**a different arrangement** from the shipped build's, so what it "
                        "attributes is not the run beside it"
                    )
                    + f", and held {fmt_mib(median(readings))} against "
                    f"{fmt_mib_median_spread(theirs)} — {fmt_rss_delta(delta)}, {off:.0f}% "
                    + (
                        f"apart, inside the {INSTRUMENT_TOLERANCE_PCT}% a second build of the "
                        "same source is allowed"
                        if off <= INSTRUMENT_TOLERANCE_PCT
                        else f"apart, **outside** the {INSTRUMENT_TOLERANCE_PCT}% a second "
                        "build of the same source is allowed, so the sum the instrument "
                        "decomposes is not the sum the gate measured"
                    )
                    + "."
                )

    instrument_table = md_table(
        [
            "Leg",
            "Readers",
            "Peak RSS",
            "glibc heap high-water",
            "Rust live high-water",
            "Arenas",
            "Freed and held at exit",
            "mmap-backed at exit",
        ],
        instrument_rows,
    )
    two_heap_table = md_table(
        [
            "Leg",
            "Readers",
            "Peak RSS — the process",
            "Live high-water — Rust, what passed through `GlobalAlloc`",
            "mimalloc committed high-water — the Rust heap",
            "mimalloc reserved high-water — the Rust heap's address space",
            "glibc heap high-water — C `malloc` alone",
            "glibc arenas — C",
            "Freed and held at exit — glibc, C",
            "mmap-backed at exit — glibc, C",
        ],
        two_heap_rows,
    )
    account_table = md_table(
        [
            "Leg",
            "Peak RSS",
            "RSS − heap high-water",
            "Rust live high-water",
            "Decoder dictionaries",
            "Unattributed",
            "Covered by `fordblks`",
        ],
        account_rows,
    )

    # The program's own two terms, read off the counter rather than off a
    # difference of resident sets: `live_peak = fixed + readers x per_reader`,
    # **outside the block pool's retention list**, which `_fit_or_secant` takes
    # off first — the counter sees those buffers like any other Rust allocation,
    # so the charge's kink at `POOL_DEPTH` is in this series exactly as it is in
    # the resident one (`_depooled`). Evaluated inside its own window, at
    # the smallest arrangement, because an intercept is a physical quantity only
    # where the fit still holds where the mechanism is simplest.
    #
    # **It crosses the same publication guard the flagless axis does**, through
    # `_fit_or_secant`. This family's coverage is a *per-sitting* property where
    # the other's is a property of the axis: `RESERVE_INSTRUMENT_LIMITS` is
    # derived from `RESERVE_LIMITS` and cannot be narrowed on its own, but
    # `account_points` drops every killed leg and a kill takes the high-memory
    # end, so two kills leave two counts.
    #
    # **A leg that declined the block path is dropped from the line by name**,
    # as the flagless family drops one: it holds no block pool, so there is no
    # retention list to subtract and it is not on the same mechanism's line. No
    # registered limit declines at this block size today, which is what makes
    # this a guard rather than a filter.
    line_points = [
        pt for pt in account_points if block_path_afforded(RESERVE_MECHANISM_UNIT, pt[5])
    ]
    live_counts = {r for r, *_ in line_points}
    if len(live_counts) >= 2:
        live_fixed, live_per_reader = _fit_or_secant(
            [(r, p / MIB) for r, p, _, _, _, _ in line_points], RESERVE_MECHANISM_UNIT
        )
        if live_fixed is None:
            # Named legs, not bare counts: the secant is a difference between
            # two runs, and a reader who cannot see which two cannot re-take it.
            ends = " → ".join(
                ", ".join(f"`{lab}`" for r, _, _, _, lab, _ in line_points if r == count)
                + f" at {count} reader(s)"
                for count in (min(live_counts), max(live_counts))
            )
            live_line = (
                f"**What a reader costs the program**, as a **secant** and not a fit: "
                f"**{live_per_reader:,.1f} MiB** a reader outside the pool over {ends}, "
                f"across the {len(line_points)} leg(s) that survived. Those cover "
                f"{len(live_counts)} distinct reader count(s), under the "
                f"{RESERVE_FIT_MIN_COUNTS} a two-term model needs before its intercept is a "
                "reading, so no fixed term and no residual are published."
            )
        else:
            smallest = min(line_points)
            at_smallest = live_fixed + live_per_reader * smallest[0]
            # The check `evidence` rule 2 asks for, and it is made against the
            # **remainder** at that leg rather than against its raw high-water,
            # because the remainder is what was fitted.
            held_smallest = _depooled(
                [(smallest[0], smallest[1] / MIB)], RESERVE_MECHANISM_UNIT
            )[0][1]
            live_line = (
                f"**What the program itself held outside the block pool**, least squares over "
                f"the {len(line_points)} leg(s) that survived, the pool's retention list "
                f"subtracted first because it is known before the sitting and bends the line "
                f"at `POOL_DEPTH`: fixed "
                f"**{live_fixed:,.0f} MiB**, a reader **{live_per_reader:,.1f} MiB**. At the "
                f"smallest arrangement in its own window — {smallest[0]} reader(s) — it "
                f"predicts {at_smallest:,.0f} MiB against {held_smallest:,.0f} MiB "
                f"measured, a residual of {abs(at_smallest - held_smallest):,.0f} MiB."
            )
        # The account against the model, which is the point of taking it: what a
        # reader costs the program, plus the C dictionary the counter is blind
        # to, against what `XzSource` bills a sub-stream. Computed here rather
        # than left to a reader, because it is the one comparison that says
        # whether the charge the budget rule divides by is the charge a reader
        # actually is. **It reads the slope alone**, which is why a secant keeps
        # it: refusing the whole line would withdraw this comparison in exactly
        # the censored sitting that needs it.
        #
        # **The two sides are commensurable only because the slope is the
        # remainder's.** `reader_bytes` is the per-worker term *without* the
        # pool's retention list — that list is `charge_bytes`' second term —
        # so a slope still carrying the list would be compared against a bill
        # that does not, which is a unit a reader of disagreement above
        # `POOL_DEPTH` (`_depooled`).
        measured_reader = live_per_reader * MIB + XZ_DICT_BYTES
        billed = reader_bytes(RESERVE_MECHANISM_UNIT)
        live_line += (
            f" Against the charge: {live_per_reader:,.1f} MiB of Rust outside the pool plus "
            f"the {_fmt_budget_bytes(XZ_DICT_BYTES)} dictionary is "
            f"**{_fmt_budget_bytes(measured_reader)}** a reader, where "
            f"`BlockCache::reader_bytes` bills {_fmt_budget_bytes(billed)} a reader — "
            f"{measured_reader / billed * 100:.0f}% of it."
        )
    else:
        live_line = (
            "**No line over the program's own high-water**: the surviving block-path legs "
            f"resolved {len(live_counts)} distinct reader count(s), and a line "
            "through one point is an intercept asserted as a measurement."
        )

    # A name, or an explicit no-name with the follow-up that would supply one.
    # The criterion is stated with the answer rather than applied silently: the
    # account's `Unattributed` column is *named* where glibc's own
    # freed-and-held figure covers at least half of it at every surviving leg.
    # A leg whose column is not positive has nothing to cover and counts as
    # covered, which is why the sentence qualifies "every leg" by sign.
    #
    # **It names that column and no other.** The charge table's `Unnamed` is
    # the bill subtracted from a black-box worst rep, a different subtraction on
    # a different build, and `fordblks` is not a coverage of it.
    covered = [
        (f / u if u > 0 else 1.0) for _, _, u, f, _, _ in account_points
    ]
    if covered and min(covered) >= 0.5:
        verdict = (
            "**The remainder has a name**: glibc's own `fordblks` — bytes the program freed, "
            "the allocator kept and the kernel still counts resident — covers at least half of "
            "the account's `Unattributed` column at every surviving leg where that column is "
            f"positive (worst {min(covered) * 100:.0f}%). That is the dynamic "
            "mmap threshold's signature and not program structure: a block-sized buffer stops "
            "being mmap-backed after the first one is freed, and the arena it lands in never "
            "returns it, which is why `hblkhd` reads zero on a run that decoded blocks "
            "throughout. It is not a coverage of the charge table's `Unnamed` column, which "
            "subtracts the bill from a black-box worst rep."
        )
    elif covered:
        verdict = (
            "**The remainder has no name here**, in those words: `fordblks` covers as little as "
            f"{min(covered) * 100:.0f}% of the account's `Unattributed` column, so what is left is neither the program's own live "
            "bytes, the decoder's dictionaries, nor allocator retention as glibc reports it. "
            "What would name it is `cd scripts && uv run measure.py --heaptrack-recipe`, which "
            "attributes every `malloc` to a call stack — on a `system` build, C and Rust alike — and a `--diff` "
            "between two of these arrangements would name the site rather than the term."
        )
    else:
        verdict = (
            "**The remainder has no name here**, in those words: every instrument leg was "
            "censored, so nothing was reported to attribute."
        )

    # -- the mechanism legs, against the flagless leg they differ from one ---
    reference = by_flagless[(RESERVE_MECHANISM_INPUT, RESERVE_MECHANISM_LIMIT)]
    ref_readings = session.get_rss(figure, reference)
    ref_killed = session.kills(figure, reference)
    ref_arrangement = arrangement(reference)
    ref_jobs = ref_arrangement[0] if ref_arrangement else 0

    def mech_cell(readings: Sequence[float], killed: int) -> str:
        """One mechanism leg's reading, or the fact that it was killed."""
        if not readings:
            return f"**OOM-killed**, {killed} rep(s)"
        return fmt_mib_median_spread(readings) + (
            f" · **{killed} rep(s) OOM-killed**" if killed else ""
        )

    mech_rows = [
        [
            "the reference — arenas uncapped",
            mech_cell(ref_readings, ref_killed),
            "—",
        ]
    ]
    for label, spec in mechanism:
        readings = rss(spec)
        killed = session.kills(figure, spec)
        # A delta against a censored reference, or from a censored leg, is a
        # difference of two things at least one of which is a bound. Said,
        # never printed as a number.
        if not readings or not ref_readings:
            against = "— · a censored leg is a bound, not a number"
        else:
            delta = median(readings) - median(ref_readings)
            against = f"{fmt_rss_delta(delta)} ({delta / median(ref_readings) * 100:+.0f}%)"
        mech_rows.append([label, mech_cell(readings, killed), against])
    mech_table = md_table(["Leg", "Peak RSS", "Against the reference"], mech_rows)

    # -- the path step ------------------------------------------------------
    step_rows, step_medians = [], []
    for spec in steps:
        readings = rss(spec)
        step_medians.append(median(readings))
        budget = int(spec.command.removeprefix(RESERVE_STEP_FAMILY))
        step_rows.append(
            [
                f"`--memory {stated_allowance(budget)}` — {_fmt_budget_bytes(budget)} of buffers"
                + (
                    " — one reader afforded"
                    if block_path_afforded(RESERVE_MECHANISM_UNIT, budget)
                    else " — **one byte short**, block decode declined"
                ),
                fmt_mib_median_spread(readings),
            ]
        )
    step_table = md_table(["Leg", "Peak RSS"], step_rows)

    # -- the stated-budget axis ---------------------------------------------
    by_key = {
        (spec.input, spec.command.removeprefix(RESERVE_FAMILY).rpartition("-")[0],
         int(spec.command.rpartition("-")[2])): spec
        for spec in stated
    }
    rows, worst_stated = [], {}
    for name, source in RESERVE_INPUTS:
        for token, _, arena in RESERVE_ARENAS:
            cells = [f"{source}, {arena}"]
            for budget in RESERVE_BUDGETS:
                spec = by_key[(name, token, budget)]
                readings = rss(spec)
                reserve = median(readings) - budget / 1024
                worst_stated[token] = max(worst_stated.get(token, reserve), reserve)
                cells.append(f"{fmt_mib_median_spread(readings)} · {fmt_rss_delta(reserve)}")
            rows.append(cells)
    table = md_table(
        ["Leg", *(_fmt_budget(b) + " stated" for b in RESERVE_BUDGETS)], rows
    )

    base = rss(_RESERVE_BASELINE)
    base_reserve = median(base) - (LIBRARY_DEFAULT_BUDGET / 1024)
    # **The account above models one heap**: glibc's, holding the counter's
    # live bytes and `liblzma`'s dictionaries alike, which is what an
    # instrument counting in front of glibc reported. One counting in front of
    # mimalloc reports two (`mimalloc_scope`) — the Rust heap apart, glibc's
    # holding only what reached C `malloc` — so every subtraction above takes
    # the counter's bytes out of a heap that never held them. Withheld rather
    # than printed, and said in those words; the counter's own line stands,
    # being no allocator's. The two-heap account is `KD34`'s owner's to draw.
    two_heaps = any(
        "mimalloc_scope" in report
        for spec in instrument
        for report in session.instrument_reports(figure, spec)
    )
    if two_heaps:
        attribution = (
            "**What the process says it held**, on the introspection build running the "
            "same flagless shape as the axis above. These are this figure's instrument legs, "
            "declared in its register entry; the build takes an atomic on every allocation and "
            "`pgdt --version` names it, so it is never timed. **It counted in front of "
            "mimalloc, so the process held two heaps**: the Rust heap is mimalloc's, and "
            "glibc's holds only what reached C `malloc`. The account this figure draws — the "
            "counter's high-water and the decoder dictionaries subtracted from glibc's — "
            "subtracts the Rust heap from a heap that never held it, so it is **withheld** "
            "here rather than printed; the two-heap account is `KD34`'s owner's to draw. "
            "**The readings stand**, a reading being no model: each column below is headed "
            "with the memory it covers, and none is subtracted from another. mimalloc's "
            "`committed` counts an arena's slices from when it first hands them out, touched "
            "or not, so it is no resident reading, and `reserved` is address space:\n\n"
            + two_heap_table
            + "\n\nWhat stands of the account is the counter's own line, which no allocator "
            "moves:\n\n"
            + live_line
        )
    else:
        attribution = (
            "**What the process says it held**, on the introspection build running the "
            "same flagless shape as the axis above. These are this figure's instrument legs, "
            "declared in its register entry; the build takes an atomic on every allocation and "
            "`pgdt --version` names it, so it is never timed. The "
            "two families do not cover the same memory — the Rust column is what passed through "
            "`GlobalAlloc`, every glibc column is the whole process, C included — and the gap "
            "between them is decoder working set plus bookkeeping plus retention, never retention "
            "alone:\n\n"
            + instrument_table
            + "\n\n**The account, term by term.** Each term is a high-water *of its own*, so the "
            "row bounds any single instant rather than describing one, and the last column is a "
            "residual of maxima. `liblzma` allocates through C `malloc`, so its per-reader "
            f"dictionary — {XZ_DICT_BYTES:,} bytes, read off a stack rather than modelled — is "
            "added back by hand: the counter cannot see it and glibc cannot separate it, and a "
            "decomposition that subtracted the two families would charge it to retention. Two "
            "columns can leave the range a resident term would keep, and both say the same thing: "
            "`RSS − heap high-water` goes **negative** where the arenas' summed high-water exceeds "
            "peak RSS, which it may, because `system max` is address space each arena obtained and "
            "no two arenas reach their maxima at once; and `fordblks` covers **more** than the "
            "remainder where retention at exit is larger than the gap between two maxima taken at "
            "different instants. Neither is an error in the reading — both are what "
            "non-simultaneity looks like, and they are why the last column is a share rather than "
            "a subtraction anyone should carry forward:\n\n"
            + account_table
            + "\n\n"
            + live_line
            + "\n\n"
            + verdict
        )

    return (
        "**What a flagless scan resolves, and what it then holds.** Each cell is peak resident "
        "set, the worker count and budget the run itself reported, and what the *worst* rep left "
        "of the allocation — which is the number a cgroup's killer reads, where the median is "
        "context. Nothing is stated on these legs: the container's limit is the whole input, "
        "which is why it is the axis.\n\n**Each cell also names the path it ran**, because the "
        "two hold different things and a column mixing them is two series printed as one: a leg "
        "takes the block path only where the budget it resolved affords one block-decoding "
        "reader of that file — `BlockCache::affordable`, which charges that reader's share "
        "of the pool's retention list with it, so the line is "
        f"{_fmt_budget_bytes(charge_bytes(RESERVE_FLAGLESS_INPUTS[0][2], 1))} at "
        f"{RESERVE_FLAGLESS_INPUTS[0][1]} and "
        f"{_fmt_budget_bytes(charge_bytes(RESERVE_FLAGLESS_INPUTS[-1][2], 1))} at "
        f"{RESERVE_FLAGLESS_INPUTS[-1][1]}. A *streaming* cell is a reading of the fallback "
        "decoder and belongs to no fit and no charge below.\n\n"
        + flagless_table
        + "\n\n**Resident against the reader count**, least squares over the legs that took the "
        "block path, the band being the same line over the per-rep extremes — a check on the "
        "charge's shape, not what either constant is read off. **Both terms are "
        "what a leg held *outside* the block pool's retention list**: that term is "
        "`(POOL_DEPTH.max(jobs) − 1) × unit`, known before the sitting and mirror-checked "
        "against the library's own constants, and it is constant below "
        f"`POOL_DEPTH` = {LIBRARY_POOL_DEPTH} and grows by a unit a reader above — so a leg's "
        "resident rises by one unit a reader at the bottom of this axis and by two at the top, "
        "and both families straddle the bend. Subtracting it and fitting the remainder is how a "
        "known term stays out of the intercept; fitting one straight line across the kink "
        "instead reads the intercept ≈49 MiB high at 24 MiB blocks and ≈421 MiB high at 128, "
        "and the slope 31% low. Each line names the "
        "window it covers, since a leg is censored exactly when its resident ran closest to its "
        "ceiling and a fit over what survives is a fit over the legs that had room. A family "
        f"covering fewer than {RESERVE_FIT_MIN_COUNTS} distinct reader counts publishes a "
        "**secant** instead — the slope between its two ends, with no fixed term and no "
        "residual: the remainder still has two terms, so below that the residual printed beside "
        "it is zero by construction rather than a reading, while the slope is a difference the "
        "axis measured:\n\n"
        + "\n".join(fits)
        + (
            "\n\n**What the killed legs still prove**, stated as constraints and **not** fitted: "
            "a kill is the one reading here that says resident is *above* a number rather than "
            "at one, and interval censoring is the right treatment of it at the wrong size "
            f"for {len(RESERVE_LIMITS)} legs. This is the end of the axis the fit above does "
            "not cover:\n\n"
            + "\n".join(constraints)
            if constraints
            else ""
        )
        + "\n\n**The charge against what was held**, cell by cell, which is the check this "
        "figure runs rather than a constant it searches for. The criterion has three lines and "
        "all of them are registered before the sitting: the unnamed remainder must be "
        "**non-negative**, a negative one being an over-bill — bytes the rule charged that "
        "nothing holds, and so a reader the allocation would have afforded — and it is read "
        "against two ceilings, which say different things. Above "
        f"**`MEMORY_UNPOOLED_BOUND`** ({_fmt_budget_bytes(LIBRARY_MEMORY_UNPOOLED_BOUND)}), the "
        "number `margin_allowance` predicts a count's resident with, the allocation still holds "
        "and the **bound** is wrong; above "
        f"**`MEMORY_RESERVE`** ({_fmt_budget_bytes(LIBRARY_MEMORY_RESERVE)}), which is by "
        "construction what covers everything the charge does not bill, the **rule** is. The "
        "`Criterion` column names the band rather than saying only that a cell faulted, because "
        f"the three are read differently: a cell in the `{BAND_BOUND}` band is a finding the "
        "sitting discharges itself, re-deriving the bound from these same remainders, while a "
        "cell above the reserve is an arrangement the discovery cannot keep inside its "
        "allocation and an over-bill is a charge nothing holds — neither of those two is "
        "apparatus scatter, and neither leaves the sitting anything to repair, so both refute "
        "the model. None of the three keeps the table out of this document. The "
        "middle column is the "
        "second term of that bill, reported apart because it is the one unbounded in the block "
        "size: `BufferPool::slots` clamps the block pool at `POOL_DEPTH.max(jobs)` with "
        f"`POOL_DEPTH` = {LIBRARY_POOL_DEPTH} and `BlockCache::slot` drains to one below it "
        "before taking the buffer `retain` pushes back, so the pool holds "
        "`(POOL_DEPTH.max(jobs) − 1) × unit` on top of the block each reader has in flight — "
        "billed at **every** count (`io.rs`, `WorkerMemory`). What is left is what the charge "
        "does not bill, and this column does not name it: the instrument legs below attribute "
        "a remainder of their own, which is a different subtraction on a different build. A "
        "leg that declined the block path is absent, holding none of these "
        "terms; a censored one is absent too, its reading being a bound:\n\n"
        + model_table
        + "\n\n"
        + model_verdict
        + "\n\n"
        + attribution
        + (
            "\n\n**The check, which is what makes the two instruments independent**: the "
            "black-box legs measure the same arrangement through `getrusage`, sharing no "
            "mechanism with the report above, so a term the process names has to show up in "
            "the sum the wrapper measures.\n\n" + "\n".join(checks)
            if checks
            else ""
        )
        + f"\n\n**What each mechanism moves**, at one block size and one allocation — "
        f"`{RESERVE_MECHANISM_INPUT}` flagless in `-m {RESERVE_MECHANISM_LIMIT}`, which resolved "
        + (f"{ref_jobs} readers" if ref_arrangement else "a count it never lived to report")
        + ". The reference is the axis row above, not a re-take. **One leg, where three were "
        "registered**: the arena cap bounds how many arenas can hold a retained block, and "
        "at a cell whose uncapped process already runs no more arenas than the cap allows it "
        "cannot move one — the instrument's `Arenas` column says which this cell is."
        + (
            " **On this build the cap reaches C alone**: the instrument counted in front of "
            "mimalloc, which it is refused beside any other allocator, so the shipped build's "
            "Rust heap — the block buffers included — is mimalloc's, and glibc's arenas hold "
            "only what C allocates, `liblzma`'s decoder state among it."
            if two_heaps
            else ""
        )
        + " The two allocator legs are dropped rather than re-aimed "
        "because jemalloc and mimalloc do not have glibc's dynamic mmap threshold — swapping "
        "them removes the mechanism instead of measuring it. What replaced them is the "
        "instrument above, which reports the retention rather than differencing two runs:\n\n"
        + mech_table
        + "\n\n**What the block path costs against the streaming fallback**, one byte of budget "
        f"apart in the same {RESERVE_MECHANISM_LIMIT} allocation at `--jobs {RESERVE_JOBS}`: "
        f"{fmt_rss_delta(step_medians[0] - step_medians[-1])} between the two, "
        f"{step_medians[0] / max(step_medians[-1], 1):.1f}×. `BlockCache::affordable` compares "
        "the budget against what **one** reader costs — the per-reader term plus that one "
        "reader's share of the pool's retention list — so the pair straddles that "
        f"comparison at {_fmt_budget_bytes(charge_bytes(RESERVE_MECHANISM_UNIT, 1))} and differs "
        "in nothing else. It is what an operator deciding whether to set `--memory` "
        "needs, and no other figure states it:\n\n"
        + step_table
        + f"\n\n**The stated-budget axis.** Each cell "
        f"is peak resident set, then that reading **minus the read-buffer budget the run's "
        f"`--memory` states** — what the reserve has to cover. Where the margin lowers a typed "
        f"count's budget below the one stated, the cell understates what the run holds above "
        f"the budget it resolved, which the run reports. Every row states `--jobs {RESERVE_JOBS}` in a {PARALLEL_MEMORY} container, an "
        f"apparatus departure from the register's 512 MB, which is smaller than the largest "
        f"budget under test; the flagless legs above each carry their own allocation instead.\n\n"
        + table
        + f"\n\n**The worst cell of each arena leg**, which is what a `--jobs {RESERVE_JOBS}` scan "
        "holds above the budget its `--memory` states, the count typed so that the margin "
        "lowers only the budget — not the flagless arrangement `MEMORY_RESERVE` was read "
        "off, which the legs above are: "
        + ", ".join(
            f"{arena} **{fmt_rss_delta(worst_stated[token])}**"
            for token, _, arena in RESERVE_ARENAS
        )
        + ".\n\n"
        f"**The shipped serial arrangement is read beside it and not on either axis**: "
        f"`control` at `--jobs 1` with no budget stated runs at the library's "
        f"{_fmt_budget(LIBRARY_DEFAULT_BUDGET)} default and holds "
        f"{fmt_mib_median_spread(base)}, a reserve of **{fmt_rss_delta(base_reserve)}**. That "
        "run is `peak-rss`'s own `control` row — the same binary, command, input and regime — "
        "so it is one reading this table shares with that one rather than a second "
        "measurement of it.\n"
        + note
        + "\nPer-rep readings (peak RSS):\n"
        + "\n".join(per_rep)
        + "\n"
    )


def _fmt_ns(ns: float) -> str:
    return f"{ns / 1000:.2f} µs" if ns >= 1000 else f"{ns:.0f} ns"


def _per_rep(figure: str, session: Session, specs: Sequence[RunSpec]) -> str:
    lines = [
        f"- {spec.label}: {fmt_readings(session.get(figure, spec))}" for spec in specs
    ]
    return "Per-rep readings (s):\n" + "\n".join(lines) + "\n"


# -- what the data level costs, and what its statistics buy ----------------

#: `statistics-gathering`'s inputs, in the table's row order: the three
#: scan-throughput inputs, and the `--arrays --composite` file. **The control
#: and the arrays file bracket the census**: a control row holds no array, so
#: the census reads only its counted columns, and every arrays row holds three
#: nested values it inspects as well, so the two rows' profiles of the `data`
#: leg attribute the census on the shape most dumps have and on the one it
#: exists for. **Two of the inputs hold no `COPY` row**, and a data-level
#: `parse` over them observes nothing: their rows are what the level costs
#: where it records nothing, which is the control's row read from the other
#: side.
_STATISTICS_ROWS: tuple[tuple[str, str], ...] = (
    ("control", "`COPY` block"),
    ("arrays", "`COPY` block, arrays in every row"),
    ("large_object", "Large-object region"),
    ("insert_run", "`INSERT` run"),
)


def _statistics_specs() -> list[RunSpec]:
    return [
        RunSpec(
            "pgdt", name, f"{STATISTICS_FAMILY}{leg}-rss", "warm", f"{label}, {leg} level"
        )
        for name, label in _STATISTICS_ROWS
        for leg, _ in STATISTICS_LEGS
    ]


def run_statistics_gathering(session: Session) -> str:
    """Whole-file `parse` at the data level against the metadata level, warm,
    in one container of the figure's own, with resident recorded beside each
    and not refined."""
    figure = "statistics-gathering"
    specs = _statistics_specs()
    floors = [
        RunSpec("none", name, "dd", "warm", f"dd floor {name}") for name, _ in _STATISTICS_ROWS
    ]
    # One input at a time, its floor while it is staged: the figure is split
    # one sweep per input, and no row reads one input against another.
    for (name, _), floor in zip(_STATISTICS_ROWS, floors):
        session.sweep(figure, [spec for spec in specs if spec.input == name], session.cfg.reps(5))
        session.sweep(figure, [floor], session.cfg.reps(3))
    legs = [leg for leg, _ in STATISTICS_LEGS]
    rows, per_rep = [], []
    for (name, label), floor in zip(_STATISTICS_ROWS, floors):
        by_leg = {
            spec.command.removeprefix(STATISTICS_FAMILY).split("-")[0]: spec
            for spec in specs
            if spec.input == name
        }
        walls = {leg: session.get(figure, by_leg[leg]) for leg in legs}
        rss = {leg: session.get_rss(figure, by_leg[leg]) for leg in legs}
        rows.append(
            [
                label,
                *(fmt_median_spread(walls[leg]) for leg in legs),
                fmt_delta(median(walls["metadata"]), median(walls["data"])),
                *(fmt_mib_median_spread(rss[leg]) for leg in legs),
                f"{fmt_s(median(session.get(figure, floor)))} s",
            ]
        )
        for leg in legs:
            per_rep.append(
                f"- {label}, {leg} level: {fmt_readings(walls[leg])}; "
                + ", ".join(fmt_mib(v) for v in rss[leg])
            )
        per_rep.append(f"- {label}, `dd` → `/dev/null`: {fmt_readings(session.get(figure, floor))}")
    table = md_table(
        [
            "Input",
            "Metadata level",
            "Data level",
            "Δ",
            "Peak RSS, metadata",
            "Peak RSS, data",
            "`dd` floor",
        ],
        rows,
    )
    return (
        table
        + f"\n\nEvery run is `pgdt parse` over the whole file at `--jobs {SWEEP_JOBS}`, "
        f"its level stated as `{NO_STATISTICS}` or `{GATHER_STATISTICS}` — the default's "
        "base size, gathered exactly where a flagless `parse` coarsens wide rows — "
        f"**in a {STATISTICS_MEMORY} container**, against the register's "
        f"{session.cfg.memory}: statistics are billed against the limit's margin "
        '(`docs/design/decisions.md`, "D85"), so the limit is chosen generously that none '
        "declines, and the resident column says what it left. The Δ is the census, the "
        "unrepresentable count and the statistics together; which costs what is read off "
        "a profile of the data level, not off this table. Resident is recorded, not "
        "attributed.\n\nPer-rep readings (s; peak RSS):\n"
        + "\n".join(per_rep)
        + "\n"
    )


def _pruning_specs() -> list[RunSpec]:
    return [
        RunSpec(
            "pgdt",
            "pruning",
            f"{PRUNING_FAMILY}{name}-{leg}",
            "warm",
            f"{name}, --statistics {leg}",
        )
        for name in PRUNING_FILTERS
        for leg in PRUNING_LEGS
    ]


def pruning_problems(reported: Mapping[str, Mapping[str, str]]) -> list[str]:
    """Why a `statistics-pruning` sitting's legs do not price what it claims,
    keyed by `<filter>-<leg>`.

    **Each is a refusal, not a cell**: an unpruned leg that skipped something is
    not the reference, a pruned leg that skipped nothing prices consulting
    statistics rather than what they buy, and two legs returning different row
    counts are the phase's correctness check failing under the timer.

    **`PRUNING_UNNARROWED` is held to the opposite**, and admitted there alone:
    its pruned leg must say it consulted statistics — the note is printed only
    where it did — and skipped no group and stopped no read, or it prices
    something other than consulting them."""
    bad = []
    for name in PRUNING_FILTERS:
        none, used = reported.get(f"{name}-none", {}), reported.get(f"{name}-all", {})
        if "skipped_groups" in none or "unread_bytes" in none:
            bad.append(f"{name}: `--statistics none` reported skipping bytes")
        if name != PRUNING_UNNARROWED:
            if int(used.get("skipped_groups", "0")) == 0:
                bad.append(f"{name}: `--statistics all` skipped no row group")
        elif "skipped_groups" not in used:
            bad.append(f"{name}: `--statistics all` reported consulting no statistics")
        elif int(used["skipped_groups"]) != 0 or "unread_bytes" in used:
            bad.append(f"{name}: `--statistics all` skipped bytes its statistics cannot narrow")
        returned = none.get("rows_returned")
        if returned is None or returned != used.get("rows_returned"):
            bad.append(
                f"{name}: the two legs returned {none.get('rows_returned')} and "
                f"{used.get('rows_returned')} row(s)"
            )
    return bad


def run_statistics_pruning(session: Session) -> str:
    """`pgdt query` under a selective range on a sorted column, under an
    equality a dictionary answers, and under an equality its statistics cannot
    narrow, each with its statistics and with `--statistics none`, warm."""
    figure = "statistics-pruning"
    specs = _pruning_specs()
    session.sweep(figure, specs, session.cfg.reps(6))
    floor = RunSpec("none", "pruning", "dd", "warm", "dd floor pruning")
    session.sweep(figure, [floor], session.cfg.reps(3))
    reported = {
        spec.command.removeprefix(PRUNING_FAMILY): session.reported.get(spec.key(figure), {})
        for spec in specs
    }
    problems = pruning_problems(reported)
    if problems:
        raise RuntimeError("statistics-pruning does not price pruning: " + "; ".join(problems))
    rows, per_rep = [], []
    for name, (expr, label) in PRUNING_FILTERS.items():
        walls = {
            leg: session.get(
                figure, RunSpec("pgdt", "pruning", f"{PRUNING_FAMILY}{name}-{leg}", "warm", "")
            )
            for leg in PRUNING_LEGS
        }
        used = reported[f"{name}-all"]
        unread = int(used["skipped_bytes"]) + int(used.get("unread_bytes", "0"))
        rows.append(
            [
                f"{label}: `{expr}`",
                fmt_median_spread(walls["none"]),
                fmt_median_spread(walls["all"]),
                fmt_delta(median(walls["none"]), median(walls["all"])),
                f"**{median(walls['none']) / median(walls['all']):.1f}×**",
                f"{int(used['skipped_groups']):,} of {int(used['groups']):,}",
                f"{int(used.get('unread_bytes', '0')):,}",
                f"{unread / int(used['bytes']) * 100:.2f}%",
                f"{int(used['rows_returned']):,}",
            ]
        )
        for leg in PRUNING_LEGS:
            per_rep.append(f"- {label}, `--statistics {leg}`: {fmt_readings(walls[leg])}")
    per_rep.append(f"- `dd` → `/dev/null`: {fmt_readings(session.get(figure, floor))}")
    table = md_table(
        [
            "Filter",
            "`--statistics none`",
            "Statistics used",
            "Δ",
            "Speedup",
            "Groups skipped",
            "Bytes a stop left unread",
            "Of the rows' bytes, not read",
            "Rows returned",
        ],
        rows,
    )
    profile = session.stager.profile("pruning")
    return (
        table
        + f"\n\nOne file — the control's rows with `v_category` appended, {profile['rows']:,} rows "
        f"of {profile['columns']} columns — queried warm at `--jobs {SWEEP_JOBS}`, "
        "`--schema-mode typed`, against a cache one untimed `parse` stating "
        f"`{GATHER_STATISTICS}` wrote in the same container, so the two legs of a row differ "
        "by `--statistics` alone. The first two rows price what pruning buys; the third, "
        "which skips nothing, what consulting the statistics costs a query they cannot "
        "narrow. The cache is decoded whole whatever the query states, so both legs pay "
        "its statistics' decode. The skipped groups and bytes are the query's own notes; the "
        "bytes a stop left unread are a lower bound, and the share not read adds them to the "
        f"skipped groups'. `dd` → `/dev/null` on the same file: "
        f"**{fmt_s(median(session.get(figure, floor)))} s**.\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- what DataFusion's dynamic filters buy a query -------------------------


def _dynfilter_specs(figure: str) -> list[RunSpec]:
    return [
        RunSpec("dfcli", "dynfilter", command, "warm", command.removeprefix(DYNFILTER_FAMILY))
        for command in dynfilter_shapes(figure)
    ]


def dynfilter_problems(figure: str, reported: Mapping[str, Mapping[str, str]]) -> list[str]:
    """Why a dynamic-filter sitting's legs do not price what it claims, keyed
    by `<query>-<leg>`.

    **The legs of a row must return one answer**, byte for byte: they differ
    by a flag and a setting the spec says change no row, so a difference is
    the phase's correctness check failing under the timer, and a table beside
    it would price a wrong answer. An answer with no row proves nothing either
    way."""
    bad = []
    for name in DYNFILTER_QUERIES[figure]:
        legs = [reported.get(f"{name}-{leg}", {}) for leg in DYNFILTER_LEGS]
        digests = {leg.get("result_digest") for leg in legs}
        if None in digests or "" in digests or len(digests) != 1:
            bad.append(f"{name}: the legs answered differently, or not at all")
        elif int(legs[0].get("result_rows", "0")) == 0:
            bad.append(f"{name}: the query returned no row")
    return bad


#: What each leg's per-rep line calls it.
DYNFILTER_LEG_NAMES = {
    "off": "filter off",
    "on": "filter on",
    DYNFILTER_ROWS_LEG: "rows evaluated",
}


def _run_dynfilter(session: Session, figure: str, kind: str) -> str:
    """One dynamic-filter figure: each of `kind`'s queries under each of
    `DYNFILTER_LEGS`, warm, and the `dd` floor."""
    specs = _dynfilter_specs(kind)
    startup = RunSpec("dfcli", "dynfilter", DYNFILTER_STARTUP, "warm", "startup")
    session.sweep(figure, [*specs, startup], session.cfg.reps(6))
    answer = session.reported.get(startup.key(figure), {}).get("startup_answer")
    if answer != "1":
        raise RuntimeError(
            f"{figure}: the startup leg answered {answer!r} to `{STARTUP_SQL}`, so it "
            "timed something other than a startup"
        )
    floor = RunSpec("none", "dynfilter", "dd", "warm", "dd floor dynfilter")
    session.sweep(figure, [floor], session.cfg.reps(3))
    prefix = f"{DYNFILTER_FAMILY}{kind}-"
    reported = {
        spec.command.removeprefix(prefix): session.reported.get(spec.key(figure), {})
        for spec in specs
    }
    problems = dynfilter_problems(kind, reported)
    if problems:
        raise RuntimeError(f"{figure} does not price one answer: " + "; ".join(problems))
    rows, per_rep = [], []
    for name, (sql, label) in DYNFILTER_QUERIES[kind].items():
        walls = {
            leg: session.get(
                figure, RunSpec("dfcli", "dynfilter", f"{prefix}{name}-{leg}", "warm", "")
            )
            for leg in DYNFILTER_LEGS
        }
        answer = reported[f"{name}-on"]
        answered = (
            f"{int(answer['result_first']):,}" if kind == "join" else answer["result_rows"]
        )
        rows.append(
            [
                f"{label}: `{sql.removeprefix('SELECT ')}`",
                fmt_median_spread(walls["off"]),
                fmt_median_spread(walls["on"]),
                fmt_delta(median(walls["off"]), median(walls["on"])),
                fmt_median_spread(walls[DYNFILTER_ROWS_LEG]),
                fmt_delta(median(walls["on"]), median(walls[DYNFILTER_ROWS_LEG])),
                answered,
            ]
        )
        for leg in DYNFILTER_LEGS:
            per_rep.append(f"- {label}, {DYNFILTER_LEG_NAMES[leg]}: {fmt_readings(walls[leg])}")
    started = session.get(figure, startup)
    per_rep.append(f"- startup, `{STARTUP_SQL}`: {fmt_readings(started)}")
    per_rep.append(f"- `dd` → `/dev/null`: {fmt_readings(session.get(figure, floor))}")
    flag = DYNFILTER_FLAGS[kind]
    table = md_table(
        [
            "Query",
            "Filter off",
            "Filter on",
            "Δ, on against off",
            "Rows evaluated",
            "Δ, rows against on",
            "Rows the join matched" if kind == "join" else "Rows returned",
        ],
        rows,
    )
    profile = session.stager.profile("dynfilter")
    return (
        table
        + f"\n\nOne file — the control's rows with `u_key` and `bucket` appended, and three "
        f"small build tables, {profile['rows']:,} rows in all — queried warm by "
        f"`pgdt sql -c` at `{DFCLI_PARTITIONS}={SWEEP_JOBS}`, against a cache one "
        f"untimed `pgdt parse` stating `{GATHER_STATISTICS}` wrote in the same container, and "
        "the legs of a row answer alike, byte for byte: `Filter off` and `Filter on` differ by "
        f"the producer's flag alone, `{flag}` `false` and `true`, the on leg's filter being "
        "whatever the scan makes of it at the provider's default, and `Rows evaluated` differs "
        f"from `Filter on` by `-c '{DYNFILTER_ROWS_SQL}'` alone, run ahead of the query in the "
        "same process. Every leg's reading carries the "
        "program's startup — loading it, starting its runtime and registering the dump — "
        f"which a leg answering `{STARTUP_SQL}` over the same cache, taken in the same "
        f"interleave, reads as {fmt_median_spread(started)}. "
        "`dd` → `/dev/null` on the same file: "
        f"**{fmt_s(median(session.get(figure, floor)))} s**.\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


def run_dynamic_filter_join(session: Session) -> str:
    """A selective join on a clustered key and on an unclustered one, and a
    join whose filter rejects nothing, each with DataFusion's join dynamic
    filter off, on, and on with its rows evaluated."""
    return _run_dynfilter(session, "dynamic-filter-join", "join")


def run_dynamic_filter_topk(session: Session) -> str:
    """An `ORDER BY … LIMIT` over an unsorted column, with DataFusion's TopK
    dynamic filter off, on, and on with its rows evaluated."""
    return _run_dynfilter(session, "dynamic-filter-topk", "topk")


# --------------------------------------------------------------------------
# The register. Order is run order: a figure that shares a reading comes after
# the figure that takes it.
# --------------------------------------------------------------------------

FIGURES: list[Figure] = [
    Figure(
        id="scan-throughput-cold",
        also_quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run cold, on the SSD",
        stage="cold",
        depends=(*SCAN, *MAP, *READ, *GEN_SHAPES),
        cold_inputs=("control", "large_object", "insert_run"),
        run=run_scan_throughput_cold,
    ),
    Figure(
        id="scan-throughput-warm",
        also_quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run warm, on tmpfs",
        stage="warm",
        depends=(*SCAN, *MAP, *READ, *GEN_SHAPES),
        warm_inputs=("control", "large_object", "insert_run"),
        warm_groups=(("control",), ("large_object",), ("insert_run",)),
        run=run_scan_throughput_warm,
    ),
    # The third device class, and the only one that can price the I/O
    # defaults: on the HDD and the SATA SSD the device is the whole cost and
    # on tmpfs there is no device at all, so a readahead, `fadvise` or
    # chunk-size change has nowhere to show. Like the other two, it borrows
    # nothing: its `COPY` row is its own reading.
    Figure(
        id="scan-throughput-nvme",
        also_quoted_by=(
            "docs/design/pg-dump-compatibility.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run cold, on the NVMe",
        stage="cold-nvme",
        depends=(*SCAN, *MAP, *READ, *GEN_SHAPES),
        nvme_inputs=("control", "large_object", "insert_run"),
        run=run_scan_throughput_nvme,
    ),
    # The one of the three I/O defaults that is a value rather than a scheme,
    # and the only one that can be swept without a second build: `--chunk-size`
    # is a flag, so every row here is the same binary over the same file.
    # It borrows nothing -- the default row is a `parse` under a flag the
    # throughput tables do not pass, so it is its own reading even where the
    # number would look interchangeable with theirs.
    Figure(
        id="chunk-size",
        section="What the read chunk size is worth",
        stage="warm+cold+cold-nvme",
        depends=(*SCAN, *MAP, *READ, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control",),
        cold_inputs=("control",),
        nvme_inputs=("control",),
        run=run_chunk_size,
    ),
    Figure(
        id="nested-end-to-end",
        section="A typed query over nested columns costs 6.3 µs a row more than a string one",
        stage="warm",
        depends=_declare(*NESTED, *DECODE, *MAP, *READ, *QUERY_CLI, *GEN_PERF, *CACHED_QUERY),
        warm_inputs=("control", "composite", "arrays"),
        warm_groups=_NESTED_SWEEPS,
        subtracts=(
            Subtraction(
                "nested-end-to-end", ("control", "arrays"), "the headline, paired per rep"
            ),
            *(
                Subtraction(
                    "nested-end-to-end",
                    ("control", nested),
                    "each nested file's `strings` leg against its control's",
                )
                for _, nested in _NESTED_SWEEPS
            ),
        ),
        run=run_nested_end_to_end,
    ),
    Figure(
        id="cross-file-floor",
        section="The cross-file subtraction bottoms out at about half a microsecond a row",
        stage="warm",
        depends=_declare(*NESTED, *DECODE, *READ, *QUERY_CLI, *GEN_PERF, *CACHED_QUERY),
        warm_inputs=("control", "control43"),
        shares=(
            #: Consumed, not republished: row 1 is `_per_row_diffs` over the
            #: nested sweep's own reps, which publishes a per-row difference
            #: rather than either of the readings it is taken from. So it
            #: orders the run and pulls the source in, and is not an edge of
            #: the sharing closure.
            Shared("nested-end-to-end", "row 1's per-rep differences"),
        ),
        subtracts=(
            Subtraction(
                "nested-end-to-end", ("control", "composite"), "row 1, the composite's share"
            ),
            Subtraction("cross-file-floor", ("control", "control43"), "row 2, the seed floor"),
        ),
        run=run_cross_file_floor,
    ),
    Figure(
        id="per-block-quadratic",
        section="Per-block cache saving is quadratic in block count, and so is the map",
        stage="warm",
        depends=(*MAP_BUILD, *READ, *CACHE, *GEN_BLOCKS, *GEN_PERF),
        warm_inputs=tuple(name for name, _ in _QUADRATIC_ROWS),
        #: The prose reads each block count's `parse` against the one-block
        #: control of the same bytes.
        subtracts=tuple(
            Subtraction(
                "per-block-quadratic",
                (_QUADRATIC_ROWS[0][0], name),
                "a block count against the control",
            )
            for name, _ in _QUADRATIC_ROWS[1:]
        ),
        run=run_per_block_quadratic,
    ),
    # A figure whose reading is not a time. It is registered rather than left a
    # koji row because a claim outside the register has no `depends` edge to go
    # red when the read path moves, and koji's run captures no resident figure
    # a correction could come from. `depends` therefore carries the read path first — that is the
    # mechanism the claim is about — and the map and the cache, which are what
    # a per-block cost would accumulate in.
    Figure(
        id="peak-rss",
        also_quoted_by=(
            "docs/manual/dump-inspection.md",
            "README.md",
        ),
        #: The manual and the README state the *claim* this figure licenses to
        #: a reader who cannot check it against the code, and neither names the
        #: figure — which is what keeps them declared while `io.rs`, which
        #: quotes the reading and says `peak-rss` doing it, is computed.
        #:
        #: **Only half of the manual's sentence is this figure's.** "Does not
        #: grow with the size of the dump" is these rows; "grows with the
        #: number of tables, by roughly 10 KB each" is a per-*table* claim this
        #: figure's inputs cannot license, since `blocks4000` gives every table
        #: exactly one `COPY` block and the per-table and per-block axes
        #: coincide in it. That half is `rss-attribution`'s, which separates
        #: them by stopping a leg at the preamble.
        section="What a scan holds resident, per byte and per block",
        stage="warm",
        # `MAP` rather than `MAP_BUILD`: what the latter adds is `stream.rs`,
        # which `SCAN` already names, and a path declared twice is printed
        # twice by `--list`.
        depends=(*RSS_INSTRUMENT, *READ, *SCAN, *MAP, *CACHE, *GEN_PERF, *GEN_BLOCKS),
        warm_inputs=_RSS_ROWS,
        subtracts=tuple(
            Subtraction("peak-rss", (_RSS_PIVOT, name), "a row against the pivot")
            for name in _RSS_ROWS
            if name != _RSS_PIVOT
        ),
        run=run_peak_rss,
    ),
    Figure(
        id="map-only",
        section="Per-block cache saving is quadratic in block count, and so is the map (map alone)",
        stage="warm",
        depends=(*MAP_BUILD, *READ, *QUERY_CLI, *GEN_BLOCKS),
        warm_inputs=("blocks1000", "blocks2000", "blocks4000"),
        #: The prose reads the series' growth per doubling, row by row.
        subtracts=(
            Subtraction("map-only", ("blocks1000", "blocks2000"), "the first doubling"),
            Subtraction("map-only", ("blocks2000", "blocks4000"), "the second doubling"),
        ),
        run=run_map_only,
    ),
    Figure(
        id="preamble-prepass",
        section="The preamble prepass is bounded by the schema, not by the dump",
        stage="warm",
        #: Its second row is `per-block-quadratic`'s 4000-block `parse` reading,
        #: borrowed rather than re-measured (`requires`, below) -- so this
        #: figure inherits that one's staleness edges as well as its own, or a
        #: change to the map moves a row here that reads green.
        depends=(*PREAMBLE, *MAP_BUILD, *READ, *CACHE, *GEN_BLOCKS),
        warm_inputs=("blocks4000",),
        shares=(
            Shared(
                "per-block-quadratic",
                "the full-`parse` row, which is the quadratic table's 4000-block `parse` cell",
                (RunSpec("pgdt", "blocks4000", "parse-cache-out", "warm", ""),),
            ),
        ),
        run=run_preamble_prepass,
    ),
    Figure(
        id="nested-decode-micro",
        section="Nested decode costs what it copies, and an element is now a borrowed slice",
        stage="criterion",
        depends=("pgdump_query/src/nested.rs", "pgdump_query/benches/decoders.rs"),
        run=run_nested_decode_micro,
    ),
    # `depends` carries the scan path as well as the decode one, which no other
    # query figure does: the zero-column row is an absolute reading of replay
    # with nothing decoded, so a change in what replay costs moves it directly
    # rather than cancelling out of a difference.
    Figure(
        id="projection-widths",
        section="What a column costs: five projection widths over one file",
        stage="warm",
        depends=_declare(*SCAN, *NESTED, *DECODE, *READ, *QUERY_CLI, *GEN_PERF, *CACHED_QUERY),
        warm_inputs=("arrays",),
        run=run_projection_widths,
    ),
    # The only figure in the register that passes a filter, and therefore the
    # only one a change to `predicate.rs` can move. `depends` carries the scan
    # path for the same reason `projection-widths` does — every row is an
    # absolute reading of replay with nothing decoded — and `QUERY_CLI`
    # because `--where` is parsed there. `batch.rs` is deliberately absent:
    # no row survives any of these predicates, so `push_row` never runs.
    Figure(
        id="predicate-terms",
        section="What a filter term costs, and how much of it is the walk to its field",
        stage="warm",
        depends=_declare(*PREDICATE, *SCAN, *READ, *QUERY_CLI, *GEN_PERF, *CACHED_QUERY),
        warm_inputs=("control",),
        run=run_predicate_terms,
    ),
    # `depends` is the union of what moves the three shapes it times, because
    # a table of *ratios* between allocators is invalidated by a change in
    # what any of the three shapes allocates -- which is most of the row path.
    # The CLI's own manifest is in there too: that is where the legs are
    # declared and where the shipped default lives, so a change to it changes
    # what this figure is a figure of.
    Figure(
        id="allocator",
        section="Which allocator a figure was taken under",
        stage="warm",
        depends=(
            *SCAN,
            *MAP,
            *NESTED,
            *DECODE,
            *READ,
            *QUERY_CLI,
            *GEN_PERF,
            # Its two `query` shapes read a data-level cache (`CACHED_QUERY`).
            *CACHE,
            *STATISTICS,
            # Where the legs are declared and where the shipped default lives,
            # so a change to it changes what this figure is a figure of.
            # `src/alloc.rs` needs no line of its own: `QUERY_CLI` is the
            # directory.
            "pgdt/Cargo.toml",
        ),
        warm_inputs=("control",),
        shares=ALLOCATOR_SHARES,
        run=run_allocator,
    ),
    Figure(
        id="xz-decode-scaling",
        section="What a second decode worker buys, and what the twenty-fourth does not",
        stage="warm-parallel",
        # Not the library's read path: no `pgdt` runs here at all. What can move
        # this figure is the decoder, the instrument that drives it, and the
        # generators behind the two files — including the perf generator, which
        # the control leg's bytes are a compression of.
        depends=(
            "vendor/xz-seek/src/",
            "pgdump_query/examples/xz_decode.rs",
            "scripts/generate_xz_input.py",
            *GEN_PERF,
        ),
        warm_inputs=("control_xz", "koji_xz"),
        memory=DECODE_MEMORY,
        run=run_xz_decode_scaling,
    ),
    # The parallel scan's throughput claim, and the first figure in the register
    # whose axis is the worker count of `pgdt` itself. `depends` is the union of
    # everything a parallel scan runs through — the scanner, the map, the read
    # path, the leader, the decoder, the CLI where `--jobs` is parsed, and the
    # provider its typed legs scan through — plus both generators behind its
    # inputs. It is wide on purpose: this figure is the one that would be
    # quietly wrong if any of them changed.
    Figure(
        id="parallel-scan-throughput",
        section="What a second scan worker buys, and where the plain path stops",
        stage="warm-parallel",
        depends=(
            *SCAN,
            *MAP,
            *READ,
            *NESTED,
            *DECODE,
            *QUERY_CLI,
            "pgdump_query/src/leader.rs",
            "vendor/xz-seek/src/",
            "scripts/generate_xz_input.py",
            *GEN_PERF,
            # Its provider legs read a data-level cache (`CACHED_QUERY`), and
            # prune by its statistics under their filter.
            *CACHE,
            *STATISTICS,
            "pgdump_query/src/prune.rs",
            *DATAFUSION,
        ),
        warm_inputs=("control", "control_xz"),
        memory=PARALLEL_MEMORY,
        run=run_parallel_scan_throughput,
    ),
    # The parallel scan's *memory* claim: one stated number bounds the read
    # path. `depends` is narrower than its sibling's — nothing here decodes a
    # field or renders a row, the shape being `parse` — but it carries the same
    # read path, leader and decoder, which is where a resident set is decided.
    Figure(
        id="parallel-peak-rss",
        section="What a parallel scan holds resident, at two block sizes",
        stage="warm-parallel",
        depends=(
            *RSS_INSTRUMENT,
            *SCAN,
            *MAP,
            *READ,
            *QUERY_CLI,
            "pgdump_query/src/leader.rs",
            "vendor/xz-seek/src/",
            "scripts/generate_xz_input.py",
            *GEN_PERF,
        ),
        # **The query path's sub-stream sizing does not reach this figure.** It
        # is in `stream::plan_partitions`, and every leg here is
        # `pgdt parse`, which reaches `worker_count` through
        # `leader::scan_region` instead — so this figure is excused by
        # reachability where its sibling is not, though both declare the same
        # read path.
        warm_inputs=("control_xz", "control_xz128"),
        memory=PARALLEL_MEMORY,
        run=run_parallel_peak_rss,
    ),
    # The attribution's instrument.
    #
    # **It cannot be taken alone, and the `Shared` edge is what says so.** Its
    # `parse` reference row runs `peak-rss`'s `blocks500` and `blocks4000`
    # shapes — same binary, same command, same apparatus — and a reading is
    # keyed by figure *and* spec, so without the edge the two are separate
    # measurements of one run: the standalone readings and `peak-rss`'s
    # disagreed at 9.58/44.26 MiB against 9.73/43.78, two numbers for one
    # measurement a section apart. Standing in that share is also what confines
    # this figure to a stamped sweep (`sitting_problems`; `measurements.md`,
    # "A figure may be published outside the sweep").
    Figure(
        id="rss-attribution",
        #: The manual's per-*table* claim is declared here rather than on
        #: `peak-rss`, which cannot tell per-table from per-block on its own
        #: inputs: `blocks4000` gives every table exactly one `COPY` block, so
        #: the two coincide in it and only these legs separate them. The
        #: manual's sentence carries a second claim — that nothing accumulates
        #: per byte — which is `peak-rss`'s, so both figures declare that file.
        also_quoted_by=("docs/manual/dump-inspection.md",),
        section="What the per-block resident growth is made of",
        stage="warm",
        # What `peak-rss` declares, plus the two mechanisms only this figure's
        # own legs reach: the preamble prepass, which is where the per-*table*
        # structure is paid, and the CLI's `query` path, which the four
        # no-match legs run. The CLI manifest is here for the reason the
        # `allocator` figure carries it — three of the nine legs are its legs,
        # and that file is where they are declared.
        depends=(
            *RSS_INSTRUMENT,
            *READ,
            *SCAN,
            *MAP,
            *CACHE,
            *PREAMBLE,
            *QUERY_CLI,
            "pgdt/Cargo.toml",
            *GEN_BLOCKS,
        ),
        #: Read off `_attribution_specs` rather than respelling the two
        #: `RunSpec`s: a leg is identified by its binary and shape, and a second
        #: spelling of one is what drifts.
        shares=(
            Shared(
                "peak-rss",
                "the `parse` reference row at both block counts",
                _attribution_specs()[0][1:],
            ),
        ),
        warm_inputs=_ATTRIBUTION_INPUTS,
        #: Every leg's slope, its `parse` row's over `peak-rss`'s reps where
        #: this sitting took that figure.
        subtracts=(
            Subtraction("rss-attribution", _ATTRIBUTION_INPUTS, "each leg's per-block slope"),
            Subtraction("peak-rss", _ATTRIBUTION_INPUTS, "the `parse` row's slope, borrowed"),
        ),
        run=run_rss_attribution,
    ),
    # The budget rule's one number, the compressed path's account, and the third
    # resident-set instrument.
    #
    # **It stands in one edge, and the second is a stated non-edge.** Onto
    # `peak-rss`: its serial-default row is that figure's `control` row spec for
    # spec, so the two share a reading rather than take one each. Onto
    # `rss-attribution`: **none**, and that is a reading of the leg set rather
    # than an omission — that figure's every leg runs over
    # `blocks500`/`blocks4000`, the two block-count shapes whose axis it is,
    # where every leg here runs over `control`, `control_xz` or `control_xz128`,
    # so no two of their runs are the same run. *Rejected:* manufacturing an
    # edge by adding a block-count leg here — it would re-measure the plain path
    # on the compressed path's figure, to buy a borrow nothing reads. What the
    # three do share is a *sitting*: all three read resident, so one sweep takes
    # them together, and that collapse is the closure the harness computes
    # rather than an edge any one of them declares.
    Figure(
        id="reserve",
        # The manual's `--memory` guidance and its claim that a
        # flagless scan stays inside its allocation are read off this table
        # without naming it, so they are declared rather than computed.
        also_quoted_by=("docs/manual/dump-inspection.md",),
        section="What a scan holds above the budget it was given",
        # Two regimes: every axis here occupies every hardware thread and is
        # gated as `warm-parallel`, while the serial-default row is the shape
        # `peak-rss` takes and is gated as that figure gates it.
        stage="warm+warm-parallel",
        # What `parallel-peak-rss` declares, which is where a resident set
        # under a stated budget is decided, plus the map and the cache — a
        # per-block cost accumulates in those and would read here as reserve.
        depends=(
            *RSS_INSTRUMENT,
            *SCAN,
            *MAP,
            *READ,
            *CACHE,
            *QUERY_CLI,
            "pgdump_query/src/leader.rs",
            "vendor/xz-seek/src/",
            "scripts/generate_xz_input.py",
            *GEN_PERF,
        ),
        # Every family's inputs, deduplicated: the stated axis's two, the
        # flagless axis's two block sizes, and the baseline's — which is one of
        # the stated axis's already and is named anyway, since the day it is not,
        # a staging list built off `RESERVE_INPUTS` alone leaves that row's file
        # unstaged.
        warm_inputs=tuple(
            dict.fromkeys(
                (
                    *(name for name, _ in RESERVE_INPUTS),
                    *(name for name, _, _ in RESERVE_FLAGLESS_INPUTS),
                    _RESERVE_BASELINE.input,
                )
            )
        ),
        #: The shipped serial arrangement, which is `peak-rss`'s `control` row
        #: spec for spec.
        shares=(
            Shared(
                "peak-rss",
                "the shipped serial arrangement's peak resident set",
                (_RESERVE_BASELINE,),
            ),
        ),
        memory=PARALLEL_MEMORY,
        run=run_reserve,
    ),
    # The data level's two figures, each standing in no sharing edge, so each
    # publishes outside a sweep from its own commit. `depends` for the first is
    # what a data-level scan runs through — the census in the map, the count
    # beside it, the gathering — and what writes what it recorded; its
    # `metadata` leg is the scan-throughput shape and declares that figure's
    # paths.
    Figure(
        id="statistics-gathering",
        section="What the data level costs a parse",
        stage="warm",
        depends=(
            *RSS_INSTRUMENT,
            *SCAN,
            *MAP,
            *READ,
            *CACHE,
            *STATISTICS,
            *UNREPRESENTABLE,
            *QUERY_CLI,
            *GEN_SHAPES,
        ),
        warm_inputs=tuple(name for name, _ in _STATISTICS_ROWS),
        warm_groups=tuple((name,) for name, _ in _STATISTICS_ROWS),
        memory=STATISTICS_MEMORY,
        run=run_statistics_gathering,
    ),
    # Everything the timed query reads through — the replay, the cache it
    # loads, the pruning plan, the filter, the typed decode and batch build of
    # the rows that pass — plus the gathering, which decides what the untimed
    # builder leaves for the query to prune with.
    Figure(
        id="statistics-pruning",
        section="What row-group statistics buy a query",
        stage="warm",
        depends=(
            *SCAN,
            *MAP,
            *READ,
            *CACHE,
            *STATISTICS,
            "pgdump_query/src/prune.rs",
            *NESTED,
            *DECODE,
            *QUERY_CLI,
            *GEN_PRUNING,
        ),
        warm_inputs=("pruning",),
        run=run_statistics_pruning,
    ),
    # The spec's two figures for DataFusion's dynamic filters, each standing in
    # no sharing edge. Everything the timed query reads through — the provider
    # and the binary, the replay, the cache, the pruning plan, the filter, the
    # typed decode — plus the gathering, which decides what the untimed builder
    # leaves the scan to prune with.
    Figure(
        id="dynamic-filter-join",
        section="What DataFusion's dynamic filters buy a query",
        table_label="A join's filter, over its probe table",
        stage="warm",
        depends=(
            *SCAN,
            *MAP,
            *READ,
            *CACHE,
            *STATISTICS,
            "pgdump_query/src/prune.rs",
            *NESTED,
            *DECODE,
            *DATAFUSION,
            # `pgdt sql` is `pgdt`'s: its dispatch and allocator are the binary's.
            *QUERY_CLI,
            *GEN_DYNFILTER,
        ),
        warm_inputs=("dynfilter",),
        run=run_dynamic_filter_join,
    ),
    Figure(
        id="dynamic-filter-topk",
        section="What DataFusion's dynamic filters buy a query",
        table_label="A TopK's filter, over the table it sorts",
        stage="warm",
        depends=(
            *SCAN,
            *MAP,
            *READ,
            *CACHE,
            *STATISTICS,
            "pgdump_query/src/prune.rs",
            *NESTED,
            *DECODE,
            *DATAFUSION,
            # `pgdt sql` is `pgdt`'s: its dispatch and allocator are the binary's.
            *QUERY_CLI,
            *GEN_DYNFILTER,
        ),
        warm_inputs=("dynfilter",),
        run=run_dynamic_filter_topk,
    ),
]

FIGURES_BY_ID = {f.id: f for f in FIGURES}

#: Instruments that are **built but whose figure has not been taken**.
#:
#: A sweep does not run these and the doc carries no table *of this harness's*
#: for them, which is why they sit outside `ALL_FIGURES`: the marker
#: reconciliation would otherwise demand a section with no numbers under it.
#: `--figure <id>` still selects one, which is how the reading gets taken — and
#: taking it moves the entry into `FIGURES`, where the doc-side checks start
#: applying.
#:
#: The distinction is worth a list rather than a comment because *built* and
#: *taken* fail differently. An instrument nobody built is work; an instrument
#: built and never run is a claim nobody checked, and it is invisible unless
#: something names it.
#:
#: **Empty is the healthy state, not a disused mechanism, and it is empty
#: now.** A figure published outside a stamped sweep declares inside its own
#: marker the commit it was taken at, and a sitting run from a working tree
#: carrying its own uncommitted apparatus has no such commit to name — so an
#: instrument built ahead of that commit waits here rather than in `FIGURES`.
#: *Rejected:* `composite-isolated`, which isolated one column by declaring it
#: two ways over byte-identical rows — `projection-widths` makes the same
#: isolation a subtraction between two adjacent rows of one table over one
#: file.
UNTAKEN: list[Figure] = []

#: A figure that no sweep produces, because it is computed *across* two of
#: them. It still gets a section, a marker and both declared edges — it is one
#: table like any other, and the register is what `--check` reconciles against
#: the doc.
DERIVED: list[Figure] = [
    Figure(
        id="session-drift",
        section="What a session's own drift costs, measured rather than asserted",
        stage="derived",
        # Nothing in the library invalidates this one: it measures the
        # apparatus, not the code. What can move it is the harness's own timing
        # path.
        depends=("scripts/measure.py",),
    )
]

ALL_FIGURES = FIGURES + DERIVED
ALL_BY_ID = {f.id: f for f in ALL_FIGURES}

#: Everything `--figure` may name: the sweep's figures plus the untaken
#: instruments. Not `ALL_FIGURES`, which is the set the *doc* must carry.
SELECTABLE = FIGURES + UNTAKEN
SELECTABLE_BY_ID = {f.id: f for f in SELECTABLE}

#: Every figure the register knows, whether or not a sweep takes it.
EVERY_FIGURE = FIGURES + UNTAKEN + DERIVED
EVERY_BY_ID = {f.id: f for f in EVERY_FIGURE}

# --------------------------------------------------------------------------
# Consumers: who repeats a figure, computed from who names it.
# --------------------------------------------------------------------------

#: Where a consumer can live. Documents, the two crates and the scripts that
#: are not this harness -- a figure's numbers are quoted in prose and its
#: claims are asserted in doc comments, and both of those are in here.
CONSUMER_ROOTS: tuple[tuple[str, str], ...] = (
    ("docs", "**/*.md"),
    ("pgdump_query", "**/*.rs"),
    ("pgdt", "**/*.rs"),
    ("scripts", "*.py"),
)
#: Top-level documents, named rather than globbed: the working tree also holds
#: `CLAUDE.local.md`, which is nobody's checkout but this one's.
CONSUMER_FILES: tuple[str, ...] = ("README.md", "CONTRIBUTING.md", "CLAUDE.md")
#: Not consumers, however often they name a figure.
#:
#: `measurements.md` is where the table *is*, so listing it would make every
#: fold-in look like a cross-document edit. The register and its tests are the
#: declaration itself. And `docs/status/history/` is dated: an entry states
#: what was true on its day and is never revised, which is the same exemption
#: `scripts/citations.py` grants a closed day — a fold-in that re-read one
#: could only make it untrue.
CONSUMER_SKIP: tuple[str, ...] = (
    "docs/design/measurements.md",
    "docs/status/history/",
    "scripts/measure.py",
    "scripts/test_measure.py",
    "scripts/acknowledged.py",
)

#: How a document addresses a figure: the id in backticks, a trailing `*`
#: standing for a family of them (`scan-throughput-*`), or the marker spelling
#: the doc itself uses.
_FIGURE_MENTION = re.compile(r"`([a-z0-9][a-z0-9-]*\*?)`|figure: ([a-z0-9-]+)")


def consumer_files() -> list[str]:
    """Every repo-relative path the consumer scan reads, sorted."""
    found: set[str] = set()
    for root, pattern in CONSUMER_ROOTS:
        for path in (REPO / root).glob(pattern):
            found.add(str(path.relative_to(REPO)))
    for name in CONSUMER_FILES:
        if (REPO / name).exists():
            found.add(name)
    return sorted(p for p in found if not p.startswith(CONSUMER_SKIP))


@functools.lru_cache(maxsize=1)
def _mentions_by_file() -> tuple[tuple[str, frozenset[str]], ...]:
    """Each scanned file, and the figure names it spells."""
    out = []
    for path in consumer_files():
        try:
            text = (REPO / path).read_text(errors="replace")
        except OSError:
            continue
        names = {a or b for a, b in _FIGURE_MENTION.findall(text)}
        if names:
            out.append((path, frozenset(names)))
    return tuple(out)


def consumers(fig: Figure) -> tuple[str, ...]:
    """What a moved figure obliges a fold-in to re-read.

    **Computed from who names the figure, not declared beside it.** `depends`
    is the edge into a figure; this is the edge out, and it used to be a tuple
    per `Figure` that only a session noticing a stale citation ever corrected
    — so retargeting a document left every tuple naming the old one. A
    consumer that cites `` `peak-rss` `` or `<!-- figure: peak-rss -->` says so
    in its own text, and a glob (`` `scan-throughput-*` ``) reaches the family
    it spells.

    `also_quoted_by` is the residue: a consumer that repeats the *numbers*
    without naming the figure, which is what the manual and the README do —
    they state the claim to a reader who will never see a figure id, and no
    scan can find them."""
    named = sorted(
        path
        for path, names in _mentions_by_file()
        if any(n == fig.id or (n.endswith("*") and fig.id.startswith(n[:-1])) for n in names)
    )
    return tuple(dict.fromkeys((*named, *fig.also_quoted_by)))


def registered_regimes() -> tuple[str, ...]:
    """Every regime the registered figures are taken in, read off their own
    `stage` declarations.

    One list, not two. The regimes a sweep actually runs in are already stated
    per figure -- `--stage` selects on them -- so anything that has to iterate
    them derives the set from there rather than repeating it. The tuple that
    used to be written out by hand omitted `warm-parallel` from the day it was
    added, which is the failure a second list has every time: the copy nobody
    has to touch to add a regime is the copy that goes stale.
    """
    return tuple(
        sorted(
            {
                token
                for fig in EVERY_FIGURE
                for token in fig.stage.split("+")
                if token not in NON_REGIME_STAGES
            }
        )
    )


# --------------------------------------------------------------------------
# The borrow graph: what a re-take drags with it.
# --------------------------------------------------------------------------


def sharing_edges() -> dict[str, set[str]]:
    """The republication graph, undirected.

    Undirected because the defect is symmetric: `allocator` borrowing
    `scan-throughput-warm`'s `COPY` reading puts the same number in two
    tables, and re-taking *either* of them alone leaves the doc carrying two
    numbers for one measurement. Direction only says which figure measures
    it."""
    edges: dict[str, set[str]] = {}
    for fig in EVERY_FIGURE:
        for shared in fig.shares:
            if not shared.republished:
                continue
            edges.setdefault(fig.id, set()).add(shared.source)
            edges.setdefault(shared.source, set()).add(fig.id)
    return edges


def sharing_closure(fid: str) -> list[str]:
    """Every other figure that publishes a reading this one would move.

    Transitive, which is the whole point of declaring the graph: the note a
    run function used to write by hand named its *direct* sources, and a
    source that is itself borrowed by a third table drags that table too.
    Returned in register order, so the closure reads as a run order."""
    edges = sharing_edges()
    seen, queue = {fid}, [fid]
    while queue:
        for nxt in edges.get(queue.pop(), ()):
            if nxt not in seen:
                seen.add(nxt)
                queue.append(nxt)
    return [f.id for f in EVERY_FIGURE if f.id in seen and f.id != fid]


def share_readings(session: Session, figure: str) -> str:
    """Borrow every reading this figure declares, and say what happened.

    The provenance paragraph is generated from the declaration rather than
    written at the call site. That is what makes it name the closure: a call
    site knows the source it just asked for, and nothing else."""
    fig = EVERY_BY_ID[figure]
    taken: list[Shared] = []
    missing: list[Shared] = []
    for shared in fig.shares:
        if not shared.republished:
            continue
        got = session.borrow(figure, shared)
        (taken if len(got) == len(shared.republished) else missing).append(shared)
    lines = []
    if taken:
        lines.append(
            "\nShared, not measured again — the same binary, command and input:\n"
            + "".join(f"- {s.what}, from `{s.source}`.\n" for s in taken)
        )
    if missing:
        closure = ", ".join(f"`{f}`" for f in sharing_closure(figure))
        lines.append(
            "\n**Partial sweep**: this session did not emit every figure this table shares a "
            "reading with, so these were measured here instead:\n"
            + "".join(f"- {s.what}, which is `{s.source}`'s.\n" for s in missing)
            + "\nThose are the same measurements and must agree, so the doc now carries two "
            f"numbers for one reading. The whole set that shares readings with this table is "
            f"{closure} — re-take it together before folding any of them in.\n"
        )
    return "".join(lines)


def publication_refusals(figures: Sequence[Figure]) -> list[str]:
    """Why this sitting's tables could not enter the doc, one line per figure.

    A sitting short of the whole sweep publishes outside the session stamp, and
    a figure may only do that when it stands in no edge of the borrow graph in
    either direction — a republished share puts one measurement's number in two
    tables, a derivation makes one table's row a difference over another's reps,
    and either one crossing two sittings is the doc asserting a differencing it
    no longer has. It is the same condition `--check` fails a sitting marker on,
    asked before the measurement is spent rather than after.

    Refusing rather than warning is deliberate: there is no partial version of
    it, and the harness already refuses this way where a selection cannot be
    honestly taken (`resolve_selection` under `--alone`).

    Asked only of a run whose tables could enter the doc at all. `--alone` is
    not exempted from it -- such a run is unpublishable, so the refusal has no
    publication to refuse and stops firing by construction."""
    return [
        f"{fig.id} is read beside {', '.join(entangled)}, so only a sweep can move its table"
        for fig in figures
        if (entangled := entangled_with(fig.id))
    ]


def closure_gaps(figures: Sequence[Figure]) -> list[str]:
    """What a selection shares a reading with and does not take, one line each.

    Said before the first reading rather than discovered at fold-in time. A
    figure here is not necessarily *wrong* — the doc publishes a partial
    sitting so long as it says so — but it is a table whose absolutes may not
    be set beside the ones it shares a reading with, and this is the last
    moment adding them to the selection is free."""
    selected = {f.id for f in figures}
    out = []
    for fig in figures:
        absent = [f for f in sharing_closure(fig.id) if f not in selected]
        if absent:
            out.append(f"{fig.id} — shares a reading with {', '.join(absent)}")
    return out


def derivation_edges() -> list[tuple[str, str, str]]:
    """Every non-republishing borrow, as `(consumer, source, what)`.

    The direction the closure deliberately does not carry: a derived quantity
    is not the source's number a second time, so the two tables are free to be
    published from different sittings — right up until the source is re-taken,
    at which point the consumer's row is a difference over reps the doc no
    longer holds anywhere.

    Naming is the whole remedy. Widening the closure would drag the consumer
    into every sitting that touches its source (`Shared`, above), where what
    the hazard actually needs is that nobody folds the source in without being
    told which table is now derived from readings that are gone. In register
    order, so a report of it reads as a run order."""
    return [
        (fig.id, shared.source, shared.what)
        for fig in EVERY_FIGURE
        for shared in fig.shares
        if not shared.republished
    ]


def derived_consumers(fid: str) -> list[tuple[str, str]]:
    """Figures whose own table is computed over `fid`'s reps, and what of."""
    return [(c, what) for c, source, what in derivation_edges() if source == fid]


def derivation_gaps(figures: Sequence[Figure]) -> list[str]:
    """Consumers of a selected figure's reps that this sitting does not take.

    The counterpart of `closure_gaps` for the derived direction, and said at the
    same moment for the same reason: `--figure nested-end-to-end` is a perfectly
    good sitting, but folding it in strands `cross-file-floor`'s first row, and
    that is knowable before the first reading rather than at fold-in time."""
    selected = {f.id for f in figures}
    return [
        f"{fig.id} — {consumer} derives {what} from its reps"
        for fig in figures
        for consumer, what in derived_consumers(fig.id)
        if consumer not in selected
    ]


# --------------------------------------------------------------------------
# Comment-only commits: the one oracle the harness computes for itself.
# --------------------------------------------------------------------------


#: Suffixes the comment oracle can read. Anything else -- a manifest, a SQL
#: fixture, a snapshot -- is unanalyzable, and unanalyzable is a synonym for
#: "not comment-only": the oracle's whole value is that it never has to be
#: trusted, so every uncertainty resolves against the skip.
_COMMENTABLE = (".rs", ".py", ".md")


def _rust_comment_lines(text: str) -> set[int] | None:
    """The 1-based lines of a Rust source that are comment or blank.

    `None` means the file defeated the scanner, which is the conservative
    answer everywhere: a block comment opened after code on the same line, or
    a `/*` inside a string literal, would need a real lexer to place, and a
    wrong answer here excuses a change that moved a number. Line comments --
    `//`, `///`, `//!` -- are the case this project actually writes, and a
    whole-line `/* ... */` block is read as well."""
    out: set[int] = set()
    depth = 0
    for n, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if depth:
            out.add(n)
            depth += line.count("/*") - line.count("*/")
            if depth < 0:
                return None
            continue
        if not line or line.startswith("//"):
            out.add(n)
            continue
        if line.startswith("/*"):
            out.add(n)
            depth += line.count("/*") - line.count("*/")
            if depth < 0:
                return None
            continue
        if "/*" in line or "*/" in line:
            # Code and a block delimiter on one line: where the comment starts
            # and ends is a lexing question, so the file is not read at all.
            return None
    return out if depth == 0 else None


#: A triple-quoted region counts as a comment only where it *opens* like a
#: docstring -- the line is nothing but the quote, or the quote plus prose.
#: `x = """..."""` is a value, and a value is code.
_PY_DOCSTRING_OPEN = re.compile(r"""^(?:[rRbBuUfF]{0,2})("{3}|'{3})""")


def _python_comment_lines(text: str) -> set[int] | None:
    """The 1-based lines of a Python source that are comment, docstring or blank."""
    out: set[int] = set()
    closer: str | None = None
    for n, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if closer is not None:
            out.add(n)
            if closer in line:
                if line.count(closer) > 1:
                    return None
                closer = None
            continue
        if not line or line.startswith("#"):
            out.add(n)
            continue
        m = _PY_DOCSTRING_OPEN.match(line)
        if not m:
            if '"""' in line or "'''" in line:
                # A triple quote arriving after code: an assigned literal, a
                # nested quote, or the end of something this scanner never saw
                # open. All three are beyond it.
                return None
            continue
        quote = m.group(1)
        out.add(n)
        rest = line[m.end():]
        if quote not in rest:
            closer = quote
        elif rest.count(quote) > 1:
            return None
    return out if closer is None else None


def comment_lines(path: str, text: str) -> set[int] | None:
    """Which of a blob's lines carry nothing a build can see.

    Markdown is *entirely* comment: it compiles to nothing and runs in no
    figure's command shape, so every line of it qualifies and the scanner is
    an arithmetic one."""
    if path.endswith(".md"):
        return set(range(1, len(text.splitlines()) + 1))
    if path.endswith(".rs"):
        return _rust_comment_lines(text)
    if path.endswith(".py"):
        return _python_comment_lines(text)
    return None


_HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def hunk_ranges(diff: str) -> tuple[list[int], list[int]]:
    """The pre-image and post-image line numbers a `-U0` diff touches.

    Read off the hunk headers rather than by counting `+`/`-` lines: with no
    context there is nothing else in a hunk, and a header states both sides'
    ranges outright."""
    removed: list[int] = []
    added: list[int] = []
    for line in diff.splitlines():
        m = _HUNK_RE.match(line)
        if not m:
            continue
        astart, alen, bstart, blen = m.groups()
        alen = 1 if alen is None else int(alen)
        blen = 1 if blen is None else int(blen)
        removed += list(range(int(astart), int(astart) + alen))
        added += list(range(int(bstart), int(bstart) + blen))
    return removed, added


def _git(args: list[str]) -> str | None:
    """`git`, with a failure reported as `None` rather than raised.

    Every caller here is asking a question whose unanswerable case is already
    "not comment-only", so a missing blob, an unresolvable sha or a path that
    did not exist yet all take the same route as a file the scanner cannot
    read."""
    try:
        return subprocess.run(
            ["git", *args], cwd=REPO, capture_output=True, text=True, check=True
        ).stdout
    except (subprocess.CalledProcessError, OSError):
        return None


@functools.lru_cache(maxsize=None)
def comment_only_commit(commit: str, path: str) -> bool:
    """Does this commit change nothing but comments, inside this one path?

    **The third oracle, computed rather than written down.** It used to be a
    claim an acknowledgement entry made -- and three of seven consecutive
    commits on `main` existed only to make it, each one a commit whose own
    diff was comments retargeting citations. The property is syntactic, so the
    harness decides it: every line the commit adds falls inside a comment of
    the post-image, every line it removes fell inside a comment of the
    pre-image, and a file the scanner cannot place is not comment-only.

    Refused before the scanner runs: a merge, which has no single pre-image; a
    file added, deleted or renamed, where "the lines it changed" is the whole
    of it; and a suffix the oracle does not read."""
    if not path.endswith(_COMMENTABLE):
        return False
    sha = _git(["rev-parse", f"{commit}^{{commit}}"])
    if sha is None:
        return False
    sha = sha.strip()
    parents = _git(["rev-list", "--parents", "-n", "1", sha])
    if parents is None or len(parents.split()) != 2:
        return False
    status = _git(["show", "--format=", "--name-status", "-M", sha, "--", path])
    if status is None:
        return False
    kinds = {line.split("\t")[0][:1] for line in status.splitlines() if line.strip()}
    if kinds and kinds != {"M"}:
        return False
    diff = _git(["show", "--format=", "--unified=0", sha, "--", path])
    if not diff:
        # No hunks under this path at all: nothing to excuse, and saying
        # "comment-only" about a path the commit did not touch would be a
        # claim nobody asked for.
        return False
    removed, added = hunk_ranges(diff)
    before = _git(["show", f"{sha}^:{path}"])
    after = _git(["show", f"{sha}:{path}"])
    if before is None or after is None:
        return False
    was = comment_lines(path, before)
    now = comment_lines(path, after)
    if was is None or now is None:
        return False
    return all(n in was for n in removed) and all(n in now for n in added)


# --------------------------------------------------------------------------
# Acknowledged commits: a declared path changed, and no reading moved.
# --------------------------------------------------------------------------

# Both names live in `acknowledged.py` and are imported at the top of this
# file. They are there, not here, because `session-drift` declares this path:
# an entry added to this file would mark stale the figure it is excusing, and
# no entry can name its own sha. That module's docstring has the full reason.


def resolved_acknowledgements(
    acks: Sequence[Acknowledged] = ACKNOWLEDGED,
) -> list[Acknowledged]:
    """`ACKNOWLEDGED` with every commit resolved to a full sha, so comparison
    against `git log` output is not a string-length accident."""
    out = []
    for ack in acks:
        sha = run(["git", "rev-parse", f"{ack.commit}^{{commit}}"], cwd=REPO, capture=True).strip()
        out.append(dataclasses.replace(ack, commit=sha))
    return out


def excuses(acks: Sequence[Acknowledged], commit: str, figure_id: str) -> Acknowledged | None:
    """The acknowledgement covering this commit for this id, if any.

    **A blanket entry reaches every figure and no declared section.** An empty
    `figures` tuple is the claim that no figure's subject can see the change,
    and the readings a figure publishes are durations a sweep takes; a declared
    section's are not, so the blanket claim is not about them. koji's readings
    are a byte-for-byte comparison of what a scan concludes, and excusing those
    is a claim about block offsets and row totals — reachable only by naming
    `koji`, so that its author decided it rather than inheriting it from an
    entry written about timings. The permission itself is unchanged: an entry
    that *names* a declared section still excuses it, which is the discharge
    `measurements.md`, "A commit can be acknowledged" settled, since the
    harness cannot re-take a section it does not own."""
    for ack in acks:
        if ack.commit != commit:
            continue
        if figure_id in ack.figures:
            return ack
        if not ack.figures and figure_id not in NOT_OURS:
            return ack
    return None


def excused_paths(
    figure_id: str,
    hits: Sequence[str],
    commits_by_path: dict[str, Sequence[str]],
    dirty: set[str],
    acks: Sequence[Acknowledged],
    comment_only: Callable[[str, str], bool] = comment_only_commit,
) -> list[str]:
    """Which of a figure's touched paths are settled — by an acknowledgement,
    or by the commit changing nothing but comments inside that path.

    The two settlements are not interchangeable and only one of them is
    written by hand. `comment_only` is passed in so a test can hold the
    register's half on its own, and its default is the real oracle so that a
    caller cannot get the weaker answer by forgetting it.

    Two refusals, and both are the conservative direction:

    - **An uncommitted path is never excused.** There is no commit to point
      at, so nobody has reviewed the diff — a dirty tree under a measured path
      is exactly what `git_head` already calls unpublishable.
    - **Every commit touching the path must be excused, not just one.** A path
      changed by an excused commit and an unexamined one is stale on the
      strength of the second.
    """
    out = []
    for path in hits:
        if path in dirty:
            continue
        commits = commits_by_path.get(path) or ()
        if commits and all(
            excuses(acks, c, figure_id) or comment_only(c, path) for c in commits
        ):
            out.append(path)
    return out


def inert_excuses(
    figure_id: str,
    hits: Sequence[str],
    commits_by_path: dict[str, Sequence[str]],
    acks: Sequence[Acknowledged],
    comment_only: Callable[[str, str], bool] = comment_only_commit,
) -> list[tuple[str, list[str], list[str]]]:
    """Per still-red path: the entries that excuse it, and the commits that do not.

    An entry excuses a commit, so a later unexamined commit on the same path
    leaves the earlier entry correct and doing nothing. `excused_paths` drops
    that path, and the entry then appears in no output at all — which reads
    exactly like a missing entry, and has once been mistaken for one. So a
    stale path that carries at least one excused commit says so, naming the
    commits that are actually holding it red.

    Returns `(path, excused shas, blocking shas)`, only for paths where the
    first list is non-empty; a path nothing excuses is red for the ordinary
    reason and needs no commentary. A comment-only commit is in neither list:
    it is settled without an entry, so naming it as what holds the path red
    would send the reader to a diff that changes nothing.
    """
    out = []
    for path in hits:
        excused, blocking = [], []
        for commit in commits_by_path.get(path) or ():
            if excuses(acks, commit, figure_id):
                excused.append(commit)
            elif not comment_only(commit, path):
                blocking.append(commit)
        if excused:
            out.append((path, excused, blocking))
    return out


def commits_touching(paths: Iterable[str], since: str) -> dict[str, list[str]]:
    """The commits in `since..HEAD` that changed each path."""
    out: dict[str, list[str]] = {}
    for path in paths:
        log = run(
            ["git", "log", "--format=%H", f"{since}..HEAD", "--", path],
            cwd=REPO,
            capture=True,
        )
        out[path] = [line.strip() for line in log.splitlines() if line.strip()]
    return out


def dirty_paths() -> set[str]:
    status = run(["git", "status", "--porcelain"], cwd=REPO, capture=True)
    return {line[3:].strip() for line in status.splitlines() if line.strip()}


def is_ancestor(earlier: str, later: str) -> bool:
    proc = subprocess.run(
        ["git", "merge-base", "--is-ancestor", earlier, later],
        cwd=str(REPO),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return proc.returncode == 0


def drift_table(first: Path, second: Path) -> str:
    """Two sweeps of the same figures, differenced reading by reading.

    The standing rules assert session-to-session drift of "~10%" on the
    strength of history rather than a measurement, and the ninth rule's
    "under ~0.5 µs/row is apparatus" corollary leans on it. Two sweeps taken
    the same day, on identical inputs and binaries, are the only reading of the
    instrument itself this apparatus has ever had."""
    a = json.loads(first.read_text())
    b = json.loads(second.read_text())
    shared = sorted(set(a["readings"]) & set(b["readings"]))
    if not shared:
        raise ValueError("the two sweeps share no reading; they measured different figures")
    rows, deltas = [], []
    for key in shared:
        first_v, second_v = a["readings"][key], b["readings"][key]
        if not first_v or not second_v:
            continue
        m1, m2 = median(first_v), median(second_v)
        pct = (m2 - m1) / m1 * 100
        deltas.append(abs(pct))
        figure, _, reading = key.partition("/")
        rows.append([figure, f"`{reading}`", fmt_s(m1), fmt_s(m2), f"{pct:+.1f}%"])
    table = md_table(
        [
            "Figure",
            "Reading",
            f"sweep 1 ({a['date']})",
            f"sweep 2 ({b['date']})",
            "Δ",
        ],
        rows,
    )
    return (
        table
        + f"\n\nBoth sweeps ran against commit `{a['commit']}` and `{b['commit']}` on identical "
        f"inputs and binaries. Over {len(deltas)} shared readings the median absolute drift is "
        f"**{median(deltas):.1f}%** and the largest is **{max(deltas):.1f}%**.\n"
    )


def _fmt_fine(value: float) -> str:
    """Seconds at the precision the arms reports read: to the microsecond,
    which `pgdt sql`'s image reports (`TIME_FORMAT`)."""
    return f"{value:.6f}"


def relative_spread(values: Sequence[float]) -> float:
    """A leg's spread within one sitting, as a percentage of its median."""
    lo, hi = spread(values)
    return (hi - lo) / median(values) * 100


def _arm_readings(raw: dict, which: str) -> tuple[list[str], dict[str, dict[str, list[float]]]]:
    arms = raw.get("arms")
    if not arms or len(arms.get("names", ())) < 2:
        raise ValueError(
            f"{which} took one arm, so there is nothing to set beside it: take it with "
            "`--pin-cpus alternate` or `--stage-binaries alternate`"
        )
    return arms["names"], arms["readings"]


def _shared_keys(*tables: Mapping[str, Sequence[float]]) -> list[str]:
    keys = set(tables[0])
    for table in tables[1:]:
        keys &= set(table)
    return sorted(k for k in keys if all(table[k] for table in tables))


def arms_table(raw: dict) -> str:
    """One sitting's arms set beside each other, reading by reading: each
    arm's median, its spread within the sitting, and each later arm's move
    against the first.

    What it prices is a term both arms' readings share every minute of the
    machine with, which is why `alternate` takes them leg by leg in one
    sitting — the staged cold figure's Δ is what staging removes from a cold
    absolute. What it cannot price is drift, which only `--drift` across two
    sittings reads."""
    names, readings = _arm_readings(raw, "this sitting")
    keys = _shared_keys(*(readings.get(n, {}) for n in names))
    if not keys:
        raise ValueError("the arms share no reading")
    first = names[0]
    rows, wider = [], {n: 0 for n in names[1:]}
    moves: dict[str, list[float]] = {n: [] for n in names[1:]}
    for key in keys:
        figure, _, reading = key.partition("/")
        row = [figure, f"`{reading}`"]
        for n in names:
            vals = readings[n][key]
            lo, hi = spread(vals)
            row.append(
                f"{_fmt_fine(median(vals))} ({_fmt_fine(lo)}–{_fmt_fine(hi)}, "
                f"{relative_spread(vals):.1f}%)"
            )
        base = median(readings[first][key])
        for n in names[1:]:
            pct = (median(readings[n][key]) - base) / base * 100
            moves[n].append(pct)
            row.append(f"{pct:+.2f}%")
            if relative_spread(readings[n][key]) > relative_spread(readings[first][key]):
                wider[n] += 1
        rows.append(row)
    table = md_table(
        ["Figure", "Reading", *(f"{n}: median (range, spread)" for n in names),
         *(f"{n} against {first}" for n in names[1:])],
        rows,
    )
    summary = "".join(
        f"\n- **{n}** against **{first}**: median move {median(moves[n]):+.2f}%, "
        f"range {min(moves[n]):+.2f}% to {max(moves[n]):+.2f}%; spread wider than "
        f"{first}'s on {wider[n]} of {len(keys)} readings."
        for n in names[1:]
    )
    return f"{table}\n\nCommit `{raw['commit']}`, {raw['date']}, {len(keys)} readings.\n{summary}\n"


def arm_drift_table(a: dict, b: dict) -> str:
    """Two sittings' arms, each differenced across the pair as
    `drift_table` differences one.

    **The criterion it is read against is `M178`'s, written before any
    sitting** (`docs/status/history/2026-09-28.md`): pinning is adopted only
    if its median absolute move between sittings is **at most half** the
    unpinned arm's, over the pairs of three sittings, each gap recorded rather
    than set, **and no leg's spread widens** — a pinned leg whose spread within a sitting exceeds
    the unpinned leg's in that same sitting counts against it. Either half
    failing refutes it; the table prints both, and the ratio, rather than
    deciding over pairs it sees one of."""
    names_a, ra = _arm_readings(a, "the first sitting")
    names_b, rb = _arm_readings(b, "the second sitting")
    names = [n for n in names_a if n in names_b]
    if len(names) < 2:
        raise ValueError("the two sittings share fewer than two arms")
    first = names[0]
    rows, drift = [], {}
    for n in names:
        keys = _shared_keys(ra.get(n, {}), rb.get(n, {}))
        deltas = [
            abs((median(rb[n][k]) - median(ra[n][k])) / median(ra[n][k]) * 100) for k in keys
        ]
        if not deltas:
            raise ValueError(f"arm {n} shares no reading across the two sittings")
        drift[n] = median(deltas)
        rows.append([n, str(len(keys)), f"**{median(deltas):.2f}%**", f"{max(deltas):.2f}%"])
    table = md_table(["Arm", "Readings", "Median absolute move", "Largest"], rows)
    lines = []
    for n in names[1:]:
        ratio = drift[n] / drift[first] if drift[first] else float("inf")
        wider = []
        for which, raw in (("sitting 1", ra), ("sitting 2", rb)):
            keys = _shared_keys(raw.get(first, {}), raw.get(n, {}))
            count = sum(
                relative_spread(raw[n][k]) > relative_spread(raw[first][k]) for k in keys
            )
            wider.append(f"{count} of {len(keys)} in {which}")
        lines.append(
            f"- **{n}**: its drift is {ratio:.2f}× **{first}**'s (at most 0.50 is the "
            f"criterion); legs whose spread is wider than {first}'s: {', '.join(wider)} "
            "(none is the criterion)."
        )
    return (
        f"{table}\n\nSittings at `{a['commit']}` ({a['date']}) and `{b['commit']}` "
        f"({b['date']}).\n\n" + "\n".join(lines) + "\n"
    )


def cmd_arms(run_dir: str) -> int:
    print(arms_table(json.loads((Path(run_dir) / "raw.json").read_text())))
    return 0


def cmd_drift(first: str, second: str) -> int:
    fig = ALL_BY_ID["session-drift"]
    # A derived figure is computed across two sweeps of one commit, and that
    # commit is usually the stamp's -- the sweep pair is what stamps the doc.
    # Where it is not, this table is published outside the stamp like any
    # other and declares where it came from. A pair whose legs disagree
    # declares nothing: nothing was to be committed between them, so the table
    # is already invalid and its own closing sentence names both commits.
    raws = [json.loads((Path(d) / "raw.json").read_text()) for d in (first, second)]
    commits = {raw["commit"] for raw in raws}
    stamp = stamped_commit(REPO / "docs/design/measurements.md")
    taken = commits.pop() if len(commits) == 1 else None
    sitting = (
        taken
        if taken and stamp and resolve_commit(taken) != resolve_commit(stamp)
        else None
    )
    # Named beside the sitting, as any sitting marker names it, where the two
    # sweeps agree on the register image's glibc — which one sweep pair
    # re-taking one apparatus does unless the image was re-pinned between them.
    glibcs = {raw.get("glibc") for raw in raws}
    glibc = glibcs.pop() if sitting and len(glibcs) == 1 else None
    print(f"## {fig.section}\n")
    print(figure_marker(fig.id, sitting, reproduce="--drift <sweep> <sweep>", glibc=glibc) + "\n")
    print(drift_table(Path(first) / "raw.json", Path(second) / "raw.json"))
    # A pair of experiment sittings is read arm by arm as well; `drift_table`
    # above is the first arm's, which is what every table renders.
    if all(len(raw.get("arms", {}).get("names", ())) > 1 for raw in raws):
        print(arm_drift_table(*raws))
    return 0


@dataclass(frozen=True)
class Outside:
    """A `measurements.md` section the figure register does not hold.

    Every *figure* announces itself twice — a marker and an apparatus line —
    and a section outside the register used to announce itself with nothing,
    while the doc's session stamp asserted "every figure below was taken by
    `scripts/measure.py`" over it. So a section outside the register carries an
    `<!-- outside-register: <id> -->` marker of its own, and `--check`
    reconciles those markers against this register both ways and holds each
    declared section to carrying no figure marker. A section cannot be both.

    **What makes staying outside safe rather than merely silent is `depends`.**
    Being outside the register says the harness cannot re-take the readings; it
    was also saying, by omission, that nothing would ever be told when they went
    wrong — `--stale` read `ALL_BY_ID` and could not name a declared section
    however far its inputs moved. koji went four campaigns that way, and the
    claim `peak-rss` was registered to rescue had sat in exactly that blind spot
    for a whole slice. So a declared section carries the same invalidation edge
    a figure does, and `--stale` reports it beside them. What it cannot carry is
    the other half of a figure's contract — the harness will not re-take it — so
    a red here is discharged by a run somebody makes by hand, or by an
    acknowledgement, exactly as a figure's is.

    **The commit the readings were taken at lives in the doc's marker, not
    here.** It is the argument the figure sittings already settled
    (`measurements.md`, "A figure may be published outside the sweep"), one
    mechanism along: `session-drift` declares `scripts/measure.py`, so recording
    a koji run's commit in this file would mark a figure stale for recording
    where another reading came from, every time koji is re-run. The marker is
    already the doc's convention for provenance, and `outside_sittings` reads it
    back out."""

    id: str
    #: The doc heading it sits under, for `--list` and for the check's report.
    section: str
    #: Why the harness does not own it. Printed by `--list`.
    why: str
    #: Repo-relative paths whose change invalidates the readings this section
    #: publishes — a figure's `depends`, for a section that is not one. Empty
    #: is right only where the section publishes no number the design quotes,
    #: which is the very thing that puts `benches` outside; `--check` holds a
    #: section that declares paths to declaring the commit it was taken at, and
    #: one that declares none to declaring no commit.
    depends: tuple[str, ...] = ()


#: Sections measurements.md carries that this harness deliberately does not
#: own — the other half of the boundary `--check` reconciles.
NOT_OURS = {
    o.id: o
    for o in [
        Outside(
            "koji",
            "koji full scan",
            "784 GB on the HDD, ~54 minutes, a different medium, and a byte-for-byte regression "
            "check rather than a throughput figure. The harness owns the *invocation* — "
            "`--koji-recipe`, which prints it — and never runs it.",
            # What a koji run publishes is what a scan *concludes*: the block
            # list, the per-block offsets, the row and byte totals, and the
            # `info --detail` report they are compared as text against. So the
            # read path, the scanner, the map and the cache, which decide the
            # first four, and the preamble prepass, the type resolution and the
            # CLI, which decide the fifth — the last of those is not
            # hypothetical, the 2026-09-05 comparison differing from the
            # 2026-08-27 one by exactly one line of user-defined-type detail the
            # report had gained in between. `resolve.rs` and `pgtype.rs` are
            # named here rather than in a shared constant because this is the
            # only section in the document whose readings a change to them can
            # falsify: every figure that resolves a type publishes a duration,
            # and a duration is not what the report's text identity is.
            depends=(
                *READ,
                *SCAN,
                *MAP,
                *CACHE,
                *PREAMBLE,
                "pgdump_query/src/resolve.rs",
                "pgdump_query/src/pgtype.rs",
                *QUERY_CLI,
            ),
        ),
        Outside(
            "benches",
            "benches/decoders.rs per-type pairs, benches/whole_file.rs",
            "Tripwires, not figures: they quote no number in the doc, so there is no table to "
            "emit. That is a decision, not an oversight -- `cargo bench -p pgdump_query` runs "
            "them.",
            # No edge, and the empty tuple is the claim rather than an omission:
            # this section publishes no reading, so there is nothing a diff
            # could invalidate. It is the same sentence that puts it outside the
            # register, said in the field that would otherwise have to be
            # guessed at.
            depends=(),
        ),
    ]
}


def resolve_selection(ids: Iterable[str], alone: bool = False) -> list[Figure]:
    """Selected figures plus whatever they borrow from, in run order.

    `alone` takes exactly what was named, which is how a **diagnostic** sitting
    is asked for: the borrowed rows are then measured by the figure itself, its
    table carries the harness's partial-sweep note, and the whole run is marked
    unpublishable (`Config.unpublishable_reason`). It is refused where a figure
    *consumes* another's readings without republishing them, since there is no
    local measurement for that figure to fall back on -- the reading it wants
    is the other figure's reps, not a run it could take."""
    wanted: set[str] = set()

    def add(fid: str) -> None:
        if fid in wanted:
            return
        if fid not in SELECTABLE_BY_ID:
            raise SystemExit(f"unknown figure {fid!r}; `--list` names them all")
        wanted.add(fid)
        if alone:
            return
        for dep in SELECTABLE_BY_ID[fid].requires:
            add(dep)

    for fid in ids:
        add(fid)
    if alone:
        for fid in sorted(wanted):
            for shared in SELECTABLE_BY_ID[fid].shares:
                if not shared.republished and shared.source not in wanted:
                    raise SystemExit(
                        f"--alone refuses {fid}: it reads {shared.source}'s own reps "
                        f"({shared.what}) and cannot measure them for itself"
                    )
    return [f for f in SELECTABLE if f.id in wanted]


# --------------------------------------------------------------------------
# Staleness: which figures a diff has invalidated.
# --------------------------------------------------------------------------

#: The stamp is prose and prose wraps, so this spans lines — with a bounded
#: lazy run rather than an open one, so it cannot leap from the word
#: "measure.py" in one paragraph to a commit hash pages later.
STAMP_RE = re.compile(
    r"measure\.py.{0,200}?commit `([0-9a-f]{7,40})`", re.IGNORECASE | re.DOTALL
)

#: How the doc names a figure. A heading is free to quote a number and free to
#: change when the number moves -- so the harness must not address a section by
#: its title. The marker is the id, and it is what `--check` reconciles.
MARKER_RE = re.compile(r"<!--\s*figure:\s*([a-z0-9-]+)")


def markers_in(doc: Path) -> list[str]:
    """Every figure id `measurements.md` claims to carry, in order."""
    return MARKER_RE.findall(doc.read_text())


#: How a figure says it was taken **outside** the sitting the session stamp
#: names. The commit goes *inside* that figure's own marker rather than in a
#: sibling comment: a sibling can go missing on its own and its absence is
#: silent, and a figure missing one reads as stamp-sitting, which is the single
#: error this mechanism exists to make impossible. It is present only where the
#: sitting differs from the stamp -- the convention `outside-register` already
#: uses, where declaring nothing is the ordinary case -- so the stamp's commit
#: is never repeated once per figure in a document it could disagree with.
SITTING_RE = re.compile(r"<!--\s*figure:\s*([a-z0-9-]+)[^>]*?taken at `([0-9a-f]{7,40})`")


def figure_marker(
    fid: str,
    sitting: str | None = None,
    reproduce: str | None = None,
    glibc: str | None = None,
) -> str:
    """The marker a table is emitted under: the figure's id, the commit it was
    taken at and the glibc it ran under where the stamp does not speak for
    them, and how to reproduce it.

    Written in one place so that what `emit` puts above a table is by
    construction what `markers_in`, `figure_sittings` and `marker_glibcs` read
    back out of the doc after the paste."""
    reproduce = reproduce or f"--figure {fid}"
    taken = f" — taken at `{sitting}`" if sitting else ""
    under = f" — under glibc {glibc}" if glibc else ""
    return (
        f"<!-- figure: {fid}{taken}{under} — reproduce with "
        f"`cd scripts && uv run measure.py {reproduce}` -->"
    )


#: A glibc version as `glibc_named` writes one, and the two places it is read
#: back from: a figure's marker, and the session stamp's clause after its
#: commit — bounded, and barred from a full stop, so it cannot run on into the
#: accounting sentence or the next paragraph.
_GLIBC_VERSION = r"(\d+(?:\.\d+)+(?: and \d+(?:\.\d+)+)*)"
MARKER_GLIBC_RE = re.compile(rf"<!--\s*figure:\s*([a-z0-9-]+)[^>]*?under glibc {_GLIBC_VERSION}")
STAMP_GLIBC_RE = re.compile(
    rf"measure\.py.{{0,200}}?commit `[0-9a-f]{{7,40}}`[^.]{{0,120}}?glibc {_GLIBC_VERSION}",
    re.IGNORECASE | re.DOTALL,
)


def marker_glibcs(text: str) -> dict[str, str]:
    """Each figure whose marker names the glibc it ran under, and that glibc."""
    return dict(MARKER_GLIBC_RE.findall(text))


def stamp_glibc(text: str) -> str | None:
    """The glibc the session stamp names, read out of the doc's text."""
    match = STAMP_GLIBC_RE.search(text)
    return match.group(1) if match else None


def marker_glibc(whole_sweep: bool, stamped: str | None, ran_under: str | None) -> str | None:
    """The glibc a figure's marker names: its own, wherever the stamp does not
    speak for it.

    The stamp names the register image's, so a figure of the sweep whose
    program ran there names nothing — `taken at`'s convention, the datum
    present only where it differs. It differs for a program run in another
    place (`cargo bench` on the host), and for
    every figure of a sitting of its own, whose marker already names the commit
    the stamp does not and names the glibc beside it."""
    if ran_under is None or (whole_sweep and ran_under == stamped):
        return None
    return ran_under


def glibc_problems(text: str) -> list[str]:
    """Where the doc does not say which glibc a figure ran under, one line each:
    a stamp naming none, or a sitting marker naming none. A figure of the
    sweep run outside the register's image is the harness's to mark, and is not
    visible from the doc.

    **A stamp or sitting older than `glibc_of` is not exempt.** The ones
    published before it name a glibc read off the image records and
    `pacman.log` (`docs/status/history/2026-09-27.md`, "`M174`: the images are
    pinned, and each figure names its glibc"), which is as certain as asking: a
    digest is immutable and each tag's record is unmoved since before its
    sitting. The next sitting of each replaces them, so an exemption would be a
    cutoff kept here for a state the next sweep ends. A `--render` of one of
    those sittings names no glibc, its record never having held one, so this
    refuses the paste until the values are restored from that entry."""
    out = []
    if stamp_in(text) is not None and stamp_glibc(text) is None:
        out.append(
            "the session stamp names no glibc, so nothing says which one the figures it "
            "covers ran under"
        )
    named = marker_glibcs(text)
    for fid, sha in sorted(figure_sittings(text).items()):
        if fid not in named:
            out.append(
                f"{fid} declares a sitting of its own ({sha}) and names no glibc — write "
                f"`under glibc <version>` inside its marker"
            )
    return out


def figure_sittings(text: str) -> dict[str, str]:
    """Each figure the doc carries from a sitting of its own, and its commit.

    This is the datum every reader of the session stamp argues from: `--stale`
    ranges each figure from here, an acknowledgement is spent per figure
    against it, and `--verify-additive` regenerates a figure's inputs at it.
    Half-applying that -- one reader still reasoning from a commit the doc
    itself says is not the figure's -- is the same defect one level down."""
    return dict(SITTING_RE.findall(text))


#: How a section says it is *outside* the register. The symmetry with
#: `MARKER_RE` is the point: a figure announces itself and so does a section
#: that is not one, so "is this table one of ours" is read off the doc rather
#: than off a prose sentence some paragraphs away (`measurements.md`, "The
#: apparatus").
OUTSIDE_RE = re.compile(r"<!--\s*outside-register:\s*([a-z0-9-]+)")

#: And how it says when its readings were taken, which is what gives `--stale`
#: a range to argue a declared section's `depends` over. Same shape as
#: `SITTING_RE` and for the same reasons: inside the section's own marker, so
#: it cannot go missing on its own, and in the document rather than in this
#: file, which `session-drift` declares.
OUTSIDE_SITTING_RE = re.compile(
    r"<!--\s*outside-register:\s*([a-z0-9-]+)[^>]*?taken at `([0-9a-f]{7,40})`"
)


def outside_sittings(text: str) -> dict[str, str]:
    """Each declared section that names the commit its readings were taken at.

    Absent for a section that publishes no reading — `benches` — where there is
    no run to date and nothing for a range to be measured from."""
    return dict(OUTSIDE_SITTING_RE.findall(text))


def outside_bases(text: str, override: str | None = None) -> dict[str, str | None]:
    """The commit each declared section's readings are argued from.

    Only the sections that declare an invalidation edge: one that declares none
    publishes nothing a diff can falsify, so it has no staleness to have a base
    for. `override` is `--since`, which asks one question of the whole document
    and so is not per section — the same contract `figure_bases` has."""
    declared = outside_sittings(text)
    return {
        o.id: (override or declared.get(o.id)) for o in NOT_OURS.values() if o.depends
    }


def outside_sitting_problems(
    text: str,
    resolve: Callable[[str], str | None] = resolve_commit,
) -> list[str]:
    """Why a declared section's provenance line may not stand, one line each.

    The two halves of the field pair have to agree, and neither failure is
    visible from the document alone. A section declaring paths but no commit
    reads as a live edge and is inert — `--stale` has no range to intersect,
    which is the state koji was in before it declared anything, arrived at from
    the other side. A section declaring a commit but no paths asserts a
    provenance nothing reads, which is how a field stops being maintained."""
    declared = outside_sittings(text)
    out: list[str] = []
    for oid, outside in sorted(NOT_OURS.items()):
        sha = declared.get(oid)
        if outside.depends and sha is None:
            out.append(
                f"{oid} declares an invalidation edge and no commit its readings were taken "
                "at, so nothing can say what they have gone stale against — write "
                f"`taken at ` inside its `<!-- outside-register: {oid} … -->` marker"
            )
        elif sha is not None and not outside.depends:
            out.append(
                f"{oid} declares the commit {sha} and no invalidation edge, so nothing reads "
                "that commit; either give `measure.NOT_OURS` the paths its readings depend on "
                "or drop the provenance from its marker"
            )
        elif sha is not None and resolve(sha) is None:
            out.append(f"{oid} declares {sha}, which is not a commit in this repository")
    for oid in sorted(set(declared) - set(NOT_OURS)):
        out.append(
            f"{oid} names the commit {declared[oid]} and is no section this harness disowns"
        )
    return out

#: An ATX heading. Matched per line rather than with `re.MULTILINE` over the
#: whole text, because the doc's shell blocks contain comment lines that start
#: with `#` and would otherwise cut a section in half.
HEADING_RE = re.compile(r"^(#{1,6}) +\S")


def headings(text: str) -> list[tuple[int, int]]:
    """Every heading's offset and level, code fences excluded.

    The fence skip is load-bearing: `measurements.md` publishes a `sh` block
    whose first line is `# maps to EOF (the table never matches) and never
    saves`, which reads as a level-1 heading and would end the section it is
    printed inside."""
    out: list[tuple[int, int]] = []
    pos = 0
    fenced = False
    for line in text.splitlines(keepends=True):
        if line.startswith("```"):
            fenced = not fenced
        elif not fenced:
            match = HEADING_RE.match(line)
            if match:
                out.append((pos, len(match.group(1))))
        pos += len(line)
    return out


def section_label_problems(text: str) -> list[str]:
    """Each figure whose `section` label does not begin with the heading its
    marker sits under, one line each.

    The label renders as a sitting's heading, so a label left quoting the
    number its heading was rewritten from would print that number at the next
    sweep. "Begin with" rather than "equal" leaves room for a suffix telling
    two figures under one heading apart, as `map-only`'s does. A marker under
    no heading has no heading to match, and says so.

    **A declared section's label is not held.** `Outside.section` is read only
    by `--list`, beside its id, and never renders as a heading, so the stale
    number this guards against cannot reach print through one; `benches`'
    label names its files instead, which is what `--list` wants of it."""
    marks = headings(text)
    out = []
    for match in MARKER_RE.finditer(text):
        fig = ALL_BY_ID.get(match.group(1))
        if fig is None:
            continue
        above = [pos for pos, _ in marks if pos < match.start()]
        if not above:
            out.append(f"{fig.id} sits under no heading")
            continue
        line = text[above[-1] :].split("\n", 1)[0]
        heading = line.lstrip("#").strip()
        if not fig.section.startswith(heading):
            out.append(f"{fig.id}: label {fig.section!r}, heading {heading!r}")
    return out


def outside_register_sections(text: str) -> list[tuple[str, list[str]]]:
    """Each section the doc declares outside the register, and the figure
    markers found inside it — which must be none.

    A section runs from its heading to the next heading at or above its own
    level, so a declared section covers its own subheadings. A marker under no
    heading at all takes the text above the first one, which is the preamble:
    the sentence scoping the session stamp lives there, and it is not a
    section anybody may declare."""
    marks = headings(text)
    found = []
    for match in OUTSIDE_RE.finditer(text):
        above = [(pos, level) for pos, level in marks if pos < match.start()]
        if above:
            start, level = above[-1]
            below = [pos for pos, lvl in marks if pos > start and lvl <= level]
        else:
            start = 0
            below = [pos for pos, _ in marks]
        end = below[0] if below else len(text)
        found.append((match.group(1), MARKER_RE.findall(text[start:end])))
    return found


#: The lead-in `share_readings` writes when a borrow could not be satisfied,
#: and the one the doc carries verbatim in substance when such a table is
#: published (`measurements.md`, "The apparatus").
PARTIAL_RE = re.compile(r"\*\*Partial sweep\*\*")


def partial_sittings(text: str) -> list[tuple[str, list[str]]]:
    """Tables the doc publishes that measured a reading they share.

    One reading, two numbers: the figure named and the figures it shares with
    each carry a measurement of the same run. The doc **permits** this, so long
    as the harness's note is carried with the table -- which is why `--check`
    reports it rather than failing on it. What it buys is that the set to
    re-take is named at the moment someone is reading the doc to decide, rather
    than recomputed by hand from the harness's source.

    Attribution is by position: a note belongs to the nearest figure marker
    above it, which is how the emitted table is laid out."""
    marks = [(m.start(), m.group(1)) for m in MARKER_RE.finditer(text)]
    seen: list[str] = []
    for note in PARTIAL_RE.finditer(text):
        above = [fid for pos, fid in marks if pos < note.start()]
        if above and above[-1] not in seen:
            seen.append(above[-1])
    return [(fid, sharing_closure(fid)) for fid in seen]


def stamp_in(text: str) -> str | None:
    """The commit a session stamp names, read out of the doc's text."""
    match = STAMP_RE.search(text)
    return match.group(1) if match else None


def stamped_commit(doc: Path) -> str | None:
    """The commit the doc's session stamp names, so `--stale` has a default."""
    return stamp_in(doc.read_text())


def figure_bases(text: str, override: str | None = None) -> dict[str, str | None]:
    """The commit each figure is argued from: its own sitting where its marker
    declares one, the session stamp otherwise.

    **One function, every reader of the stamp.** `--stale`, acknowledgement
    spentness and `--verify-additive` all used to read the stamp directly, and
    each of them was then wrong for a figure taken elsewhere -- reporting it
    stale against commits it *postdates*, spending an acknowledgement its range
    cannot reach, and regenerating its inputs at a revision it was not taken at.

    `override` is `--since`, which asks a deliberate question of every figure at
    once ("what has moved since X") and so is not per figure. `None` for a
    figure means nothing says where to argue from: no marker and no stamp."""
    if override:
        return {fid: override for fid in ALL_BY_ID}
    stamp = stamp_in(text)
    sittings = figure_sittings(text)
    return {fid: sittings.get(fid, stamp) for fid in ALL_BY_ID}


def entangled_with(fid: str) -> list[str]:
    """Every figure whose readings this one's table cannot be separated from.

    Both edges of the borrow graph in both directions: a republished share
    puts one measurement's number in two tables, and a derivation makes one
    table's row a difference over another's reps. A figure with neither may be
    published from a sitting of its own, because what one sitting buys is the
    ability to *difference* these tables against each other, and a figure that
    stands in no edge endangers none of it."""
    fig = EVERY_BY_ID.get(fid)
    consumed = [s.source for s in fig.shares if not s.republished] if fig else []
    return sorted({*sharing_closure(fid), *(c for c, _ in derived_consumers(fid)), *consumed})


def sitting_problems(
    sittings: Mapping[str, str],
    stamp: str | None,
    resolve: Callable[[str], str | None] = resolve_commit,
    ancestor: Callable[[str, str], bool] | None = None,
) -> list[str]:
    """Why a figure may not carry the sitting its marker declares, one line each.

    Three refusals, and the third is the one that looks entirely plausible in
    the doc:

    - **A figure that shares or is derived from may not be published outside
      the sweep at all.** That is the doc asserting a differencing that crosses
      sittings, and it is the same class of error as a section declared outside
      the register while carrying a figure marker.
    - **A sitting that names the stamp's own commit is a marker that should not
      be there.** A figure the stamped sweep took declares nothing, so a marker
      repeating the stamp is a second copy of one fact that can go stale
      against it.
    - **A sitting must descend from the stamp.** An older one cannot
      legitimately exist -- a sweep replaces every table at once, so a figure
      the sweep took carries no marker and a figure it did not take was folded
      in later -- which makes a non-descendant either a marker a sweep left
      behind or a hand edit, both of which republish a fresh table under a lying
      provenance and put `--stale` back on the wrong commit. It costs one
      `is_ancestor` call."""
    ancestor = ancestor or is_ancestor
    out: list[str] = []
    for fid, sha in sorted(sittings.items()):
        if fid not in EVERY_BY_ID:
            # `--check`'s unknown-marker report already names it; saying it
            # twice would make one rename look like two problems.
            continue
        entangled = entangled_with(fid)
        if entangled:
            out.append(
                f"{fid} declares a sitting of its own ({sha}) and is read beside "
                f"{', '.join(entangled)} — a figure published outside the stamped sweep must "
                "share no reading and stand in no derivation, since one sitting is what lets "
                "these tables be differenced against each other"
            )
            continue
        if stamp is None:
            out.append(
                f"{fid} declares a sitting ({sha}) in a document whose session stamp names no "
                "commit, so there is nothing for it to be outside of"
            )
            continue
        got, base = resolve(sha), resolve(stamp)
        if got is None:
            out.append(f"{fid} declares {sha}, which is not a commit in this repository")
            continue
        if base is None:
            out.append(
                f"{fid} declares {sha}, but the session stamp's {stamp} is not a commit in this "
                "repository, so nothing can say whether the sitting descends from it"
            )
            continue
        if got == base:
            out.append(
                f"{fid} declares the stamp's own commit ({sha}); a figure the stamped sweep took "
                "carries no sitting marker, and one that carries none is read from the stamp"
            )
            continue
        if not ancestor(base, got):
            out.append(
                f"{fid} declares {sha}, which does not descend from the stamp's {stamp} — a "
                "sitting older than the stamp cannot legitimately exist, so this is a marker a "
                "sweep left behind or a hand edit, and either republishes a fresh table under a "
                "lying provenance"
            )
    return out


def sitting_accounting(outside: Sequence[tuple[str, str]], total: int | None = None) -> str:
    """The stamp's accounting of which figures came from the sitting it names.

    **Generated, not reconciled.** `session_stamp` already computes the harder
    half of the sentence, and the count beside it was hand-maintained — which
    is exactly the thing that goes wrong silently: the sentence claiming all
    eighteen markers was false for as long as one figure stood outside the
    sweep and said so only in prose three paragraphs away."""
    total = len(ALL_FIGURES) if total is None else total
    if not outside:
        return f"**All {total} figures below come from that sitting.**"
    named = ", ".join(f"`{fid}` (`{sha}`)" for fid, sha in outside)
    rest = (
        "The other carries its own sitting commit inside its marker, and every reader of this "
        "stamp argues from that instead"
        if len(outside) == 1
        else f"The other {len(outside)} carry their own sitting commits inside their markers, "
        "and every reader of this stamp argues from those instead"
    )
    return f"**{total - len(outside)} of the {total} figures below come from that sitting.** {rest}: {named}."


def declared_hits(fig: Figure | Outside, changed: Iterable[str]) -> list[str]:
    """The paths in `changed` that `fig` declares. A declared path is a prefix:
    a directory matches everything under it.

    **One predicate, every caller.** `--stale` argues from it that a figure has
    gone stale, and the same way over a section the register does not hold.
    Writing either separately would make it a second authority over what can
    move a reading — and an `Outside` declares its edge in the same field a
    `Figure` does precisely so that one predicate still answers for both."""
    return sorted({c for c in changed for d in fig.depends if c == d or c.startswith(d)})


def figures_touched(changed: Iterable[str]) -> list[tuple[Figure, list[str]]]:
    """Figures whose declared paths a diff touches.

    **Every figure, not just the ones a sweep runs.** The derived
    `session-drift` declares `scripts/measure.py` because the harness's own
    timing path is the apparatus it measures; iterating `FIGURES` here left
    that declaration inert, which is the failure mode a declared dependency
    exists to prevent.
    """
    changed = list(changed)
    out = []
    for fig in ALL_BY_ID.values():
        hits = declared_hits(fig, changed)
        if hits:
            out.append((fig, hits))
    return out


def changed_paths(since: str) -> list[str]:
    diff = run(["git", "diff", "--name-only", since], cwd=REPO, capture=True)
    status = run(["git", "status", "--porcelain"], cwd=REPO, capture=True)
    paths = [line.strip() for line in diff.splitlines() if line.strip()]
    paths += [line[3:].strip() for line in status.splitlines() if line.strip()]
    return sorted(set(paths))


# --------------------------------------------------------------------------
# The run.
# --------------------------------------------------------------------------


def git_head() -> tuple[str, bool]:
    """The commit the figures were taken against, and whether the tree differs
    from it *in a way that could move a number*.

    An uncommitted doc or a new harness file cannot change a reading, and
    calling the run unpublishable for one would make the check noise. What
    counts is an uncommitted change under some figure's declared paths --
    exactly the predicate `--stale` uses."""
    head = run(["git", "rev-parse", "--short", "HEAD"], cwd=REPO, capture=True).strip()
    status = run(["git", "status", "--porcelain"], cwd=REPO, capture=True)
    changed = [line[3:].strip() for line in status.splitlines() if line.strip()]
    return head, bool(figures_touched(changed))


def session_stamp(
    head: str,
    dirty: bool,
    allocator: str | None = None,
    outside: Sequence[tuple[str, str]] = (),
    glibc: str | None = None,
) -> str:
    """The line the doc carries, and the line `stamped_commit` reads back.

    The allocator is part of it because a figure here is a **CLI** figure,
    taken under whatever `pgdt` links against -- see `measurements.md`, "Which
    allocator a figure was taken under". It is read out of the binary rather
    than assumed, so the day the CLI's default changes the stamp changes with
    it; `None` where there is no binary to ask, which is `--dry-run` and the
    unit tests. **The glibc is the register image's**, asked of the image
    (`glibc_of`) for the same reason; a figure whose program ran anywhere else
    names its own in its marker (`marker_glibc`).

    **It is scoped to the register, not to everything printed below it.** The
    doc also carries sections this harness does not own, and the stamp's claim
    was false over them for as long as it said "every figure below"; the scope
    is what a section's `<!-- outside-register: <id> -->` marker declares it
    out of, and `--check` holds the two in step.

    **`outside` is the figures the doc carries from a sitting of their own**,
    and the sentence accounting for them is generated here rather than written
    by hand beside it — see `sitting_accounting`."""
    return (
        f"**Session stamp.** Every figure below — every section carrying a "
        f"`<!-- figure: … -->` marker, and no other — was taken by `scripts/measure.py` on "
        f"{date.today().isoformat()}, against commit `{head}`"
        f"{taken_against(dirty, allocator, glibc)}. "
        + sitting_accounting(outside)
    )


def taken_against(dirty: bool, allocator: str | None, glibc: str | None = None) -> str:
    """What qualifies a commit in a stamp: the tree's state, the allocator and
    the register image's glibc.

    Shared with the header a *partial* sitting writes instead of a stamp, so
    that the two say the same thing about the same run."""
    suffix = " (with uncommitted changes under a measured path)" if dirty else ""
    under = [f"the `{allocator}` allocator"] if allocator else []
    under += [f"glibc {glibc}"] if glibc else []
    return suffix + (", under " + " and ".join(under) if under else "")


def partial_lead(
    taken: int,
    head: str,
    dirty: bool,
    allocator: str | None,
    unpublishable: str | None,
    glibc: str | None = None,
) -> str:
    """The header a sitting short of the sweep writes instead of a stamp.

    **The fold-in instruction is conditional on the run being publishable at
    all.** A `--alone` sitting emits its tables under the `NOT PUBLISHABLE`
    banner, and a header that told the reader to fold each table in would be
    directing them to do the thing the banner a few lines down refuses."""
    fold_in = (
        "Its tables do not enter the document at all — see the banner below."
        if unpublishable
        else "Leave its session stamp alone, and fold each table in with the `taken at` "
        "commit and glibc its own marker carries; `--check` refuses that marker on a figure that "
        "shares a reading or stands in a derivation, which is every figure a sweep is the "
        "only way to move."
    )
    return (
        f"**A sitting of its own, not a sweep.** This run took {taken} of the "
        f"{len(FIGURES)} figures a sweep takes, with `scripts/measure.py` on "
        f"{date.today().isoformat()}, against commit `{head}`"
        f"{taken_against(dirty, allocator, glibc)} — so this run does not stamp the document. "
        + fold_in
    )


def censored_note(kills: Mapping[str, int]) -> str:
    """The paragraph that goes above a table one of whose legs was OOM-killed.

    It sits in the figure's own section rather than only in the run's header,
    because a section is what gets pasted: a banner at the top of `tables.md`
    is not carried by the one table somebody copies out of it.
    """
    legs = ", ".join(f"`{key.partition('/')[2]}` ({n} rep(s))" for key, n in sorted(kills.items()))
    return (
        "> **A leg of this table was OOM-killed, so it must not be published.** "
        f"{legs}. Those cells are censored — the reading is a bound on a peak the process "
        "never reached, so it enters neither the fit nor the headroom column — and the rest "
        "of the table was measured beside a leg that did not complete.\n\n"
    )


def recorded_input_sizes(stager: Stager, figures: Sequence[Figure]) -> dict[str, int]:
    """Every selected figure's inputs, by name, at the size they were on disk.

    Recorded so `--render` can rebuild a table's byte counts and rates without
    the inputs still existing: warm staging is evicted at the end of a sitting,
    and the SSD cache is explicitly safe to delete.
    """
    sizes: dict[str, int] = {}
    for fig in figures:
        for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
            path = stager.cfg.cache_dir / input_file(name)
            if path.exists():
                sizes[name] = path.stat().st_size
    return sizes


class ReplaySession(Session):
    """A `Session` that measures nothing and serves a past sitting's readings.

    `--render` exists because a renderer's *presentation* is code, and code
    changes after the readings are taken. Before it, folding such a change in
    meant hand-editing the table already pasted into `measurements.md` -- and a
    hand-edit has no oracle, so a table could come to disagree with the harness
    that claims to produce it (the sub-stream annotation landed one column left
    of the leg it described, and nothing caught it).

    So: the readings are the sitting's, and everything derived from them is
    recomputed by the same renderer that would run during a sweep.

    **A staged input is replaced by a sparse file of the recorded size.** Every
    renderer asks an input for its size and never for its contents -- the size
    is what a rate is per -- so a stand-in of the right length reproduces the
    byte counts exactly while needing no cache, no tmpfs and no disk.
    `test_measure.py` is what holds renderers to that.
    """

    def __init__(self, cfg: Config, raw: dict, sizes_dir: Path, log: Callable[[str], None]) -> None:
        super().__init__(cfg, Stager(cfg, log), log)
        self.readings = raw["readings"]
        self.rss = raw.get("rss", {})
        self.killed = raw.get("killed", {})
        self.reported = raw.get("reported", {})
        #: Absent from every sitting taken before the report was a file, which
        #: renders as a figure that carries no instrument rather than as one
        #: whose reports went missing.
        self.instrument = raw.get("instrument", {})
        self.telemetry = raw.get("telemetry", [])
        self.records = raw.get("runs", [])
        self._sizes = raw.get("input_sizes", {})
        self._sizes_dir = sizes_dir

    def sweep(self, figure: str, specs: Sequence[RunSpec], reps: int) -> None:
        """A no-op: every reading this sitting holds is already loaded."""

    def ran_in(self, figure: str, where: str) -> None:
        """A no-op: which glibc each figure ran under is the sitting's own
        record (`figure_glibc`), and asking a place now would name today's."""

    def take(self, spec: RunSpec, rep: int) -> float:
        raise AssertionError(f"--render must measure nothing, but {spec.label} was run")

    def drop_caches(self) -> None:
        raise AssertionError("--render must measure nothing, but the page cache was dropped")

    def input_path(self, name: str, regime: str) -> Path:
        # The regime decides nothing here -- every renderer asks an input for
        # its size and never for its device -- but it is still resolved, so a
        # renderer naming a regime nothing declares fails under `--render` as
        # it would under a sweep, at a second's cost instead of a sweep's.
        regime_spec(regime)
        if name not in self._sizes:
            raise KeyError(
                f"this sitting recorded no size for input {name!r}, so its table cannot be "
                "re-rendered. Sittings taken before --render existed carry no `input_sizes`; "
                "re-take the figure to render it."
            )
        path = self._sizes_dir / input_file(name)
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("wb") as fh:  # sparse: costs no blocks
                fh.truncate(self._sizes[name])
        return path

    def input_size(self, name: str, regime: str) -> int:
        """The recorded size, whatever the regime: nothing is staged here."""
        return file_size(self.cfg, self.input_path(name, regime), name)

    def stage(self, figure: str, group: int) -> None:
        """A no-op: nothing is staged to serve a past sitting's readings."""


def render(cfg: Config, run_dir: Path) -> int:
    """Rebuild one sitting's `tables.md` from its `raw.json`, measuring nothing.

    The header is the sitting's own, verbatim: it describes when and at what
    commit the readings were taken, which re-rendering does not change. Only
    the per-figure sections are rebuilt, because those are what a renderer
    change moves.
    """
    raw_path = run_dir / "raw.json"
    if not raw_path.exists():
        print(f"no raw.json in {run_dir}", file=sys.stderr)
        return 2
    raw = json.loads(raw_path.read_text())
    missing = [k for k in ("header", "input_sizes", "readings") if k not in raw]
    if missing:
        print(
            f"{raw_path} predates --render and is missing {', '.join(missing)}; "
            "re-take the figure to render it.",
            file=sys.stderr,
        )
        return 2

    # `SELECTABLE_BY_ID`, not `FIGURES_BY_ID`: a sitting is re-renderable
    # exactly when `--figure` could have taken it, and an **untaken**
    # instrument is the case that needs re-rendering most — it is taken
    # diagnostically, repeatedly, while its renderer is still being written.
    by_id = SELECTABLE_BY_ID
    unknown = [fid for fid in raw["figures"] if fid not in by_id]
    if unknown:
        print(f"unknown figure(s) in {raw_path}: {', '.join(unknown)}", file=sys.stderr)
        return 2
    # A figure the sitting failed took no readings, so replaying it would die
    # on a missing key rather than rebuild a table -- and the run's own record
    # already says which those were.
    failed = {fid for fid, _ in raw.get("failures", [])}
    figures = [by_id[fid] for fid in raw["figures"] if fid not in failed]
    head, whole_sweep = raw["commit"], raw.get("whole_sweep", False)
    # An older sitting recorded the *selection* here, so re-deriving it is what
    # keeps a re-render from restoring a stamp the run was not entitled to.
    whole_sweep = stamps_the_document(whole_sweep, sorted(failed))

    with tempfile.TemporaryDirectory(prefix="pgdt-render-") as tmp:
        session = ReplaySession(cfg, raw, Path(tmp), lambda msg: None)
        parts: list[str] = []
        sections_seen: set[str] = set()
        for fig in figures:
            session.figure_id = fig.id
            body = fig.run(session)
            apparatus = apparatus_note([r for r in session.records if r.get("figure") == fig.id])
            quoting = consumers(fig)
            note = (
                "\n**The fold-in must also re-read**, because these repeat this figure's "
                "numbers or the claim it licenses: "
                + ", ".join(f"`{q}`" for q in quoting)
                + ".\n"
                if quoting
                else ""
            )
            heading = "" if fig.section in sections_seen else f"## {fig.section}\n\n"
            sections_seen.add(fig.section)
            label = f"**{fig.table_label}**\n\n" if fig.table_label else ""
            marker = figure_marker(
                fig.id,
                None if whole_sweep else head,
                glibc=marker_glibc(
                    whole_sweep, raw.get("glibc"), raw.get("figure_glibc", {}).get(fig.id)
                ),
            )
            kills = session.figure_kills(fig.id)
            bar = censored_note(kills) if kills else ""
            parts.append(f"{heading}{marker}\n\n{label}{bar}{body}\n{apparatus}{note}")

    out = run_dir / "tables.md"
    out.write_text("\n".join(raw["header"]) + "\n" + "\n".join(parts))
    print(f"re-rendered {out} from {raw_path} — {len(figures)} figure(s), nothing measured")
    return 0


def stamps_the_document(selected_sweep: bool, failures: Sequence[object]) -> bool:
    """Whether a sitting may re-stamp `measurements.md` with a session stamp.

    **Selecting the whole sweep is not taking it, and the stamp speaks for
    tables this sitting did not produce.** A run that selected every figure and
    lost one leaves those tables in the document from whatever sitting put them
    there, so a stamp saying every figure below came from this one is false of
    exactly the tables nobody re-took. That is not a smaller version of a
    partial sitting -- it is the same thing, and it declares itself the same
    way: each table it *did* take carries the commit inside its own marker.

    It is a function so that the predicate is testable and named. Computed
    inline it was `{f.id for f in FIGURES} <= {f.id for f in figures}` evaluated
    before the first reading, which cannot see a failure at all."""
    return selected_sweep and not failures


def emit(cfg: Config, figures: Sequence[Figure]) -> int:
    out_root = cfg.out_dir / f"measure-{time.strftime('%Y%m%dT%H%M%S')}"
    out_root.mkdir(parents=True, exist_ok=True)
    log_path = out_root / "log.txt"
    log_file = log_path.open("w")

    def log(msg: str) -> None:
        print(msg, flush=True)
        log_file.write(msg + "\n")
        log_file.flush()

    head, dirty = git_head()
    allocator = None if cfg.dry_run else binary_allocator(cfg.bin_pgdt)
    # The register image's glibc, which the stamp names: asked before anything
    # is staged, so an image that is no glibc costs a second rather than a
    # sitting. A figure run anywhere else asks its own place (`Session.ran_in`).
    glibc = None if cfg.dry_run else glibc_of(cfg, cfg.image)
    # A sitting short of the whole sweep does not re-stamp the document, so
    # each table it emits carries the commit it was taken at inside its own
    # marker and every reader of the stamp argues from that
    # (`measurements.md`, "A figure may be published outside the sweep").
    #
    # **Selecting the whole sweep is not taking it.** This is the *selection*;
    # a figure that fails takes no table, so a run that selected all of them
    # and lost one leaves the document's other tables from an older sitting
    # while a stamp would claim they came from this one. The predicate that
    # decides the stamp is computed after the loop, once `failures` is known.
    selected_sweep = {f.id for f in FIGURES} <= {f.id for f in figures}
    log(
        f"measure.py — {len(figures)} figure(s), commit {head}{' (dirty)' if dirty else ''}"
        + (f", allocator {allocator}" if allocator else "")
        + (f", glibc {glibc}" if glibc else "")
        + ("" if selected_sweep else ", a sitting of its own (each table declares this commit)")
    )
    log(f"output: {out_root}")
    if cfg.arms != (Arm(),):
        log(
            "arms: " + ", ".join(a.name for a in cfg.arms)
            + (" — each leg taken under each in turn; tables render the first" if len(cfg.arms) > 1 else "")
        )
    unpublishable = cfg.unpublishable_reason
    if unpublishable:
        log(
            "!! not publishable, and its tables must not be folded into measurements.md: "
            + unpublishable
        )

    # Named before the first reading, not discovered at fold-in: a selection
    # that takes half of a shared reading is publishable only with the note
    # that says so, and the closure is what the note has to name.
    gaps = closure_gaps(figures)
    if gaps:
        log("!! this sitting is short of the set it shares readings with:")
        for gap in gaps:
            log("!!   " + gap)
    # The reverse edge, which the closure deliberately does not carry: a table
    # this sitting is not taking is a difference over reps it *is* re-taking.
    # Named, not dragged in -- `derived_consumers` says why.
    derivations = derivation_gaps(figures)
    if derivations:
        log("!! this sitting re-takes reps a figure it does not take derives from:")
        for line in derivations:
            log("!!   " + line)

    stager = Stager(cfg, log)
    stager.plan(figures)
    problems = stager.preflight(figures)
    problems += apparatus_preflight(cfg, log)
    log(
        f"tmpfs budget: {stager.budget() / GIB:.2f} GiB "
        f"({WARM_FULL_INPUTS} full-size inputs + {WARM_SLACK // MIB} MiB; largest warm set "
        f"{max(stager.group_need.values(), default=0) / GIB:.2f} GiB)"
        + ("" if stager.budget() == warm_bound(cfg) else ", capped by PGDT_MEASURE_TMPFS_BUDGET_GIB")
    )
    if problems:
        for problem in problems:
            log(f"!! {problem}")
        log("nothing was run: every one of these is knowable before the first measurement.")
        log_file.close()
        return 2
    session = Session(cfg, stager, log, out_root)
    if glibc is not None:
        session.glibcs[cfg.image] = glibc
    # The apparatus, in the order it has to be established: pin the governor
    # (a machine-wide change, restored on the way out), then start sampling.
    # `--dry-run` touches neither: it runs nothing worth witnessing and must
    # not need root.
    governor = None
    if not cfg.dry_run:
        if cfg.pin_governor:
            governor = GovernorPin(sudo=cfg.sudo, log=log)
            governor.__enter__()
        session.sampler.start()

    #: One entry per figure that produced a table: its id, the section heading
    #: it opens (empty where it shares one), and everything below the marker.
    #: Assembled into `parts` after the loop, when the marker is decidable.
    rendered: list[tuple[str, str, str]] = []
    sections_seen: set[str] = set()
    failures: list[tuple[str, str]] = []
    # Figures that lost a leg to the OOM killer. A kill inside `KILL_TOLERANT`
    # is a reading rather than an apparatus failure, so the sitting goes on —
    # but the figure it belongs to may not be published from it, because a
    # censored cell is a bound rather than a number and the table would read as
    # though the arrangement had been measured.
    censored: list[tuple[str, int]] = []
    for i, fig in enumerate(figures):
        blocked = [r for r, _ in failures if r in fig.requires]
        if blocked:
            # A figure that borrows a reading from one that failed cannot be
            # measured on its own terms; say so rather than dying on the
            # missing key.
            log(f"\n=== {fig.id} skipped — borrows from {', '.join(blocked)}, which failed")
            failures.append((fig.id, f"skipped: borrows from {', '.join(blocked)}"))
            continue
        log(f"\n=== {fig.id} ({fig.stage}) — {fig.section}")
        session.figure_id = fig.id
        # Per figure and reset each time, so a figure that needs more than the
        # recorded 512 MB cannot leave the next one running under its ceiling.
        session.memory = fig.memory
        if fig.memory:
            log(f"    container memory {fig.memory}, against the recorded {cfg.memory}")
        first_record = len(session.records)
        started = time.time()
        try:
            for name in fig.cold_inputs:
                stager.ensure_generated(name)
            for name in fig.nvme_inputs:
                stager.nvme_path(name)
            session.stage(fig.id, 0)
            body = fig.run(session)
        except StagingError as exc:
            # Every later figure stages through the same area, so the sweep
            # ends here rather than failing each of them in turn.
            log(f"!! {fig.id} failed staging, and the sweep is aborted: {exc}")
            failures.append((fig.id, str(exc)))
            for rest in figures[i + 1 :]:
                failures.append((rest.id, "skipped: the sweep was aborted by a staging failure"))
            break
        except Exception as exc:  # one figure failing must not lose the others
            log(f"!! {fig.id} failed: {exc}")
            failures.append((fig.id, str(exc)))
            continue
        took = time.time() - started
        kills = session.figure_kills(fig.id)
        if kills:
            censored.append((fig.id, sum(kills.values())))
            log(
                f"!! {fig.id}: {sum(kills.values())} rep(s) OOM-killed across "
                f"{len(kills)} leg(s) — recorded as censored readings; this figure "
                "must not be published from this sitting"
            )
            for key, n in sorted(kills.items()):
                log(f"!!   {key}: {n}")
        apparatus = apparatus_note(session.records[first_record:])
        if apparatus:
            log("    " + apparatus.strip())
        log(f"--- {fig.id} done in {took:.0f} s")
        quoting = consumers(fig)
        note = (
            "\n**The fold-in must also re-read**, because these repeat this figure's numbers "
            "or the claim it licenses: " + ", ".join(f"`{q}`" for q in quoting) + ".\n"
            if quoting
            else ""
        )
        heading = "" if fig.section in sections_seen else f"## {fig.section}\n\n"
        sections_seen.add(fig.section)
        label = f"**{fig.table_label}**\n\n" if fig.table_label else ""
        bar = censored_note(kills) if kills else ""
        # The marker is filled in below, not here: whether this run re-stamps
        # the document is a fact about the *whole* sitting, and a figure taken
        # third cannot know that the nineteenth will fail.
        rendered.append((fig.id, heading, f"{label}{bar}{body}\n{apparatus}{note}"))

    # What the sitting actually took. A selection short of the sweep never
    # stamped; a selection of the whole sweep that lost a figure must not
    # either, because the tables it did not take stay in the document from
    # whatever sitting put them there and the stamp speaks for those too.
    whole_sweep = stamps_the_document(selected_sweep, failures)
    if selected_sweep and failures:
        log(
            f"\n!! the whole sweep was selected and {len(rendered)} of {len(figures)} figures "
            "produced a table, so this sitting does not re-stamp the document: each table it "
            "did take declares this commit inside its own marker instead"
        )
    # The glibc each figure's programs ran under, from the places the sitting
    # recorded; its marker names it wherever the stamp does not speak for it.
    ran_under = {
        fid: glibc_named(session.places.get(fid, ()), session.glibcs) for fid, _, _ in rendered
    }
    markers = {
        fid: figure_marker(
            fid,
            None if whole_sweep else head,
            glibc=marker_glibc(whole_sweep, glibc, ran_under[fid]),
        )
        for fid, _, _ in rendered
    }
    parts = [f"{heading}{markers[fid]}\n\n{tail}" for fid, heading, tail in rendered]

    stager.cleanup()
    if session.groups:
        session.place_harness(session._harness_home)
    if not cfg.dry_run:
        session.sampler.stop()
    if governor is not None:
        governor.__exit__()

    # What the doc still carries from somewhere other than this sitting: the
    # markers already in it, minus whatever this run just replaced.
    doc = REPO / "docs/design/measurements.md"
    taken = {f.id for f in figures}
    outside = sorted(
        (fid, sha)
        for fid, sha in (figure_sittings(doc.read_text()) if doc.exists() else {}).items()
        if fid not in taken
    )
    # A whole sweep stamps the document; a sitting of its own does not, and
    # says what it is instead of writing a stamp nobody may paste.
    if whole_sweep:
        lead = session_stamp(head, dirty, allocator, outside, glibc)
    else:
        # `len(rendered)`, not `len(figures)`: the lead counts the tables below
        # it, and a selected figure that failed produced none.
        lead = partial_lead(len(rendered), head, dirty, allocator, unpublishable, glibc)
    header = [
        "# measure.py output",
        "",
        lead,
        "",
        "Each section below is one figure, ready to paste under its heading in "
        "`docs/design/measurements.md`.",
        "",
    ]
    if unpublishable:
        header += ["> **NOT PUBLISHABLE.** " + unpublishable, ""]
    elif governor is not None and not governor.ok:
        # The governor is part of the recorded apparatus, so failing to pin it
        # departs from that apparatus exactly as a resized input does.
        header += [
            f"> **NOT PUBLISHABLE.** The `{SWEEP_GOVERNOR}` governor could not be pinned "
            f"({governor.error}), so these readings were taken under whatever scaling "
            "policy the machine was left in. Fix that and re-run.",
            "",
        ]
    if gaps:
        header += [
            "> **Short of the set it shares readings with.** Each of these publishes a reading "
            "a figure this sitting did not take also publishes, so their absolutes may not be "
            "set beside those tables' until all of them are re-taken together: "
            + "; ".join(gaps)
            + ".",
            "",
        ]
    if derivations:
        header += [
            "> **A table this sitting did not take is derived from these reps.** Its published "
            "row is computed over readings replaced below, so folding one of these in without "
            "re-taking that table leaves a difference over reps the doc no longer holds: "
            + "; ".join(derivations)
            + ".",
            "",
        ]
    if censored:
        header += [
            "> **NOT PUBLISHABLE — a leg was OOM-killed.** "
            + "; ".join(f"`{fid}` lost {n} rep(s)" for fid, n in censored)
            + ". The kill is a reading and is recorded as one, but a censored cell is a "
            "bound on a peak the process never reached rather than the peak the "
            "arrangement holds, so the table above it describes an arrangement that was "
            "not measured. Fix what kills it, or state the finding from the kill itself, "
            "and re-take the figure.",
            "",
        ]
    if failures:
        header += ["> **Figures that failed:** " + ", ".join(f"`{f}` ({m})" for f, m in failures), ""]

    (out_root / "tables.md").write_text("\n".join(header) + "\n" + "\n".join(parts))
    (out_root / "raw.json").write_text(
        json.dumps(
            {
                "commit": head,
                "dirty": dirty,
                "date": date.today().isoformat(),
                "config": {k: str(v) for k, v in dataclasses.asdict(cfg).items()},
                "figures": [f.id for f in figures],
                "failures": failures,
                # Everything below this line exists so `--render` can rebuild
                # `tables.md` from this file alone, measuring nothing. A
                # presentation-only change to a renderer is folded in by
                # re-rendering the sitting, never by hand-editing the pasted
                # table -- which is how a table came to disagree with the
                # harness that would have produced it.
                "allocator": allocator,
                "glibc": glibc,
                "figure_glibc": ran_under,
                "places": {fid: sorted(where) for fid, where in session.places.items()},
                "glibcs": session.glibcs,
                "whole_sweep": whole_sweep,
                "header": header,
                "input_sizes": recorded_input_sizes(stager, figures),
                "readings": session.readings,
                "rss": session.rss,
                "killed": session.killed,
                "reported": session.reported,
                "instrument": session.instrument,
                "telemetry": session.telemetry,
                # Every arm's readings, where a leg was taken under more than
                # one; `readings` above is the first arm's (`Arm`).
                **(
                    {
                        "arms": {
                            "primary": cfg.arms[0].name,
                            "names": [a.name for a in cfg.arms],
                            "readings": session.arm_readings,
                        }
                    }
                    if len(cfg.arms) > 1
                    else {}
                ),
                "governor": {
                    "requested": SWEEP_GOVERNOR if cfg.pin_governor else None,
                    "pinned": bool(governor and governor.ok),
                    "error": governor.error if governor else None,
                },
                "runs": session.records,
            },
            indent=1,
        )
        + "\n"
    )
    log(f"\nwrote {out_root / 'tables.md'} and {out_root / 'raw.json'}")
    log_file.close()
    # A censored sitting exits non-zero like a failed one. It *completed*, and
    # that is the point: "the sitting finished" is no longer the same claim as
    # "the figure may be published", so the exit code has to carry the second.
    # A detached `--figure reserve --alone` whose gate is "no leg was killed"
    # is then answerable without reading the log.
    return 1 if (failures or censored) else 0


def cmd_list() -> None:
    print("Figures (run order; one figure is one table):\n")
    for fig in ALL_FIGURES:
        print(f"  {fig.id:<24} [{fig.stage}]  {fig.section}")
        if fig.requires:
            print(f"  {'':<24}  borrows readings from: {', '.join(fig.requires)}")
        closure = sharing_closure(fig.id)
        if closure:
            print(f"  {'':<24}  re-take it with: {', '.join(closure)}")
        print(f"  {'':<24}  invalidated by: {', '.join(fig.depends)}")
        print(f"  {'':<24}  quoted by: {', '.join(consumers(fig)) or '(nothing else)'}")
        reproduce = (
            "--drift <sweep> <sweep>" if fig.stage == "derived" else f"--figure {fig.id}"
        )
        print(f"  {'':<24}  reproduce: cd scripts && uv run measure.py {reproduce}")
        print()
    if UNTAKEN:
        print("Built, not taken — no table in the doc until someone runs it:\n")
        for fig in UNTAKEN:
            print(f"  {fig.id:<24} [{fig.stage}]  {fig.section}")
            print(f"  {'':<24}  invalidated by: {', '.join(fig.depends)}")
            print(f"  {'':<24}  take it: cd scripts && uv run measure.py --figure {fig.id}")
            print()
    print("Not emitted here, deliberately (each declares itself in the doc "
          "with `<!-- outside-register: <id> -->`):\n")
    for outside in NOT_OURS.values():
        print(f"  {outside.id:<24} {outside.section}\n  {'':<24}  {outside.why}")
        edge = ", ".join(outside.depends) or "(nothing — it publishes no reading)"
        print(f"  {'':<24}  invalidated by: {edge}\n")


KOJI_DUMP = _env("PGDT_KOJI_DUMP", "/mnt/wd12t/fedora/koji/koji-2026-07-23.dump")


def koji_recipe(cfg: Config, name: str, wrap: bool, jobs: int = SWEEP_JOBS) -> str:
    """The koji invocation, printed rather than run.

    koji is deliberately outside the sweep — a different medium, ~54 minutes,
    and a byte-for-byte regression check rather than a throughput figure — but
    the *recipe* was living in three hand-maintained copies, which is how a
    documented command was found that no longer ran. This is the one copy.

    Three things here have each cost a run, and a test asserts all three:

    * `exec`, so `pgdt` is PID 1 and `nerdctl stop` reaches the interrupt guard.
      A compound command cannot be `exec`'d, which is why nothing is appended to
      report the exit status — `nerdctl inspect` reports it either way, for a
      run that finished *or* was signalled.
    * the cgroup limit, which is part of the apparatus.
    * `--dtcache` under the mounted `/out`. The dump is read-only, so the
      colocated default lands in the container's ephemeral layer and is
      destroyed with it — an hour of scanning thrown away with no error, since
      the write itself succeeds.

    **The worker count is a parameter here, where every other invocation this
    module builds pins `SWEEP_JOBS`.** koji's leg is not a table in the
    register: what it checks is that a scan of the real sample comes back
    byte-identical, and the parallel scan's verification is a leg at some
    count against a serial one. So the count is stated on the command line the
    way it is everywhere else — nothing inherits the CLI default — and *which*
    count is the caller's to say. The wrap sequence states one count for both
    of its legs, since resuming a scan under a different arrangement is a
    second variable in a check that has one.
    """
    mounts = (
        f'  -v "{cfg.bin_pgdt}:/pgdt:ro" \\\n'
        f'  -v "{cfg.out_dir}:/out" \\\n'
        f'  -v "{KOJI_DUMP}:/dump.sql:ro" \\\n'
    )
    def leg(container: str, cache: str, log: str) -> str:
        return (
            f"sudo nerdctl run -d --name {container} "
            f"-m {cfg.memory} --memory-swap {cfg.memory} \\\n"
            + mounts
            + f"  {cfg.image} \\\n"
            f"  sh -c 'exec /pgdt parse --source /dump.sql --dtcache /out/{cache} "
            f"--jobs {jobs} {NO_STATISTICS} >> /out/{log} 2>&1'"
        )

    out = ["cargo build --release -p pgdt   # default target: glibc", "mkdir -p runs", ""]
    if not wrap:
        out += [
            leg(name, f"{name}.dtcache", f"{name}-scan.log"),
            "",
            f"# still going?   sudo nerdctl inspect -f '{{{{.State.Status}}}}' {name}",
            f"# peak RSS:      sudo grep VmHWM /proc/$(sudo nerdctl inspect -f "
            "'{{.State.Pid}}' " + name + ")/status",
            "#   Read it while the run is still going: the kernel keeps the high-water mark,",
            "#   so one read covers everything up to it, and it is gone the moment the",
            "#   process exits. `exec` above makes pgdt PID 1, so that is the pid to read.",
            "#   The container cgroup's memory.peak is the wrong instrument here — it is",
            "#   charged the page cache of a 784 GB read and reports the limit, not pgdt.",
            f"# exit status:   sudo nerdctl inspect -f '{{{{.State.ExitCode}}}}' {name}",
            "#   143 = SIGTERM, which is what `nerdctl stop` sends: the image sets no",
            "#   STOPSIGNAL, and --stop-signal on `run` is accepted and then ignored.",
            "#   For the SIGINT arm: sudo nerdctl kill -s SIGINT " + name + "  (exit 130)",
            f"# wall clock:    sudo nerdctl inspect -f "
            "'{{.State.StartedAt}} {{.State.FinishedAt}}' " + name,
            f"# the log:       runs/{name}-scan.log",
        ]
    else:
        out += [
            "# leg 1 — cold, interrupted partway.",
            leg(f"{name}-wrap1", f"{name}-wrap.dtcache", f"{name}-wrap-scan.log"),
            f"sleep 1200 && sudo nerdctl stop -t 120 {name}-wrap1",
            f"sudo nerdctl inspect -f '{{{{.State.ExitCode}}}}' {name}-wrap1   # 143 (SIGTERM)",
            "",
            "# the interrupted cache must come back typed — both counts zero",
            f"sudo nerdctl run --rm -m {cfg.memory} --memory-swap {cfg.memory} \\",
            # keep the trailing line-continuation: the mounts run straight on
            # into the image name below.
            mounts.rstrip("\n"),
            f"  {cfg.image} /pgdt info --dtcache /out/{name}-wrap.dtcache --detail \\",
            "  | grep -c 'not declared\\|metadata not scanned'",
            "",
            "# leg 2 — resume the identical command, then compare to a full run's cache",
            f"sudo nerdctl rm -f {name}-wrap1",
            leg(f"{name}-wrap2", f"{name}-wrap.dtcache", f"{name}-wrap-scan.log"),
            f"cmp runs/{name}-wrap.dtcache runs/<a previous full run>.dtcache",
        ]
    return "\n".join(out)


def cmd_koji(wrap: bool, jobs: int) -> int:
    cfg = Config()
    print(
        "# koji is not part of the sweep: a different medium, ~54 minutes, and a\n"
        "# byte-for-byte regression check rather than a throughput figure. Run this\n"
        "# detached and read it in a later session (CLAUDE.md, \"Long-running processes\").\n"
        f"# The scan below runs at --jobs {jobs}; --koji-jobs states another.\n"
    )
    print(koji_recipe(cfg, "pgdt-koji", wrap, jobs))
    return 0


# --------------------------------------------------------------------------
# The profiling recipe: printed, never run.
#
# A sampling profile is not a figure
# (`measurements.md`, "The apparatus"): it runs in
# seconds, it attributes cost per function rather than per subtraction, and it
# needs no quiet machine, because the answer it gives is a proportion. So it is
# **not** a `Figure`: no reps, no median, no apparatus gate, no
# `measurements.md` marker. What it shares with koji is why the invocation
# lives here at all -- a command kept in prose is a command that stops running,
# and this one has five ways to produce a plausible-looking profile of the
# wrong thing.
# --------------------------------------------------------------------------

#: The profiler. `perf` needs no change to this machine's
#: `perf_event_paranoid = 2`, which permits user-space sampling of one's own
#: processes -- and user space is what this phase is about. `samply` is the
#: alternative if a richer reader is wanted, and costs a sysctl.
PERF = _env("PGDT_PROFILE_PERF", "perf")

#: Where libc's detached debug symbols come from when the machine has none.
#: **This is not a nicety.** A stripped libc puts ~48% of a warm `parse` profile
#: into bare addresses in `libc.so.6` -- and those addresses are
#: `__memmove_avx_unaligned_erms` and `__memset_avx2_unaligned_erms`, which are
#: the two functions a phase about zero-copy most needs to see.
#:
#: **A distribution's detached-symbol package is the better source and this is
#: the fallback**: installed under `/usr/lib/debug`, symbols are found through
#: the `.gnu_debuglink` with no network, no environment and no cache, and the
#: package manager keeps them in lockstep with libc. So the recipe looks for an
#: installed package first — under `/usr/lib/debug/.build-id/`, because that is
#: the copy of the same file whose *name* proves it matches this libc, where
#: `.gnu_debuglink`'s does not — and fetches only when there is none, and prints
#: which of the two it took, because the failure this whole constant exists
#: against is a profile that came back looking fine off the wrong symbols. The
#: fetch covers the machine that has not installed one and the DSO no package
#: offers. Empty disables the step whole, which is right on a machine whose
#: libc already carries symbols — there the package lookup would report a miss
#: it has no fetch to answer — and wrong on one that silently does not.
#:
#: **The fetch is `perf buildid-cache --debuginfod[=URLs] -a <dso>`**, `perf`'s
#: own interface to its own cache, rather than a `curl` re-deriving that cache's
#: layout from outside — a path construction that fails silently, writing the
#: `debug` file somewhere `perf` does not read and yielding bare addresses with
#: no error. The flag lives on `buildid-cache` alone, with `perf` top-level and
#: `perf report` rejecting it, which is what once read as `perf` not having one.
DEBUGINFOD = _env("PGDT_PROFILE_DEBUGINFOD", "https://debuginfod.archlinux.org")

#: Sampling frequency, in Hz. Prime, so it cannot fall into lockstep with a
#: periodic phase of the thing being sampled.
#:
#: **4999 rather than the customary 997, and that is measured rather than
#: preferred.** `parse` is the thin shape -- ~0.85 s warm on the control, so
#: 997 Hz yields ~850 samples -- and over three reps at 997 the `memmove`
#: bucket read 25.8 / 33.6 / 30.9 % and the two `memchr` kernels swapped rank.
#: At 4999 the same three reps read 30.9 / 30.6 / 30.2 and the ranks hold. What
#: does *not* tighten is the worker thread's `memset`, 29.2 / 21.3 / 18.9, so
#: that spread is the workload rather than the instrument -- which is worth
#: knowing before a profile is read as though every bucket were equally solid.
#: The cost is data-file size (~0.7 MB for `parse`, ~19 MB for the longest
#: shape) on a gitignored directory, and the kernel's own ceiling here is
#: `perf_event_max_sample_rate` = 49000.
PERF_FREQ = 4999

#: The command shapes profiled. `parse` is discovery at the metadata level,
#: read against the scan-throughput tables and `statistics-gathering`'s
#: `metadata` leg; `query-strings` is row extraction
#: before a column is typed and `query-typed` the whole path, read against
#: `nested-end-to-end`, each over the data-level cache its figure reads
#: (`profile_builder_argv`). **The data level's `parse`**, `statistics-gathering`'s
#: `data` leg run without its resident wrapper, is what attributes that
#: figure's Δ among the census, the unrepresentable count and the statistics
#: (`docs/design/roadmap.md`, "Attribution is introspective; only the gate is
#: blind"): no figure differences a build without one of them. **Every `pgdt`
#: shape is profiled at one worker**, and the plain `parse`'s loss past four
#: (D25, unseparated) is not profiled: its suspect, a fused worker waiting on
#: a `POOL_DEPTH` slot, is off-CPU, where an on-CPU `perf record` sees
#: nothing, so its attribution would be an introspective counter of time
#: blocked on the pool, and nothing consumes one while D2 recommends one
#: plain worker.
PROFILE_SHAPES: tuple[str, ...] = (
    "parse",
    f"{STATISTICS_FAMILY}data-rss",
    "query-strings",
    "query-typed",
)

#: The two inputs. The brace-free control is the shape most dumps have; the
#: `--arrays --composite` file is where the nested path is reached at all, and
#: where the census inspects array shapes.
PROFILE_INPUTS: tuple[str, ...] = ("control", "arrays")

#: The row of a `pgdt sql` figure whose legs are profiled as a
#: pair and read by the introspection build, as `(figure, query)`:
#: `dynamic-filter-join`'s costing row, where evaluating rows rejects none.
#: **A pair because the row is a difference**: what evaluating rows adds over
#: the default leg is read bucket by bucket, one leg's profile against the
#: other's, and a profile of either alone answers nothing. Its input is
#: `dynfilter`, its cache the figure's own `GATHER_STATISTICS`, and each leg's
#: command `dfcli_invocation`'s, so the profiled run is the timed one.
DFCLI_ACCOUNT: tuple[str, str] = ("join", "costing")
#: The pair's legs: the filter on at the provider's default against rows
#: evaluated, so the setting alone separates them — the comparison
#: `decisions.md`, "D93" is refused and reopened on.
DFCLI_ACCOUNT_LEGS: tuple[str, str] = ("on", DYNFILTER_ROWS_LEG)
#: How many times the introspection build runs each of those legs. Its
#: spans are summed within a run; three runs say how far one moves.
DFCLI_INTROSPECT_REPS = 3


def profile_argv(command: str, source: Path | str, cache: Path | str) -> list[str]:
    """The `pgdt` arguments one profiled shape runs.

    Deliberately the same flags `_script` hands the sweep, because a profile is
    only readable against the figure it explains -- and a profile of a shape no
    figure times answers a question nobody asked. The two are separate
    functions because `_script` builds a *container* command line with a `time`
    builtin in front of it and this one builds a host argv;
    `test_measure.py`'s `ProfileRecipe` reconciles them shape by shape, so a
    change to a measured invocation that misses this one fails there rather
    than being discovered in a profile that quietly measured something else."""
    if command == "parse":
        return [
            "parse",
            "--source", str(source),
            "--dtcache", str(cache),
            "--jobs", str(SWEEP_JOBS),
            *NO_STATISTICS.split(),
        ]
    if command.startswith(STATISTICS_FAMILY):
        # The leg's own `parse`, the resident wrapper left off: a profile is
        # the process's proportions, and the wrapper is a second process.
        leg, _, suffix = command.removeprefix(STATISTICS_FAMILY).partition("-")
        flags = dict(STATISTICS_LEGS)
        if leg not in flags or suffix != "rss":
            raise ValueError(f"unknown profile shape {command!r}")
        return [
            "parse",
            "--source", str(source),
            "--dtcache", str(cache),
            "--jobs", str(SWEEP_JOBS),
            *flags[leg].split(),
        ]
    if command in ("query-strings", "query-typed"):
        # Over the cache `profile_builder_argv` writes first, unprofiled.
        mode = command.split("-")[1]
        return [
            "query",
            "--source", str(source),
            "--table", "public.perf",
            "--dtcache", str(cache),
            "--statistics", "none",
            "--schema-mode", mode,
            "--jobs", str(SWEEP_JOBS),
        ]
    raise ValueError(f"unknown profile shape {command!r}")


def profile_builder_argv(command: str, source: Path | str, cache: Path | str) -> list[str] | None:
    """The `pgdt parse` a profiled shape's cache is written by first, outside
    the profile, or `None` for a shape that reads none.

    `DATA_LEVEL_BUILDER`'s flags, as `profile_argv` is `_script`'s: a query
    shape is timed over a data-level cache, so a profile of it over none would
    attribute a mapping pass the figure never times. `test_measure.py`'s
    `ProfileRecipe` holds the two builders together."""
    if not command.startswith(DATA_LEVEL_QUERIES):
        return None
    return [
        "parse",
        "--source", str(source),
        "--dtcache", str(cache),
        "--jobs", str(SWEEP_JOBS),
        *GATHER_STATISTICS.split(),
    ]


def profile_recipe(cfg: Config) -> str:
    """The whole sequence, with every path filled in.

    Six things here decide whether the profile is of the thing it claims to
    be, and each fails *silently* -- a profile comes back, it just describes
    something else. `test_measure.py` asserts all six:

    * **the `profiling` binary, never `target/release/pgdt`.** `release`
      carries no line tables and no frame pointers, so `perf` attributes every
      sample to an address it cannot name and the report is a flat list of
      `[unknown]`. The published figures stay on `release`, which is why this
      is a second binary rather than a change to the one they use.
    * **`-C force-frame-pointers=yes` on the build line.** Cargo has no profile
      key for frame pointers, so `[profile.profiling]` alone gives line tables
      and a call graph that stops at the leaf. `RUSTFLAGS` fingerprints
      separately, so this build does not evict `target/release/`.
    * **`--call-graph fp`, matching that build.** `dwarf` would need full debug
      info this profile does not carry, and its 8 KiB stack copies per sample
      would change the thing being measured.
    * **warm input, on tmpfs.** A profile taken off the SSD is a profile of
      `pread` waiting for a device; the proportions this phase reads are CPU
      proportions. Staging is a host copy for the same reason the sweep's is --
      writing 3 GiB of tmpfs from inside the 512 MB container charges those
      pages to its cgroup.
    * **libc's symbols, resolved before the first `perf record`.** Without them
      ~48% of a warm `parse` profile is bare addresses, and they are the
      `memmove` and `memset` a phase about zero-copy exists to see. The step
      keys on libc's build ID, which is the only thing that distinguishes
      symbols that match from symbols that merely have the right filename: an
      installed package answers at `/usr/lib/debug/.build-id/`, and only a miss
      there reaches the `debuginfod` fetch. It prints which of the two it took,
      because both a skew and a failed fetch otherwise surface as a profile
      that looks entirely plausible -- see `DEBUGINFOD` above.
    * **the worker count, stated rather than inherited.** `profile_argv` states
      one for the same reason `_script` does, and here the consequence is
      sharper than a moved number: a sampling profile's buckets are per
      *thread*, so a profile taken at the machine's available parallelism
      attributes a scan among workers the figure it explains never ran. Every
      `pgdt` shape states `SWEEP_JOBS`, and `DFCLI_ACCOUNT`'s legs the
      partition count `dfcli_invocation` states. The shape-equality assertion
      is what holds this function and `_script` together, and it compares two
      shapes that each pin a count rather than two that each inherit one.

    And one thing that is not a mistake but reads like one: **no container.**
    A profile is about proportions, and the cgroup adds capability plumbing
    without changing them.

    **`DFCLI_ACCOUNT`'s pair runs `pgdt sql`**, off the same
    profiling build, over a cache the figure's own `pgdt parse` writes beside
    the dump where `--dump` looks, each leg stating `dfcli_invocation`'s
    environment and arguments. Beside it, **the introspection build**
    (`--features introspect`, in a target directory of its own so no timed
    binary is overwritten) runs the same legs, each run writing what
    `pgdump_query::instrument` timed of its row evaluation to the file
    `INSTRUMENT_OUT_VAR` names: a `runs/` artifact, like the profiles, and
    never a figure."""
    warm = cfg.warm_dir
    binary = REPO / "target/profiling/pgdt"
    sql = f"{binary} sql"
    introspect_target = cfg.alloc_build_root / "dfcli-introspect"
    introspect = f"{introspect_target / 'release/pgdt'} sql"
    cache = warm / "profile.dtcache"
    out = cfg.out_dir
    figure, query = DFCLI_ACCOUNT
    account_input = "dynfilter"
    account_dump = warm / f"{account_input}.sql"
    account_cache = warm / f"{account_input}.sql.dtcache"

    lines: list[str] = []
    step = 0

    def head(*text: str) -> None:
        """A numbered step. Numbered as emitted rather than by literal, so the
        optional debuginfod step does not leave a hole in the sequence on a
        machine whose libc already carries symbols."""
        nonlocal step
        lines.extend([f"# {step}. {text[0]}", *(f"#    {t}" for t in text[1:])])
        step += 1

    head(
        "The tool. `perf_event_paranoid = 2` already permits user-space",
        "sampling of one's own processes, so nothing here needs a sysctl.",
    )
    lines += [f"{PERF} --version", ""]

    head(
        "The profiling build. Frame pointers are not a Cargo profile key,",
        "so they come from RUSTFLAGS; `release` is untouched either way.",
    )
    lines += [
        'RUSTFLAGS="-C force-frame-pointers=yes" \\',
        "  cargo build --profile profiling -p pgdt",
        "",
    ]

    head(
        "The introspection build, whose `sql` the pair runs, in a target directory",
        "of its own: no timed binary is overwritten, and it is never timed.",
    )
    lines += [
        "cargo build --release -p pgdt --features introspect \\",
        f"  --target-dir {introspect_target}",
        "",
    ]

    if DEBUGINFOD:
        head(
            "Name the libc frames. Without them ~48% of a warm `parse` profile",
            "is bare addresses in libc.so.6 -- and they are the memmove and",
            "memset a zero-copy phase exists to see. An installed detached-symbol",
            "package is the better source and is preferred here: no network, and",
            "kept in lockstep with libc by the package manager. See",
            "CONTRIBUTING.md, 'Profiling'. The fetch is the fallback, and it is",
            "perf's own, into perf's own build-id cache. Both branches say which",
            "source the profiles below will be reading.",
        )
        lines += [
            f"LIBC=$(ldd {binary} | awk '/libc\\.so/{{print $3}}')",
            'BID=$(readelf -n "$LIBC" | awk \'/Build ID/{print $NF}\')',
            'DBG=/usr/lib/debug/.build-id/$(printf %.2s "$BID")'
            '/$(printf %s "$BID" | cut -c3-).debug',
            'if [ -e "$DBG" ]; then',
            '  echo "libc symbols: installed package, $DBG"',
            "else",
            '  echo "libc symbols: no package for build ID $BID; fetching"',
            f'  {PERF} buildid-cache --debuginfod={DEBUGINFOD} -a "$LIBC"',
            "fi",
            "",
        ]

    # Every input any profile below reads, in declaration order and without
    # repetition: the cross product's, then the `pgdt sql` pair's.
    staged = list(PROFILE_INPUTS)
    staged += [account_input] if account_input not in staged else []

    head("Stage the inputs warm, on the host.")
    lines.append(f"mkdir -p {warm} {out}")
    for name in staged:
        lines.append(f"cp -n {cfg.cache_dir / f'{name}.sql'} {warm / f'{name}.sql'}")
    lines.append("")

    def record(shape: str, name: str) -> None:
        stem = f"profile-{shape}-{name}"
        argv = " ".join(profile_argv(shape, warm / f"{name}.sql", cache))
        builder = profile_builder_argv(shape, warm / f"{name}.sql", cache)
        lines.extend(
            [
                "",
                f"rm -f {cache}",
                *([f"{binary} {' '.join(builder)} >/dev/null"] if builder else []),
                f"{PERF} record -F {PERF_FREQ} --call-graph fp "
                f"-o {out / (stem + '.data')} \\",
                f"  -- {binary} {argv} >/dev/null",
                f"{PERF} report -i {out / (stem + '.data')} --stdio --no-children \\",
                f"  --percent-limit 0.5 > {out / (stem + '.txt')}",
            ]
        )

    head("The profiles. Each is a runs/ artifact, not a figure.")
    for name in PROFILE_INPUTS:
        for shape in PROFILE_SHAPES:
            record(shape, name)
    lines.append("")
    head(
        f"The pair `{figure}`'s `{query}` row is read as: its legs",
        f"{' and '.join(DFCLI_ACCOUNT_LEGS)}, the setting alone apart, over a cache the",
        "figure's own `pgdt parse` writes beside the dump, where `--dump` looks.",
        "Read as a difference, bucket by bucket.",
    )
    lines += [
        f"rm -f {account_cache}",
        f"{binary} parse --source {account_dump} --dtcache {account_cache} "
        f"--jobs {SWEEP_JOBS} {GATHER_STATISTICS} >/dev/null",
    ]
    for leg in DFCLI_ACCOUNT_LEGS:
        env, argv = dfcli_invocation(figure, query, leg, str(account_dump))
        stem = f"profile-dfcli-{figure}-{query}-{leg}"
        lines.extend(
            [
                "",
                f"{' '.join(env)} {PERF} record -F {PERF_FREQ} --call-graph fp "
                f"-o {out / (stem + '.data')} \\",
                f"  -- {dfcli_shell([], sql, argv).lstrip()} >/dev/null",
                f"{PERF} report -i {out / (stem + '.data')} --stdio --no-children \\",
                f"  --percent-limit 0.5 > {out / (stem + '.txt')}",
            ]
        )
    lines.append("")
    head(
        "The per-term reading: the same legs on the introspection build, each run",
        "writing what it timed of its row evaluation. The default leg evaluates no",
        "row, so it times no span but `Chunk`: the control that only rows are timed.",
    )
    for leg in DFCLI_ACCOUNT_LEGS:
        env, argv = dfcli_invocation(figure, query, leg, str(account_dump))
        for rep in range(1, DFCLI_INTROSPECT_REPS + 1):
            report = out / f"introspect-dfcli-{figure}-{query}-{leg}-{rep}.txt"
            lines.append(
                f"{INSTRUMENT_OUT_VAR}={report} {dfcli_shell(env, introspect, argv)} "
                ">/dev/null"
            )
    lines.append("")
    head("Tear down: tmpfs is 16 G and these inputs do not fit beside a sweep's.")
    lines.append(
        f"rm -f {' '.join(str(warm / f'{n}.sql') for n in staged)} {cache} {account_cache}"
    )
    return "\n".join(lines)


def cmd_profile() -> int:
    cfg = Config()
    print(
        "# A profile is not a figure: no medians, no apparatus gate, no marker in\n"
        "# measurements.md. It is a runs/ artifact, read for proportions\n"
        "# (measurements.md, \"The apparatus\"). The whole\n"
        "# sequence is minutes, so it is not a detached job.\n"
    )
    print(profile_recipe(cfg))
    return 0


# --------------------------------------------------------------------------
# The heaptrack recipe: printed, never run.
#
# The third invocation the harness owns without executing, and the
# **libc-level** half of the instrument pair. `introspect.rs`'s counting
# `#[global_allocator]` intercepts Rust's `GlobalAlloc` and nothing else, so
# every byte `liblzma` asks for is invisible to it and fully present in RSS
# (`decisions.md`, "D13"). heaptrack
# hooks `malloc`, which is the layer that stays correct as more C is vendored.
# **On the shipped build it sees C alone**: mimalloc is linked without
# `override`, so Rust's allocations never reach `malloc`, and the counter and
# mimalloc's own statistics are what read them; only a `system` build's
# recording attributes Rust's allocations too.
#
# It is not a figure, for koji's reason and the profile's: no reps, no median,
# no apparatus gate, no `measurements.md` marker. What it produces is a **name**
# for a term and the arithmetic tying it to a reading a figure took, which is
# all `measurements.md`, "What an instrument can see", licenses an instrument to
# produce.
# --------------------------------------------------------------------------

#: The recorder. `heaptrack_print` is derived from it rather than configured
#: beside it: the two ship in one package, so a machine that has moved one has
#: moved both, and two knobs would let a session analyse with a printer that
#: does not match the recorder's file format.
HEAPTRACK = _env("PGDT_HEAPTRACK", "heaptrack")

#: The demangler the report is piped through.
#:
#: **Rust symbols reach `heaptrack_print` mangled, and they are v0.** This
#: workspace's binaries carry 7,351 `_R`-prefixed symbols and no `_ZN` ones, and
#: heaptrack's own Rust demangling is *post*-1.5.0 and absent from the build
#: installed here — no binary of it references `rustc_demangle`, and
#: `heaptrack_interpret` resolves only `__cxa_demangle`. `c++filt` demangles v0
#: natively, so one pipe buys back every Rust frame in the report; without it
#: the C frames read fine and the Rust ones above them are noise, which is a
#: report that looks half-broken rather than one that looks wrong.
HEAPTRACK_DEMANGLE = _env("PGDT_HEAPTRACK_DEMANGLE", "c++filt")

#: The two shapes recorded, and they are a **pair** rather than a survey.
#:
#: `reserve`'s path step: a stated budget either side of the one-reader charge
#: `charge_bytes(unit, 1)` — `reader_bytes` plus the pool's retention list — one
#: byte apart, so the two runs differ by whether `BlockCache::affordable` admits
#: a block-decoding reader and by nothing else (`RESERVE_STEP_BUDGETS`). Read
#: as a difference — `heaptrack_print --diff` — that pair names the whole block
#: path's allocation at the `malloc` boundary, decoder included, which is the
#: measure-change-measure loop no single recording gives.
#:
#: **Why this pair and not the flagless family.** Every other `reserve` leg
#: differs from its neighbour by a container limit, and a container is what this
#: recipe deliberately does not use; the step is the one axis that lives
#: entirely in the argv, so it is the one a host recording can reproduce
#: exactly. The input is `RESERVE_MECHANISM_INPUT` for the same reason that
#: figure's mechanism legs use it — the budgets are computed from its block
#: size, so a second input would price a step nobody measured.
HEAPTRACK_AXIS: tuple[tuple[str, str], ...] = tuple(
    (f"{RESERVE_STEP_FAMILY}{budget}", RESERVE_MECHANISM_INPUT)
    for budget in RESERVE_STEP_BUDGETS
)


def heaptrack_argv(command: str, source: Path | str, cache: Path | str) -> list[str]:
    """The `pgdt` arguments one recorded shape runs.

    `profile_argv`'s sibling, and the same reconciliation applies: these are the
    flags `_script` hands the sweep, because an attribution is only readable
    against the figure it explains. `test_measure.py`'s `HeaptrackRecipe`
    compares the two shape by shape, so a flag that moves in the timed shape and
    not here fails there rather than in a recording that quietly measured
    something else."""
    if command.startswith(RESERVE_STEP_FAMILY):
        budget = command.removeprefix(RESERVE_STEP_FAMILY)
        if not budget.isdigit() or int(budget) not in RESERVE_STEP_BUDGETS:
            raise ValueError(f"{command!r} names a budget the figure does not carry")
        return [
            "parse",
            "--source", str(source),
            "--dtcache", str(cache),
            "--jobs", str(RESERVE_JOBS),
            "--memory", str(stated_allowance(int(budget))),
            *NO_STATISTICS.split(),
        ]
    raise ValueError(f"unknown heaptrack shape {command!r}")


def heaptrack_recipe(cfg: Config) -> str:
    """The whole sequence, with every path filled in.

    Five things here decide whether the recording describes what it claims to,
    and each fails by returning a plausible report of something else.
    `test_measure.py` asserts all five:

    * **the `profiling` binary, never `target/release/pgdt`.** heaptrack
      resolves symbols from either, but `release` carries no line tables, so a
      report off it has no `.rs:` reference anywhere in it — 3,627 of them
      against 0, measured on the same recording — and every Rust frame is a bare
      name with no file behind it. The published figures stay on `release`,
      which is why this is the second binary the profile recipe already builds.
    * **no `-C force-frame-pointers=yes`, and that is a stated non-requirement
      rather than an omission.** heaptrack unwinds `.eh_frame`, where `perf`
      needs frame pointers, so the flag buys nothing here — verified by
      recording against a binary whose `main` opens `push %rax`, which resolved
      the decoder stack whole. Writing it anyway would fingerprint a second
      build of the same source for no reading.
    * **`--record-only`.** heaptrack otherwise hands the finished file to
      `heaptrack_gui`, which is not installed everywhere and, where it is,
      may not start — Arch ships it in the same package as the CLI tools, and
      it fails on missing KF6 libraries on a machine with no such desktop. That
      failure is cosmetic, the `.zst` being already written, but a recipe that
      ends in an error message is a recipe a session stops trusting.
    * **`--merge-backtraces=0` on the report.** `heaptrack_print` merges by
      default and its own `--help` says the merged peak consumption is not
      correct: a merged frame states the *summed* peak of every backtrace under
      it beside a call count that belongs to the merge, so one decoder's 8.39 MB
      can be printed as "8.39M over 2 calls" and eight of them as 67.11 MB. That
      is the shape of the one unexplained reading this instrument has produced.
    * **`c++filt`.** See `HEAPTRACK_DEMANGLE`: the Rust frames arrive v0-mangled
      and this build of heaptrack cannot demangle them.

    And one thing that is not a mistake but reads like one: **no container.**
    heaptrack multiplies allocation cost and RSS by its own bookkeeping, so a
    recording made under a cgroup would be a recording of heaptrack meeting the
    limit. The gate is where a cgroup belongs (`roadmap.md`, "Attribution is
    introspective; only the gate is blind"); this is the other half."""
    warm = cfg.warm_dir
    binary = REPO / "target/profiling/pgdt"
    cache = warm / "heaptrack.dtcache"
    out = cfg.out_dir
    printer = f"{HEAPTRACK}_print"

    lines: list[str] = []
    step = 0

    def head(*text: str) -> None:
        nonlocal step
        lines.extend([f"# {step}. {text[0]}", *(f"#    {t}" for t in text[1:])])
        step += 1

    head(
        "The tool. It hooks malloc through LD_PRELOAD, so it sees liblzma's",
        "dictionary -- which is the whole reason for it: the counting global",
        "allocator sees only Rust's allocations, and on this build, whose Rust",
        "heap is mimalloc's, those never reach malloc, so it sees C alone.",
    )
    lines += [f"{HEAPTRACK} --version", ""]

    head(
        "The build. No RUSTFLAGS: heaptrack unwinds .eh_frame, so frame",
        "pointers buy nothing and would fingerprint a second build. The",
        "`profiling` profile is what buys source lines; release has none.",
    )
    lines += ["cargo build --profile profiling -p pgdt", ""]

    staged = sorted({name for _, name in HEAPTRACK_AXIS})
    head(
        "Stage the input warm, on the host. Not for the reading's sake --",
        "heaptrack counts allocations, which a cold read does not change --",
        "but because its own overhead makes the recording the slow part.",
    )
    lines.append(f"mkdir -p {warm} {out}")
    for name in staged:
        lines.append(f"cp -n {cfg.cache_dir / input_file(name)} {warm / input_file(name)}")
    lines.append("")

    def stem(shape: str, name: str) -> str:
        return f"heaptrack-{shape}-{name}"

    head(
        "The recordings. --record-only: nothing is handed to heaptrack_gui,",
        "which need not be installed and, where it is, may not start.",
        "Each starts from no cache -- a `parse` resumes from one, so the",
        "second run of a pair would otherwise scan nothing at all.",
    )
    for shape, name in HEAPTRACK_AXIS:
        src = warm / input_file(name)
        argv = " ".join(heaptrack_argv(shape, src, cache))
        lines += [
            "",
            f"rm -f {cache} {out / (stem(shape, name) + '.zst')}",
            f"{HEAPTRACK} --record-only -o {out / stem(shape, name)} \\",
            f"  {binary} {argv} >/dev/null",
        ]
    lines.append("")

    head(
        "The reports. --merge-backtraces=0, because a merged frame's peak is",
        "the sum over the backtraces merged into it and heaptrack_print's own",
        "--help says it is not correct; c++filt, because the Rust frames are",
        "v0-mangled and this heaptrack cannot demangle them.",
    )
    for shape, name in HEAPTRACK_AXIS:
        lines.append(
            f"{printer} --merge-backtraces=0 {out / (stem(shape, name) + '.zst')} \\"
        )
        lines.append(f"  | {HEAPTRACK_DEMANGLE} > {out / (stem(shape, name) + '.txt')}")
    lines.append("")

    (first, first_input), (second, second_input) = HEAPTRACK_AXIS
    head(
        "The pair read as a difference: the block path against the streaming",
        "fallback, one byte of budget apart. This is what a single recording",
        "cannot give -- every term the admitted reader adds, named, with no",
        "subtraction between sittings and no spread to carry.",
    )
    lines += [
        f"{printer} --merge-backtraces=0 {out / (stem(first, first_input) + '.zst')} \\",
        f"  --diff {out / (stem(second, second_input) + '.zst')} \\",
        f"  | {HEAPTRACK_DEMANGLE} > {out / 'heaptrack-step-diff.txt'}",
        "",
    ]

    head("Tear down: the staged input, and the cache the recordings shared.")
    lines.append(
        "rm -f "
        + " ".join(str(warm / input_file(n)) for n in staged)
        + f" {cache}"
    )
    return "\n".join(lines)


def cmd_heaptrack() -> int:
    cfg = Config()
    print(
        "# A heaptrack recording is not a figure: no medians, no apparatus gate,\n"
        "# no marker in measurements.md. It is a runs/ artifact read for\n"
        "# attribution (measurements.md, \"What an instrument can see\"). The\n"
        "# sequence is minutes, so it is not a detached job.\n"
    )
    print(heaptrack_recipe(cfg))
    return 0


#: Lines `tables.md` carries that address the session folding a table in, never
#: a reader of `measurements.md`. Pasting a whole section drags them along --
#: which is what happened when the fold-in note landed under a figure and stood
#: there as though it were part of the table's own commentary.
SCAFFOLDING = ("**The fold-in must also re-read**",)


def scaffolding_in(text: str) -> list[str]:
    """Harness scaffolding that reached the document."""
    return [
        f"{marker} (line {i})"
        for i, line in enumerate(text.splitlines(), 1)
        for marker in SCAFFOLDING
        if line.startswith(marker)
    ]


def cmd_check(doc: Path) -> int:
    """Reconcile the register against the doc: which figures have landed a
    marker, which markers name nothing, which labels their headings left
    behind, where the register's boundary runs,
    which figures were taken outside the stamped sweep, which documents a
    fold-in must re-read because they repeat a figure's numbers, and whether
    every command shape states the worker count it is taken at."""
    text = doc.read_text()
    found = markers_in(doc)
    sittings = figure_sittings(text)
    outside = outside_register_sections(text)
    undeclared = [o.id for o in NOT_OURS.values() if o.id not in {i for i, _ in outside}]
    unknown_outside = sorted({i for i, _ in outside if i not in NOT_OURS})
    both = [(i, figs) for i, figs in outside if figs]
    unknown = [m for m in found if m not in ALL_BY_ID]
    dangling = [
        f"{fig.id} borrows from {shared.source}"
        for fig in EVERY_FIGURE
        for shared in fig.shares
        if shared.source not in EVERY_BY_ID
    ]
    duplicated = sorted({m for m in found if found.count(m) > 1})
    missing = [f.id for f in ALL_FIGURES if f.id not in found]
    unpinned = worker_count_problems()
    misspinned = pinned_count_problems()
    thin_axis = reserve_axis_problems()
    unargued_band = charge_band_problems()
    gathering = statistics_flag_problems()
    scaffolding = scaffolding_in(text)
    mislabelled = section_label_problems(text)

    print(
        f"{doc.relative_to(REPO)} carries {len(set(found))} of {len(ALL_FIGURES)} figure markers.\n"
    )
    if missing:
        print("Not yet folded in (no `<!-- figure: <id> -->` under a heading):")
        for fid in missing:
            print(f"  {fid}")
        print()
    if unknown:
        print("Markers naming no figure — a rename that did not reach the register:")
        for m in sorted(set(unknown)):
            print(f"  {m}")
        print()
    if duplicated:
        print("Markers appearing more than once — one figure is one table:")
        for m in duplicated:
            print(f"  {m}")
        print()
    if dangling:
        print("Borrows naming no figure — a rename that did not reach the register:")
        for line in dangling:
            print(f"  {line}")
        print()
    if mislabelled:
        print(
            "Labels not beginning with their marker's heading — a heading rewritten when its\n"
            "number moved, and the figure's `section` in `scripts/measure.py` left behind:"
        )
        for line in mislabelled:
            print(f"  {line}")
        print()
    if unpinned:
        print(
            "Command shapes inheriting a worker count — a shape that states none measures\n"
            "whatever the CLI's `--jobs` happens to default to on the day, which has already\n"
            f"moved twice. State `--jobs {SWEEP_JOBS}`, and `{DFCLI_PARTITIONS}={SWEEP_JOBS}` "
            f"on every run of `{SQL_SHELL.lstrip('/')}`:"
        )
        for command in unpinned:
            print(f"  {command}")
        print()
    elif misspinned:
        print(
            "Command shapes stating a worker count that is neither the apparatus's nor a\n"
            f"declared axis. State `--jobs {SWEEP_JOBS}`, or add the family to "
            "`measure.JOBS_AXIS`\nand give it a figure whose rows are the counts:"
        )
        for line in misspinned:
            print(f"  {line}")
        print()
    else:
        print(
            f"No command shape inherits a worker count: each states `--jobs {SWEEP_JOBS}`, "
            f"or `--workers` for the\ndecode instrument, `PARALLEL_JOBS` for the "
            f"{len(JOBS_AXIS)} families whose axis it is, or\n`--jobs {RESERVE_JOBS}` for "
            "the reserve's stated legs; the reserve's flagless legs state none\nby "
            "declaration, and `dd` is not a run of ours. Every run of "
            f"`{SQL_SHELL.lstrip('/')}`\nstates `{DFCLI_PARTITIONS}={SWEEP_JOBS}` besides.\n"
        )
    if gathering:
        print(
            "Command shapes running `parse` with statistics gathered — the CLI's default,\n"
            f"which times the gathering rather than the scan. State `{NO_STATISTICS}`:"
        )
        for command in gathering:
            print(f"  {command}")
        print()
    if thin_axis:
        print(
            "Block sizes whose flagless axis is registered wrong — one whose limits cannot\n"
            f"reach the {RESERVE_FIT_MIN_COUNTS} distinct reader counts a two-term fit needs, "
            "so the family publishes a\nsecant however the sitting goes:"
        )
        for line in thin_axis:
            print(f"  {line}")
        print()
    if unargued_band:
        print(
            "Fault bands `BAND_STANCE` does not name — the rule is an enumeration so that a\n"
            "line added later refutes the model until somebody argues it out, which is only\n"
            "true while every band the code defines carries a stance:"
        )
        for line in unargued_band:
            print(f"  {line}")
        print()
    outside_dates = outside_sittings(text)
    if outside:
        print(
            "Declared outside the register — sections whose readings this harness did not\n"
            "take, and which the session stamp above therefore does not cover. Each still\n"
            "declares what invalidates it, so `--stale` can name it:"
        )
        for oid, _ in outside:
            known = NOT_OURS.get(oid)
            print(f"  {oid:<24} {known.section if known else '(unknown)'}")
            if known is None:
                continue
            edge = ", ".join(known.depends) or "(nothing — it publishes no reading)"
            print(f"  {'':<24}  invalidated by: {edge}")
            if known.depends:
                print(f"  {'':<24}  taken at: {outside_dates.get(oid, '(undeclared)')}")
        print()
    if undeclared:
        print(
            "Outside the register and saying so nowhere in the doc — add an\n"
            "`<!-- outside-register: <id> -->` marker under the section's heading:"
        )
        for oid in undeclared:
            print(f"  {oid:<24} {NOT_OURS[oid].section}")
        print()
    if unknown_outside:
        print("Declarations naming no section the harness disowns — see `--list`:")
        for oid in unknown_outside:
            print(f"  {oid}")
        print()
    if both:
        print(
            "Declared outside the register and carrying a figure marker — a section is one\n"
            "or the other, and the figure's apparatus line contradicts the declaration:"
        )
        for oid, figs in both:
            print(f"  {oid} carries {', '.join(sorted(set(figs)))}")
        print()
    partial = partial_sittings(text)
    if partial:
        print(
            "One reading published twice — a table below measured a reading it shares.\n"
            "The doc permits it while the harness's note is carried with the table; what it\n"
            "costs is that the set must be re-taken together before any of it moves:"
        )
        for fid, closure in partial:
            print(f"  {fid}")
            print(f"      re-take with: {', '.join(closure) or '(nothing — it shares no reading)'}")
        print()
    derived = derivation_edges()
    if derived:
        # Reported rather than failed on, for the same reason a blessed partial
        # sitting is: nothing here is wrong, and a check that is permanently red
        # stops being read. What it buys is that the relationship is in front of
        # whoever is reading this to decide what a re-take drags.
        print(
            "Derived, not republished — one table's row is computed over another's reps.\n"
            "Not a closure edge: the number is not in the doc twice, so the two may come from\n"
            "different sittings. What it costs is that re-taking the source alone strands the\n"
            "consumer's row on reps the doc no longer holds:"
        )
        for consumer, source, what in derived:
            print(f"  {consumer} derives {what} from {source}")
        print()
    if UNTAKEN:
        print("Built, not taken (no marker expected — see `--list`):")
        for fig in UNTAKEN:
            print(f"  {fig.id}")
        print()
    stamp = stamped_commit(doc)
    if sittings:
        print(
            "Published outside the stamped sweep — each of these declares the commit it was\n"
            "taken at inside its own marker, and every reader of the stamp argues from that:"
        )
        for fid, sha in sorted(sittings.items()):
            print(f"  {fid:<24} {sha}")
        print()
    bad_sittings = sitting_problems(sittings, stamp)
    if bad_sittings:
        print("Sittings the doc may not carry:")
        for line in bad_sittings:
            print(f"  {line}")
        print()
    unnamed_glibc = glibc_problems(text)
    if unnamed_glibc:
        print(
            "Figures whose glibc the doc does not name — the stamp names the register image's,\n"
            "and a sitting marker the one its own sitting ran under:"
        )
        for line in unnamed_glibc:
            print(f"  {line}")
        print()
    bad_outside = outside_sitting_problems(text)
    if bad_outside:
        print(
            "Declared sections whose provenance does not stand — an edge nothing can be\n"
            "measured from, or a commit nothing reads:"
        )
        for line in bad_outside:
            print(f"  {line}")
        print()
    # Generated, never reconciled: the count beside the stamp was hand-written
    # and was wrong for as long as one figure stood outside the sweep.
    accounting = sitting_accounting(sorted(sittings.items()))
    miscounted = " ".join(accounting.split()) not in " ".join(text.split())
    if miscounted:
        print(
            "The session stamp's accounting sentence is not the one the harness generates.\n"
            "Replace it with this, verbatim:\n"
        )
        print(f"  {accounting}\n")
    acks = resolved_acknowledgements()
    # A declared section is a name an acknowledgement may carry, and a base
    # spentness is computed against: `--stale` reds it the same way, and the
    # harness cannot re-take it, so an entry is the *only* discharge that does
    # not cost an hour on the HDD.
    unknown_ack, spent_ack = acknowledgement_problems(
        acks,
        {**ALL_BY_ID, **NOT_OURS},
        spent_acknowledgements(acks, {**figure_bases(text), **outside_bases(text)}),
    )
    if acks:
        print("Acknowledged commits — a declared path changed and no reading moved:")
        for ack in acks:
            print(f"  {ack.commit[:7]}  {ack.why}")
        print()
    if unknown_ack:
        print("Acknowledgements naming no figure — a rename that did not reach the register:")
        for line in unknown_ack:
            print(f"  {line}")
        print()
    if spent_ack:
        print(
            "Spent acknowledgements — every figure they cover has moved past them; delete\n"
            "these entries:"
        )
        for line in spent_ack:
            print(f"  {line}")
        print()
    if scaffolding:
        print(
            "Harness scaffolding pasted into the document — these lines address the\n"
            "session folding a table in, not a reader of the document; delete them:"
        )
        for line in scaffolding:
            print(f"  {line}")
        print()
    print("What else a moved figure invalidates:")
    for fig in ALL_FIGURES:
        print(f"  {fig.id}")
        for q in consumers(fig):
            print(f"      {q}")
    return (
        1
        if (
            scaffolding
            or unknown
            or duplicated
            or dangling
            or mislabelled
            or unpinned
            or misspinned
            or gathering
            or thin_axis
            or unargued_band
            or undeclared
            or unknown_outside
            or both
            or bad_sittings
            or unnamed_glibc
            or bad_outside
            or miscounted
            or unknown_ack
            or spent_ack
        )
        else 0
    )


def acknowledgement_problems(
    acks: Sequence[Acknowledged], known: Iterable[str], spent: Iterable[str]
) -> tuple[list[str], list[str]]:
    """Entries naming a figure that does not exist, and entries already spent.

    Spent is the one that silts up. An acknowledgement covers a commit inside
    one figure's `--stale` range; once every figure it names is argued from a
    commit past it, no range can reach it again and the entry excuses nothing.
    Left standing, the register accumulates permanent excuses whose diffs
    nobody will ever re-read — which is how a mechanism that exists to keep a
    signal honest turns into the thing dulling it. Which entries those are is
    `spent_acknowledgements`, which reads each figure's own base.
    """
    known = set(known)
    spent = set(spent)
    unknown = sorted(
        f"{ack.commit[:7]} names {fid}" for ack in acks for fid in ack.figures if fid not in known
    )
    return unknown, sorted(ack.commit[:7] for ack in acks if ack.commit in spent)


def spent_acknowledgements(
    acks: Sequence[Acknowledged],
    bases: Mapping[str, str | None],
    ancestor: Callable[[str, str], bool] | None = None,
) -> list[str]:
    """The commits no figure they cover can reach any more.

    **Spentness is per figure**, because the range each figure is argued from
    is. An entry recorded against a commit between the stamp and a figure taken
    later is spent *for that figure* — its `--stale` range starts after the
    commit — and still live for every figure read from the stamp. So an entry
    goes only when every figure it covers has moved past it; going on the stamp
    alone would delete an excuse that is still doing work, and the next diff
    under that path would read stale with the reason gone.

    **`bases` carries the declared sections too, and a blanket entry covers
    none of them.** The mapping is the union of both, so that an entry naming
    `koji` is spent against koji's own marker rather than against the stamp —
    but an empty `figures` tuple means every figure and no declared section
    (`excuses`), and reading it as the whole mapping would have made a section
    hold a blanket entry alive that never reached it."""
    ancestor = ancestor or is_ancestor
    out = []
    for ack in acks:
        covered = ack.figures or tuple(fid for fid in bases if fid not in NOT_OURS)
        revs = [bases.get(fid) for fid in covered]
        if revs and all(rev is not None and ancestor(ack.commit, rev) for rev in revs):
            out.append(ack.commit)
    return out


#: The size `--verify-additive` generates at. Small enough that verifying every
#: input is seconds rather than the sweep's tens of gigabytes, and large enough
#: that every value shape the generators draw from appears many times over: the
#: same seed draws the same row sequence at any size, so a prefix that matches
#: byte for byte is the row logic matching, not a coincidence of length.
VERIFY_SIZE_GIB = 0.03


def cmd_verify_additive(since: str | None) -> int:
    """Regenerate every figure's inputs at two revisions and compare bytes.

    The one class of staleness that can be settled mechanically. A figure's
    `depends` names its generator, so *any* edit to that script marks it stale
    — including one that only adds a flag. Rather than trusting a reading of
    the diff, generate both ways and compare: identical bytes mean the figure
    would have been taken on the same input, which is the whole of what its
    generator dependency claims.

    What this cannot settle is a change to library code or to the harness's own
    timing path, where there is no cheap oracle and the honest answer is a
    sweep. Those stay stale, and `--stale` keeps saying so.

    **A figure's inputs are regenerated at that figure's own base**, which is
    its sitting where its marker declares one and the session stamp otherwise.
    Reading the stamp for all of them rested on "the figure was taken at the
    stamp, so its inputs existed then" — a sentence a figure published outside
    the sweep falsifies, and one whose failure is silent: the older revision
    generates *something*, and the comparison then answers a question nobody
    asked.
    """
    doc = REPO / "docs/design/measurements.md"
    bases = figure_bases(doc.read_text(), since)
    if any(bases[fig.id] is None for fig in ALL_FIGURES):
        print(
            "no --since given and measurements.md carries no session stamp naming a commit; "
            "pass --since <rev>",
            file=sys.stderr,
        )
        return 2

    changed_since = {rev: set(changed_paths(rev)) for rev in sorted({*bases.values()})}
    # Only the inputs a *published* figure is taken on. An input that exists
    # solely for an untaken instrument has no bytes in the doc to be wrong
    # about, and -- as `composite_text` proved on this mechanism's first run --
    # it may not be generatable at the old revision at all, because the commit
    # under test is what added its flag. A published figure's inputs cannot be
    # new that way: the figure was taken at its own base, so they existed then.
    wanted: dict[str, dict[str, InputSpec]] = {}
    covered: dict[str, set[str]] = {}
    generators: dict[str, set[str]] = {}
    for fig in ALL_FIGURES:
        rev = bases[fig.id]
        for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
            spec = INPUTS[name]
            if f"scripts/{spec.generator}" in changed_since[rev]:
                wanted.setdefault(rev, {})[name] = spec
                covered.setdefault(rev, set()).add(fig.id)
                generators.setdefault(rev, set()).add(spec.generator)
    if not wanted:
        print("no generator any published figure depends on changed since that figure's base.")
        return 0

    cfg = dataclasses.replace(Config(), size_gib=VERIFY_SIZE_GIB)
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    differing: list[str] = []
    for rev, specs in wanted.items():
        who = sorted(covered[rev])
        print(
            f"{len(generators[rev])} generator(s) changed since {rev} "
            f"({', '.join(sorted(generators[rev]))}), which {len(who)} figure(s) are taken "
            f"against: {', '.join(who)}"
        )
        print(
            f"regenerating {len(specs)} input(s) at {VERIFY_SIZE_GIB} GiB under both revisions\n"
        )
        # Under `runs/`: gitignored, so a crashed run leaves no untracked tree
        # inside the repo being measured.
        safe = re.sub(r"[^A-Za-z0-9._-]", "_", rev)
        work = cfg.out_dir / f"worktree-verify-{safe}"
        run(["git", "worktree", "add", "--detach", str(work), rev], cwd=REPO, quiet=True)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                for name, spec in sorted(specs.items()):
                    old_out = tmp_path / f"{name}.old"
                    new_out = tmp_path / f"{name}.new"
                    run(spec.argv(cfg, old_out), cwd=work / "scripts", quiet=True)
                    run(spec.argv(cfg, new_out), cwd=SCRIPTS, quiet=True)
                    old, new = old_out.read_bytes(), new_out.read_bytes()
                    if old == new:
                        print(f"  {name:<16} identical ({len(new)} bytes)")
                    else:
                        differing.append(f"{name} (since {rev})")
                        at = next(
                            (i for i, (a, b) in enumerate(zip(old, new)) if a != b),
                            min(len(old), len(new)),
                        )
                        print(
                            f"  {name:<16} DIFFERS at byte {at} ({len(old)} vs {len(new)} bytes)"
                        )
                    old_out.unlink()
                    new_out.unlink()
        finally:
            run(["git", "worktree", "remove", "--force", str(work)], cwd=REPO, quiet=True)
        print()

    if differing:
        print(
            f"{len(differing)} input(s) changed: {', '.join(differing)}. Those figures are "
            "genuinely stale and need a sweep; do not acknowledge them."
        )
        return 1

    accounted = sorted({fid for ids in covered.values() for fid in ids})
    print(
        f"Every input is byte-identical, so no figure was taken on different bytes. "
        f"The {len(accounted)} figure(s) this accounts for:\n"
    )
    for fid in accounted:
        print(f"  {fid}")
    print(
        "\nThat is the evidence an `Acknowledged` entry carries — record the commit that changed "
        "the generator, these figure ids, and this command as its `verified`."
    )
    return 0


def cmd_stale(since: str | None) -> int:
    """Which figures a diff has invalidated, each argued from its own base —
    and, in a stanza of its own, which section the register does not hold.

    A figure published outside the stamped sweep declares the commit it was
    taken at, and its range starts there: reading the stamp for every figure
    reported one stale against three commits it *postdates*, which is a red
    nobody can clear — not by a measurement, which would spend a sitting to
    conceal that the range was the defect, and not by an acknowledgement, which
    would assert "this commit moved no reading" about a commit that ran before
    the reading was taken."""
    doc = REPO / "docs/design/measurements.md"
    text = doc.read_text()
    bases = figure_bases(text, since)
    if any(rev is None for rev in bases.values()):
        print(
            "no --since given and measurements.md carries no session stamp naming a commit; "
            "pass --since <rev>",
            file=sys.stderr,
        )
        return 2
    # A declared section is argued from its own marker, never from the stamp:
    # the stamp is scoped to the register and says nothing about a section
    # outside it, so reading it here would date koji's readings to a sweep that
    # did not take them.
    obases = outside_bases(text, since)
    undated = sorted(oid for oid, rev in obases.items() if rev is None)
    obases = {oid: rev for oid, rev in obases.items() if rev is not None}
    everything = {**bases, **obases}
    changed_since = {rev: changed_paths(rev) for rev in sorted({*everything.values()})}
    for rev, changed in changed_since.items():
        who = sorted(fid for fid, base in everything.items() if base == rev)
        scope = (
            "every figure and declared section"
            if len(who) == len(everything)
            else ", ".join(who)
        )
        print(f"{len(changed)} path(s) changed since {rev} — {scope}")
    print()
    if undated:
        print(
            "Declared outside the register with an invalidation edge and no commit its\n"
            "readings were taken at — nothing can say what these have gone stale against\n"
            "(`--check` names it too):"
        )
        for oid in undated:
            print(f"  {oid}")
        print()
    touched = [
        (fig, hits)
        for fig in ALL_BY_ID.values()
        if (hits := declared_hits(fig, changed_since[bases[fig.id]]))
    ]
    outside_touched = [
        (NOT_OURS[oid], hits)
        for oid, rev in sorted(obases.items())
        if (hits := declared_hits(NOT_OURS[oid], changed_since[rev]))
    ]
    if not touched and not outside_touched:
        print("no figure's declared paths were touched, and no declared section's.")
        return 0

    acks = resolved_acknowledgements()
    dirty = dirty_paths()
    # Per base, because a commit's position in a range depends on where the
    # range starts: the same path is touched by different commits for a figure
    # read from the stamp and one read from its own sitting.
    by_base = {
        rev: commits_touching(
            {
                p
                for who, hits in (*touched, *outside_touched)
                if everything[who.id] == rev
                for p in hits
            },
            rev,
        )
        for rev in changed_since
    }

    stale: list[tuple[Figure, list[str]]] = []
    outside_stale: list[tuple[Outside, list[str]]] = []
    excused_by: dict[str, list[str]] = {}
    # Keyed by commit and path rather than by commit: the oracle is answered
    # per path, and a commit that is comments in `io.rs` and code in
    # `measure.py` is exactly the shape three of the register's own entries
    # had.
    comment_by: dict[tuple[str, str], list[str]] = {}
    # One loop over both, because an acknowledgement excuses a *commit* and
    # says nothing about what kind of reading is on the other side of it: a
    # declared section is discharged by the same entry, in the same register,
    # and splitting the walk would have been the second authority over that
    # question.
    for who, hits in (*touched, *outside_touched):
        by_path = by_base[everything[who.id]]
        ok = excused_paths(who.id, hits, by_path, dirty, acks)
        left = [h for h in hits if h not in ok]
        if left:
            (outside_stale if isinstance(who, Outside) else stale).append((who, left))
        for path in ok:
            for commit in by_path[path]:
                if excuses(acks, commit, who.id):
                    excused_by.setdefault(commit, []).append(who.id)
                else:
                    comment_by.setdefault((commit, path), []).append(who.id)

    if comment_by:
        print(
            "Comment-only, skipped — every hunk these commits make inside the path falls\n"
            "within a comment, so nothing there can move a reading and no entry is owed:\n"
        )
        for (commit, path), ids in comment_by.items():
            print(f"  {commit[:7]}  {path}")
            print(f"           skipped for: {', '.join(sorted(set(ids)))}")
        print()

    if excused_by:
        print("Acknowledged — the commit is recorded as moving no reading:\n")
        for commit, ids in excused_by.items():
            ack = next(a for a in acks if a.commit == commit)
            short = commit[:7]
            print(f"  {short}  {ack.why}")
            print(f"           verified: {ack.verified or '(read by hand; nothing re-checks it)'}")
            print(f"           excuses: {', '.join(sorted(set(ids)))}")
        print()

    stamp = stamp_in(text)

    def report_outside() -> None:
        """The declared sections whose readings a diff has moved.

        Printed apart from the figures because the two ask different things of
        the reader. A stale figure names a sweep; a stale section names a run
        nobody here can make — koji is an hour on a different medium — so what
        this stanza buys is the choice between making that run by hand and
        writing down why the change cannot have moved it. Silence, which is
        what this printed before the edge existed, was not one of the two."""
        if not outside_stale:
            return
        print(
            "\nDeclared outside the register and moved anyway — the harness does not re-take\n"
            "these, so each is discharged by a run somebody makes by hand or by an\n"
            "acknowledgement, exactly as a figure's red is:"
        )
        for outside, hits in outside_stale:
            print(f"  {outside.id:<24} stale — {', '.join(hits)} (since {obases[outside.id]})")
            for path, excused, blocking in inert_excuses(
                outside.id, hits, by_base[obases[outside.id]], acks
            ):
                names = ", ".join(c[:7] for c in excused)
                held = ", ".join(c[:7] for c in blocking) or "an uncommitted change"
                print(f"      {path}: {names} excused here but inert — held red by {held}")

    if not stale:
        if not outside_stale:
            print("nothing is stale: every touched path is accounted for.")
            return 0
        print("no figure is stale: every touched path of one is accounted for.")
        report_outside()
        return 1

    for fig, hits in stale:
        base = bases[fig.id]
        own = "" if base == stamp or since else f" (since its own sitting {base})"
        print(f"  {fig.id:<24} stale — {', '.join(hits)}{own}")
        by_path = by_base[base]
        for path, excused, blocking in inert_excuses(fig.id, hits, by_path, acks):
            names = ", ".join(c[:7] for c in excused)
            held = ", ".join(c[:7] for c in blocking) or "an uncommitted change"
            print(f"      {path}: {names} excused here but inert — held red by {held}")
    if all(f.stage == "derived" for f, _ in stale):
        # A derived figure is computed from two sweeps' `raw.json`, so it is
        # re-taken in seconds and forces no sweep on anything else.
        print(
            "\nEvery stale figure here is derived: re-take it with "
            "`uv run measure.py --drift <sweep> <sweep>`, which measures nothing."
        )
        report_outside()
        return 1
    # `--figure` is what takes one on its own, so a derived figure is not in
    # this list however few edges it stands in: `--drift` is how it moves.
    alone = sorted(
        fig.id for fig, _ in stale if fig.id in SELECTABLE_BY_ID and not entangled_with(fig.id)
    )
    print(
        "\nA stale figure is re-taken with the whole doc: one sweep replaces every table "
        "(`uv run measure.py --all`), because the doc differences across tables."
    )
    if alone:
        print(
            "These stand in no borrow edge, so each may instead be re-taken on its own and "
            f"declare the sitting in its marker: {', '.join(alone)}."
        )
    report_outside()
    return 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--list", action="store_true", help="name every figure and what invalidates it")
    parser.add_argument("--figure", action="append", default=[], help="figure id (repeatable, or comma-separated)")
    parser.add_argument(
        "--stage",
        choices=["cold", "cold-nvme", "warm", "warm-parallel", "criterion"],
        help="every figure of one stage",
    )
    parser.add_argument(
        "--alone",
        action="store_true",
        help="take exactly the figures named, borrowing nothing — a diagnostic sitting, "
        "whose tables carry the partial-sweep note and the NOT PUBLISHABLE banner",
    )
    parser.add_argument("--all", action="store_true", help="the whole sweep — what the doc's session stamp means")
    parser.add_argument("--stale", action="store_true", help="say which figures a diff has invalidated")
    parser.add_argument(
        "--render",
        metavar="RUN_DIR",
        help="rebuild a past sitting's tables.md from its raw.json, measuring nothing — "
        "how a presentation-only renderer change is folded in, instead of by hand",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="reconcile the register against measurements.md's figure markers and its "
        "outside-register declarations, and name the documents a moved figure invalidates",
    )
    parser.add_argument(
        "--verify-additive",
        action="store_true",
        help="regenerate every figure's inputs at --since and now, and compare bytes — the "
        "evidence an acknowledged commit carries",
    )
    parser.add_argument(
        "--since", help="revision for --stale/--verify-additive (default: the doc's session stamp)"
    )
    parser.add_argument(
        "--drift",
        nargs=2,
        metavar=("SWEEP", "SWEEP"),
        help="two runs/measure-* directories: emit the session-drift table across them, "
        "and each arm's drift where both took more than one",
    )
    parser.add_argument(
        "--arms",
        metavar="RUN_DIR",
        help="one runs/measure-* directory taken under more than one arm: set its arms' "
        "readings beside each other",
    )
    parser.add_argument(
        "--koji-recipe",
        action="store_true",
        help="print koji's detached invocation — the harness owns it but never runs it",
    )
    parser.add_argument(
        "--wrap",
        action="store_true",
        help="with --koji-recipe: the stop-report-resume-compare sequence instead",
    )
    parser.add_argument(
        "--koji-jobs",
        type=int,
        default=SWEEP_JOBS,
        metavar="N",
        help=f"with --koji-recipe: the worker count the scan states (default {SWEEP_JOBS}); "
        "koji's parallel leg is a leg at some count against a serial one, so the recipe "
        "takes it rather than pinning it",
    )
    parser.add_argument(
        "--profile-recipe",
        action="store_true",
        help="print the sampling-profile sequence — the harness owns it but never runs it",
    )
    parser.add_argument(
        "--heaptrack-recipe",
        action="store_true",
        help="print the libc-level heap-attribution sequence — the harness owns it but "
        "never runs it",
    )
    parser.add_argument("--reps", type=int, help="override every figure's rep count (smoke runs only)")
    parser.add_argument("--dry-run", action="store_true", help="print what would run, measure nothing")
    parser.add_argument("--keep-warm", action="store_true", help="leave staged inputs on tmpfs")
    parser.add_argument(
        "--pin-governor",
        action="store_true",
        help=f"pin every CPU to the {SWEEP_GOVERNOR} governor for the sweep and restore it "
        "afterwards; an apparatus change, and a measured no-op on amd-pstate-epp",
    )
    parser.add_argument(
        "--pin-cpus",
        choices=ARM_CHOICES,
        help="place each leg on one L3 group, or two, by the workers it states, and the "
        "harness on the other die; `alternate` takes every leg unpinned and pinned in turn. "
        "Refuted, so anything but `off` is unpublishable",
    )
    parser.add_argument(
        "--stage-binaries",
        choices=ARM_CHOICES,
        help="run the timed binaries from tmpfs, read once untimed before each run; "
        "`alternate` takes every leg staged and unstaged in turn. The recorded apparatus, "
        "so anything but `on` is unpublishable",
    )
    args = parser.parse_args(argv)

    if args.list:
        cmd_list()
        return 0
    if args.drift:
        return cmd_drift(*args.drift)
    if args.arms:
        return cmd_arms(args.arms)
    if args.koji_recipe:
        return cmd_koji(args.wrap, args.koji_jobs)
    if args.profile_recipe:
        return cmd_profile()
    if args.heaptrack_recipe:
        return cmd_heaptrack()
    if args.check:
        return cmd_check(REPO / "docs/design/measurements.md")
    if args.stale:
        return cmd_stale(args.since)
    if args.render:
        # A default `Config`: a render reads no input and runs no container, so
        # nothing the sitting's own paths or sizes would have decided applies.
        return render(Config(), Path(args.render))
    if args.verify_additive:
        return cmd_verify_additive(args.since)

    ids: list[str] = []
    for item in args.figure:
        ids += [p for p in item.split(",") if p]
    if args.stage:
        # Split on `+` rather than matching as a substring: a figure taken in
        # two regimes declares `cold+warm`, and both must select it, while
        # `cold` must *not* reach `cold-nvme` -- a different device, whose
        # absolutes belong to no cold-SSD sitting.
        ids += [f.id for f in FIGURES if args.stage in f.stage.split("+")]
    if args.all:
        ids = [f.id for f in FIGURES]
    if not ids:
        parser.error(
            "nothing selected: pass --figure, --stage, --all, --list, --stale or --verify-additive"
        )

    cfg = Config(
        reps_override=args.reps,
        dry_run=args.dry_run,
        keep_warm=args.keep_warm,
        alone=args.alone,
        pin_governor=args.pin_governor or Config().pin_governor,
        pin_cpus=args.pin_cpus or Config().pin_cpus,
        stage_binaries=args.stage_binaries or Config().stage_binaries,
    )
    try:
        arms = cfg.arms
    except ValueError as exc:
        parser.error(str(exc))
    if any(a.pinned for a in arms):
        problem = pin_topology_problem(l3_groups())
        if problem:
            parser.error(problem)
    figures = resolve_selection(ids, alone=args.alone)
    # A sitting short of the sweep publishes outside the session stamp, which
    # only a figure standing in no borrow edge may do. Asked here, before the
    # measurement is spent, rather than at `--check` after it -- and asked only
    # of a sitting that could be folded in at all, which is what
    # `cfg.publishable` answers: a smoke run and a `--alone` sitting both emit
    # their tables under the NOT PUBLISHABLE banner, so there is no publication
    # for this to refuse.
    if cfg.publishable and not {f.id for f in FIGURES} <= {f.id for f in figures}:
        refusals = publication_refusals(figures)
        if refusals:
            parser.error(
                "this sitting would publish outside the document's session stamp: "
                + "; ".join(refusals)
                + ". Take the whole doc (`--all`), which re-stamps it, or `--alone` for a "
                "diagnostic sitting, whose tables come back marked NOT PUBLISHABLE."
            )
    # The shipped binary, built in the first second, before the run directory
    # exists, and about provenance rather than existence. `print` rather than
    # the sitting's log, which `emit` has not opened yet — a build that fails
    # here must not leave an empty run directory behind it.
    ensure_pgdt_binary(cfg, lambda msg: print(msg, flush=True))
    if not cfg.dry_run and not cfg.bin_pgdt.exists():
        parser.error(
            f"{cfg.bin_pgdt} is missing, and PGDT_MEASURE_BIN names a binary this harness does "
            f"not build. Point it at one that exists, or unset it and let the harness build "
            f"{CARGO_RELEASE_BIN}."
        )
    return emit(cfg, figures)


if __name__ == "__main__":
    raise SystemExit(main())
