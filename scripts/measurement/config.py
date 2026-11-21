"""The harness's configuration: where everything is on this machine, each default overridable by an
environment variable (`Config`), and the image every figure's binary runs in."""

from __future__ import annotations

import os
import shlex
from dataclasses import dataclass, field
from pathlib import Path

import release

from measurement import machine_state


REPO = Path(__file__).resolve().parent.parent.parent

SCRIPTS = REPO / "scripts"

GIB = 1024**3

MIB = 1024**2

#: The only path the release build of `pgdt` writes, in the release image, for
#: the host's target (`release.Build`). It is a constant rather than a literal
#: inside `Config` because `ensure_pgdt_binary` reads it back: the harness may
#: claim to have built `cfg.bin_pgdt` only when the two are the same file.
REGISTER_BIN = release.Build().binary(release.STATE / "target")


def register_image() -> str:
    """The image every figure's binary runs in: the release image's base, by
    the digest `release/Dockerfile` pins (`release.pinned_base`).

    **One pin names the build's glibc and the runtime's**, so the binary a
    figure times runs under the glibc it was linked against, which is the
    floor a release promises (`docs/design/decisions.md`, "D104"). No
    variable moves it: a second image would be a second pin."""
    return release.pinned_base(release.DOCKERFILE.read_text())


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
    # **The one image every figure's binary runs in**, the release image's base
    # (`register_image`), so every binary the release image builds runs under
    # the glibc it linked against (`measurements.md`, "The apparatus").
    # **Pinned by digest**, the tag before it being for a reader only: what a
    # figure names is the glibc the image answers (`glibc_of`) rather than a
    # tag's. A digest the machine lacks is pulled by the first run.
    image: str = field(default_factory=register_image)
    memory: str = _env("PGDT_MEASURE_MEMORY", "512m")
    sudo: str = _env("PGDT_MEASURE_SUDO", "sudo")

    bin_pgdt: Path = Path(_env("PGDT_MEASURE_BIN", str(REGISTER_BIN)))
    # The `allocator` figure's three legs, the `release-profile` figure's
    # three, the instrument, and the host builds the printed recipes make.
    # Each is a full cargo target dir, so it goes on scratch rather than
    # under `runs/`, which holds logs and small binaries; the binaries
    # themselves are copied into `runs/`. A separate
    # target dir per leg is not tidiness: a `--features` build writes the
    # shipped build's output path, so building a leg in the shipped build's
    # dir would silently replace `bin_pgdt` and every other figure in the same
    # sweep would be timed under the wrong allocator (`image_target_dir`).
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
        if self.arms != (machine_state.Arm(),):
            return (
                f"This run placed its legs under an apparatus not adopted "
                f"(`--pin-cpus {self.pin_cpus}`, `--stage-binaries {self.stage_binaries}`). "
                "It is an experiment's sitting, read with `--arms` and `--drift`; "
                "it is not a figure."
            )
        return None

    @property
    def arms(self) -> tuple["machine_state.Arm", ...]:
        """The arrangements every leg of this run is taken under, the first
        being the one `readings` and every table carry (`Arm`)."""
        pins = machine_state.arm_values(self.pin_cpus, "--pin-cpus", machine_state.Arm().pinned)
        stages = machine_state.arm_values(self.stage_binaries, "--stage-binaries", machine_state.Arm().staged)
        return tuple(machine_state.Arm(pinned=p, staged=s) for p in pins for s in stages)

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
