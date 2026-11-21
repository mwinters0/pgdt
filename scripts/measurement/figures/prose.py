"""Paragraphs several figures' tables print: the per-rep readings, the cached query's note."""

from __future__ import annotations

from typing import Sequence
from typing import TYPE_CHECKING

from measurement import formatting
from measurement import shapes

if TYPE_CHECKING:
    from measurement.runspec import RunSpec
    from measurement.timing import Session


# -- nested end to end ------------------------------------------------------


def _cached_query_note(subject: str) -> str:
    """What a query figure's notes say of the cache `subject` reads, generated
    from the shape so the sentence cannot outlive it (`DATA_LEVEL_QUERIES`)."""
    return (
        f"{subject} reads a cache one untimed `pgdt parse` stating `{shapes.GATHER_STATISTICS}` "
        "wrote in the same container, and states `--statistics none`, so no row group is "
        "skipped: its reading carries decoding that cache whole, statistics included, and "
        "no mapping pass.\n"
    )


def _per_rep(figure: str, session: Session, specs: Sequence[RunSpec]) -> str:
    lines = [
        f"- {spec.label}: {formatting.fmt_readings(session.get(figure, spec))}" for spec in specs
    ]
    return "Per-rep readings (s):\n" + "\n".join(lines) + "\n"
