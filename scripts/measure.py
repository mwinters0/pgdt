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
* one libc: the default glibc build, in a glibc image.

**Selection is per figure, never finer.** One figure is exactly one table, and
a table is atomic -- half of one may not be re-taken. Figures that share a
reading say so and pull the other figure in rather than measuring it twice
(the warm scan-throughput table's `COPY` row *is* the census table's warm
census-on column, and must be the same number).

**Each figure declares the paths that invalidate it**, so `--stale` can say
which figures a diff has made stale. That is the half a harness alone does not
fix: a one-line change to `map::Builder::on_row` invalidated both census
figures and nothing announced it.

Two binaries this cannot build for itself, by design:

* the **census-off** binary is `map::Builder::on_row`'s body preceded by a bare
  `return;` -- a source patch no harness should perform. Build it by hand (the
  recipe is in measurements.md) and point `PGDQ_MEASURE_CENSUS_OFF_BIN` at it.
  **It carries a `.stamp` beside it naming the commit it was built from**, the
  way a generated input does, and a census figure is refused when that is not
  the commit being measured: not building it and not trusting an unstamped one
  are different rules, and the second is what says the difference between the
  two binaries is the census;
* the **pre-throttle** binary for the quadratic table's "before" column is a
  release build of a historical commit. This one *is* mechanical, so the
  harness builds it into a git worktree when it is missing, or takes
  `PGDQ_MEASURE_BEFORE_BIN`.

Machine facts stay out of here: every path is an environment variable whose
default suits the machine CLAUDE.local.md describes, and the procedure lives in
CLAUDE.md.

Usage:

    cd scripts
    uv run measure.py --list
    uv run measure.py --figure census-brace-free
    uv run measure.py --stage warm
    uv run measure.py --all
    uv run measure.py --stale --since <rev>
    uv run measure.py --profile-recipe
    uv run python -m unittest test_measure -v

Two invocations are printed rather than run, for opposite reasons: koji's
because it is an hour on another medium, and the sampling profile's because a
profile is not a figure at all. Both live here because a command kept in prose
is a command that stops running.
"""

from __future__ import annotations

import argparse
import atexit
import dataclasses
import hashlib
import json
import os
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
from typing import Callable, Iterable, Sequence

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

REPO = Path(__file__).resolve().parent.parent
SCRIPTS = REPO / "scripts"

GIB = 1024**3
MIB = 1024**2

# The commit preceding `SaveThrottle`, which is the quadratic table's "before"
# column. A whole-commit comparison, not a throttle-isolating one -- see that
# section in measurements.md.
BEFORE_COMMIT = "b726f6b"


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
        _env("PGDQ_MEASURE_CACHE_DIR", "/mnt/ssd/fedora/scratch/pgdump_query/measure")
    )
    # tmpfs, for every warm figure.
    warm_dir: Path = Path(_env("PGDQ_MEASURE_WARM_DIR", "/dev/shm/pgdq"))
    # A second *device*, not a second cache: the `cold-nvme` regime exists to
    # read the same bytes off a disk fast enough that the parse is not hidden
    # behind it, so what this path names has to be NVMe and not the SSD
    # `cache_dir` points at. Inputs are copied here from that cache and kept,
    # exactly as they are kept there -- it is disk, not RAM.
    nvme_dir: Path = Path(
        _env("PGDQ_MEASURE_NVME_DIR", "/var/tmp/pgdump_query/measure")
    )
    # How much of the tmpfs the harness may fill. **Normally computed, not
    # configured**: the harness knows which inputs each figure needs and how
    # big they are, so the budget is the largest figure's own need plus
    # WARM_MARGIN. A constant here would be a guess -- and a guess that is
    # exactly the nominal sum fails on the few KB every generator overshoots
    # by, twenty minutes into a sweep. Set the variable only to cap it below
    # what the machine would otherwise allow.
    warm_budget: float | None = (
        float(os.environ["PGDQ_MEASURE_TMPFS_BUDGET_GIB"])
        if "PGDQ_MEASURE_TMPFS_BUDGET_GIB" in os.environ
        else None
    )
    out_dir: Path = Path(_env("PGDQ_MEASURE_OUT_DIR", str(REPO / "runs")))

    container: str = _env("PGDQ_MEASURE_CONTAINER", "sudo nerdctl")
    image: str = _env("PGDQ_MEASURE_IMAGE", "postgres:16")
    memory: str = _env("PGDQ_MEASURE_MEMORY", "512m")
    sudo: str = _env("PGDQ_MEASURE_SUDO", "sudo")

    bin_pgdq: Path = Path(_env("PGDQ_MEASURE_BIN", str(REPO / "target/release/pgdq")))
    bin_nocensus: Path = Path(
        _env("PGDQ_MEASURE_CENSUS_OFF_BIN", str(REPO / "runs/pgdq-nocensus"))
    )
    bin_before: Path = Path(
        _env("PGDQ_MEASURE_BEFORE_BIN", str(REPO / "runs/pgdq-before-throttle"))
    )
    # The `allocator` figure's three legs. Each is a full cargo target dir, so
    # it goes on scratch rather than under `runs/`, which holds logs and small
    # binaries; the binaries themselves are copied into `runs/`. A separate
    # target dir per leg is not tidiness: a `--features` build writes
    # `target/release/pgdq`, so building a leg in the default dir would
    # silently replace `bin_pgdq` and every other figure in the same sweep
    # would be timed under the wrong allocator.
    alloc_build_root: Path = Path(
        _env(
            "PGDQ_MEASURE_ALLOC_BUILD_ROOT",
            "/mnt/ssd/fedora/scratch/pgdump_query/alloc-builds",
        )
    )

    # The size of the seven large inputs. 3.00 GiB is the recorded apparatus;
    # anything else marks the run unpublishable.
    size_gib: float = float(_env("PGDQ_MEASURE_SIZE_GIB", "3.0"))
    # Pin every CPU to SWEEP_GOVERNOR for the sweep. Off by default -- see
    # that constant for the measurement that says why.
    pin_governor: bool = _env("PGDQ_MEASURE_PIN_GOVERNOR", "") not in ("", "0", "no")
    # Rep-count override, for smoke runs only. None means each figure's own.
    reps_override: int | None = None
    dry_run: bool = False
    keep_warm: bool = False

    @property
    def bin_nocensus_stamp(self) -> Path:
        """The commit the census-off binary was built from, recorded beside it
        exactly as a generated input's `.stamp` sits beside the input.

        Derived from the binary's own path rather than given its own
        environment variable, so pointing `PGDQ_MEASURE_CENSUS_OFF_BIN` at
        another build moves the stamp with it and cannot leave the two
        describing different files."""
        return self.bin_nocensus.with_name(self.bin_nocensus.name + ".stamp")

    @property
    def publishable(self) -> bool:
        """A run whose apparatus departs from the recorded one must not have
        its tables pasted into the doc."""
        return abs(self.size_gib - 3.0) < 1e-9 and self.reps_override is None and not self.dry_run

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
        self._thread = threading.Thread(target=self._loop, name="pgdq-sampler", daemon=True)
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
#: added regime would silently take every reading it was given.
CONTENTION_LIMITS: dict[str, dict[str, float]] = {
    "cold": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "cold-nvme": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "warm": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
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
    two seconds, two above -- the timer resolves to 1 ms either way."""
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

    def argv(self, cfg: Config, out: Path) -> list[str]:
        args = list(self.args)
        if self.scales:
            args = [
                f"{cfg.size_gib * 1024:g}" if a == "@SIZE_MB@" else a for a in args
            ]
            args = [f"{cfg.size_gib:g}" if a == "@SIZE_GB@" else a for a in args]
        return ["uv", "run", self.generator, *args, str(out)]


def _perf(name: str, *flags: str) -> InputSpec:
    return InputSpec(name, "generate_perf_data.py", (*flags, "--size-mb", "@SIZE_MB@"))


INPUTS: dict[str, InputSpec] = {
    # The brace-free control. Its lack of `{`/`[` is a contract, not an
    # accident: the census and scan-throughput figures are taken on it.
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
}
for _n in (500, 1000, 2000, 4000):
    INPUTS[f"blocks{_n}"] = InputSpec(
        f"blocks{_n}",
        "generate_block_count_bench.py",
        ("--blocks", str(_n), "--out"),
        scales=False,
    )


def input_stamp(spec: InputSpec, cfg: Config) -> str:
    """A hash of the generator's source and the arguments it was given.

    The whole-file bench's lesson, applied to the harness's own inputs: a
    generated file regenerated only when *missing* means a checkout that
    already has one benchmarks pre-change bytes forever, silently -- and the
    population that has one is exactly the population comparing a new number to
    an old one."""
    h = hashlib.sha256()
    h.update((SCRIPTS / spec.generator).read_bytes())
    h.update(repr(spec.argv(cfg, Path("OUT"))).encode())
    return h.hexdigest()


#: Headroom over the largest figure's own inputs. It absorbs the kilobytes a
#: generator overshoots its target by, and nothing more — the budget tracks the
#: need rather than a round number, so it is portable to a machine whose
#: `/dev/shm` is smaller than this one's.
WARM_MARGIN = 1.10


