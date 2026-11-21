"""The gate's section of a figure's table: each gated leg's verdict, and the margin it leaves."""

from __future__ import annotations

from typing import Sequence
from typing import TYPE_CHECKING

from measurement import formatting
from measurement import gating
from measurement import memory_model

if TYPE_CHECKING:
    from measurement.runspec import RunSpec
    from measurement.timing import Session


#: The share of its limit each gate leg's worst surviving rep is reported
#: against, by figure — **reported beside the verdict and blocking nothing**.
#: A gate leg fails on a kill alone, in every figure: the gate asks whether
#: the binary is OOM-killed under the arrangements its figures take, not how
#: close it comes (`docs/design/measurements.md`, "The apparatus").
#:
#: **`reserve` reports the library's own criterion**, `MEMORY_MARGIN_PERCENT`:
#: the flagless rule aims resident at the allocation, so what a leg leaves of
#: it is the reading the reserve is chosen against, and a shortfall on the
#: shipped build alone is `KD34`'s, which re-sets the reserve under the
#: shipped allocator.
#:
#: **`parallel-peak-rss` and `gzip-resident` are absent**: their container,
#: `budget + PARALLEL_HEADROOM`, *is* the contract under test, so the head
#: their cells print is context. Overrunning `MEMORY_UNPOOLED_BOUND` fails nothing either,
#: by the same section: it is `charge_model_problem`'s finding about the bound.
REPORTED_MARGIN_PERCENT: dict[str, int] = {"reserve": memory_model.LIBRARY_MEMORY_MARGIN_PERCENT}


def _gate_cell(
    session: "Session", figure: str, spec: RunSpec, limit: int
) -> tuple[str, bool | None]:
    """One leg's gate reading, and whether it failed.

    The cell is the worst surviving rep and what it left of `limit` — the
    number a cgroup's killer reads — or the kill. **A kill alone fails the
    leg**; what the worst rep leaves of a reported margin is `_margin_short`'s
    and decides nothing here (`REPORTED_MARGIN_PERCENT`). `None` is a leg this
    sitting did not read, which is neither a pass nor a failure: a sitting
    taken before the gate had twins still renders, and says so."""
    if not session.has(figure, spec):
        return "not read", None
    readings = session.rss.get(spec.key(figure), [])
    killed = session.kills(figure, spec)
    worst = (
        f"{formatting.fmt_mib(max(readings))} · head {(limit - max(readings) * 1024) / limit * 100:.1f}%"
        if readings
        else ""
    )
    if killed:
        return f"**OOM-killed**, {killed} rep(s)" + (f" · survivors' worst {worst}" if worst else ""), True
    if not readings:
        return "no reading", None
    return worst, False


def _margin_short(session: "Session", figure: str, spec: RunSpec, limit: int) -> bool | None:
    """Whether this leg's worst surviving rep leaves less of `limit` than the
    figure's reported margin (`REPORTED_MARGIN_PERCENT`), equality meeting it
    as the library's `margin_allowance` meets it; `None` where the figure
    reports no margin or the leg has no surviving rep. Reported, never a
    failure: `_gate_cell` fails a leg on a kill alone."""
    margin = REPORTED_MARGIN_PERCENT.get(figure)
    readings = session.rss.get(spec.key(figure), []) if session.has(figure, spec) else []
    if margin is None or not readings:
        return None
    # Bytes against bytes, scaled by a hundred rather than divided, so the
    # line falls exactly where the percentage does and a rounded head in the
    # cell cannot report what the comparison would not.
    return (limit - max(readings) * 1024) * 100 < limit * margin


def margin_note(shipped: bool | None, twin: bool | None) -> str:
    """One pair's reading against the reported margin, for the column beside
    the gate's verdict (`REPORTED_MARGIN_PERCENT`)."""
    if shipped is None and twin is None:
        return "—"
    if shipped and twin:
        return "both short"
    if shipped:
        return "**shipped build short**"
    if twin:
        return f"`{gating.GATE_LEG}` short"
    if shipped is None or twin is None:
        return "met where read"
    return "met"


