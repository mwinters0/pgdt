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
  `drop_caches` before *every* run, including before the floor;
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
  recipe is in measurements.md) and point `PGDQ_MEASURE_CENSUS_OFF_BIN` at it;
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
    uv run python -m unittest test_measure -v
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
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Callable, Iterable, Sequence

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
    cache_dir: Path = Path(_env("PGDQ_MEASURE_CACHE_DIR", "/mnt/ssd/fedora/pgdq-measure"))
    # tmpfs, for every warm figure.
    warm_dir: Path = Path(_env("PGDQ_MEASURE_WARM_DIR", "/dev/shm/pgdq"))
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

    # The size of the six large inputs. 3.00 GiB is the recorded apparatus;
    # anything else marks the run unpublishable.
    size_gib: float = float(_env("PGDQ_MEASURE_SIZE_GIB", "3.0"))
    # Rep-count override, for smoke runs only. None means each figure's own.
    reps_override: int | None = None
    dry_run: bool = False
    keep_warm: bool = False

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

    M15's lesson, applied to the harness's own inputs: a generated file that is
    only regenerated when *missing* means a checkout that already has one
    benchmarks pre-change bytes forever, silently -- and the population that
    has one is exactly the population comparing a new number to an old one."""
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
        wanted = {n for fig in figures for n in (*fig.cold_inputs, *fig.warm_inputs)}
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
        return problems

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
    regime: str  # "cold" | "warm"
    label: str

    def key(self, figure: str) -> str:
        return f"{figure}/{self.binary}/{self.input}/{self.command}/{self.regime}"


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
    if command == "query-nomatch":
        # Maps to EOF (the table never matches) and never saves.
        return f"{q} query --source /dump.sql --table public.nosuchtable --dqcache none >/dev/null"
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
        self._dry_reps: dict[str, int] = {}

    # -- one timed run ----------------------------------------------------

    def binary_path(self, which: str) -> Path:
        if which == "pgdq":
            return self.cfg.bin_pgdq
        if which == "nocensus":
            return self.cfg.bin_nocensus
        if which == "before":
            return ensure_before_binary(self.cfg, self.log)
        raise ValueError(f"unknown binary {which!r}")

    def input_path(self, name: str, regime: str) -> Path:
        if regime == "cold":
            return self.stager.cold_path(name)
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

        if spec.regime == "cold":
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
        started = time.time()
        proc = subprocess.run(argv, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if proc.returncode != 0:
            raise RuntimeError(
                f"{spec.label} exited {proc.returncode}\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}"
            )
        seconds = parse_bash_time(proc.stderr)
        self.records.append(
            {
                "spec": dataclasses.asdict(spec),
                "seconds": seconds,
                "wall_including_container": round(time.time() - started, 3),
                "argv": argv,
            }
        )
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
                seconds = self.time_run(spec)
                self.readings[spec.key(figure)].append(seconds)
                self.log(f"  rep{rep + 1} {spec.label}: {seconds:.3f} s")

    def get(self, figure: str, spec: RunSpec) -> list[float]:
        return self.readings[spec.key(figure)]

    def has(self, figure: str, spec: RunSpec) -> bool:
        return spec.key(figure) in self.readings

    def borrow(self, from_figure: str, spec: RunSpec) -> list[float] | None:
        """A reading another figure already took. The warm scan-throughput
        table's `COPY` row *is* the census table's warm census-on column --
        re-measuring it would put two different numbers in the doc for one
        measurement."""
        return self.readings.get(spec.key(from_figure))


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
    #: Figures whose readings this one uses, pulled in automatically.
    requires: tuple[str, ...] = ()
    #: Documents that repeat this figure's numbers, or the claim it licenses.
    #: `depends` is the edge into a figure -- what invalidates it; this is the
    #: edge out -- what a moved figure invalidates. Both exist for the same
    #: reason: "someone will notice" is not a mechanism, and the doc set has
    #: already drifted this way (`architecture.md` quotes 4003 -> 195 saves and
    #: a 19.7 s map where `measurements.md`'s table says 103 and 18.62).
    quoted_by: tuple[str, ...] = ()
    run: Callable[[Session], str] = field(default=lambda s: "")


#: The paths behind each mechanism a figure can depend on. Declared narrowly
#: on purpose: a blanket `pgdump_query/src/` would mark every figure stale on
#: every commit, which is a `--stale` nobody reads. The cost of narrowness is
#: that a change *outside* these paths that moves a figure goes unannounced —
#: which is why the doc carries a session stamp as well, so "are these figures
#: from before or after my change" has a second answer.
SCAN = ("pgdump_query/src/scan.rs", "pgdump_query/src/copy.rs", "pgdump_query/src/stream.rs")
MAP = ("pgdump_query/src/map.rs",)
CACHE = ("pgdump_query/src/cache.rs",)
NESTED = ("pgdump_query/src/nested.rs", "pgdump_query/src/batch.rs")
PREAMBLE = ("pgdump_query/src/index.rs", "pgdump_query/src/preamble.rs")
#: A query figure also reads through the CLI's own row rendering.
QUERY_CLI = ("pgdump_query-cli/src/main.rs",)

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
    borrowed = session.borrow(
        "census-brace-free", RunSpec("pgdq", "control", "parse", regime, "")
    )
    if borrowed:
        session.readings[specs[0].key(figure)] = list(borrowed)
        to_run = specs[1:]
        note = (
            "\nThe `COPY` row is the census table's census-on column for this regime — the same "
            "binary, command and input, not a second measurement of it.\n"
        )
    else:
        to_run = specs
        note = (
            "\n**Partial sweep**: `census-brace-free` was not emitted in this session, so the "
            "`COPY` row was measured here rather than shared with it. The two are the same "
            "measurement and must agree, so emit them together before folding either in.\n"
        )
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


# -- the census pair --------------------------------------------------------


def _census_figure(session: Session, figure: str, input_name: str) -> str:
    """The census table: two binaries, one input, both regimes.

    Cold and warm are two rows of one table, so the table is only taken whole:
    half a comparison table may not be re-taken."""
    rows = []
    per_rep = []
    for regime in ("cold", "warm"):
        off = RunSpec("nocensus", input_name, "parse", regime, f"census off ({regime})")
        on = RunSpec("pgdq", input_name, "parse", regime, f"census on ({regime})")
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


def run_per_block_quadratic(session: Session) -> str:
    figure = "per-block-quadratic"
    specs = []
    for n in _BLOCK_COUNTS:
        for binary in ("before", "pgdq"):
            specs.append(
                RunSpec(binary, f"blocks{n}", "parse-cache-out", "warm", f"{binary} blocks{n}")
            )
    session.sweep(figure, specs, session.cfg.reps(2))

    rows, per_rep = [], []
    for n in _BLOCK_COUNTS:
        dump = session.input_path(f"blocks{n}", "warm")
        before = session.get(figure, RunSpec("before", f"blocks{n}", "parse-cache-out", "warm", ""))
        after = session.get(figure, RunSpec("pgdq", f"blocks{n}", "parse-cache-out", "warm", ""))
        saves_before, _ = count_saves(session.cfg, session.binary_path("before"), dump, session.log)
        saves_after, cache_size = count_saves(
            session.cfg, session.binary_path("pgdq"), dump, session.log
        )
        rows.append(
            [
                str(n),
                _fmt_bytes(file_size(session.cfg, dump, f"blocks{n}")),
                _fmt_bytes(cache_size),
                f"{fmt_s(median(before))} s",
                f"{fmt_s(median(after))} s",
                f"{saves_before} → {saves_after}",
            ]
        )
        per_rep.append(f"- {n} blocks — before: {fmt_readings(before)}; after: {fmt_readings(after)}")
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
    full = session.borrow("per-block-quadratic", RunSpec("pgdq", "blocks4000", "parse-cache-out", "warm", ""))
    note = ""
    if full is None:
        full_spec = RunSpec("pgdq", "blocks4000", "parse-cache-out", "warm", "full parse blocks4000")
        session.sweep(figure, [full_spec], session.cfg.reps(2))
        full = session.get(figure, full_spec)
        note = (
            "\n**Partial sweep**: the full-`parse` row was measured here rather than shared with "
            "the quadratic table's 4000-block \"after\" column, which is the same measurement.\n"
        )
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
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="The census on brace-free rows costs 7% of a warm scan",
        stage="cold+warm",
        depends=(*MAP, *SCAN, *GEN_PERF),
        cold_inputs=("control",),
        warm_inputs=("control",),
        run=run_census_brace_free,
    ),
    Figure(
        id="census-arrays",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="The census on array-bearing rows nearly quadruples a warm scan",
        stage="cold+warm",
        depends=(*MAP, *SCAN, *GEN_PERF),
        cold_inputs=("arrays",),
        warm_inputs=("arrays",),
        run=run_census_arrays,
    ),
    Figure(
        id="scan-throughput-cold",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
            "docs/status/STATUS.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run cold, on the SSD",
        stage="cold",
        depends=(*SCAN, *MAP, *GEN_SHAPES),
        cold_inputs=("control", "large_object", "insert_run"),
        requires=("census-brace-free",),
        run=run_scan_throughput_cold,
    ),
    Figure(
        id="scan-throughput-warm",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
            "docs/status/STATUS.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run warm, on tmpfs",
        stage="warm",
        depends=(*SCAN, *MAP, *GEN_SHAPES),
        warm_inputs=("control", "large_object", "insert_run"),
        requires=("census-brace-free",),
        run=run_scan_throughput_warm,
    ),
    Figure(
        id="nested-end-to-end",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="A typed query over nested columns costs 14 µs a row more than a string one",
        stage="warm",
        depends=(*NESTED, *MAP, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control", "composite", "arrays"),
        run=run_nested_end_to_end,
    ),
    Figure(
        id="census-attribution",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="The untyped baseline is not file-independent (census attribution)",
        stage="warm",
        depends=(*MAP, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control", "arrays"),
        run=run_census_attribution,
    ),
    Figure(
        id="cross-file-floor",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/design/roadmap-P4-composite-decoding-notes.md",
            "docs/status/STATUS.md",
        ),
        section="The cross-file subtraction bottoms out at about half a microsecond a row",
        stage="warm",
        depends=(*NESTED, *QUERY_CLI, *GEN_PERF),
        warm_inputs=("control", "control43"),
        requires=("nested-end-to-end",),
        run=run_cross_file_floor,
    ),
    Figure(
        id="per-block-quadratic",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="Per-block cache saving is quadratic in block count, and so is the map",
        stage="warm",
        depends=(*MAP, *CACHE, *GEN_BLOCKS),
        warm_inputs=tuple(f"blocks{n}" for n in _BLOCK_COUNTS),
        run=run_per_block_quadratic,
    ),
    Figure(
        id="map-only",
        quoted_by=(
            "docs/design/architecture.md",
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="Per-block cache saving is quadratic in block count, and so is the map (map alone)",
        stage="warm",
        depends=(*MAP, *QUERY_CLI, *GEN_BLOCKS),
        warm_inputs=("blocks1000", "blocks2000", "blocks4000"),
        run=run_map_only,
    ),
    Figure(
        id="preamble-prepass",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
        section="The preamble prepass is bounded by the schema, not by the dump",
        stage="warm",
        depends=(*PREAMBLE, *GEN_BLOCKS),
        warm_inputs=("blocks4000",),
        requires=("per-block-quadratic",),
        run=run_preamble_prepass,
    ),
    Figure(
        id="nested-decode-micro",
        quoted_by=(
            "docs/design/roadmap-P7-scan-performance-inbox.md",
        ),
        section="Nested decode costs what it copies, and an element is an allocation",
        stage="criterion",
        depends=("pgdump_query/src/nested.rs", "pgdump_query/benches/decoders.rs"),
        run=run_nested_decode_micro,
    ),
]

FIGURES_BY_ID = {f.id: f for f in FIGURES}

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
            "docs/design/roadmap-P7-scan-performance-inbox.md",
            "docs/status/STATUS.md",
        ),
    )
]

ALL_FIGURES = FIGURES + DERIVED
ALL_BY_ID = {f.id: f for f in ALL_FIGURES}


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

#: Figures measurements.md carries that this harness deliberately does not own.
NOT_OURS = {
    "koji full scan": (
        "784 GB on the HDD, ~54 minutes, a different medium, and a byte-for-byte regression "
        "check rather than a throughput figure. The harness owns the *invocation* — "
        "`--koji-recipe`, which prints it — and never runs it."
    ),
    "benches/decoders.rs per-type pairs, benches/whole_file.rs": (
        "Tripwires, not figures: they quote no number in the doc, so there is no table to emit. "
        "That is a decision, not an oversight -- `cargo bench -p pgdump_query` runs them."
    ),
}


def resolve_selection(ids: Iterable[str]) -> list[Figure]:
    """Selected figures plus whatever they share a reading with, in run order."""
    wanted: set[str] = set()

    def add(fid: str) -> None:
        if fid in wanted:
            return
        if fid not in FIGURES_BY_ID:
            raise SystemExit(f"unknown figure {fid!r}; `--list` names them all")
        wanted.add(fid)
        for dep in FIGURES_BY_ID[fid].requires:
            add(dep)

    for fid in ids:
        add(fid)
    return [f for f in FIGURES if f.id in wanted]


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


def stamped_commit(doc: Path) -> str | None:
    """The commit the doc's session stamp names, so `--stale` has a default."""
    match = STAMP_RE.search(doc.read_text())
    return match.group(1) if match else None


