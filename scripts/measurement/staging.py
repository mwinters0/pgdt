"""Staging inputs where a regime reads them: generated once onto the SSD, copied into tmpfs for a
warm figure under a reserved bound, or onto the NVMe for a cold-NVMe one (`Stager`)."""

from __future__ import annotations

import dataclasses
import json
import os
import shutil
from pathlib import Path
from typing import Callable, Sequence
from typing import TYPE_CHECKING

import contained

from measurement import config
from measurement import host
from measurement import inputs
from measurement import machine_state
from measurement import shapes
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.config import Config
    from measurement.figures.declaration import Figure
    from measurement.runspec import RunSpec


#: How many full-size inputs a sweep ever holds on tmpfs at once: the two one
#: paired cross-file difference needs. A figure whose warm set is larger is
#: split into sweeps of its own (`Figure.warm_groups`); the room is never grown
#: to hold it (`docs/design/measurements.md`, "The apparatus").
WARM_FULL_INPUTS = 2

#: Over those inputs' nominal size: the kilobytes a generator overshoots its
#: target by, and nothing more.
WARM_SLACK = 64 * config.MIB


def warm_bound(cfg: Config) -> int:
    """The tmpfs budget, by design: `WARM_FULL_INPUTS` full-size inputs plus
    `WARM_SLACK`. A constant of the apparatus rather than a sum over the
    figures selected, so a figure that outgrows it fails `test_staging.py`
    instead of growing the room every sweep needs."""
    return int(WARM_FULL_INPUTS * cfg.size_gib * config.GIB) + WARM_SLACK

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
        if machine_state.regime_spec(spec.regime).area != "warm" or not homes:
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
    groups = register.EVERY_BY_ID[figure].staging_groups
    homes = [gi for gi, g in enumerate(groups) if spec.input in g]
    if len(groups) < 2 or (homes and homes[0] == group):
        return dataclasses.replace(spec, sweep=None)
    if group not in homes:
        raise ValueError(f"{figure}'s sweep {group} does not stage {spec.input!r}")
    return dataclasses.replace(spec, sweep=group)