def gate_verdict(shipped: bool | None, twin: bool | None) -> tuple[str, bool]:
    """What one pair says, and whether it blocks.

    Only a failure on the shipped build alone blocks (`GATE_LEG`). A failure
    on the `GATE_LEG` build alone does not: the shipped build held, and that
    is the claim under test."""
    if shipped is None or twin is None:
        return "not judged", False
    if shipped and twin:
        return "fails on both — filed, does not block", False
    if shipped:
        return "**fails on the shipped build alone — blocks**", True
    if twin:
        return f"passes; the `{gating.GATE_LEG}` leg failed", False
    return "passes", False


def gate_section(
    session: "Session", figure: str, legs: Sequence[tuple[str, RunSpec, int]]
) -> str:
    """The gate's table for one figure: each shipped leg beside its
    `gate_twin`, against the limit it ran under, and the verdict per pair.

    `legs` is `(row label, shipped spec, limit in bytes)`. A sitting holding
    no twin at all — every one taken before the gate had them — renders one
    sentence instead of a column of "not read"."""
    if not any(session.has(figure, gating.gate_twin(spec)) for _, spec, _ in legs):
        return (
            f"**The gate is not judged here**: this sitting read no `{gating.GATE_LEG}` leg beside "
            "the shipped one, so a failure in it could not be told the allocator's from "
            "the code's.\n"
        )
    margin = REPORTED_MARGIN_PERCENT.get(figure)
    rows, blocking, both, short = [], [], [], []
    for label, spec, limit in legs:
        shipped, shipped_failed = _gate_cell(session, figure, spec, limit)
        twin, twin_failed = _gate_cell(session, figure, gating.gate_twin(spec), limit)
        verdict, blocks = gate_verdict(shipped_failed, twin_failed)
        if blocks:
            blocking.append(label)
        if shipped_failed and twin_failed:
            both.append(label)
        row = [label, shipped, twin, verdict]
        if margin is not None:
            shipped_short = _margin_short(session, figure, spec, limit)
            twin_short = _margin_short(session, figure, gating.gate_twin(spec), limit)
            if shipped_short and not twin_short:
                short.append(label)
            row.append(margin_note(shipped_short, twin_short))
        rows.append(row)
    headings = ["Leg", "Shipped build", f"`{gating.GATE_LEG}`", "Gate"]
    if margin is not None:
        headings.append(f"Against the {margin}% margin — reported, blocks nothing")
    table = formatting.md_table(headings, rows)
    if blocking:
        summary = (
            f"**The gate blocks**: {len(blocking)} leg(s) failed on the shipped build alone — "
            + ", ".join(blocking)
            + ". That is the allocator's, attributed on the instrument build in a sitting "
            "of its own, never off this table, and it reopens "
            '[`decisions.md`](decisions.md), "D13", only where its Reopens line says.'
        )
    else:
        summary = "**The gate passes**: no leg failed on the shipped build alone."
    if both:
        summary += (
            f" {len(both)} failed on both legs — "
            + ", ".join(both)
            + " — which is not the allocator's and is filed rather than blocking."
        )
    if short:
        summary += (
            f" {len(short)} leg(s) leave less than the {margin}% margin on the shipped build "
            "alone — " + ", ".join(short) + " — which is reported and blocks nothing: the "
            "reserve is re-set under the shipped allocator by `KD34`'s owner."
        )
    criterion = (
        "A leg fails where it is killed. The last column says whether the worst surviving "
        f"rep leaves {margin}% of the limit, `MEMORY_MARGIN_PERCENT`, the criterion the "
        "reserve is chosen against: reported beside the verdict, it blocks nothing"
        if margin is not None
        else "A leg fails where it is killed, its container being the contract under test"
    )
    return (
        f"**The gate.** Each leg is read again on the `{gating.GATE_LEG}` build in the same sitting, "
        "the two differing by the allocator alone, and each cell is the worst surviving rep "
        f"and what it left of the limit, or the kill. {criterion}.\n\n"
        + table + "\n\n" + summary + "\n"
    )
