"""`statistics-gathering` and `statistics-pruning`: what gathering statistics costs, and what the
pruning they allow buys."""

from __future__ import annotations

from typing import Mapping
from typing import TYPE_CHECKING

from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


def _statistics_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec(
            "pgdt", name, f"{shapes.STATISTICS_FAMILY}{leg}-rss", "warm", f"{label}, {leg} level"
        )
        for name, label in register._STATISTICS_ROWS
        for leg, _ in shapes.STATISTICS_LEGS
    ]


def run_statistics_gathering(session: Session) -> str:
    """Whole-file `parse` at the data level against the metadata level, warm,
    in one container of the figure's own, with resident recorded beside each
    and not refined."""
    figure = "statistics-gathering"
    specs = _statistics_specs()
    floors = [
        runspec.RunSpec("none", name, "dd", "warm", f"dd floor {name}") for name, _ in register._STATISTICS_ROWS
    ]
    # One input at a time, its floor while it is staged: the figure is split
    # one sweep per input, and no row reads one input against another.
    for (name, _), floor in zip(register._STATISTICS_ROWS, floors):
        session.sweep(figure, [spec for spec in specs if spec.input == name], session.cfg.reps(5))
        session.sweep(figure, [floor], session.cfg.reps(3))
    legs = [leg for leg, _ in shapes.STATISTICS_LEGS]
    rows, per_rep = [], []
    for (name, label), floor in zip(register._STATISTICS_ROWS, floors):
        by_leg = {
            spec.command.removeprefix(shapes.STATISTICS_FAMILY).split("-")[0]: spec
            for spec in specs
            if spec.input == name
        }
        walls = {leg: session.get(figure, by_leg[leg]) for leg in legs}
        rss = {leg: session.get_rss(figure, by_leg[leg]) for leg in legs}
        rows.append(
            [
                label,
                *(formatting.fmt_median_spread(walls[leg]) for leg in legs),
                formatting.fmt_delta(formatting.median(walls["metadata"]), formatting.median(walls["data"])),
                *(formatting.fmt_mib_median_spread(rss[leg]) for leg in legs),
                f"{formatting.fmt_s(formatting.median(session.get(figure, floor)))} s",
            ]
        )
        for leg in legs:
            per_rep.append(
                f"- {label}, {leg} level: {formatting.fmt_readings(walls[leg])}; "
                + ", ".join(formatting.fmt_mib(v) for v in rss[leg])
            )
        per_rep.append(f"- {label}, `dd` → `/dev/null`: {formatting.fmt_readings(session.get(figure, floor))}")
    table = formatting.md_table(
        [
            "Input",
            "Metadata level",
            "Data level",
            "Δ",
            "Peak RSS, metadata",
            "Peak RSS, data",
            "`dd` floor",
        ],
        rows,
    )
    return (
        table
        + f"\n\nEvery run is `pgdt parse` over the whole file at `--jobs {shapes.SWEEP_JOBS}`, "
        f"its level stated as `{shapes.NO_STATISTICS}` or `{shapes.GATHER_STATISTICS}` — the default's "
        "base size, gathered exactly where a flagless `parse` coarsens wide rows — "
        f"**in a {shapes.STATISTICS_MEMORY} container**, against the register's "
        f"{session.cfg.memory}: statistics are billed against the limit's margin "
        '(`docs/design/decisions.md`, "D85"), so the limit is chosen generously that none '
        "declines, and the resident column says what it left. The Δ is the census, the "
        "unrepresentable count and the statistics together; which costs what is read off "
        "a profile of the data level, not off this table. Resident is recorded, not "
        "attributed.\n\nPer-rep readings (s; peak RSS):\n"
        + "\n".join(per_rep)
        + "\n"
    )


def _pruning_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec(
            "pgdt",
            "pruning",
            f"{shapes.PRUNING_FAMILY}{name}-{leg}",
            "warm",
            f"{name}, --statistics {leg}",
        )
        for name in shapes.PRUNING_FILTERS
        for leg in shapes.PRUNING_LEGS
    ]


