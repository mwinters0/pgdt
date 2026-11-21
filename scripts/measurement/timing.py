"""The session: every timed command a sitting runs (`Session.time_run`), the sweeps that interleave
them, and the readings, resident sets, instruction counts and kills it keeps."""

from __future__ import annotations

import dataclasses
import hashlib
import os
import re
import shlex
import shutil
import subprocess
import time
from pathlib import Path
from typing import Callable, Sequence
from typing import TYPE_CHECKING

import release

from measurement import config
from measurement import contention
from measurement import formatting
from measurement import gating
from measurement import host
from measurement import inputs
from measurement import machine_state
from measurement import parsing
from measurement import shapes
from measurement import staging
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.config import Config
    from measurement.runspec import RunSpec, Shared
    from measurement.staging import Stager


def with_allowance(script: str, spec: RunSpec) -> str:
    """`script`, `spec`'s shape, with its `ALLOWANCE_SLOT` replaced by the
    spec's allowance. **Refused where the two disagree**: a shape leaving its
    allowance to a spec stating none would run with the slot as its argument,
    and a spec stating one over a shape with no slot would be keyed by an
    allowance nothing ran under."""
    if (shapes.ALLOWANCE_SLOT in script) != (spec.allowance is not None):
        raise ValueError(
            f"{spec.label or spec.command}: its shape "
            + ("leaves" if shapes.ALLOWANCE_SLOT in script else "leaves no")
            + f" allowance to the sitting, and the spec states {spec.allowance}"
        )
    return script if spec.allowance is None else script.replace(shapes.ALLOWANCE_SLOT, str(spec.allowance))

#: Where `Session.prebuild`'s container sees the directory its cache is
#: written to.
PREBUILT_MOUNT = "/out"