def nominal_size(cfg: Config, name: str) -> int:
    """What an input would weigh, for a dry run that has not generated it."""
    spec = INPUTS[name]
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
        self.needs: dict[str, list[int]] = {}
        self.figure_need: dict[str, int] = {}
        self._profiles: dict[str, dict] = {}
        self._pretended: set[str] = set()
        self._staged: dict[str, int] = {}
        self._adopt_warm_dir()

    # -- generation -------------------------------------------------------

    def cold_path(self, name: str) -> Path:
        self.ensure_generated(name)
        return self.cfg.cache_dir / f"{name}.sql"

    def ensure_generated(self, name: str) -> Path:
        spec = INPUTS[name]
        out = self.cfg.cache_dir / f"{name}.sql"
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
        self.log(f"  generating {name} -> {out}")
        run(spec.argv(self.cfg, out), cwd=SCRIPTS)
        stamp.write_text(want + "\n")
        # A regenerated input invalidates whatever was profiled or staged off
        # the old bytes.
        self._profiles.pop(name, None)
        self._staged.pop(name, None)
        warm = self.cfg.warm_dir / f"{name}.sql"
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
        dst = self.cfg.nvme_dir / f"{name}.sql"
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
        and is a candidate for eviction like anything else."""
        if self.cfg.dry_run or not self.cfg.warm_dir.exists():
            return
        for path in self.cfg.warm_dir.glob("*.sql"):
            self._staged[path.stem] = path.stat().st_size

    def _warm_bytes(self) -> int:
        return sum(self._staged.values())

    def warm_path(self, name: str, figure_index: int = 0) -> Path:
        src = self.ensure_generated(name)
        dst = self.cfg.warm_dir / f"{name}.sql"
        size = file_size(self.cfg, src, name)
        if self._staged.get(name) == size and (self.cfg.dry_run or dst.exists()):
            return dst
        self._staged.pop(name, None)
        self._make_room(size, figure_index)
        self._staged[name] = size
        if self.cfg.dry_run:
            self.log(f"  [dry-run] would stage {name} -> {dst}")
            return dst
        self.cfg.warm_dir.mkdir(parents=True, exist_ok=True)
        self.log(f"  staging {name} -> {dst}")
        shutil.copyfile(src, dst)
        return dst

    def _make_room(self, need: int, figure_index: int) -> None:
        budget = self.budget()
        while self._warm_bytes() + need > budget:
            if not self._staged:
                raise RuntimeError(
                    f"{need / GIB:.2f} GiB does not fit in a {budget / GIB:.2f} GiB "
                    "tmpfs budget even when empty"
                )
            # Evict what no remaining figure wants; failing that, what is
            # wanted latest. An input the *current* figure wants is never a
            # victim -- that would be a budget too small for one figure.
            victim = max(self._staged, key=lambda n: self._next_need(n, figure_index))
            if self._next_need(victim, figure_index) <= figure_index:
                raise RuntimeError(
                    "the tmpfs budget cannot hold one figure's inputs at once: still needs "
                    f"{victim} and {need / GIB:.2f} GiB more; raise "
                    "PGDQ_MEASURE_TMPFS_BUDGET_GIB"
                )
            self.log(f"  evicting {victim} from tmpfs")
            del self._staged[victim]
            if not self.cfg.dry_run:
                (self.cfg.warm_dir / f"{victim}.sql").unlink(missing_ok=True)

    def plan(self, figures: Sequence[Figure]) -> None:
        """Record which figures want each input warm, so eviction can pick the
        one nothing is waiting on -- and, failing that, the one wanted
        latest. Also size each figure, which is what the budget is computed
        from."""
        self.needs = {}
        for i, fig in enumerate(figures):
            for name in fig.warm_inputs:
                self.needs.setdefault(name, []).append(i)
        self.figure_need = {
            fig.id: sum(self.expected_size(n) for n in fig.warm_inputs) for fig in figures
        }

    def expected_size(self, name: str) -> int:
        """What an input weighs: measured if it has been generated, nominal if
        not. Nominal is the low estimate -- every generator overshoots its
        target by a few KB -- which is what WARM_MARGIN is for."""
        path = self.cfg.cache_dir / f"{name}.sql"
        if path.exists():
            return path.stat().st_size
        return nominal_size(self.cfg, name)

    def budget(self) -> int:
        """The tmpfs ceiling: one figure's inputs plus headroom, since only one
        figure's inputs are ever needed at once and eviction handles the rest.
        An explicit PGDQ_MEASURE_TMPFS_BUDGET_GIB overrides it."""
        if self.cfg.warm_budget is not None:
            return int(self.cfg.warm_budget * GIB)
        largest = max(self.figure_need.values(), default=0)
        return int(largest * WARM_MARGIN)

    def preflight(self, figures: Sequence[Figure]) -> list[str]:
        """Everything knowable before the first run: does each figure fit the
        budget, does the budget fit the tmpfs, do the inputs fit the disk they
        are generated onto.

        This exists because the alternative is finding out twenty minutes in,
        with a figure already lost and its dependants failing behind it."""
        problems: list[str] = []
        budget = self.budget()
        for fig in figures:
            need = self.figure_need.get(fig.id, 0)
            if need > budget:
                problems.append(
                    f"{fig.id} needs {need / GIB:.2f} GiB of tmpfs at once, over the "
                    f"{budget / GIB:.2f} GiB budget"
                )
        try:
            self.cfg.warm_dir.mkdir(parents=True, exist_ok=True)
            warm_free = shutil.disk_usage(self.cfg.warm_dir).free + self._warm_bytes()
        except OSError as exc:
            problems.append(f"{self.cfg.warm_dir} is not usable as a staging area: {exc}")
            warm_free = budget
        if budget > warm_free:
            problems.append(
                f"{self.cfg.warm_dir} has {warm_free / GIB:.2f} GiB usable, under the "
                f"{budget / GIB:.2f} GiB this sweep needs resident. Point "
                "PGDQ_MEASURE_WARM_DIR at a larger memory-backed filesystem, or lower "
                "PGDQ_MEASURE_SIZE_GIB — which makes the run unpublishable"
            )
        wanted = {
            n
            for fig in figures
            for n in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs)
        }
        missing = sum(
            self.expected_size(n)
            for n in wanted
            if not (self.cfg.cache_dir / f"{n}.sql").exists()
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
            if not (self.cfg.nvme_dir / f"{n}.sql").exists()
        )
        if want > free:
            return [
                f"{self.cfg.nvme_dir} has {free / GIB:.2f} GiB free, under the "
                f"{want / GIB:.2f} GiB of inputs the cold-NVMe figures still need copied "
                "there. Point PGDQ_MEASURE_NVME_DIR at an NVMe volume with room"
            ]
        return []

    def _next_need(self, name: str, figure_index: int) -> float:
        return min(
            (i for i in self.needs.get(name, []) if i >= figure_index), default=float("inf")
        )

    def cleanup(self) -> None:
        if self.cfg.keep_warm or self.cfg.dry_run:
            return
        for name in list(self._staged):
            path = self.cfg.warm_dir / f"{name}.sql"
            if path.exists():
                self.log(f"  removing {path}")
                path.unlink()
            del self._staged[name]
        # The caches a run writes into the staging directory go with it.
        for leftover in self.cfg.warm_dir.glob("*.dqcache"):
            leftover.unlink(missing_ok=True)

    # -- profiling --------------------------------------------------------

    def profile(self, name: str) -> dict:
        """Row and column counts for an input, off `pgdq info --json`.

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
        tmp = self.cfg.cache_dir / f"{name}.profile.dqcache"
        tmp.unlink(missing_ok=True)
        run(
            [str(self.cfg.bin_pgdq), "parse", "--source", str(path), "--dqcache", str(tmp)],
            quiet=True,
        )
        out = run(
            [str(self.cfg.bin_pgdq), "info", "--dqcache", str(tmp), "--json"], capture=True
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

    binary: str  # "pgdq" | "nocensus" | "before" | "none" (dd)
    input: str
    command: str
    regime: str  # "cold" | "cold-nvme" | "warm"
    label: str

    def key(self, figure: str) -> str:
        return f"{figure}/{self.binary}/{self.input}/{self.command}/{self.regime}"


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
    """The `pgdq query` flags that ask for one width.

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
    nothing folds two terms (`architecture.md`, "Predicates"), but a table
    whose rows differ only in how often one term is repeated invites the
    reader to wonder."""
    if shape not in PREDICATE_SHAPES:
        raise ValueError(f"no predicate registered as {shape!r}")
    column, terms = PREDICATE_SHAPES[shape]
    if terms < 1:
        raise ValueError(f"predicate {shape!r} asks for no terms")
    return " OR ".join(f"{column}=zzz{i}" for i in range(1, terms + 1))


#: The read chunk sizes `chunk-size` is taken at, in bytes, smallest first.
#: `1 << 20` is the shipped default (`scan::DEFAULT_CHUNK_SIZE`) and every
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
    run with no number under it."""
    q = "time /pgdq"
    if command == "parse":
        return f"{q} parse --source /dump.sql --dqcache /tmp/x.dqcache >/dev/null"
    if command == "parse-preamble":
        return f"{q} parse --preamble-only --source /dump.sql --dqcache /tmp/x.dqcache >/dev/null"
    if command == "parse-cache-out":
        # The cache goes to the mounted tmpfs, not the container's own layer,
        # and the removal is outside the timer.
        return (
            "rm -f /out/measure.dqcache; "
            f"{q} parse --source /dump.sql --dqcache /out/measure.dqcache >/dev/null"
        )
    if command in ("query-typed", "query-strings"):
        mode = command.split("-")[1]
        return (
            f"{q} query --source /dump.sql --table public.perf --dqcache none "
            f"--schema-mode {mode} >/dev/null"
        )
    if command.startswith("query-project-"):
        # Typed, always: the figure is about what building a column costs, and
        # `strings` builds every column the same cheap way.
        width = command.rpartition("-")[2]
        if not width.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        return (
            f"{q} query --source /dump.sql --table public.perf --dqcache none "
            f"--schema-mode typed {projection_flags(int(width))} >/dev/null"
        )
    if command.startswith("query-where-"):
        # `strings`, always, for two reasons that agree. The typed `=` decodes
        # the literal once against the column's own type, so `zzz1` on an
        # `integer` column is `Error::PredicateValueDecode` before the first
        # row; and the zero-copy path is the one this lever is read against,
        # since `strings` is where the field split and the walk are most of
        # what the library does (`architecture.md`, "The library's own per-row
        # budget", whose split-and-walk and decode rows do not move with the
        # mode).
        expr = predicate_expr(command.removeprefix("query-where-"))
        return (
            f"{q} query --source /dump.sql --table public.perf --dqcache none "
            f"--schema-mode strings --where '{expr}' >/dev/null"
        )
    if command == "query-nomatch":
        # Maps to EOF (the table never matches) and never saves.
        return f"{q} query --source /dump.sql --table public.nosuchtable --dqcache none >/dev/null"
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
            f"{q} parse --source /dump.sql --dqcache /tmp/x.dqcache "
            f"--chunk-size {size} >/dev/null"
        )
    if command == "dd":
        return "time dd if=/dump.sql of=/dev/null bs=4M"
    raise ValueError(f"unknown command shape {command!r}")


class Session:
    def __init__(self, cfg: Config, stager: Stager, log: Callable[[str], None]) -> None:
        self.cfg = cfg
        self.stager = stager
        self.log = log
        self.readings: dict[str, list[float]] = {}
        self.records: list[dict] = []
        self.figure_index = 0
        self.figure_id = ""
        self._dry_reps: dict[str, int] = {}
        self._last_telemetry: dict[str, float] = {}
        self.sampler = Sampler()
        #: Every reading's telemetry, in the order taken, so a sweep can be
        #: audited after the fact even where the gate let a reading through.
        self.telemetry: list[dict] = []

    # -- one timed run ----------------------------------------------------

    def binary_path(self, which: str) -> Path:
        if which == "pgdq":
            return self.cfg.bin_pgdq
        if which == "nocensus":
            return self.cfg.bin_nocensus
        if which == "before":
            return ensure_before_binary(self.cfg, self.log)
        if which.startswith("alloc:"):
            return ensure_allocator_binary(self.cfg, which.removeprefix("alloc:"), self.log)
        raise ValueError(f"unknown binary {which!r}")

    def input_path(self, name: str, regime: str) -> Path:
        if regime == "cold":
            return self.stager.cold_path(name)
        if regime == "cold-nvme":
            return self.stager.nvme_path(name)
        return self.stager.warm_path(name, self.figure_index)

    def drop_caches(self) -> None:
        argv = shlex.split(self.cfg.sudo) + ["sh", "-c", "sync; echo 3 > /proc/sys/vm/drop_caches"]
        if self.cfg.dry_run:
            self.log("  [dry-run] " + " ".join(argv))
            return
        run(argv)

    def time_run(self, spec: RunSpec) -> float:
        dump = self.input_path(spec.input, spec.regime)
        mounts = [f"{dump}:/dump.sql:ro"]
        if spec.binary != "none":
            mounts.insert(0, f"{self.binary_path(spec.binary)}:/pgdq:ro")
        if spec.command == "parse-cache-out":
            mounts.append(f"{self.cfg.warm_dir}:/out")
        argv = [
            *self.cfg.container_argv(),
            "run",
            "--rm",
            "-m",
            self.cfg.memory,
            "--memory-swap",
            self.cfg.memory,
        ]
        for m in mounts:
            argv += ["-v", m]
        argv += [self.cfg.image, "bash", "-c", _script(spec.command)]

        # Every cold regime drops the cache, whichever device it names: what
        # "cold" means is that the bytes come off the disk, and a `cold-nvme`
        # reading taken out of page cache would measure RAM and read as a
        # device with no floor at all.
        if spec.regime.startswith("cold"):
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
            return 0.4 + digest[0] / 255 * 5.0
        # The counters bracket the run as tightly as possible: two procfile
        # reads, outside the timer, either side of the subprocess. Their
        # difference is what the machine did *during this rep* -- which is the
        # only window that means anything for a reading half a second long.
        started = time.time()
        before, mono_start = Counters.read(), time.monotonic()
        proc = subprocess.run(argv, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        mono_end, after = time.monotonic(), Counters.read()
        if proc.returncode != 0:
            raise RuntimeError(
                f"{spec.label} exited {proc.returncode}\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}"
            )
        seconds = parse_bash_time(proc.stderr)
        telemetry = counter_delta(before, after)
        telemetry.update(self.sampler.window(mono_start, mono_end))
        self.records.append(
            {
                "spec": dataclasses.asdict(spec),
                "seconds": seconds,
                "wall_including_container": round(time.time() - started, 3),
                "telemetry": telemetry,
                "argv": argv,
            }
        )
        self.telemetry.append({"key": spec.key(self.figure_id), **telemetry})
        self._last_telemetry = telemetry
        return seconds

    # -- sweeps -----------------------------------------------------------

    def sweep(self, figure: str, specs: Sequence[RunSpec], reps: int) -> None:
        """One interleaved sweep: every rep runs every spec in turn, and the
        second half of the reps runs them in the opposite order.

        Both halves matter. Interleaving keeps a session's slow upward drift
        off whichever file went first; reversing keeps the pair's own warming
        off whichever binary went first."""
        keys = [s.key(figure) for s in specs]
        for k in keys:
            self.readings.setdefault(k, [])
        for rep in range(reps):
            order = list(specs) if rep < (reps + 1) // 2 else list(reversed(specs))
            for spec in order:
                seconds = self.take(spec, rep)
                self.readings[spec.key(figure)].append(seconds)

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
            verdict = contention_verdict(self._last_telemetry, spec.regime)
            if verdict is None:
                self.log(f"  rep{rep + 1} {spec.label}: {seconds:.3f} s")
                return seconds
            self.log(
                f"  rep{rep + 1} {spec.label}: {seconds:.3f} s — DISCARDED, "
                f"machine contended ({verdict}); attempt {attempt}/{GATE_RETRIES}"
            )
        raise RuntimeError(
            f"{spec.label}: {GATE_RETRIES} consecutive readings were taken under contention "
            f"({contention_verdict(self._last_telemetry, spec.regime)}) — "
            "the machine is too busy to measure on"
        )

    def get(self, figure: str, spec: RunSpec) -> list[float]:
        return self.readings[spec.key(figure)]

    def has(self, figure: str, spec: RunSpec) -> bool:
        return spec.key(figure) in self.readings

    def borrow(self, figure: str, shared: Shared) -> list[RunSpec]:
        """Copy the readings one declared `Shared` names into `figure`'s keys.

        A reading another figure already took. The warm scan-throughput table's
        `COPY` row *is* the census table's warm census-on column -- re-measuring
        it would put two different numbers in the doc for one measurement.

        Returns the specs that were satisfied, which is empty when the source
        figure was not in this sitting. What is *not* satisfied is left absent
        rather than faked, so the figure's own sweep measures it and the note
        says so."""
        got = []
        for spec in shared.republished:
            readings = self.readings.get(spec.key(shared.source))
            if readings:
                self.readings[spec.key(figure)] = list(readings)
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


def census_binary_problem(
    cfg: Config, resolve: Callable[[str], str | None] = resolve_commit
) -> str | None:
    """Why the census-off binary may not be measured against this commit, if it
    may not. `None` means it may.

    **Refusing to build it and refusing to trust it are two rules, and only the
    first was here.** The harness will not apply the source patch -- a harness
    that patches its own subject can produce any figure it likes -- so the
    binary arrives from a hand build, and nothing recorded which tree it came
    from. Every *generated input* answers exactly that question with a `.stamp`
    beside it; this is the same file for the same reason, and the recipe in
    `measurements.md` writes it.

    The cost of not having it is not hypothetical. A census figure is a
    subtraction between this binary and `target/release/pgdq`, so **everything
    that differs between the two trees is attributed to the census**: the
    binary found 40 commits behind on 2026-09-05 would have charged ten slices
    of read-path work to the census, in the one table that was already the
    register's largest correction, with nothing to tell the two errors apart.

    What the stamp buys is bounded, and worth saying: it is only as honest as
    the hand that wrote it, so it cannot catch a re-stamp without a rebuild.
    What it does catch is *age*, which is the failure that actually happened
    and the one nothing else can see. Three things stay deliberately out of
    scope: a dirty tree, which the session stamp already declares;
    `bin_before`, which is a build of a fixed historical commit -- not HEAD by
    design -- and which the harness builds for itself and therefore knows the
    provenance of; and `--dry-run`, which checks no binary at all because it
    measures nothing and must run where none exists, so the refusal it would
    give lands seconds later instead, at the first second of the sitting that
    would have published the figure.
    """
    if not cfg.bin_nocensus.exists():
        return (
            f"{cfg.bin_nocensus} is missing. The census-off binary is a source patch no harness "
            "should perform: add a bare `return;` as the first statement of "
            "`map::Builder::on_row`, `cargo build --release -p pgdump_query-cli`, copy the binary "
            f"to {cfg.bin_nocensus}, then revert. measurements.md's census section has the recipe."
        )
    want = resolve("HEAD")
    if want is None:
        return (
            "HEAD does not resolve to a commit, so nothing can say which source "
            f"{cfg.bin_nocensus} ought to have been built from."
        )
    stamp = cfg.bin_nocensus_stamp
    if not stamp.exists():
        return (
            f"{stamp} is missing, so nothing says which source {cfg.bin_nocensus} was built from "
            "— and a census figure is that binary differenced against this one. Rebuild it from "
            "measurements.md's census recipe, which ends by writing that stamp."
        )
    text = stamp.read_text().strip()
    got = resolve(text) if text else None
    if got is None:
        return (
            f"{stamp} reads {text!r}, which is not a commit in this repository. It must name the "
            f"commit {cfg.bin_nocensus} was built from."
        )
    if got != want:
        return (
            f"{cfg.bin_nocensus} was built at {got[:7]}, not the {want[:7]} being measured, so a "
            "census figure would charge everything between the two to the census. Rebuild it from "
            f"measurements.md's census recipe, which ends by re-writing {stamp}."
        )
    return None


def ensure_before_binary(cfg: Config, log: Callable[[str], None]) -> Path:
    """The pre-throttle release build, from a git worktree.

    Unlike the census-off patch this is mechanical -- a commit and a release
    build -- so the harness does it rather than asking for a binary. It is
    cached under runs/, keyed by the commit."""
    if cfg.bin_before.exists():
        return cfg.bin_before
    work = cfg.out_dir / f"worktree-{BEFORE_COMMIT}"
    log(f"  building the pre-throttle binary at {BEFORE_COMMIT}")
    if cfg.dry_run:
        return cfg.bin_before
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    if not work.exists():
        run(["git", "worktree", "add", "--detach", str(work), BEFORE_COMMIT], cwd=REPO)
    try:
        run(
            ["cargo", "build", "--release", "-p", "pgdump_query-cli"],
            cwd=work,
        )
        shutil.copyfile(work / "target/release/pgdq", cfg.bin_before)
        cfg.bin_before.chmod(0o755)
    finally:
        run(["git", "worktree", "remove", "--force", str(work)], cwd=REPO)
    return cfg.bin_before


#: The three legs of the `allocator` figure, in the order the table carries
#: them. `system` is the feature-free build -- the platform allocator, glibc's
#: `malloc` on the recorded apparatus -- and is spelled the way the binary
#: spells it rather than as `glibc`, because the crate cannot know which libc
#: it was linked against and the measurement records the image.
ALLOCATOR_LEGS: tuple[str, ...] = ("system", "jemalloc", "mimalloc")

#: What `pgdq --version` appends. The harness *asks the binary* rather than
#: trusting the flags it passed: a leg mislabelled by one word gives a
#: perfectly plausible table of the wrong comparison, which is the same family
#: of failure as profiling the `release` binary and calling it `profiling`.
ALLOCATOR_RE = re.compile(r"\(allocator: ([a-z]+)\)")


def binary_allocator(binary: Path) -> str:
    """Which allocator a built `pgdq` links against, read out of the binary.

    Not an optional nicety: the day the CLI's default feature set changes,
    `target/release/pgdq` becomes a different binary and every apparatus line
    that still names the old allocator is wrong with nothing to notice. This is
    what the session stamp reports."""
    out = run([str(binary), "--version"], capture=True)
    match = ALLOCATOR_RE.search(out)
    if match is None:
        raise RuntimeError(
            f"{binary} --version does not name an allocator ({out.strip()!r}) — "
            "it is too old to be an `allocator` figure leg"
        )
    return match.group(1)


#: Legs whose (absent) build a dry run has already reported. Only a dry run
#: needs it: a real build leaves the binary behind, which is the memo.
_ALLOC_ANNOUNCED: set[str] = set()
#: Legs already built *by this process*. The cache is deliberately per-run and
#: not the file on disk: `runs/pgdq-alloc-<leg>` from an earlier session was
#: built from whatever the source said then, and reusing it compares a fresh
#: reference binary against a stale leg -- which is exactly the "plausible table
#: of the wrong comparison" this figure's assertions exist to stop, and it fails
#: silently because a leg still answers `--version` with its own allocator name.
_ALLOC_BUILT: set[str] = set()


def ensure_allocator_binary(cfg: Config, leg: str, log: Callable[[str], None]) -> Path:
    """One leg of the `allocator` figure, built and then interrogated.

    Three details are load-bearing and each fails by producing a table of
    something else:

    * **`--no-default-features`**, so the `system` leg stays the platform
      allocator whatever the CLI's default becomes. Without it this figure
      stops being re-takeable the moment a leg is adopted -- which is the one
      thing the figure exists to decide.
    * **Its own target dir**, so `target/release/pgdq` -- every other figure's
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
    out = cfg.out_dir / f"pgdq-alloc-{leg}"
    if leg in _ALLOC_BUILT:
        return out
    features = [] if leg == "system" else ["--features", leg]
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
            "cargo", "build", "--release", "-p", "pgdump_query-cli",
            "--no-default-features", *features,
            "--target-dir", str(target),
        ],
        cwd=REPO,
    )
    shutil.copyfile(target / "release/pgdq", out)
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


