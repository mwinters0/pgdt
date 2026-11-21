"""`allocator`: the shipped allocator against the others, leg by leg."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import borrowing
from measurement import formatting
from measurement import runspec
from measurement.builds import leg_binaries
from measurement.figures import prose
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.config import Config
    from measurement.timing import Session


def _allocator_reference(cfg: Config) -> str:
    """The allocator the *shipped* binary links against, read out of it.

    This figure's reference column is the shipped binary itself rather than
    a fourth build of the same source, for two reasons. It is the binary every
    other figure in the doc was taken with, so the ratios are ratios against
    the published numbers instead of against a build nothing else uses -- and
    two builds of one source can differ by ~10% from code layout alone
    (`measurements.md`, "Two builds of one source can differ by layout"), which
    is larger than the effect being measured. And it keeps the doc carrying
    **one** number per measurement: the reference readings are borrowed from
    the figures that already take them, the warm throughput table's `COPY` row
    and the nested table's control rows.

    Reading the name off the binary rather than assuming the default is what
    makes the figure survive its own answer: adopt a leg and this becomes the
    reference, with the other two measured against it and no code change."""
    if cfg.dry_run:
        return leg_binaries.ALLOCATOR_LEGS[0]
    return leg_binaries.binary_allocator(cfg.bin_pgdt)


def allocator_columns(reference: str) -> list[str]:
    """The table's columns: the shipped allocator first, then the rest in
    register order. The reference is the column the ratios are against, so it
    is the one a reader needs first."""
    return [reference] + [leg for leg in leg_binaries.ALLOCATOR_LEGS if leg != reference]


def _allocator_specs(reference: str) -> list[runspec.RunSpec]:
    """Every leg of every shape, plus the co-measured floor.

    The reference leg runs as the plain `pgdt` binary; the others are built
    per leg. The floor is one row rather than three: `dd` links no allocator,
    so a per-leg floor would be three readings of one thing. It is here for
    the same reason every other warm table co-measures one -- a session's own
    drift is what a warm absolute is read against."""
    if reference not in leg_binaries.ALLOCATOR_LEGS:
        raise ValueError(f"unknown allocator leg {reference!r}")
    specs = []
    for command, label, _ in register._ALLOCATOR_SHAPES:
        for leg in allocator_columns(reference):
            binary = "pgdt" if leg == reference else f"alloc:{leg}"
            specs.append(runspec.RunSpec(binary, "control", command, "warm", f"{label} ({leg})"))
    specs.append(runspec.RunSpec("none", "control", "dd", "warm", "`dd` → `/dev/null` (warm)"))
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
            leg_binaries.ensure_allocator_binary(session.cfg, leg, session.log)
    # The reference column, shape by shape, from whichever figure already took
    # it. Borrowed rather than retaken so the doc carries one number per
    # measurement; measured here, with a note, when this figure runs alone.
    note = borrowing.share_readings(session, figure)
    to_run = [s for s in specs if s.key(figure) not in session.readings]
    session.sweep(figure, to_run, session.cfg.reps(5))

    rows = []
    for command, label, _ in register._ALLOCATOR_SHAPES:
        cells = [label]
        reference_median = formatting.median(
            session.get(figure, runspec.RunSpec("pgdt", "control", command, "warm", ""))
        )
        for leg in columns:
            binary = "pgdt" if leg == reference else f"alloc:{leg}"
            values = session.get(figure, runspec.RunSpec(binary, "control", command, "warm", ""))
            cell = formatting.fmt_median_spread(values)
            if leg != reference:
                cell += f" — {formatting.median(values) / reference_median:.2f}×"
            cells.append(cell)
        rows.append(cells)
    rows.append(
        [
            "`dd` → `/dev/null` — the co-measured floor",
            formatting.fmt_median_spread(session.get(figure, specs[-1])),
            *(["—"] * (len(columns) - 1)),
        ]
    )
    header = [
        f"`{leg}`" + (" — the shipped binary" if leg == reference else "")
        for leg in columns
    ]
    table = formatting.md_table(["Warm, on tmpfs", *header], rows)

    if session.cfg.dry_run:
        provenance = "Legs are not built or interrogated under `--dry-run`.\n"
    else:
        legs = ", ".join(
            f"`{leg}`"
            if leg == reference
            else f"`{leg_binaries.binary_allocator(leg_binaries.ensure_allocator_binary(session.cfg, leg, session.log))}`"
            for leg in columns
        )
        provenance = (
            "Every leg was asked what it links against before it was timed — "
            f"{legs} — so a leg whose build silently dropped its feature cannot be "
            "published as a comparison of two identical binaries.\n"
        )
    return (
        table
        + "\n\n"
        + provenance
        + "\n"
        + prose._cached_query_note("Each `query` row")
        + note
        + "\n"
        + prose._per_rep(figure, session, specs)
    )