def nominal_size(cfg: Config, name: str) -> int:
    """What an input would weigh, for a dry run that has not generated it."""
    spec = inputs.INPUTS[name]
    if spec.nominal_bytes is not None:
        return spec.nominal_bytes
    if not spec.scales:
        return 2 * config.MIB
    return int(cfg.size_gib * config.GIB)


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
        return self.cfg.cache_dir / inputs.input_file(name)

    def ensure_generated(self, name: str) -> Path:
        spec = inputs.INPUTS[name]
        out = self.cfg.cache_dir / inputs.input_file(name)
        stamp = self.cfg.cache_dir / f"{name}.stamp"
        want = inputs.input_stamp(spec, self.cfg)
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
        host.run(spec.argv(self.cfg, out), cwd=config.SCRIPTS)
        stamp.write_text(want + "\n")
        # A regenerated input invalidates whatever was profiled or staged off
        # the old bytes.
        self._profiles.pop(name, None)
        self._staged.pop(name, None)
        warm = self.cfg.warm_dir / inputs.input_file(name)
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
        dst = self.cfg.nvme_dir / inputs.input_file(name)
        stamp = self.cfg.nvme_dir / f"{name}.stamp"
        want = inputs.input_stamp(inputs.INPUTS[name], self.cfg)
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
        for name in inputs.INPUTS:
            path = self.cfg.warm_dir / inputs.input_file(name)
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
        dst = self.cfg.warm_dir / inputs.input_file(name)
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
                    f"{need / config.GIB:.2f} GiB does not fit in a {budget / config.GIB:.2f} GiB "
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
                    f"{victim} and {need / config.GIB:.2f} GiB more"
                )
            self.log(f"  evicting {victim} from tmpfs")
            del self._staged[victim]
            if not self.cfg.dry_run:
                (self.cfg.warm_dir / inputs.input_file(victim)).unlink(missing_ok=True)

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
        path = self.cfg.cache_dir / inputs.input_file(name)
        if path.exists():
            return path.stat().st_size
        return nominal_size(self.cfg, name)

    def budget(self) -> int:
        """The tmpfs ceiling: `warm_bound`, or the explicit
        PGDT_MEASURE_TMPFS_BUDGET_GIB where that is lower."""
        bound = warm_bound(self.cfg)
        if self.cfg.warm_budget is not None:
            return min(bound, int(self.cfg.warm_budget * config.GIB))
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
                f"{self.cfg.warm_dir} could not reserve {want / config.GIB:.2f} GiB, what the "
                f"{budget / config.GIB:.2f} GiB tmpfs budget may still stage there: {exc}. A "
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
                        f"{which} needs {need / config.GIB:.2f} GiB of tmpfs at once, over the "
                        f"{budget / config.GIB:.2f} GiB budget"
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
            source = inputs.INPUTS[name].derives_from
            while source:
                wanted.add(source)
                source = inputs.INPUTS[source].derives_from
        missing = sum(
            self.expected_size(n)
            for n in wanted
            if not (self.cfg.cache_dir / inputs.input_file(n)).exists()
        )
        try:
            self.cfg.cache_dir.mkdir(parents=True, exist_ok=True)
            cache_free = shutil.disk_usage(self.cfg.cache_dir).free
        except OSError as exc:
            problems.append(f"{self.cfg.cache_dir} is not usable as an input cache: {exc}")
            return problems
        if missing > cache_free:
            problems.append(
                f"{self.cfg.cache_dir} has {cache_free / config.GIB:.2f} GiB free, under the "
                f"{missing / config.GIB:.2f} GiB of inputs still to generate"
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
            if not (self.cfg.nvme_dir / inputs.input_file(n)).exists()
        )
        if want > free:
            return [
                f"{self.cfg.nvme_dir} has {free / config.GIB:.2f} GiB free, under the "
                f"{want / config.GIB:.2f} GiB of inputs the cold-NVMe figures still need copied "
                "there. Point PGDT_MEASURE_NVME_DIR at an NVMe volume with room"
            ]
        return []

    def _next_need(self, name: str, step: int) -> float:
        return min((i for i in self.needs.get(name, []) if i >= step), default=float("inf"))

    def cleanup(self) -> None:
        if self.cfg.keep_warm or self.cfg.dry_run:
            return
        for name in list(self._staged):
            path = self.cfg.warm_dir / inputs.input_file(name)
            if path.exists():
                self.log(f"  removing {path}")
                path.unlink()
            del self._staged[name]
        # The caches a run writes into the staging directory go with it, and
        # so do the binaries a staged leg ran (`Session.stage_binary`).
        for leftover in self.cfg.warm_dir.glob("*.pgdt"):
            leftover.unlink(missing_ok=True)
        shutil.rmtree(self.cfg.warm_dir / STAGED_BIN_DIR, ignore_errors=True)

    # -- profiling --------------------------------------------------------

    def profile(self, name: str) -> dict:
        """Row and column counts for an input, off `pgdt info --json`.

        Not a figure: it runs untimed, once per input, and only the tables
        that quote a per-row cost need it — in a container under the sweep's
        limit, as every run of a built binary is (`contained.py`)."""
        if name in self._profiles:
            return self._profiles[name]
        cache = self.cfg.cache_dir / f"{name}.profile.json"
        stamp = inputs.input_stamp(inputs.INPUTS[name], self.cfg)
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
        self.log(f"  profiling {name} (untimed)")
        path = self.warm_path(name) if name in self._staged else self.cold_path(name)
        tmp = self.cfg.cache_dir / f"{name}.profile.pgdt"
        tmp.unlink(missing_ok=True)
        # `--jobs` even though nothing here is timed: the row counts this
        # returns are the divisor under every per-row number in the document,
        # and no invocation this harness makes inherits a CLI default that
        # moves underneath it (`SWEEP_JOBS`).
        def inside(argv: list[str]) -> list[str]:
            return contained.argv(
                argv, self.cfg.memory, container=self.cfg.container_argv()
            )

        host.run(
            inside([
                str(self.cfg.bin_pgdt), "parse",
                "--source", str(path),
                "--cache", str(tmp),
                "--jobs", str(shapes.SWEEP_JOBS),
                *shapes.NO_STATISTICS.split(),
            ]),
            quiet=True,
        )  # fmt: skip
        out = host.run(
            inside([str(self.cfg.bin_pgdt), "info", "--cache", str(tmp), "--json"]), capture=True
        )
        tmp.unlink(missing_ok=True)
        index = json.loads(out)
        rows, columns, data_bytes = 0, 0, 0
        for span in index["spans"]:
            copy = span.get("body", {}).get("Data", {}).get("Copy")
            if not copy:
                continue
            rows += copy["record"]["row_count"]
            columns = max(columns, len(copy["header"]["columns"]))
            address = copy["address"]
            data_bytes += address["terminator_offset"] - address["data_offset"]
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