def count_saves(
    cfg: Config, binary: Path, dump: Path, log: Callable[[str], None]
) -> tuple[int, int]:
    """Cache writes during one `parse`, from `strace`.

    Traced on the host and untimed, so strace's overhead reaches no figure.
    **Both** `open` and `openat`: glibc uses one and musl the other, and
    tracing a single call silently reports zero saves against the other libc.
    `std::fs::write` opens once per save; the first open is the load's miss."""
    cache = cfg.warm_dir / "savecount.dqcache"
    cache.unlink(missing_ok=True)
    if cfg.dry_run:
        return (0, 0)
    proc = subprocess.run(
        [
            "strace", "-f", "-e", "trace=open,openat",
            str(binary), "parse", "--source", str(dump), "--dqcache", str(cache),
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


@dataclass
class Figure:
    id: str
    #: A human label for the measurements.md section this table belongs under.
    #: **Not an address**: the doc addresses a figure by its `<!-- figure: id -->`
    #: marker, so a heading may quote a number and may be rewritten when the
    #: number moves without desynchronising anything. `--check` reconciles the
    #: two.
    section: str
    stage: str  # "cold" | "warm" | "criterion"
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
    #: Documents that repeat this figure's numbers, or the claim it licenses.
    #: `depends` is the edge into a figure -- what invalidates it; this is the
    #: edge out -- what a moved figure invalidates. Both exist for the same
    #: reason: "someone will notice" is not a mechanism, and the doc set has
    #: already drifted this way (`architecture.md` quotes 4003 -> 195 saves and
    #: a 19.7 s map where `measurements.md`'s table says 103 and 18.62).
    quoted_by: tuple[str, ...] = ()
    run: Callable[[Session], str] = field(default=lambda s: "")

    @property
    def requires(self) -> tuple[str, ...]:
        """The figures whose readings this one uses, pulled in automatically.

        Derived from `shares` rather than declared beside it: two lists of the
        same fact drift, and the one that drifts is the one no run function
        reads."""
        return tuple(dict.fromkeys(s.source for s in self.shares))


#: The paths behind each mechanism a figure can depend on. Declared narrowly
#: on purpose: a blanket `pgdump_query/src/` would mark every figure stale on
#: every commit, which is a `--stale` nobody reads. The cost of narrowness is
#: that a change *outside* these paths that moves a figure goes unannounced —
#: which is why the doc carries a session stamp as well, so "are these figures
#: from before or after my change" has a second answer.
SCAN = ("pgdump_query/src/scan.rs", "pgdump_query/src/copy.rs", "pgdump_query/src/stream.rs")
#: Every figure that times a `pgdq` run over a file reads its bytes through
#: this one module, whatever else the figure is about, so it is its own
#: mechanism rather than part of `SCAN`: `nested-end-to-end` and
#: `census-attribution` declare no scanner path and are still moved by it.
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
NESTED = ("pgdump_query/src/nested.rs", "pgdump_query/src/batch.rs")
PREAMBLE = ("pgdump_query/src/index.rs", "pgdump_query/src/preamble.rs")
#: A query figure also reads through the CLI's own row rendering. The whole
#: `src/` directory rather than `main.rs`: a declared path is matched by
#: prefix, so naming the one file leaves every other module in that crate as a
#: staleness edge nobody declared -- and the crate now has three
#: (`main.rs`, `where_expr.rs`, `alloc.rs`).
QUERY_CLI = ("pgdump_query-cli/src/",)

GEN_PERF = ("scripts/generate_perf_data.py",)
GEN_BLOCKS = ("scripts/generate_block_count_bench.py",)
GEN_SHAPES = (
    "scripts/generate_perf_data.py",
    "scripts/generate_large_object_bench.py",
    "scripts/generate_insert_run_bench.py",
)


# -- scan throughput --------------------------------------------------------

_THROUGHPUT_ROWS = (
    ("control", "parse", "`COPY` block"),
    ("large_object", "parse", "Large-object region"),
    ("insert_run", "parse", "`INSERT` run"),
    ("control", "dd", "`dd` → `/dev/null`"),
)


def _throughput_specs(regime: str) -> list[RunSpec]:
    return [
        RunSpec("none" if cmd == "dd" else "pgdq", inp, cmd, regime, f"{label} ({regime})")
        for inp, cmd, label in _THROUGHPUT_ROWS
    ]


def _throughput_table(session: Session, figure: str, specs: Sequence[RunSpec]) -> str:
    floor = median(session.get(figure, specs[-1]))
    rows = []
    for spec, (_, _, label) in zip(specs, _THROUGHPUT_ROWS):
        values = session.get(figure, spec)
        size = file_size(session.cfg, session.input_path(spec.input, spec.regime), spec.input)
        against = "—" if spec.command == "dd" else f"{median(values) / floor:.2f}× the floor's time"
        rows.append(
            [label, fmt_median_spread(values), fmt_rate(size, median(values)), against]
        )
    return md_table(["Input", "Wall", "Rate", "Against the floor"], rows)


def _throughput_figure(session: Session, figure: str, regime: str, reps: int) -> str:
    """The four-row throughput table, in one regime.

    Its `COPY` row is not a measurement of its own: it is the census table's
    census-on column for the same regime -- same binary, same command, same
    input. Measuring it twice would put two numbers in the doc for one
    measurement, which is the defect the whole sweep exists to remove."""
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
            "pgdq",
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


# -- the census pair --------------------------------------------------------


def _census_specs(input_name: str, regime: str) -> tuple[RunSpec, RunSpec]:
    """The census table's two binaries, in one regime.

    A named pair rather than two constructions inline, because the census-on
    spec is **borrowed** by two other figures -- the warm throughput table's
    `COPY` row and the allocator table's reference column -- and a borrow is a
    dictionary lookup that silently returns nothing if the key drifts."""
    return (
        RunSpec("nocensus", input_name, "parse", regime, f"census off ({regime})"),
        RunSpec("pgdq", input_name, "parse", regime, f"census on ({regime})"),
    )


def _census_figure(session: Session, figure: str, input_name: str) -> str:
    """The census table: two binaries, one input, both regimes.

    Cold and warm are two rows of one table, so the table is only taken whole:
    half a comparison table may not be re-taken."""
    rows = []
    per_rep = []
    for regime in ("cold", "warm"):
        off, on = _census_specs(input_name, regime)
        session.sweep(figure, [off, on], session.cfg.reps(6))
        off_v, on_v = session.get(figure, off), session.get(figure, on)
        label = "cold, on the SSD" if regime == "cold" else "warm, on tmpfs"
        rows.append(
            [label, fmt_median_spread(off_v), fmt_median_spread(on_v), fmt_delta(median(off_v), median(on_v))]
        )
        per_rep.append(f"- {label} — census off: {fmt_readings(off_v)}; census on: {fmt_readings(on_v)}")

        floor = RunSpec("none", input_name, "dd", regime, f"dd floor ({regime})")
        session.sweep(figure, [floor], session.cfg.reps(3))

    table = md_table(["", "Census off", "Census on", "Δ"], rows)

    profile = session.stager.profile(input_name)
    warm_off = session.get(figure, RunSpec("nocensus", input_name, "parse", "warm", ""))
    warm_on = session.get(figure, RunSpec("pgdq", input_name, "parse", "warm", ""))
    delta = median(warm_on) - median(warm_off)
    per_row_ns = delta / profile["rows"] * 1e9 if profile["rows"] else 0.0
    floors = {
        r: median(session.get(figure, RunSpec("none", input_name, "dd", r, "")))
        for r in ("cold", "warm")
    }
    notes = (
        f"\nThe census costs **{delta:+.3f} s per {profile['bytes'] / GIB:.2f} GiB** of these rows — "
        f"{per_row_ns:.0f} ns per {profile['columns']}-column row of {profile['row_bytes']:,} bytes, "
        f"over {profile['rows']:,} rows.\n"
        f"`dd` → `/dev/null` on the same file in the same container: "
        f"**{fmt_s(floors['warm'])} s** warm, **{fmt_s(floors['cold'])} s** cold, so the census-off "
        f"scan is within {median(warm_off) / floors['warm']:.1f}× of what the kernel charges to hand "
        "over the bytes.\n"
    )
    return table + "\n" + notes + "\nPer-rep readings (s):\n" + "\n".join(per_rep) + "\n"


def run_census_brace_free(session: Session) -> str:
    return _census_figure(session, "census-brace-free", "control")


def run_census_arrays(session: Session) -> str:
    return _census_figure(session, "census-arrays", "arrays")


# -- nested end to end ------------------------------------------------------

_NESTED_FILES = (
    ("control", "control — 16 scalar columns"),
    ("composite", "`--composite` — the same 16 plus one composite"),
    ("arrays", "`--arrays --composite` — the same 16 plus three nested"),
)


def _nested_specs() -> list[RunSpec]:
    specs = []
    for name, _ in _NESTED_FILES:
        for mode in ("strings", "typed"):
            specs.append(RunSpec("pgdq", name, f"query-{mode}", "warm", f"{name} {mode}"))
    return specs


def run_nested_end_to_end(session: Session) -> str:
    figure = "nested-end-to-end"
    specs = _nested_specs()
    session.sweep(figure, specs, session.cfg.reps(5))
    rows, per_rep = [], []
    for name, label in _NESTED_FILES:
        s = RunSpec("pgdq", name, "query-strings", "warm", "")
        t = RunSpec("pgdq", name, "query-typed", "warm", "")
        sv, tv = session.get(figure, s), session.get(figure, t)
        profile = session.stager.profile(name)
        diff = median(tv) - median(sv)
        rows.append(
            [
                label,
                f"{profile['rows']:,}",
                f"{fmt_s(median(sv))} s",
                f"{fmt_s(median(tv))} s",
                f"**{diff / profile['rows'] * 1e6:.2f} µs/row**",
                f"{median(tv) / median(sv):.2f}×",
            ]
        )
        per_rep.append(f"- {name} — `strings`: {fmt_readings(sv)}; `typed`: {fmt_readings(tv)}")
    table = md_table(
        ["File", "Rows", "`strings`", "`typed`", "`typed` − `strings`", "Ratio"], rows
    )
    return table + "\n\nPer-rep readings (s):\n" + "\n".join(per_rep) + "\n"


# -- the cross-file floor ---------------------------------------------------


def _per_row_diffs(session: Session, figure: str, a: str, b: str) -> list[float]:
    """Per-rep µs/row differences between two files' own typed−strings costs.

    Each file's typed leg is differenced against *its own* strings leg first,
    which is what makes the subtraction legitimate: whatever the untyped
    baseline is worth on a given file cancels out of that file's own
    difference, and would not cancel out of a cross-file ratio."""
    out = []
    for name in (a, b):
        profile = session.stager.profile(name)
        s = session.get(figure, RunSpec("pgdq", name, "query-strings", "warm", ""))
        t = session.get(figure, RunSpec("pgdq", name, "query-typed", "warm", ""))
        out.append([(tv - sv) / profile["rows"] * 1e6 for sv, tv in zip(s, t)])
    reps = min(len(out[0]), len(out[1]))
    return [out[1][i] - out[0][i] for i in range(reps)]


def run_cross_file_floor(session: Session) -> str:
    figure = "cross-file-floor"
    # Row 1 is read off the nested sweep's own reps -- the same five runs, not
    # a second pass over the same two files.
    share = _per_row_diffs(session, "nested-end-to-end", "control", "composite")
    # Row 2 is this figure's own: two files that differ only in their seed.
    specs = []
    for name in ("control", "control43"):
        for mode in ("strings", "typed"):
            specs.append(RunSpec("pgdq", name, f"query-{mode}", "warm", f"{name} {mode}"))
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
    return md_table(["Reading", "Reps", "Paired median", "Per-rep readings"], rows)


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
    neither `cross-file-floor`'s subtraction floor nor the census's
    file-dependent untyped baseline enters. That is what makes this a
    supersession of that apparatus rather than one more figure beside it.
    """
    figure = "projection-widths"
    specs = [
        RunSpec("pgdq", "arrays", f"query-project-{width}", "warm", f"{width}-column")
        for width, _, _ in _PROJECTION_ROWS
    ]
    # Six, like the two differencing figures this replaces: the reading that
    # matters is a paired difference between adjacent rows, and pairing is
    # per rep.
    session.sweep(figure, specs, session.cfg.reps(6))
    profile = session.stager.profile("arrays")
    rows, per_rep, previous = [], [], None
    for width, label, buys in _PROJECTION_ROWS:
        spec = RunSpec("pgdq", "arrays", f"query-project-{width}", "warm", "")
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
        "median.\n\nPer-rep readings (s):\n"
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
        RunSpec("pgdq", "control", f"query-where-{shape}", "warm", label)
        for shape, label, _ in _PREDICATE_ROWS
    ]
    # Six, as `projection-widths` takes: the reading that matters is a paired
    # difference between two rows, and pairing is per rep.
    session.sweep(figure, specs, session.cfg.reps(6))
    profile = session.stager.profile("control")
    rows, per_rep, previous = [], [], None
    for shape, label, buys in _PREDICATE_ROWS:
        spec = RunSpec("pgdq", "control", f"query-where-{shape}", "warm", "")
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
        "paired rep by rep and then taken as a median.\n\nAs written:\n"
        + written
        + "\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- census attribution -----------------------------------------------------


def run_census_attribution(session: Session) -> str:
    figure = "census-attribution"
    files = ("control", "arrays")
    specs = [
        RunSpec(binary, name, "query-strings", "warm", f"{binary} {name}")
        for binary in ("pgdq", "nocensus")
        for name in files
    ]
    session.sweep(figure, specs, session.cfg.reps(5))
    rows, per_rep = [], []
    for binary, label in (("pgdq", "census on"), ("nocensus", "census off")):
        medians = [
            median(session.get(figure, RunSpec(binary, n, "query-strings", "warm", "")))
            for n in files
        ]
        rows.append(
            [label, f"{medians[0]:.3f} s", f"{medians[1]:.3f} s", f"**{medians[1] - medians[0]:+.3f} s**"]
        )
        for n in files:
            v = session.get(figure, RunSpec(binary, n, "query-strings", "warm", ""))
            per_rep.append(f"- {label}, {n}: {fmt_readings(v)}")
    table = md_table(["", "control", "`--arrays --composite`", "gap"], rows)
    return table + "\n\nPer-rep readings (s):\n" + "\n".join(per_rep) + "\n"


# -- the per-block quadratic ------------------------------------------------

_BLOCK_COUNTS = (500, 1000, 2000, 4000)

#: The series, and the control that makes it readable as one: the same byte
#: count in **one** `COPY` block. Without it the table shows a cost rising with
#: block count and cannot say how much of the cost *is* block count — the
#: one-block run is what puts the 4000-block figure three orders of magnitude
#: above the scan it protects. It was a command in the doc's prose and a number
#: nobody re-took; a control the harness does not run is a control that goes
#: stale silently.
_QUADRATIC_ROWS: tuple[tuple[str, str], ...] = (
    ("one_block", "1 (control)"),
    *tuple((f"blocks{n}", f"{n}") for n in _BLOCK_COUNTS),
)


def run_per_block_quadratic(session: Session) -> str:
    figure = "per-block-quadratic"
    specs = []
    for name, _ in _QUADRATIC_ROWS:
        for binary in ("before", "pgdq"):
            specs.append(RunSpec(binary, name, "parse-cache-out", "warm", f"{binary} {name}"))
    session.sweep(figure, specs, session.cfg.reps(2))

    rows, per_rep = [], []
    for name, label in _QUADRATIC_ROWS:
        dump = session.input_path(name, "warm")
        before = session.get(figure, RunSpec("before", name, "parse-cache-out", "warm", ""))
        after = session.get(figure, RunSpec("pgdq", name, "parse-cache-out", "warm", ""))
        saves_before, _ = count_saves(session.cfg, session.binary_path("before"), dump, session.log)
        saves_after, cache_size = count_saves(
            session.cfg, session.binary_path("pgdq"), dump, session.log
        )
        rows.append(
            [
                label,
                _fmt_bytes(file_size(session.cfg, dump, name)),
                _fmt_bytes(cache_size),
                f"{fmt_s(median(before))} s",
                f"{fmt_s(median(after))} s",
                f"{saves_before} → {saves_after}",
            ]
        )
        per_rep.append(f"- {label} — before: {fmt_readings(before)}; after: {fmt_readings(after)}")
    table = md_table(
        ["blocks", "dump", "final cache", "before", "after", "saves before → after"], rows
    )
    note = (
        f"\n\n\"Before\" is `{BEFORE_COMMIT}`, the commit preceding `SaveThrottle`; it is a "
        "whole-commit comparison, not a throttle-isolating one. Save counts are `strace -f -e "
        "trace=open,openat` on the host, untimed.\n"
    )
    return table + note + "\nPer-rep readings (s):\n" + "\n".join(per_rep) + "\n"


def _fmt_bytes(n: int) -> str:
    if n >= MIB:
        return f"{n / MIB:.1f} MB"
    return f"{n / 1024:.0f} KB"


# -- the map's own quadratic ------------------------------------------------


def run_map_only(session: Session) -> str:
    figure = "map-only"
    counts = (1000, 2000, 4000)
    specs = [
        RunSpec("pgdq", f"blocks{n}", "query-nomatch", "warm", f"map only blocks{n}")
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
    preamble = RunSpec("pgdq", "blocks4000", "parse-preamble", "warm", "preamble-only blocks4000")
    session.sweep(figure, [preamble], session.cfg.reps(5))
    note = share_readings(session, figure)
    full_spec = RunSpec("pgdq", "blocks4000", "parse-cache-out", "warm", "full parse blocks4000")
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
    ("parse", "`pgdq parse` — structure discovery", "census-brace-free"),
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
        (RunSpec("pgdq", "control", command, "warm", ""),),
    )
    for command, _, source in _ALLOCATOR_SHAPES
)


def _allocator_reference(cfg: Config) -> str:
    """The allocator the *shipped* binary links against, read out of it.

    This figure's reference column is `target/release/pgdq` itself rather than
    a fourth build of the same source, for two reasons. It is the binary every
    other figure in the doc was taken with, so the ratios are ratios against
    the published numbers instead of against a build nothing else uses -- and
    two builds of one source can differ by ~10% from code layout alone
    (`measurements.md`, "Two builds of one source can differ by layout"), which
    is larger than the effect being measured. And it keeps the doc carrying
    **one** number per measurement: the reference readings are borrowed from
    the figures that already take them, exactly as the census table's
    census-on column is shared with the warm throughput table.

    Reading the name off the binary rather than assuming `system` is what makes
    the figure survive its own answer: adopt a leg and this becomes the
    reference, with the other two measured against it and no code change."""
    if cfg.dry_run:
        return ALLOCATOR_LEGS[0]
    return binary_allocator(cfg.bin_pgdq)


def allocator_columns(reference: str) -> list[str]:
    """The table's columns: the shipped allocator first, then the rest in
    register order. The reference is the column the ratios are against, so it
    is the one a reader needs first."""
    return [reference] + [leg for leg in ALLOCATOR_LEGS if leg != reference]


def _allocator_specs(reference: str) -> list[RunSpec]:
    """Every leg of every shape, plus the co-measured floor.

    The reference leg runs as the plain `pgdq` binary; the others are built
    per leg. The floor is one row rather than three: `dd` links no allocator,
    so a per-leg floor would be three readings of one thing. It is here for
    the same reason every other warm table co-measures one -- a session's own
    drift is what a warm absolute is read against."""
    if reference not in ALLOCATOR_LEGS:
        raise ValueError(f"unknown allocator leg {reference!r}")
    specs = []
    for command, label, _ in _ALLOCATOR_SHAPES:
        for leg in allocator_columns(reference):
            binary = "pgdq" if leg == reference else f"alloc:{leg}"
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
            session.get(figure, RunSpec("pgdq", "control", command, "warm", ""))
        )
        for leg in columns:
            binary = "pgdq" if leg == reference else f"alloc:{leg}"
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
    return table + "\n\n" + provenance + note + "\n" + _per_rep(figure, session, specs)


def _fmt_ns(ns: float) -> str:
    return f"{ns / 1000:.2f} µs" if ns >= 1000 else f"{ns:.0f} ns"


def _per_rep(figure: str, session: Session, specs: Sequence[RunSpec]) -> str:
    lines = [
        f"- {spec.label}: {fmt_readings(session.get(figure, spec))}" for spec in specs
    ]
    return "Per-rep readings (s):\n" + "\n".join(lines) + "\n"


# --------------------------------------------------------------------------
# The register. Order is run order: a figure that shares a reading comes after
# the figure that takes it.
# --------------------------------------------------------------------------

FIGURES: list[Figure] = [
    Figure(
        id="census-brace-free",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="The census on brace-free rows costs 8% of a warm scan",
        stage="cold+warm",
        depends=(*MAP, *SCAN, *READ, *GEN_PERF),
        cold_inputs=("control",),
        warm_inputs=("control",),
        run=run_census_brace_free,
    ),
    Figure(
        id="census-arrays",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="The census on array-bearing rows more than triples a warm scan",
        stage="cold+warm",
        depends=(*MAP, *SCAN, *READ, *GEN_PERF),
        cold_inputs=("arrays",),
        warm_inputs=("arrays",),
        run=run_census_arrays,
    ),
    Figure(
        id="scan-throughput-cold",
        quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run cold, on the SSD",
        stage="cold",
        depends=(*SCAN, *MAP, *READ, *GEN_SHAPES),
        cold_inputs=("control", "large_object", "insert_run"),
        shares=(
            Shared(
                "census-brace-free",
                "the `COPY` row, which is the census table's census-on column for this regime",
                (RunSpec("pgdq", "control", "parse", "cold", ""),),
            ),
        ),
        run=run_scan_throughput_cold,
    ),
    Figure(
        id="scan-throughput-warm",
        quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run warm, on tmpfs",
        stage="warm",
        depends=(*SCAN, *MAP, *READ, *GEN_SHAPES),
        warm_inputs=("control", "large_object", "insert_run"),
        shares=(
            Shared(
                "census-brace-free",
                "the `COPY` row, which is the census table's census-on column for this regime",
                (RunSpec("pgdq", "control", "parse", "warm", ""),),
            ),
        ),
        run=run_scan_throughput_warm,
    ),
    # The third device class, and the only one that can price the I/O
    # defaults: on the HDD and the SATA SSD the device is the whole cost and
    # on tmpfs there is no device at all, so a readahead, `fadvise` or
    # chunk-size change has nowhere to show. It borrows nothing -- no census
    # figure is taken in this regime -- so its `COPY` row is its own reading
    # rather than a republished one.
    Figure(
        id="scan-throughput-nvme",
        quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
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
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
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
        quoted_by=(
            "docs/status/STATUS.md",
        ),
        section="A typed query over nested columns costs 13.2 µs a row more than a string one",
        stage="warm",
        depends=(*NESTED, *DECODE, *MAP, *READ, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control", "composite", "arrays"),
        run=run_nested_end_to_end,
    ),
    Figure(
        id="census-attribution",
        quoted_by=(
            "docs/status/STATUS.md",
        ),
        section="The untyped baseline is not file-independent (census attribution)",
        stage="warm",
        depends=(*MAP, *READ, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control", "arrays"),
        run=run_census_attribution,
    ),
    Figure(
        id="cross-file-floor",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="The cross-file subtraction bottoms out at about half a microsecond a row",
        stage="warm",
        depends=(*NESTED, *DECODE, *READ, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control", "control43"),
        shares=(
            #: Consumed, not republished: row 1 is `_per_row_diffs` over the
            #: nested sweep's own reps, which publishes a per-row difference
            #: rather than either of the readings it is taken from. So it
            #: orders the run and pulls the source in, and is not an edge of
            #: the sharing closure.
            Shared("nested-end-to-end", "row 1's per-rep differences"),
        ),
        run=run_cross_file_floor,
    ),
    Figure(
        id="per-block-quadratic",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="Per-block cache saving is quadratic in block count, and so is the map",
        stage="warm",
        depends=(*MAP_BUILD, *READ, *CACHE, *GEN_BLOCKS, *GEN_PERF),
        warm_inputs=tuple(name for name, _ in _QUADRATIC_ROWS),
        run=run_per_block_quadratic,
    ),
    Figure(
        id="map-only",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="Per-block cache saving is quadratic in block count, and so is the map (map alone)",
        stage="warm",
        depends=(*MAP_BUILD, *READ, *QUERY_CLI, *GEN_BLOCKS),
        warm_inputs=("blocks1000", "blocks2000", "blocks4000"),
        run=run_map_only,
    ),
    Figure(
        id="preamble-prepass",
        quoted_by=(
            "docs/status/STATUS.md",
        ),
        section="The preamble prepass is bounded by the schema, not by the dump",
        stage="warm",
        #: Its second row is `per-block-quadratic`'s 4000-block "after" reading,
        #: borrowed rather than re-measured (`requires`, below) -- so this
        #: figure inherits that one's staleness edges as well as its own, or a
        #: change to the map moves a row here that reads green.
        depends=(*PREAMBLE, *MAP_BUILD, *READ, *CACHE, *GEN_BLOCKS),
        warm_inputs=("blocks4000",),
        shares=(
            Shared(
                "per-block-quadratic",
                'the full-`parse` row, which is the quadratic table\'s 4000-block "after" column',
                (RunSpec("pgdq", "blocks4000", "parse-cache-out", "warm", ""),),
            ),
        ),
        run=run_preamble_prepass,
    ),
    # `quoted_by` carries `architecture.md` because the rejected viewing-builder
    # paragraph reads this table's view control as the floor its bound is
    # arithmetic on — a consumer nothing declared until 7.14 moved the control.
    Figure(
        id="nested-decode-micro",
        quoted_by=(
            "docs/design/architecture.md",
        ),
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
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="What a column costs: five projection widths over one file",
        stage="warm",
        depends=(*SCAN, *NESTED, *DECODE, *READ, *QUERY_CLI, *GEN_PERF),
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
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
        section="What a filter term costs, and how much of it is the walk to its field",
        stage="warm",
        depends=(*PREDICATE, *SCAN, *READ, *QUERY_CLI, *GEN_PERF),
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
        quoted_by=(
            "docs/design/architecture.md",
            "docs/status/STATUS.md",
        ),
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
            # Where the legs are declared and where the shipped default lives,
            # so a change to it changes what this figure is a figure of.
            # `src/alloc.rs` needs no line of its own: `QUERY_CLI` is the
            # directory.
            "pgdump_query-cli/Cargo.toml",
        ),
        warm_inputs=("control",),
        shares=ALLOCATOR_SHARES,
        run=run_allocator,
    ),
]

FIGURES_BY_ID = {f.id: f for f in FIGURES}

#: Instruments that are **built but whose figure has not been taken**.
#:
#: A sweep does not run these and the doc carries no table for them, which is
#: why they sit outside `ALL_FIGURES`: the marker reconciliation would
#: otherwise demand a section with no numbers under it, and `quoted_by` would
#: have to name consumers of a figure that does not exist yet. `--figure <id>`
#: still selects one, which is how the reading gets taken — and taking it moves
#: the entry into `FIGURES`, where the doc-side checks start applying.
#:
#: The distinction is worth a list rather than a comment because *built* and
#: *taken* fail differently. An instrument nobody built is work; an instrument
#: built and never run is a claim nobody checked, and it is invisible unless
#: something names it.
#:
#: **Empty is the healthy state, not a disused mechanism.** Both entries it
#: carried have left by the two exits the list has: `projection-widths` was
#: taken and moved into `FIGURES`, and `composite-isolated` — which isolated one
#: column by declaring it two ways over byte-identical rows — was deleted
#: unpublished, because `projection-widths` makes the same isolation a
#: subtraction between two adjacent rows of one table over one file.
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
        quoted_by=(
            "docs/status/STATUS.md",
        ),
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
# The borrow graph: what a re-take drags with it.
# --------------------------------------------------------------------------


def sharing_edges() -> dict[str, set[str]]:
    """The republication graph, undirected.

    Undirected because the defect is symmetric: `allocator` borrowing
    `census-brace-free`'s reading and the two throughput tables borrowing the
    same one put the same number in four tables, and re-taking *any* of them
    alone leaves the doc carrying two numbers for one measurement. Direction
    only says which figure measures it."""
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
    run function used to write by hand named its *direct* sources, and behind
    the allocator table that is two figures where the honest set is four --
    `census-brace-free` is itself borrowed by both throughput tables. Returned
    in register order, so the closure reads as a run order."""
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
    """The acknowledgement covering this commit for this figure, if any."""
    for ack in acks:
        if ack.commit == commit and (not ack.figures or figure_id in ack.figures):
            return ack
    return None


def excused_paths(
    figure_id: str,
    hits: Sequence[str],
    commits_by_path: dict[str, Sequence[str]],
    dirty: set[str],
    acks: Sequence[Acknowledged],
) -> list[str]:
    """Which of a figure's touched paths an acknowledgement accounts for.

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
        if commits and all(excuses(acks, c, figure_id) for c in commits):
            out.append(path)
    return out


def inert_excuses(
    figure_id: str,
    hits: Sequence[str],
    commits_by_path: dict[str, Sequence[str]],
    acks: Sequence[Acknowledged],
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
    reason and needs no commentary.
    """
    out = []
    for path in hits:
        excused, blocking = [], []
        for commit in commits_by_path.get(path) or ():
            (excused if excuses(acks, commit, figure_id) else blocking).append(commit)
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


def cmd_drift(first: str, second: str) -> int:
    fig = ALL_BY_ID["session-drift"]
    print(f"## {fig.section}\n")
    print(f"<!-- figure: {fig.id} — reproduce with `cd scripts && uv run measure.py "
          f"--drift <sweep> <sweep>` -->\n")
    print(drift_table(Path(first) / "raw.json", Path(second) / "raw.json"))
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
    declared section to carrying no figure marker. A section cannot be both."""

    id: str
    #: The doc heading it sits under, for `--list` and for the check's report.
    section: str
    #: Why the harness does not own it. Printed by `--list`.
    why: str


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
        ),
        Outside(
            "benches",
            "benches/decoders.rs per-type pairs, benches/whole_file.rs",
            "Tripwires, not figures: they quote no number in the doc, so there is no table to "
            "emit. That is a decision, not an oversight -- `cargo bench -p pgdump_query` runs "
            "them.",
        ),
    ]
}


def resolve_selection(ids: Iterable[str], alone: bool = False) -> list[Figure]:
    """Selected figures plus whatever they borrow from, in run order.

    `alone` takes exactly what was named, which is how a **deliberate** partial
    sitting is asked for: the borrowed rows are then measured by the figure
    itself and its table carries the harness's partial-sweep note. It is
    refused where a figure *consumes* another's readings without republishing
    them, since there is no local measurement for that figure to fall back on
    -- the reading it wants is the other figure's reps, not a run it could
    take."""
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


#: How a section says it is *outside* the register. The symmetry with
#: `MARKER_RE` is the point: a figure announces itself and so does a section
#: that is not one, so "is this table one of ours" is read off the doc rather
#: than off a prose sentence some paragraphs away (`measurements.md`, "The
#: apparatus").
OUTSIDE_RE = re.compile(r"<!--\s*outside-register:\s*([a-z0-9-]+)")

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


def stamped_commit(doc: Path) -> str | None:
    """The commit the doc's session stamp names, so `--stale` has a default."""
    match = STAMP_RE.search(doc.read_text())
    return match.group(1) if match else None


def figures_touched(changed: Iterable[str]) -> list[tuple[Figure, list[str]]]:
    """Figures whose declared paths a diff touches. A declared path is a
    prefix: a directory matches everything under it.

    **Every figure, not just the ones a sweep runs.** The derived
    `session-drift` declares `scripts/measure.py` because the harness's own
    timing path is the apparatus it measures; iterating `FIGURES` here left
    that declaration inert, which is the failure mode a declared dependency
    exists to prevent.
    """
    changed = list(changed)
    out = []
    for fig in ALL_BY_ID.values():
        hits = [c for c in changed for d in fig.depends if c == d or c.startswith(d)]
        if hits:
            out.append((fig, sorted(set(hits))))
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


def session_stamp(head: str, dirty: bool, allocator: str | None = None) -> str:
    """The line the doc carries, and the line `stamped_commit` reads back.

    The allocator is part of it because a figure here is a **CLI** figure,
    taken under whatever `pgdq` links against -- see `measurements.md`, "Which
    allocator a figure was taken under". It is read out of the binary rather
    than assumed, so the day the CLI's default changes the stamp changes with
    it; `None` where there is no binary to ask, which is `--dry-run` and the
    unit tests.

    **It is scoped to the register, not to everything printed below it.** The
    doc also carries sections this harness does not own, and the stamp's claim
    was false over them for as long as it said "every figure below"; the scope
    is what a section's `<!-- outside-register: <id> -->` marker declares it
    out of, and `--check` holds the two in step."""
    suffix = " (with uncommitted changes under a measured path)" if dirty else ""
    alloc = f", under the `{allocator}` allocator" if allocator else ""
    return (
        f"**Session stamp.** Every figure below — every section carrying a "
        f"`<!-- figure: … -->` marker, and no other — was taken by `scripts/measure.py` on "
        f"{date.today().isoformat()}, against commit `{head}`{suffix}{alloc}."
    )


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
    allocator = None if cfg.dry_run else binary_allocator(cfg.bin_pgdq)
    log(
        f"measure.py — {len(figures)} figure(s), commit {head}{' (dirty)' if dirty else ''}"
        + (f", allocator {allocator}" if allocator else "")
    )
    log(f"output: {out_root}")
    if not cfg.publishable:
        log(
            "!! apparatus overridden (size or reps): this run is a smoke test, and its tables "
            "must not be folded into measurements.md"
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
    log(
        f"tmpfs budget: {stager.budget() / GIB:.2f} GiB "
        f"(largest figure {max(stager.figure_need.values(), default=0) / GIB:.2f} GiB "
        f"+ {WARM_MARGIN - 1:.0%} margin)" if cfg.warm_budget is None
        else f"tmpfs budget: {stager.budget() / GIB:.2f} GiB (set explicitly)"
    )
    if problems:
        for problem in problems:
            log(f"!! {problem}")
        log("nothing was run: every one of these is knowable before the first measurement.")
        log_file.close()
        return 2
    session = Session(cfg, stager, log)
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

    parts: list[str] = []
    sections_seen: set[str] = set()
    failures: list[tuple[str, str]] = []
    for i, fig in enumerate(figures):
        session.figure_index = i
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
        first_record = len(session.records)
        started = time.time()
        try:
            for name in fig.cold_inputs:
                stager.ensure_generated(name)
            for name in fig.nvme_inputs:
                stager.nvme_path(name)
            for name in fig.warm_inputs:
                stager.warm_path(name, i)
            body = fig.run(session)
        except Exception as exc:  # one figure failing must not lose the others
            log(f"!! {fig.id} failed: {exc}")
            failures.append((fig.id, str(exc)))
            continue
        took = time.time() - started
        apparatus = apparatus_note(session.records[first_record:])
        if apparatus:
            log("    " + apparatus.strip())
        log(f"--- {fig.id} done in {took:.0f} s")
        consumers = (
            "\n**The fold-in must also re-read**, because these repeat this figure's numbers "
            "or the claim it licenses: " + ", ".join(f"`{q}`" for q in fig.quoted_by) + ".\n"
            if fig.quoted_by
            else ""
        )
        heading = "" if fig.section in sections_seen else f"## {fig.section}\n\n"
        sections_seen.add(fig.section)
        label = f"**{fig.table_label}**\n\n" if fig.table_label else ""
        parts.append(
            f"{heading}"
            f"<!-- figure: {fig.id} — reproduce with `cd scripts && "
            f"uv run measure.py --figure {fig.id}` -->\n\n{label}{body}\n{apparatus}{consumers}"
        )

    stager.cleanup()
    if not cfg.dry_run:
        session.sampler.stop()
    if governor is not None:
        governor.__exit__()

    header = [
        "# measure.py output",
        "",
        session_stamp(head, dirty, allocator),
        "",
        "Each section below is one figure, ready to paste under its heading in "
        "`docs/design/measurements.md`.",
        "",
    ]
    if not cfg.publishable:
        header += [
            "> **NOT PUBLISHABLE.** This run overrode the recorded apparatus "
            f"(inputs {cfg.size_gib} GiB, reps {cfg.reps_override or 'as declared'}). "
            "It proves the harness runs; it is not a figure.",
            "",
        ]
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
                "readings": session.readings,
                "telemetry": session.telemetry,
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
    return 1 if failures else 0


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
        print(f"  {'':<24}  quoted by: {', '.join(fig.quoted_by) or '(nothing else)'}")
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
        print(f"  {outside.id:<24} {outside.section}\n  {'':<24}  {outside.why}\n")


KOJI_DUMP = _env("PGDQ_KOJI_DUMP", "/mnt/wd12t/fedora/koji/koji-2026-07-23.dump")


def koji_recipe(cfg: Config, name: str, wrap: bool) -> str:
    """The koji invocation, printed rather than run.

    koji is deliberately outside the sweep — a different medium, ~54 minutes,
    and a byte-for-byte regression check rather than a throughput figure — but
    the *recipe* was living in three hand-maintained copies, which is how a
    documented command was found that no longer ran. This is the one copy.

    Three things here have each cost a run, and a test asserts all three:

    * `exec`, so `pgdq` is PID 1 and `nerdctl stop` reaches the interrupt guard.
      A compound command cannot be `exec`'d, which is why nothing is appended to
      report the exit status — `nerdctl inspect` reports it either way, for a
      run that finished *or* was signalled.
    * the cgroup limit, which is part of the apparatus.
    * `--dqcache` under the mounted `/out`. The dump is read-only, so the
      colocated default lands in the container's ephemeral layer and is
      destroyed with it — an hour of scanning thrown away with no error, since
      the write itself succeeds.
    """
    mounts = (
        f'  -v "{cfg.bin_pgdq}:/pgdq:ro" \\\n'
        f'  -v "{cfg.out_dir}:/out" \\\n'
        f'  -v "{KOJI_DUMP}:/dump.sql:ro" \\\n'
    )
    def leg(container: str, cache: str, log: str) -> str:
        return (
            f"sudo nerdctl run -d --name {container} "
            f"-m {cfg.memory} --memory-swap {cfg.memory} \\\n"
            + mounts
            + f"  {cfg.image} \\\n"
            f"  sh -c 'exec /pgdq parse --source /dump.sql --dqcache /out/{cache} "
            f">> /out/{log} 2>&1'"
        )

    out = ["cargo build --release -p pgdump_query-cli   # default target: glibc", "mkdir -p runs", ""]
    if not wrap:
        out += [
            leg(name, f"{name}.dqcache", f"{name}-scan.log"),
            "",
            f"# still going?   sudo nerdctl inspect -f '{{{{.State.Status}}}}' {name}",
            f"# exit status:   sudo nerdctl inspect -f '{{{{.State.ExitCode}}}}' {name}",
            "#   130 = SIGINT, which is what `nerdctl stop` sends: the postgres images set",
            "#   STOPSIGNAL SIGINT, and --stop-signal on `run` is accepted and then ignored.",
            "#   For the SIGTERM arm: sudo nerdctl kill -s SIGTERM " + name + "  (exit 143)",
            f"# wall clock:    sudo nerdctl inspect -f "
            "'{{.State.StartedAt}} {{.State.FinishedAt}}' " + name,
            f"# the log:       runs/{name}-scan.log",
        ]
    else:
        out += [
            "# leg 1 — cold, interrupted partway.",
            leg(f"{name}-wrap1", f"{name}-wrap.dqcache", f"{name}-wrap-scan.log"),
            f"sleep 1200 && sudo nerdctl stop -t 120 {name}-wrap1",
            f"sudo nerdctl inspect -f '{{{{.State.ExitCode}}}}' {name}-wrap1   # 130 (SIGINT)",
            "",
            "# the interrupted cache must come back typed — both counts zero",
            f"sudo nerdctl run --rm -m {cfg.memory} --memory-swap {cfg.memory} \\",
            # keep the trailing line-continuation: the mounts run straight on
            # into the image name below.
            mounts.rstrip("\n"),
            f"  {cfg.image} /pgdq info --dqcache /out/{name}-wrap.dqcache --verbose \\",
            "  | grep -c 'not declared\\|metadata not scanned'",
            "",
            "# leg 2 — resume the identical command, then compare to a full run's cache",
            f"sudo nerdctl rm -f {name}-wrap1",
            leg(f"{name}-wrap2", f"{name}-wrap.dqcache", f"{name}-wrap-scan.log"),
            f"cmp runs/{name}-wrap.dqcache runs/<a previous full run>.dqcache",
        ]
    return "\n".join(out)


def cmd_koji(wrap: bool) -> int:
    cfg = Config()
    print(
        "# koji is not part of the sweep: a different medium, ~54 minutes, and a\n"
        "# byte-for-byte regression check rather than a throughput figure. Run this\n"
        "# detached and read it in a later session (CLAUDE.md, \"Long-running processes\").\n"
    )
    print(koji_recipe(cfg, "pgdq-koji", wrap))
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
PERF = _env("PGDQ_PROFILE_PERF", "perf")

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
DEBUGINFOD = _env("PGDQ_PROFILE_DEBUGINFOD", "https://debuginfod.archlinux.org")

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

#: The three command shapes profiled, in the order the baseline table reads
#: them, and the sweep figure each is read against. `parse` is discovery;
#: `query-strings` is row extraction before a column is typed; `query-typed`
#: is the whole path.
PROFILE_SHAPES: tuple[str, ...] = ("parse", "query-strings", "query-typed")

#: The two inputs. The brace-free control is the shape most dumps have; the
#: `--arrays --composite` file is where the nested path is reached at all.
PROFILE_INPUTS: tuple[str, ...] = ("control", "arrays")


def profile_argv(command: str, source: Path | str, cache: Path | str) -> list[str]:
    """The `pgdq` arguments one profiled shape runs.

    Deliberately the same flags `_script` hands the sweep, because a profile is
    only readable against the figure it explains -- and a profile of a shape no
    figure times answers a question nobody asked. The two are separate
    functions because `_script` builds a *container* command line with a `time`
    builtin in front of it and this one builds a host argv;
    `test_measure.py`'s `ProfileRecipe` reconciles them shape by shape, so a
    change to a measured invocation that misses this one fails there rather
    than being discovered in a profile that quietly measured something else."""
    if command == "parse":
        return ["parse", "--source", str(source), "--dqcache", str(cache)]
    if command in ("query-strings", "query-typed"):
        mode = command.split("-")[1]
        return [
            "query",
            "--source", str(source),
            "--table", "public.perf",
            "--dqcache", "none",
            "--schema-mode", mode,
        ]
    raise ValueError(f"unknown profile shape {command!r}")


def profile_recipe(cfg: Config) -> str:
    """The whole sequence, with every path filled in.

    Five things here decide whether the profile is of the thing it claims to
    be, and each fails *silently* -- a profile comes back, it just describes
    something else. `test_measure.py` asserts all five:

    * **the `profiling` binary, never `target/release/pgdq`.** `release`
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

    And one thing that is not a mistake but reads like one: **no container.**
    A profile is about proportions, and the cgroup adds capability plumbing
    without changing them."""
    warm = cfg.warm_dir
    binary = REPO / "target/profiling/pgdq"
    cache = warm / "profile.dqcache"
    out = cfg.out_dir

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
        "  cargo build --profile profiling -p pgdump_query-cli",
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

    head("Stage the inputs warm, on the host.")
    lines.append(f"mkdir -p {warm} {out}")
    for name in PROFILE_INPUTS:
        lines.append(f"cp -n {cfg.cache_dir / f'{name}.sql'} {warm / f'{name}.sql'}")
    lines.append("")
    head("The profiles. Each is a runs/ artifact, not a figure.")
    for name in PROFILE_INPUTS:
        for shape in PROFILE_SHAPES:
            stem = f"profile-{shape}-{name}"
            argv = " ".join(profile_argv(shape, warm / f"{name}.sql", cache))
            lines += [
                "",
                f"rm -f {cache}",
                f"{PERF} record -F {PERF_FREQ} --call-graph fp "
                f"-o {out / (stem + '.data')} \\",
                f"  -- {binary} {argv} >/dev/null",
                f"{PERF} report -i {out / (stem + '.data')} --stdio --no-children \\",
                f"  --percent-limit 0.5 > {out / (stem + '.txt')}",
            ]
    lines.append("")
    head("Tear down: tmpfs is 16 G and six inputs do not fit beside a sweep's.")
    lines.append(
        f"rm -f {' '.join(str(warm / f'{n}.sql') for n in PROFILE_INPUTS)} {cache}"
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


def cmd_check(doc: Path) -> int:
    """Reconcile the register against the doc: which figures have landed a
    marker, which markers name nothing, where the register's boundary runs, and
    which documents a fold-in must re-read because they repeat a figure's
    numbers."""
    text = doc.read_text()
    found = markers_in(doc)
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
    if outside:
        print(
            "Declared outside the register — sections whose readings this harness did not\n"
            "take, and which the session stamp above therefore does not cover:"
        )
        for oid, _ in outside:
            print(f"  {oid:<24} {NOT_OURS[oid].section if oid in NOT_OURS else '(unknown)'}")
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
    acks = resolved_acknowledgements()
    stamp = stamped_commit(doc)
    unknown_ack, spent_ack = acknowledgement_problems(
        acks,
        ALL_BY_ID,
        [a.commit for a in acks if stamp and is_ancestor(a.commit, stamp)],
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
        print("Spent acknowledgements — the stamp has moved past them; delete these entries:")
        for line in spent_ack:
            print(f"  {line}")
        print()
    print("What else a moved figure invalidates:")
    for fig in ALL_FIGURES:
        print(f"  {fig.id}")
        for q in fig.quoted_by:
            print(f"      {q}")
    return (
        1
        if (
            unknown
            or duplicated
            or dangling
            or undeclared
            or unknown_outside
            or both
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
    one session stamp's range; once the doc is re-stamped past that commit, no
    `--stale` range can reach it again and the entry excuses nothing. Left
    standing, the register accumulates permanent excuses whose diffs nobody
    will ever re-read — which is how a mechanism that exists to keep a signal
    honest turns into the thing dulling it.
    """
    known = set(known)
    spent = set(spent)
    unknown = sorted(
        f"{ack.commit[:7]} names {fid}" for ack in acks for fid in ack.figures if fid not in known
    )
    return unknown, sorted(ack.commit[:7] for ack in acks if ack.commit in spent)


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
    """
    doc = REPO / "docs/design/measurements.md"
    rev = since or stamped_commit(doc)
    if not rev:
        print(
            "no --since given and measurements.md carries no session stamp naming a commit; "
            "pass --since <rev>",
            file=sys.stderr,
        )
        return 2

    changed = set(changed_paths(rev))
    generators = sorted(
        {spec.generator for spec in INPUTS.values() if f"scripts/{spec.generator}" in changed}
    )
    if not generators:
        print(f"no generator any figure depends on changed since {rev}.")
        return 0

    cfg = dataclasses.replace(Config(), size_gib=VERIFY_SIZE_GIB)
    # Only the inputs a *published* figure is taken on. An input that exists
    # solely for an untaken instrument has no bytes in the doc to be wrong
    # about, and -- as `composite_text` proved on this mechanism's first run --
    # it may not be generatable at the old revision at all, because the commit
    # under test is what added its flag. A published figure's inputs cannot be
    # new that way: the figure was taken at the stamp, so they existed then.
    published = {name for fig in ALL_FIGURES for name in (*fig.cold_inputs, *fig.warm_inputs)}
    specs = sorted(
        {
            (spec.name, spec)
            for name, spec in INPUTS.items()
            if spec.generator in generators and name in published
        },
        key=lambda pair: pair[0],
    )
    if not specs:
        print(
            "the changed generator(s) feed no published figure's inputs, so nothing the doc "
            "carries was taken on them."
        )
        return 0
    print(f"{len(generators)} generator(s) changed since {rev}: {', '.join(generators)}")
    print(f"regenerating {len(specs)} input(s) at {VERIFY_SIZE_GIB} GiB under both revisions\n")

    # Under `runs/`, like the pre-throttle build's worktree: gitignored, so a
    # crashed run leaves no untracked tree inside the repo being measured.
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", rev)
    work = cfg.out_dir / f"worktree-verify-{safe}"
    run(["git", "worktree", "add", "--detach", str(work), rev], cwd=REPO, quiet=True)
    identical: list[str] = []
    differing: list[str] = []
    try:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            for name, spec in specs:
                old_out = tmp_path / f"{name}.old"
                new_out = tmp_path / f"{name}.new"
                run(spec.argv(cfg, old_out), cwd=work / "scripts", quiet=True)
                run(spec.argv(cfg, new_out), cwd=SCRIPTS, quiet=True)
                old, new = old_out.read_bytes(), new_out.read_bytes()
                if old == new:
                    identical.append(name)
                    print(f"  {name:<16} identical ({len(new)} bytes)")
                else:
                    differing.append(name)
                    at = next(
                        (i for i, (a, b) in enumerate(zip(old, new)) if a != b),
                        min(len(old), len(new)),
                    )
                    print(f"  {name:<16} DIFFERS at byte {at} ({len(old)} vs {len(new)} bytes)")
                old_out.unlink()
                new_out.unlink()
    finally:
        run(["git", "worktree", "remove", "--force", str(work)], cwd=REPO, quiet=True)

    if differing:
        print(
            f"\n{len(differing)} input(s) changed. Those figures are genuinely stale and need a "
            "sweep; do not acknowledge them."
        )
        return 1

    covered = sorted(
        {
            fig.id
            for fig in ALL_BY_ID.values()
            for dep in fig.depends
            if dep in {f"scripts/{g}" for g in generators}
        }
    )
    print(
        f"\nEvery input is byte-identical, so no figure was taken on different bytes. "
        f"The {len(covered)} figure(s) this accounts for:\n"
    )
    for fid in covered:
        print(f"  {fid}")
    print(
        "\nThat is the evidence an `Acknowledged` entry carries — record the commit that changed "
        "the generator, these figure ids, and this command as its `verified`."
    )
    return 0


def cmd_stale(since: str | None) -> int:
    doc = REPO / "docs/design/measurements.md"
    rev = since or stamped_commit(doc)
    if not rev:
        print(
            "no --since given and measurements.md carries no session stamp naming a commit; "
            "pass --since <rev>",
            file=sys.stderr,
        )
        return 2
    changed = changed_paths(rev)
    print(f"{len(changed)} path(s) changed since {rev}\n")
    touched = figures_touched(changed)
    if not touched:
        print("no figure's declared paths were touched.")
        return 0

    acks = resolved_acknowledgements()
    dirty = dirty_paths()
    by_path = commits_touching({p for _, hits in touched for p in hits}, rev)

    stale: list[tuple[Figure, list[str]]] = []
    excused_by: dict[str, list[str]] = {}
    for fig, hits in touched:
        ok = excused_paths(fig.id, hits, by_path, dirty, acks)
        left = [h for h in hits if h not in ok]
        if left:
            stale.append((fig, left))
        for path in ok:
            for commit in by_path[path]:
                excused_by.setdefault(commit, []).append(fig.id)

    if excused_by:
        print("Acknowledged — the commit is recorded as moving no reading:\n")
        for commit, ids in excused_by.items():
            ack = next(a for a in acks if a.commit == commit)
            short = commit[:7]
            print(f"  {short}  {ack.why}")
            print(f"           verified: {ack.verified or '(read by hand; nothing re-checks it)'}")
            print(f"           excuses: {', '.join(sorted(set(ids)))}")
        print()

    if not stale:
        print("no figure is stale: every touched path is accounted for.")
        return 0

    for fig, hits in stale:
        print(f"  {fig.id:<24} stale — {', '.join(hits)}")
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
        return 1
    print(
        "\nA stale figure must be re-taken with the whole doc: one sweep replaces every table "
        "(`uv run measure.py --all`), because the doc differences across tables."
    )
    return 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--list", action="store_true", help="name every figure and what invalidates it")
    parser.add_argument("--figure", action="append", default=[], help="figure id (repeatable, or comma-separated)")
    parser.add_argument(
        "--stage",
        choices=["cold", "cold-nvme", "warm", "criterion"],
        help="every figure of one stage",
    )
    parser.add_argument(
        "--alone",
        action="store_true",
        help="take exactly the figures named, borrowing nothing — a deliberate partial "
        "sitting, whose tables carry the harness's partial-sweep note",
    )
    parser.add_argument("--all", action="store_true", help="the whole sweep — what the doc's session stamp means")
    parser.add_argument("--stale", action="store_true", help="say which figures a diff has invalidated")
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
        help="two runs/measure-* directories: emit the session-drift table across them",
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
        "--profile-recipe",
        action="store_true",
        help="print the sampling-profile sequence — the harness owns it but never runs it",
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
    args = parser.parse_args(argv)

    if args.list:
        cmd_list()
        return 0
    if args.drift:
        return cmd_drift(*args.drift)
    if args.koji_recipe:
        return cmd_koji(args.wrap)
    if args.profile_recipe:
        return cmd_profile()
    if args.check:
        return cmd_check(REPO / "docs/design/measurements.md")
    if args.stale:
        return cmd_stale(args.since)
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
        pin_governor=args.pin_governor or Config().pin_governor,
    )
    figures = resolve_selection(ids, alone=args.alone)
    if not cfg.dry_run and not cfg.bin_pgdq.exists():
        parser.error(f"{cfg.bin_pgdq} is missing — `cargo build --release -p pgdump_query-cli`")
    needs_nocensus = any(f.id.startswith("census") for f in figures)
    if not cfg.dry_run and needs_nocensus:
        # Existence and age in one refusal, in the first second and before the
        # run directory exists: a sweep that starts on a stale census-off
        # binary loses its census tables an hour later, and worse, may not
        # look like it lost anything.
        problem = census_binary_problem(cfg)
        if problem:
            parser.error(problem)
    return emit(cfg, figures)


if __name__ == "__main__":
    raise SystemExit(main())
