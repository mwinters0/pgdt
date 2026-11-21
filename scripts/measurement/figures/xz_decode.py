"""`xz-decode-scaling`: the `.xz` decoder's rate by worker count."""

from __future__ import annotations

from typing import TYPE_CHECKING

from generate_xz_input import KOJI_RATIO_MAX, KOJI_RATIO_MIN

from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement.builds import image
from measurement.figures import prose

if TYPE_CHECKING:
    from measurement.timing import Session


#: The row every other decode row is a ratio against — one worker, which is
#: what the serial path this project ships has today.
DECODE_BASELINE = 1

# -- xz decode scaling ------------------------------------------------------

#: The two legs, in the table's column order, with the caption each carries.
#: The generated one first, because it is the one anybody can re-take.
DECODE_LEGS: tuple[tuple[str, str], ...] = (
    ("control_xz", "Generated control"),
    ("koji_xz", "koji, 128 streams"),
)


def _decode_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec("xzdecode", leg, f"decode-{workers}", "warm-parallel", f"{label}, {workers}w")
        for leg, label in DECODE_LEGS
        for workers in shapes.DECODE_WORKERS
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
    image.ensure_xz_decode_binary(session.cfg, session.log)
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
    for workers in shapes.DECODE_WORKERS:
        cells = [str(workers) + (" *(serial)*" if workers == DECODE_BASELINE else "")]
        for leg, _ in DECODE_LEGS:
            values = by_leg[leg][workers]
            got, base = formatting.median(values), formatting.median(by_leg[leg][DECODE_BASELINE])
            rate = formatting.fmt_rate(plaintext.get(leg, 0), got)
            cells.append(f"{formatting.fmt_median_spread(values)} · {rate} · {base / got:.2f}×")
        rows.append(cells)
    table = formatting.md_table(["Workers", *(label for _, label in DECODE_LEGS)], rows)

    sizes = []
    for leg, label in DECODE_LEGS:
        compressed = staging.file_size(
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
            f"- {label}: {formatting._fmt_bytes(compressed)} compressed, {formatting._fmt_bytes(plain)} of plaintext, "
            f"{ratio}{gate}"
        )
    notes = (
        "\n\nEach cell is wall clock, the plaintext rate it implies, and the speedup over that "
        f"leg's own one-worker row. The instrument is `crates/pgdt-io/examples/xz_decode.rs` in a "
        f"{shapes.DECODE_MEMORY} container — **not** the register's 512 MB, which cannot hold "
        f"{shapes.DECODE_WORKERS[-1]} decoded 24 MiB blocks — and it refuses a run whose plan admits "
        "fewer workers than were asked for.\n\n"
        + "\n".join(sizes)
        + "\n"
    )
    return table + notes + "\n" + prose._per_rep("xz-decode-scaling", session, specs)
