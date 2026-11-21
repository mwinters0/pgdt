"""`scan-throughput-*` and `chunk-size`: what a scan costs by input shape and device, and what the
read chunk size is worth."""

from __future__ import annotations

from typing import Sequence
from typing import TYPE_CHECKING

from measurement import borrowing
from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.figures import prose

if TYPE_CHECKING:
    from measurement.timing import Session


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


def _throughput_specs(regime: str) -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec("none" if cmd == "dd" else "pgdt", inp, cmd, regime, f"{label} ({regime})")
        for inp, cmd, label in _THROUGHPUT_ROWS
    ]


def _throughput_table(session: Session, figure: str, specs: Sequence[runspec.RunSpec]) -> str:
    floors = {
        spec.input: formatting.median(session.get(figure, spec)) for spec in specs if spec.command == "dd"
    }
    rows = []
    for spec, (_, _, label) in zip(specs, _THROUGHPUT_ROWS):
        values = session.get(figure, spec)
        size = session.input_size(spec.input, spec.regime)
        against = (
            "—"
            if spec.command == "dd"
            else f"{formatting.median(values) / floors[spec.input]:.2f}× its floor's time"
        )
        rows.append(
            [label, formatting.fmt_median_spread(values), formatting.fmt_rate(size, formatting.median(values)), against]
        )
    return formatting.md_table(["Input", "Wall", "Rate", "Against the floor"], rows)


def _throughput_figure(session: Session, figure: str, regime: str, reps: int) -> str:
    """The throughput table, in one regime: three shapes and their floors.

    Its warm `COPY` row is also the allocator table's reference `parse` row --
    same binary, same command, same input -- which borrows it rather than
    measuring it twice, since two numbers in the doc for one measurement is the
    defect the whole sweep exists to remove."""
    specs = _throughput_specs(regime)
    note = borrowing.share_readings(session, figure)
    to_run = [s for s in specs if s.key(figure) not in session.readings]
    session.sweep(figure, to_run, reps)
    return (
        _throughput_table(session, figure, specs)
        + "\n"
        + note
        + "\n"
        + prose._per_rep(figure, session, specs)
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


def _chunk_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec(
            "pgdt",
            "control",
            f"parse-chunk-{size}",
            regime,
            f"{fmt_chunk(size)} ({regime})",
        )
        for regime, _ in CHUNK_REGIMES
        for size in shapes.CHUNK_SIZES
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
        by_regime[spec.regime][size] = formatting.median(session.get("chunk-size", spec))

    rows = []
    for size in shapes.CHUNK_SIZES:
        cells = [fmt_chunk(size) + (" *(default)*" if size == CHUNK_DEFAULT else "")]
        for regime, _ in CHUNK_REGIMES:
            got = by_regime[regime][size]
            ratio = got / by_regime[regime][CHUNK_DEFAULT]
            spec = next(
                s for s in specs if s.regime == regime and s.command.endswith(f"-{size}")
            )
            cells.append(f"{formatting.fmt_median_spread(session.get('chunk-size', spec))} · {ratio:.2f}×")
        rows.append(cells)
    table = formatting.md_table(["Chunk", *(label for _, label in CHUNK_REGIMES)], rows)
    return table + "\n" + prose._per_rep("chunk-size", session, specs)
