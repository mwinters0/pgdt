"""`projection-widths` and `predicate-terms`: what a query's projection and predicate cost."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.figures import prose

if TYPE_CHECKING:
    from measurement.timing import Session


# -- projection widths ------------------------------------------------------

#: The rows, in width order, each labelled by what its difference against the
#: row above buys. Ascending because every row's column list contains the one
#: above it: the difference is then the columns they differ by and nothing
#: else, over identical rows of one file.
_PROJECTION_ROWS: tuple[tuple[int, str, str], ...] = (
    (0, "0 — `--no-columns`", "the replay floor: no decode, no build, no render"),
    (1, "1 — `v_smallint`", "one cheap scalar, above the floor"),
    (16, "16 — every scalar", "the other 15 scalars"),
    (17, "17 — the scalars and `v_comp`", "**the composite column alone**"),
    (19, "19 — every column", "**the two array columns alone**"),
)


def run_projection_widths(session: Session) -> str:
    """One file at five projection widths.

    Every attribution the cross-file apparatus makes is a subtraction between
    two adjacent rows here, over identical rows of an identical file — so
    neither `cross-file-floor`'s subtraction floor nor a file-dependent untyped
    baseline enters. That is what makes this a
    supersession of that apparatus rather than one more figure beside it.
    """
    figure = "projection-widths"
    specs = [
        runspec.RunSpec("pgdt", "arrays", f"query-project-{width}", "warm", f"{width}-column")
        for width, _, _ in _PROJECTION_ROWS
    ]
    # Six, like the two differencing figures this replaces: the reading that
    # matters is a paired difference between adjacent rows, and pairing is
    # per rep.
    session.sweep(figure, specs, session.cfg.reps(6))
    profile = session.stager.profile("arrays")
    rows, per_rep, previous = [], [], None
    for width, label, buys in _PROJECTION_ROWS:
        spec = runspec.RunSpec("pgdt", "arrays", f"query-project-{width}", "warm", "")
        values = session.get(figure, spec)
        if previous is None:
            delta = "—"
        else:
            paired = [(a - b) / profile["rows"] * 1e6 for a, b in zip(values, previous)]
            delta = f"**{formatting.median(paired):+.2f} µs**"
        rows.append(
            [
                label,
                f"{formatting.fmt_s(formatting.median(values))} s",
                f"{formatting.median(values) / profile['rows'] * 1e6:.2f} µs",
                delta,
                buys,
            ]
        )
        per_rep.append(f"- {label}: {formatting.fmt_readings(values)}")
        previous = values
    table = formatting.md_table(
        ["Projection", "Median", "Per row", "Δ per row against the row above", "What that buys"],
        rows,
    )
    return (
        table
        + f"\n\nOne file — `--arrays --composite`, {profile['rows']:,} rows of "
        f"{profile['columns']} columns — read {len(_PROJECTION_ROWS)} ways, warm and typed, "
        "through the CLI. Per-row differences are paired rep by rep and then taken as a "
        "median.\n\n"
        + prose._cached_query_note("Each `query`")
        + "\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )

# -- what a filter term costs ------------------------------------------------

#: The rows of `predicate-terms`, in the order the table carries them. The
#: first four are one depth at four term counts, so their differences are what
#: one more term costs; the fifth is the fourth's terms moved to the front of
#: the row, so its difference is the walk those five terms pay; the sixth
#: closes the shallow ladder, so the reader has a slope at both depths.
_PREDICATE_ROWS: tuple[tuple[str, str, str], ...] = (
    ("deep-1", "1 term, 13th column", "the base: one term, thirteen fields in"),
    ("deep-2", "2 terms, 13th column", "one more term at that depth"),
    ("deep-3", "3 terms, 13th column", "one more"),
    ("deep-5", "5 terms, 13th column", "two more — the five-way disjunction"),
    ("shallow-5", "5 terms, 1st column", "**the walk those five terms pay**"),
    ("shallow-1", "1 term, 1st column", "four of those five terms, walk-free"),
)


def run_predicate_terms(session: Session) -> str:
    """One file, six predicates, no row surviving any of them.

    The shape the split-sharing lever pays off on, which no other figure in
    the register runs: every one of these queries evaluates every term of an
    all-false disjunction on every row and emits nothing, so what separates
    two rows is the predicate and nothing downstream of it.
    """
    figure = "predicate-terms"
    specs = [
        runspec.RunSpec("pgdt", "control", f"query-where-{shape}", "warm", label)
        for shape, label, _ in _PREDICATE_ROWS
    ]
    # Six, as `projection-widths` takes: the reading that matters is a paired
    # difference between two rows, and pairing is per rep.
    session.sweep(figure, specs, session.cfg.reps(6))
    profile = session.stager.profile("control")
    rows, per_rep, previous = [], [], None
    for shape, label, buys in _PREDICATE_ROWS:
        spec = runspec.RunSpec("pgdt", "control", f"query-where-{shape}", "warm", "")
        values = session.get(figure, spec)
        if previous is None:
            delta = "—"
        else:
            paired = [(a - b) / profile["rows"] * 1e6 for a, b in zip(values, previous)]
            delta = f"**{formatting.median(paired):+.2f} µs**"
        rows.append(
            [
                label,
                f"{formatting.fmt_s(formatting.median(values))} s",
                f"{formatting.median(values) / profile['rows'] * 1e6:.2f} µs",
                delta,
                buys,
            ]
        )
        per_rep.append(f"- {label}: {formatting.fmt_readings(values)}")
        previous = values
    table = formatting.md_table(
        ["Predicate", "Median", "Per row", "Δ per row against the row above", "What that buys"],
        rows,
    )
    written = "\n".join(
        f"- {label}: `--where '{shapes.predicate_expr(shape)}'`"
        for shape, label, _ in _PREDICATE_ROWS
    )
    return (
        table
        + f"\n\nOne file — the brace-free control, {profile['rows']:,} rows of "
        f"{profile['columns']} columns — read {len(_PREDICATE_ROWS)} ways, warm and "
        "`--schema-mode strings`, through the CLI. Every term is an equality against a "
        "literal no value of the column can equal, so every row is walked, every term is "
        "evaluated, and no row is decoded, built or rendered. Per-row differences are "
        "paired rep by rep and then taken as a median.\n\n"
        + prose._cached_query_note("Each `query`")
        + "\nAs written:\n"
        + written
        + "\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )
