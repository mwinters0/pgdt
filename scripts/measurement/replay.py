"""`--render`: a sitting's tables rebuilt from its `raw.json`, measuring nothing."""

from __future__ import annotations

from pathlib import Path
from typing import Callable, Sequence
from typing import TYPE_CHECKING

from measurement import inputs
from measurement import machine_state
from measurement import staging
from measurement import timing
from measurement.builds import leg_binaries

if TYPE_CHECKING:
    from measurement.config import Config
    from measurement.runspec import RunSpec


class ReplaySession(timing.Session):
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
    renderer but three asks an input for its size and never for its contents --
    the size is what a rate is per -- so a stand-in of the right length
    reproduces the byte counts exactly while needing no cache, no tmpfs and no
    disk. `test_replay.py` is what holds renderers to that.

    **The three are steps nothing here refuses**, and their cells are no
    reading of the sitting: `run_per_block_quadratic`'s `count_saves` and
    `run_preamble_prepass`'s header search read the sparse stand-ins, and
    `run_nested_decode_micro` runs `cargo bench` in today's release image. A
    re-render's fold-in takes those three sections from the sitting's own
    `tables-as-taken.md`, or from a re-take.
    """

    def __init__(self, cfg: Config, raw: dict, sizes_dir: Path, log: Callable[[str], None]) -> None:
        super().__init__(cfg, staging.Stager(cfg, log), log)
        self.readings = raw["readings"]
        self.rss = raw.get("rss", {})
        #: Absent from every sitting taken before a figure counted.
        self.instructions = raw.get("instructions", {})
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

    def build_before_sweep(self, specs: Sequence[RunSpec]) -> None:
        """A no-op: nothing is run, so nothing is built."""

    def prebuild(self, specs: Sequence[RunSpec]) -> None:
        """A no-op: no leg reads a cache, so none is built."""

    def ran_in(self, figure: str, where: str) -> None:
        """A no-op: which glibc each figure ran under is the sitting's own
        record (`figure_glibc`), and asking a place now would name today's."""

    def purge_delay(self, figure: str, delay: int | None) -> int | None:
        """What the sitting asked (`Session.purge_delay`), or `None` where it
        asked nothing: asking the binary now would name today's."""
        key = f"{figure}/purge-delay/{leg_binaries.purge_delay_caption(delay)}"
        return int(self.reported[key]["purge_delay"]) if key in self.reported else None

    def take(self, spec: RunSpec, rep: int) -> float:
        raise AssertionError(f"--render must measure nothing, but {spec.label} was run")

    def drop_caches(self) -> None:
        raise AssertionError("--render must measure nothing, but the page cache was dropped")

    def input_path(self, name: str, regime: str) -> Path:
        # The regime decides nothing here -- every renderer asks an input for
        # its size and never for its device -- but it is still resolved, so a
        # renderer naming a regime nothing declares fails under `--render` as
        # it would under a sweep, at a second's cost instead of a sweep's.
        machine_state.regime_spec(regime)
        if name not in self._sizes:
            raise KeyError(
                f"this sitting recorded no size for input {name!r}, so its table cannot be "
                "re-rendered. Sittings taken before --render existed carry no `input_sizes`; "
                "re-take the figure to render it."
            )
        path = self._sizes_dir / inputs.input_file(name)
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("wb") as fh:  # sparse: costs no blocks
                fh.truncate(self._sizes[name])
        return path

    def input_size(self, name: str, regime: str) -> int:
        """The recorded size, whatever the regime: nothing is staged here."""
        return staging.file_size(self.cfg, self.input_path(name, regime), name)

    def stage(self, figure: str, group: int) -> None:
        """A no-op: nothing is staged to serve a past sitting's readings."""