def figures_touched(changed: Iterable[str]) -> list[tuple[Figure, list[str]]]:
    """Figures whose declared paths a diff touches. A declared path is a
    prefix: a directory matches everything under it."""
    changed = list(changed)
    out = []
    for fig in FIGURES:
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


def session_stamp(head: str, dirty: bool) -> str:
    suffix = " (with uncommitted changes under a measured path)" if dirty else ""
    return (
        f"**Session stamp.** Every figure below was taken by `scripts/measure.py` on "
        f"{date.today().isoformat()}, against commit `{head}`{suffix}."
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
    log(f"measure.py — {len(figures)} figure(s), commit {head}{' (dirty)' if dirty else ''}")
    log(f"output: {out_root}")
    if not cfg.publishable:
        log(
            "!! apparatus overridden (size or reps): this run is a smoke test, and its tables "
            "must not be folded into measurements.md"
        )

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
        started = time.time()
        try:
            for name in fig.cold_inputs:
                stager.ensure_generated(name)
            for name in fig.warm_inputs:
                stager.warm_path(name, i)
            body = fig.run(session)
        except Exception as exc:  # one figure failing must not lose the others
            log(f"!! {fig.id} failed: {exc}")
            failures.append((fig.id, str(exc)))
            continue
        took = time.time() - started
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
            f"uv run measure.py --figure {fig.id}` -->\n\n{label}{body}\n{consumers}"
        )

    stager.cleanup()

    header = [
        "# measure.py output",
        "",
        session_stamp(head, dirty),
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
            print(f"  {'':<24}  shares readings with: {', '.join(fig.requires)}")
        print(f"  {'':<24}  invalidated by: {', '.join(fig.depends)}")
        print(f"  {'':<24}  quoted by: {', '.join(fig.quoted_by) or '(nothing else)'}")
        reproduce = (
            "--drift <sweep> <sweep>" if fig.stage == "derived" else f"--figure {fig.id}"
        )
        print(f"  {'':<24}  reproduce: cd scripts && uv run measure.py {reproduce}")
        print()
    print("Not emitted here, deliberately:\n")
    for name, why in NOT_OURS.items():
        print(f"  {name}\n    {why}\n")


KOJI_DUMP = _env("PGDQ_KOJI_DUMP", "/mnt/wd12t/fedora/koji/koji-2026-07-23.dump")


def koji_recipe(cfg: Config, name: str, wrap: bool) -> str:
    """The koji invocation, printed rather than run.

    koji is deliberately outside the sweep — a different medium, ~54 minutes,
    and a byte-for-byte regression check rather than a throughput figure — but
    the *recipe* was living in three hand-maintained copies, which is how `M19`
    found a documented command that no longer ran. This is the one copy.

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


def cmd_check(doc: Path) -> int:
    """Reconcile the register against the doc: which figures have landed a
    marker, which markers name nothing, and which documents a fold-in must
    re-read because they repeat a figure's numbers."""
    found = markers_in(doc)
    unknown = [m for m in found if m not in ALL_BY_ID]
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
    print("What else a moved figure invalidates:")
    for fig in ALL_FIGURES:
        print(f"  {fig.id}")
        for q in fig.quoted_by:
            print(f"      {q}")
    return 1 if (unknown or duplicated) else 0


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
    for fig, hits in touched:
        print(f"  {fig.id:<24} stale — {', '.join(hits)}")
    print(
        "\nA stale figure must be re-taken with the whole doc: one sweep replaces every table "
        "(`uv run measure.py --all`), because the doc differences across tables."
    )
    return 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--list", action="store_true", help="name every figure and what invalidates it")
    parser.add_argument("--figure", action="append", default=[], help="figure id (repeatable, or comma-separated)")
    parser.add_argument("--stage", choices=["cold", "warm", "criterion"], help="every figure of one stage")
    parser.add_argument("--all", action="store_true", help="the whole sweep — what the doc's session stamp means")
    parser.add_argument("--stale", action="store_true", help="say which figures a diff has invalidated")
    parser.add_argument(
        "--check",
        action="store_true",
        help="reconcile the register against measurements.md's figure markers, and name the "
        "documents a moved figure invalidates",
    )
    parser.add_argument("--since", help="revision for --stale (default: the doc's session stamp)")
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
    parser.add_argument("--reps", type=int, help="override every figure's rep count (smoke runs only)")
    parser.add_argument("--dry-run", action="store_true", help="print what would run, measure nothing")
    parser.add_argument("--keep-warm", action="store_true", help="leave staged inputs on tmpfs")
    args = parser.parse_args(argv)

    if args.list:
        cmd_list()
        return 0
    if args.drift:
        return cmd_drift(*args.drift)
    if args.koji_recipe:
        return cmd_koji(args.wrap)
    if args.check:
        return cmd_check(REPO / "docs/design/measurements.md")
    if args.stale:
        return cmd_stale(args.since)

    ids: list[str] = []
    for item in args.figure:
        ids += [p for p in item.split(",") if p]
    if args.stage:
        ids += [f.id for f in FIGURES if args.stage in f.stage]
    if args.all:
        ids = [f.id for f in FIGURES]
    if not ids:
        parser.error("nothing selected: pass --figure, --stage, --all, --list or --stale")

    cfg = Config(reps_override=args.reps, dry_run=args.dry_run, keep_warm=args.keep_warm)
    figures = resolve_selection(ids)
    if not cfg.dry_run and not cfg.bin_pgdq.exists():
        parser.error(f"{cfg.bin_pgdq} is missing — `cargo build --release -p pgdump_query-cli`")
    needs_nocensus = any(f.id.startswith("census") for f in figures)
    if not cfg.dry_run and needs_nocensus and not cfg.bin_nocensus.exists():
        parser.error(
            f"{cfg.bin_nocensus} is missing. The census-off binary is a source patch no harness "
            "should perform: add a bare `return;` as the first statement of "
            "`map::Builder::on_row`, `cargo build --release -p pgdump_query-cli`, copy the binary "
            f"to {cfg.bin_nocensus}, then revert. measurements.md's census section has the recipe."
        )
    return emit(cfg, figures)


if __name__ == "__main__":
    raise SystemExit(main())