def pruning_problems(reported: Mapping[str, Mapping[str, str]]) -> list[str]:
    """Why a `statistics-pruning` sitting's legs do not price what it claims,
    keyed by `<filter>-<leg>`.

    **Each is a refusal, not a cell**: an unpruned leg that skipped something is
    not the reference, a pruned leg that skipped nothing prices consulting
    statistics rather than what they buy, and two legs returning different row
    counts are the phase's correctness check failing under the timer.

    **`PRUNING_UNNARROWED` is held to the opposite**, and admitted there alone:
    its pruned leg must say it consulted statistics — the note is printed only
    where it did — and skipped no group and stopped no read, or it prices
    something other than consulting them."""
    bad = []
    for name in shapes.PRUNING_FILTERS:
        none, used = reported.get(f"{name}-none", {}), reported.get(f"{name}-all", {})
        if "skipped_groups" in none or "unread_bytes" in none:
            bad.append(f"{name}: `--statistics none` reported skipping bytes")
        if name != shapes.PRUNING_UNNARROWED:
            if int(used.get("skipped_groups", "0")) == 0:
                bad.append(f"{name}: `--statistics all` skipped no row group")
        elif "skipped_groups" not in used:
            bad.append(f"{name}: `--statistics all` reported consulting no statistics")
        elif int(used["skipped_groups"]) != 0 or "unread_bytes" in used:
            bad.append(f"{name}: `--statistics all` skipped bytes its statistics cannot narrow")
        returned = none.get("rows_returned")
        if returned is None or returned != used.get("rows_returned"):
            bad.append(
                f"{name}: the two legs returned {none.get('rows_returned')} and "
                f"{used.get('rows_returned')} row(s)"
            )
    return bad


def run_statistics_pruning(session: Session) -> str:
    """`pgdt query` under a selective range on a sorted column, under an
    equality a dictionary answers, and under an equality its statistics cannot
    narrow, each with its statistics and with `--statistics none`, warm."""
    figure = "statistics-pruning"
    specs = _pruning_specs()
    session.sweep(figure, specs, session.cfg.reps(6))
    floor = runspec.RunSpec("none", "pruning", "dd", "warm", "dd floor pruning")
    session.sweep(figure, [floor], session.cfg.reps(3))
    reported = {
        spec.command.removeprefix(shapes.PRUNING_FAMILY): session.reported.get(spec.key(figure), {})
        for spec in specs
    }
    problems = pruning_problems(reported)
    if problems:
        raise RuntimeError("statistics-pruning does not price pruning: " + "; ".join(problems))
    rows, per_rep = [], []
    for name, (expr, label) in shapes.PRUNING_FILTERS.items():
        walls = {
            leg: session.get(
                figure, runspec.RunSpec("pgdt", "pruning", f"{shapes.PRUNING_FAMILY}{name}-{leg}", "warm", "")
            )
            for leg in shapes.PRUNING_LEGS
        }
        used = reported[f"{name}-all"]
        unread = int(used["skipped_bytes"]) + int(used.get("unread_bytes", "0"))
        rows.append(
            [
                f"{label}: `{expr}`",
                formatting.fmt_median_spread(walls["none"]),
                formatting.fmt_median_spread(walls["all"]),
                formatting.fmt_delta(formatting.median(walls["none"]), formatting.median(walls["all"])),
                f"**{formatting.median(walls['none']) / formatting.median(walls['all']):.1f}×**",
                f"{int(used['skipped_groups']):,} of {int(used['groups']):,}",
                f"{int(used.get('unread_bytes', '0')):,}",
                f"{unread / int(used['bytes']) * 100:.2f}%",
                f"{int(used['rows_returned']):,}",
            ]
        )
        for leg in shapes.PRUNING_LEGS:
            per_rep.append(f"- {label}, `--statistics {leg}`: {formatting.fmt_readings(walls[leg])}")
    per_rep.append(f"- `dd` → `/dev/null`: {formatting.fmt_readings(session.get(figure, floor))}")
    table = formatting.md_table(
        [
            "Filter",
            "`--statistics none`",
            "Statistics used",
            "Δ",
            "Speedup",
            "Groups skipped",
            "Bytes a stop left unread",
            "Of the rows' bytes, not read",
            "Rows returned",
        ],
        rows,
    )
    profile = session.stager.profile("pruning")
    return (
        table
        + f"\n\nOne file — the control's rows with `v_category` appended, {profile['rows']:,} rows "
        f"of {profile['columns']} columns — queried warm at `--jobs {shapes.SWEEP_JOBS}`, "
        "`--schema-mode typed`, against a cache one untimed `parse` stating "
        f"`{shapes.GATHER_STATISTICS}` wrote in the same container, so the two legs of a row differ "
        "by `--statistics` alone. The first two rows price what pruning buys; the third, "
        "which skips nothing, what consulting the statistics costs a query they cannot "
        "narrow. The cache is decoded whole whatever the query states, so both legs pay "
        "its statistics' decode. The skipped groups and bytes are the query's own notes; the "
        "bytes a stop left unread are a lower bound, and the share not read adds them to the "
        f"skipped groups'. `dd` → `/dev/null` on the same file: "
        f"**{formatting.fmt_s(formatting.median(session.get(figure, floor)))} s**.\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )
