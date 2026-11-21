"""Contention telemetry: what the machine was doing while a reading was taken -- pressure, CPU
time, frequency and temperature, sampled beside every timed run (`Sampler`)."""

from __future__ import annotations

import statistics
import threading
import time
from dataclasses import dataclass
from pathlib import Path


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