#: `Session.prebuild`'s whole script: `PARALLEL_SCAN`'s builder, stating what
#: it states, writing to the mounted directory under `{name}` rather than
#: beside the read-only dump. Untimed and run once a sitting, so it is not a
#: command shape; `figures/test_parallel.py` holds it to the warm builder's flags.
PREBUILT_BUILDER = (
    f"/pgdt parse --source /dump.sql --cache {PREBUILT_MOUNT}/{{name}} "
    f"--jobs {shapes.SWEEP_JOBS} {shapes.GATHER_STATISTICS} >/dev/null"
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
KILL_TOLERANT: tuple[str, ...] = (shapes.RESERVE_FLAGLESS,)


def kill_tolerant(command: str) -> bool:
    """Whether an OOM kill of this command shape is a reading rather than an
    apparatus failure (`KILL_TOLERANT`)."""
    return command.startswith(KILL_TOLERANT)


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
        #: User-space instructions retired, under the same keys, for a counted
        #: figure's legs (`Figure.counts_instructions`): `rss`'s shape, kept
        #: beside the wall clock rather than instead of it.
        self.instructions: dict[str, list[int]] = {}
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
        #: The caches `prebuild` wrote this sitting, by the input path each
        #: was built over. `time_run` mounts only these, so a cache an earlier
        #: sitting's binary left on the device is never read.
        self._prebuilt: dict[Path, Path] = {}
        self._dry_reps: dict[str, int] = {}
        self._last_telemetry: dict[str, float] = {}
        self._last_rss: float | None = None
        self._last_instructions: int | None = None
        #: Whether the run just taken was OOM-killed under a family that
        #: tolerates it. Read by `sweep`, which files the rep as censored
        #: instead of as a reading.
        self._last_killed = False
        #: The run just taken's OOM-kill count, as `parse_oom_kills` read it:
        #: `None` where `memory.events` was unreadable. Read by
        #: `_gate_twin_of_kill`, which must say whether its twin was killed
        #: or failed some other way.
        self._last_oom: int | None = 0
        self.sampler = contention.Sampler()
        #: Every reading's telemetry, in the order taken, so a sweep can be
        #: audited after the fact even where the gate let a reading through.
        self.telemetry: list[dict] = []
        #: The images each figure's programs ran in — the register's, or the
        #: release image a bench runs in — and the glibc each answered, which
        #: is what a figure's marker names where the stamp does not speak for
        #: it (`marker_glibc`).
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
            machine_state.l3_groups() if any(a.pinned for a in self.arms) else []
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
        return machine_state.placement(shapes.stated_threads(spec.command), self.groups)

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
        dst = self.cfg.warm_dir / staging.STAGED_BIN_DIR / f"{tag}-{src.name}"
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
            self.glibcs[where] = leg_binaries.glibc_of(self.cfg, where)

    def purge_delay(self, figure: str, delay: int | None) -> int | None:
        """The purge delay `figure`'s `delay` leg runs under, asked of the
        shipped binary once a sitting (`binary_purge_delay`) and kept in
        `reported` under a key no spec takes, so `raw.json` carries it and a
        render names what the sitting asked rather than asking today's."""
        key = f"{figure}/purge-delay/{leg_binaries.purge_delay_caption(delay)}"
        if key not in self.reported:
            got = leg_binaries.binary_purge_delay(self.cfg, self.cfg.bin_pgdt, delay)
            self.log(f"  {figure}'s {leg_binaries.purge_delay_caption(delay)} leg runs under a {got} ms purge delay")
            self.reported[key] = {"purge_delay": str(got)}
        return int(self.reported[key]["purge_delay"])

    # -- one timed run ----------------------------------------------------

    def binary_path(self, which: str) -> Path:
        if which in ("pgdt", "dfcli"):
            return self.cfg.bin_pgdt
        if which == "xzdecode":
            return image.ensure_xz_decode_binary(self.cfg, self.log)
        if which == "gzinflate":
            return image.ensure_gzip_inflate_binary(self.cfg, self.log)
        if which.startswith("gzseek:"):
            return image.ensure_gzseek_binary(self.cfg, which.removeprefix("gzseek:"), self.log)
        if which.startswith("alloc:"):
            return leg_binaries.ensure_allocator_binary(self.cfg, which.removeprefix("alloc:"), self.log)
        if which.startswith("profile:"):
            return leg_binaries.ensure_profile_binary(self.cfg, which.removeprefix("profile:"), self.log)
        if which.startswith("chunk:"):
            return leg_binaries.ensure_chunk_binary(self.cfg, which.removeprefix("chunk:"), self.log)
        if which == "introspect":
            return leg_binaries.ensure_instrument_binary(self.cfg, self.log)
        raise ValueError(f"unknown binary {which!r}")

    def input_path(self, name: str, regime: str) -> Path:
        """Where this regime's copy of that input lives.

        Resolved through `REGIMES`, which raises on a name it does not carry.
        The branch below has no fallback for the same reason: an unrecognised
        regime must not resolve to a path, because a reading off the wrong
        device is a table nobody can tell from a right one.
        """
        area = machine_state.regime_spec(regime).area
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
        if machine_state.regime_spec(regime).area == "warm":
            return staging.file_size(self.cfg, self.cfg.cache_dir / inputs.input_file(name), name)
        return staging.file_size(self.cfg, self.input_path(name, regime), name)

    def stage(self, figure: str, group: int) -> None:
        """Move to that figure's group's step and stage the group, evicting
        what no later step wants first."""
        self.step = self.stager.step_of(figure, group)
        fig = register.EVERY_BY_ID.get(figure)
        groups = fig.staging_groups if fig else ()
        for name in groups[group] if group < len(groups) else ():
            self.stager.warm_path(name, self.step)

    def drop_caches(self) -> None:
        argv = shlex.split(self.cfg.sudo) + ["sh", "-c", "sync; echo 3 > /proc/sys/vm/drop_caches"]
        if self.cfg.dry_run:
            self.log("  [dry-run] " + " ".join(argv))
            return
        host.run(argv)

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
        report = parsing.parse_reported(text)
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
            / parsing.INSTRUMENT_DIR
            / f"{self._instrument_reports:04d}-{slug}.txt"
        )

    def build_before_sweep(self, specs: Sequence[RunSpec]) -> None:
        """Build every binary `specs` run that this process builds, before the
        first reading rather than lazily at the rep that first wants one: a
        `cargo` build across this machine's cores moves the very number the
        next rep takes (`run_allocator`)."""
        for which in dict.fromkeys(s.binary for s in specs):
            if which not in ("none", *gating.SHIPPED_BINARIES):
                self.binary_path(which)

    def prebuild(self, specs: Sequence[RunSpec]) -> None:
        """Build the cache every `PARALLEL_SCAN_PREBUILT` spec among `specs`
        reads, once for each input and regime, in a container of its own and
        outside any timer.

        **Removed and built again every sitting**, beside the input on the
        device the regime reads, because a cache is its build's: one an earlier
        binary wrote is refused or, worse, read. The container is the figure's,
        the builder discovering its statistics allowance from it (D85), so the
        cache is the one `PARALLEL_SCAN`'s in-container builder writes."""
        for spec in specs:
            if not spec.command.startswith(f"{shapes.PARALLEL_SCAN_PREBUILT}-jobs-"):
                continue
            dump = self.input_path(spec.input, spec.regime)
            if dump in self._prebuilt:
                continue
            cache = dump.with_name(dump.name + ".pgdt")
            memory = self.memory or self.cfg.memory
            argv = [
                *self.cfg.container_argv(), "run", "--rm",
                "-m", memory, "--memory-swap", memory,
                "-v", f"{self.binary_path(spec.binary)}:/pgdt:ro",
                "-v", f"{dump}:/dump.sql:ro",
                "-v", f"{cache.parent}:{PREBUILT_MOUNT}",
                self.cfg.image, "bash", "-c", PREBUILT_BUILDER.format(name=cache.name),
            ]
            if self.cfg.dry_run:
                self.log("  [dry-run] " + " ".join(shlex.quote(a) for a in argv))
            else:
                cache.unlink(missing_ok=True)
                self.log(f"  building {cache}, untimed")
                proc = subprocess.run(argv, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                if proc.returncode != 0 or not cache.exists():
                    raise RuntimeError(
                        f"the untimed builder of {cache} exited {proc.returncode}"
                        f"\n{proc.stderr[-2000:]}"
                    )
            self._prebuilt[dump] = cache

    def _gate_twin_of_kill(self, spec: RunSpec) -> str:
        """The gate's reading of a kill outside `KILL_TOLERANT`: the same leg
        run once now on `GATE_BINARY`, and what it did, as a sentence for the
        error the kill raises.

        **Once, and only on a kill.** Everywhere outside the two figures whose
        resident is the gate, the gate's reading is whether the leg was killed,
        and a leg that was not needs no second reading to say so; reading every
        figure twice — or `parallel-scan-throughput` alone, which shares
        `PARALLEL_MEMORY` but reads no resident set — would lengthen the sweep
        to learn nothing a pass did not already say. One run because the
        figure is ending (`emit` goes on to the next): the twin classifies the
        failure, which is all a kill there is evidence of."""
        twin = gating.gate_twin(spec)
        head = f"\n\nIts gate twin, `{twin.label}`, run once now on the `{gating.GATE_LEG}` build,"
        # Cleared first, so a twin that fails before its container starts —
        # its build, say — is not read as killed off the shipped leg's count.
        self._last_oom = 0
        try:
            self.time_run(twin)
        except Exception as err:  # the kill being raised is the error that matters
            if self._last_oom:
                return (
                    f"{head} was OOM-killed too: a kill on both legs is not the allocator's, "
                    "and is filed against the reserve (`KD34`) or as a deficiency of its own."
                )
            return f"{head} failed another way, so the kill is not classified: {str(err).splitlines()[0] if str(err) else type(err).__name__}"
        rss = f" at {formatting.fmt_mib(self._last_rss)}" if self._last_rss is not None else ""
        return (
            f"{head} survived{rss}: a kill on the shipped build alone is the allocator's, "
            "attributed on the instrument build before a remedy is chosen; it reopens D13 "
            "(`docs/design/decisions.md`) only where that entry's Reopens line says."
        )

    def time_run(self, spec: RunSpec) -> float:
        self._last_killed = False
        self._last_instrument = {}
        dump = self.input_path(spec.input, spec.regime)
        script = with_allowance(shapes._script(spec.command), spec)
        counted = self.counted(spec)
        if counted:
            script = parsing.counted_script(script)
        bins: list[tuple[Path, str]] = []
        if spec.binary != "none":
            bins = [(self.binary_path(spec.binary), "/pgdt")]
        # The instrument beside the program where the shape runs under it,
        # staged with it, so neither loads inside the timer.
        if parsing.PEAK_RSS in script.split():
            bins.append((image.ensure_peak_rss_binary(self.cfg, self.log), parsing.PEAK_RSS))
        if counted:
            bins.append((image.ensure_count_instructions_binary(self.cfg, self.log), parsing.COUNT_INSTRUCTIONS))
        mounts = [
            f"{self.stage_binary(path) if self.arm.staged else path}:{at}:ro"
            for path, at in bins
        ]
        mounts.append(f"{dump}:/dump.sql:ro")
        if spec.command.startswith(f"{shapes.PARALLEL_SCAN_PREBUILT}-jobs-"):
            # Where `--source` looks for it, and only one this sitting built.
            if dump not in self._prebuilt:
                raise RuntimeError(
                    f"{spec.label} reads a cache `Session.prebuild` did not build this sitting"
                )
            mounts.append(f"{self._prebuilt[dump]}:/dump.sql.pgdt:ro")
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
            mounts.append(f"{report_path.parent}:{parsing.INSTRUMENT_MOUNT}")
            env_argv = [
                "-e",
                f"{parsing.INSTRUMENT_OUT_VAR}={parsing.INSTRUMENT_MOUNT}/{report_path.name}",
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
            argv += ["--cpuset-cpus", release.cpu_list(cpus)]
        if counted:
            argv += [a for cap in parsing.COUNT_INSTRUCTIONS_CAPS for a in ("--cap-add", cap)]
        for m in mounts:
            argv += ["-v", m]
        argv += env_argv
        # The leg's own variable, outside the shape for the instrument's
        # reason: the shape recorded is the shape that ran, under it.
        if spec.env is not None:
            argv += ["-e", spec.env]
        # The command shape, then the harness's own question about how it
        # ended. `OOM_ORACLE` is appended here rather than written into
        # `_script` so that what a figure records as its shape is exactly what
        # was measured, and so the oracle covers every shape by construction
        # rather than by thirty branches remembering to carry it.
        # The report's format and a staged leg's untimed read go in front of
        # it on the same terms: outside the shape, so neither is recorded as
        # part of what was measured.
        argv += [
            self.cfg.image, "bash", "-c", parsing.TIME_FORMAT + preread + script + parsing.OOM_ORACLE
        ]
        # The harness beside a pinned leg goes to the other die, and back
        # beside an unpinned one, before anything it launches for this run.
        if self.groups:
            self.place_harness(machine_state.harness_cpus(self.groups) if cpus else self._harness_home)

        # Whether the cache is dropped is the regime's own declaration, not a
        # prefix on its name: the name and the staging area are two facts, and
        # a regime that read the second off the first would drop caches for
        # anything called `cold-*` however it was staged.
        if machine_state.regime_spec(spec.regime).drops_caches:
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
            self._last_instructions = 3_000_000_000 + digest[3] * 1_000_000 if counted else None
            # A plausible instrument report, for the same reason the reading is
            # plausible: a dry run must exercise every division a table performs
            # rather than stopping at the first missing key.
            self._last_stdout = (
                {
                    "plaintext": str(3 * config.GIB),
                    "delivered": str(3 * config.GIB),
                    "workers": spec.command.rpartition("-")[2],
                    "blocks": str(shapes.DECODE_WORKERS[-1] * 8),
                }
                if spec.command.startswith("decode-")
                else {}
            )
            if spec.command.startswith(shapes.DYNFILTER_FAMILY):
                # A stand-in answer, one per query and alike in both legs,
                # which the table refuses to render otherwise.
                query = spec.command.rpartition("-")[0]
                self._last_stdout = {
                    "result_rows": "1",
                    "result_first": "100",
                    "result_digest": hashlib.sha256(query.encode()).hexdigest(),
                }
                if spec.command == shapes.DYNFILTER_STARTUP:
                    self._last_stdout = {"startup_answer": "1"}
            if spec.command.startswith(shapes.PARALLEL_SCAN):
                # One answer for every provider leg of the figure, which it
                # refuses to render otherwise; the prebuilt family's prefix
                # is this one's too.
                self._last_stdout = {
                    "result_rows": "1",
                    "result_first": "100",
                    "result_digest": hashlib.sha256(shapes.PARALLEL_SCAN_SQL.encode()).hexdigest(),
                }
            if spec.command.startswith(shapes.PRUNING_FAMILY):
                # A stand-in of what the query's own notes say, for the same
                # reason: the pruning table divides by them and refuses a
                # pruned leg that skipped nothing — or, for the filter its
                # statistics cannot narrow, one that skipped anything, and a
                # leg over a cache without statistics that consulted any.
                pruned = spec.command.endswith("-all")
                skipped = (
                    0
                    if spec.command.startswith(f"{shapes.PRUNING_FAMILY}{shapes.PRUNING_UNNARROWED}-")
                    else 3060
                )
                self._last_stdout = {
                    "rows_returned": "3000",
                    **(
                        {
                            "skipped_groups": str(skipped),
                            "groups": "3072",
                            "skipped_bytes": str(skipped * config.MIB),
                            "bytes": str(3072 * config.MIB),
                        }
                        if pruned
                        else {}
                    ),
                }
            if spec.command.startswith(shapes.GZPASS_FAMILY):
                # A stand-in resolution, for the same reason: the first-pass
                # table refuses a leg whose run resolved another count, or a
                # gzip leg that opened no first pass and a plain one that did.
                jobs = spec.command.removeprefix(shapes.GZPASS_FAMILY)
                self._last_stdout = {
                    "resolved_jobs": jobs,
                    **({} if spec.input == "control" else {"first_pass_workers": jobs}),
                }
            if spec.command.startswith((shapes.LONG_RSS_FAMILY, shapes.LONG_FACTOR_FAMILY)):
                # A stand-in answer or resolution, for the same reason: the
                # figure refuses a query or filter leg answering other than its
                # input's rows, and a parse leg resolving another count.
                rest = spec.command.removeprefix(shapes.LONG_RSS_FAMILY).removeprefix(shapes.LONG_FACTOR_FAMILY)
                name, _, jobs = rest.rpartition("-") if "-" in rest else (rest, "", "")
                answer = str(inputs.long_line_answer(spec.input))
                self._last_stdout = (
                    {"result_rows": "1", "result_first": answer}
                    if name == "query"
                    else {"rows_returned": answer}
                    if name == "filter"
                    else {"resolved_jobs": jobs}
                )
            if spec.command.startswith(shapes.GZSPACING_FAMILY):
                # A stand-in index, for the same reason: the spacing table
                # divides by the unit count, and refuses stages that disagree
                # on it or a `starts` leg that began fewer units than it holds.
                stage, _, mib = spec.command.removeprefix(shapes.GZSPACING_FAMILY).partition("-")
                units = -(-3 * config.GIB // (int(mib) * config.MIB))
                self._last_stdout = {
                    "units": str(units),
                    "windows": str((units - 1) * 32 * 1024),
                    **({"started": str(units)} if stage == "starts" else {}),
                }
            if spec.memory in (token for token, _ in shapes.RESERVE_LIMITS):
                # The reserve figure's flagless legs, whose limit is their axis.
                # A stand-in resolution, for the same reason the reading above
                # is a stand-in: a dry run must exercise every division the
                # flagless table performs, and the count it fits over exists
                # only in what a real run reports. Monotone in the limit so the
                # fit is well conditioned, and deliberately **not** the
                # library's own arithmetic — nothing here may grow into a second
                # authority on what an allocation resolves to.
                rank = 1 + [token for token, _ in shapes.RESERVE_LIMITS].index(spec.memory)
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
                    "live_peak_bytes": str(200 * config.MIB + digest[2] * config.MIB // 255),
                    "mimalloc_scope": "rust-heap",
                    "mimalloc_committed_bytes": str(96 * config.MIB),
                    "mimalloc_committed_peak_bytes": str(320 * config.MIB),
                    "mimalloc_reserved_bytes": str(1024 * config.MIB),
                    "mimalloc_reserved_peak_bytes": str(1024 * config.MIB),
                    "glibc_scope": "c-malloc",
                    "mallinfo_arena": str(130 * config.MIB),
                    "mallinfo_hblkhd": "0",
                    "mallinfo_uordblks": str(66 * config.MIB),
                    "mallinfo_fordblks": str(64 * config.MIB),
                    "malloc_heaps": "6",
                    "malloc_system_current": str(130 * config.MIB),
                    "malloc_system_max": str(350 * config.MIB),
                }
                if spec.command.startswith(shapes.LONG_FACTOR_FAMILY):
                    # A stand-in plan's factor, varied by input so the figure's
                    # largest is one kind's; never the library's arithmetic.
                    self._last_instrument["planned_factor"] = f"{1 + digest[4] % 9}/{2}"
            return 0.4 + digest[0] / 255 * 5.0
        # The counters bracket the run as tightly as possible: two procfile
        # reads, outside the timer, either side of the subprocess. Their
        # difference is what the machine did *during this rep* -- which is the
        # only window that means anything for a reading half a second long.
        started = time.time()
        before, mono_start = contention.Counters.read(), time.monotonic()
        proc = subprocess.run(argv, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        mono_end, after = time.monotonic(), contention.Counters.read()
        oom = parsing.parse_oom_kills(proc.stderr)
        self._last_oom = oom
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
                self._last_instructions = None
                self._last_stdout = parsing.parse_resolution(proc.stderr)
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
                        "maxrss_bound_kib": _bound(parsing.parse_maxrss_kib),
                        "seconds_to_kill": _bound(parsing.parse_bash_time),
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
            # A kill is a gate reading wherever it lands, so the shipped
            # build's is read on `GATE_BINARY` before its figure fails on it.
            twin = self._gate_twin_of_kill(spec) if oom and spec.binary in gating.SHIPPED_BINARIES else ""
            raise RuntimeError(
                f"{spec.label} exited {proc.returncode}{why}{twin}"
                f"\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}"
            )
        seconds = parsing.parse_bash_time(proc.stderr)
        self._last_rss = parsing.parse_maxrss_kib(proc.stderr) if "rss" in spec.command else None
        self._last_instructions = parsing.parse_instructions(proc.stderr) if counted else None
        # What the run said about itself on its own streams: the decode
        # example's counts on stdout, and, for a scan, the arrangement its
        # `scan started` line names. One dict, all of it identical across reps.
        # The introspection build's report is **not** here — it is per-rep, and
        # it arrives in a file of its own (`_read_instrument`).
        self._last_stdout = {
            **parsing.parse_reported(proc.stdout),
            **parsing.parse_resolution(proc.stderr),
            **parsing.parse_first_pass(proc.stderr),
            **parsing.parse_query_notes(proc.stderr),
        }
        if report_path is not None:
            self._last_instrument = self._read_instrument(spec, report_path)
        telemetry = contention.counter_delta(before, after)
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
                # `None` on a leg its figure does not count.
                "instructions_u": self._last_instructions,
                # Zero where the container answered and nothing was killed;
                # `None` where `memory.events` could not be read, which is a
                # different answer and stays one (`parse_oom_kills`).
                "oom_kill": oom,
                # The CPU the timed command spent, beside its wall clock.
                "cpu_seconds": parsing.parse_bash_cpu(proc.stderr),
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
            "cpuset": release.cpu_list(cpus) if cpus is not None else None,
        }

    # -- sweeps -----------------------------------------------------------

    def sweep(self, figure: str, specs: Sequence[RunSpec], reps: int) -> None:
        """`_sweep_one` over `specs` -- or, for a figure split into
        `warm_groups`, over each group's specs in turn, that group staged
        first, so a group's inputs are measured only beside each other."""
        fig = register.EVERY_BY_ID.get(figure)
        groups = fig.staging_groups if fig else ()
        if len(groups) < 2:
            if named := [s.label or s.command for s in specs if s.sweep is not None]:
                raise ValueError(f"{figure} is one sweep, and {named} name a warm group")
            self._sweep_one(figure, specs, reps)
            return
        for gi, part in staging.split_specs(figure, specs, groups):
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
                    if self.counted(spec):
                        self.instructions.setdefault(key, [])
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
                if self._last_instructions is not None:
                    self.instructions.setdefault(spec.key(figure), []).append(
                        self._last_instructions
                    )
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
        for attempt in range(1, machine_state.GATE_RETRIES + 1):
            seconds = self.time_run(spec)
            label = spec.label if len(self.arms) == 1 else f"{spec.label} [{self.arm.name}]"
            if self._last_killed:
                # A killed run is not a timing reading at all, so the
                # contention gate has nothing to judge and retaking it would
                # spend `GATE_RETRIES` runs reproducing the kill.
                self.log(f"  rep{rep + 1} {label}: **OOM-killed** — recorded, censored")
                return seconds
            verdict = machine_state.contention_verdict(self._last_telemetry, spec.regime)
            if verdict is None:
                self.log(f"  rep{rep + 1} {label}: {seconds:.6f} s")
                return seconds
            self.log(
                f"  rep{rep + 1} {label}: {seconds:.6f} s — DISCARDED, "
                f"machine contended ({verdict}); attempt {attempt}/{machine_state.GATE_RETRIES}"
            )
        raise RuntimeError(
            f"{spec.label}: {machine_state.GATE_RETRIES} consecutive readings were taken under contention "
            f"({machine_state.contention_verdict(self._last_telemetry, spec.regime)}) — "
            "the machine is too busy to measure on"
        )

    def get(self, figure: str, spec: RunSpec) -> list[float]:
        return self.readings[spec.key(figure)]

    def counted(self, spec: RunSpec) -> bool:
        """Whether this run's timed command goes under the instruction counter:
        every leg running a `pgdt` of a figure declaring
        `Figure.counts_instructions`, and no co-measured floor."""
        fig = register.EVERY_BY_ID.get(self.figure_id)
        return bool(fig and fig.counts_instructions and spec.binary != "none")

    def get_instructions(self, figure: str, spec: RunSpec) -> list[int] | None:
        """The user-space instructions of one spec's accepted reps, or `None`
        where the sitting took none: a counted leg of a sitting from before
        the figure counted, which renders as uncounted rather than failing."""
        return self.instructions.get(spec.key(figure))

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
