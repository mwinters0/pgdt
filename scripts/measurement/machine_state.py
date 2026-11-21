"""The apparatus: the machine state the recorded figures assume -- the staging regimes, the CPU
arms a leg is pinned under, the governor pin, and the contention a reading is refused over."""

from __future__ import annotations

import atexit
import shlex
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence

import release

from measurement import host


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


def l3_groups(root: Path = release.CPU_SYSFS) -> list[frozenset[int]]:
    """The machine's L3 groups, each the CPUs sharing one, by lowest CPU."""
    groups = {
        release.parse_cpu_list(path.read_text())
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
            f"has {len(groups)} ({'; '.join(release.cpu_list(g) for g in groups) or 'none readable'})"
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
    gives the same answer for every row, so the field looks redundant. It is not: the
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
#: the wrong device. `test_machine_state.py` reconciles these names against the
#: regimes the registered figures declare and against `CONTENTION_LIMITS`,
#: both ways, so a new regime cannot land half-declared.
REGIMES: dict[str, Regime] = {
    "cold": Regime(area="cold", drops_caches=True),
    "cold-nvme": Regime(area="nvme", drops_caches=True),
    "warm": Regime(area="warm", drops_caches=False),
    #: The same tmpfs staging as `warm`; what differs is the contention gate
    #: below, the reading deliberately occupying every hardware thread.
    "warm-parallel": Regime(area="warm", drops_caches=False),
    #: The same NVMe staging and cache drop as `cold-nvme`; what differs is the
    #: contention gate below, the reading occupying several workers by design.
    #: Admitted by the regime test in `measurements.md`, "Scan throughput by
    #: input shape", for the provider's typed plain scan alone.
    "cold-nvme-parallel": Regime(area="nvme", drops_caches=True),
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
#: mechanically rather than by discipline -- `test_machine_state.py` reconciles these
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
#:
#: **`cold-nvme-parallel` gates as `warm-parallel` does, on steal alone**, for
#: the same reason: its legs run the provider at several partitions, so its own
#: workers are what `cpu_busy_pct` and `psi_cpu_some_pct` would see, and
#: `cold-nvme`'s 15% would discard a four-partition rep for being the reading.
#: Its witness is the same too: each cell is read against its leg's
#: one-partition row of the same sitting.
CONTENTION_LIMITS: dict[str, dict[str, float]] = {
    "cold": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "cold-nvme": {"cpu_busy_pct": 15.0, "psi_cpu_some_pct": 5.0, "cpu_steal_pct": 2.0},
    "cold-nvme-parallel": {"cpu_steal_pct": 2.0},
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
        host.run(shlex.split(self.sudo) + ["sh", "-c", script])

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
