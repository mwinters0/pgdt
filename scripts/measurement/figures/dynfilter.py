"""`dynamic-filter-join` and `dynamic-filter-topk`: DataFusion's dynamic filters through `pgdt sql`."""

from __future__ import annotations

from typing import Mapping
from typing import TYPE_CHECKING

from measurement import formatting
from measurement import runspec
from measurement import shapes

if TYPE_CHECKING:
    from measurement.timing import Session


# -- what DataFusion's dynamic filters buy a query -------------------------


def _dynfilter_specs(figure: str) -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec("dfcli", "dynfilter", command, "warm", command.removeprefix(shapes.DYNFILTER_FAMILY))
        for command in shapes.dynfilter_shapes(figure)
    ]


def dynfilter_problems(figure: str, reported: Mapping[str, Mapping[str, str]]) -> list[str]:
    """Why a dynamic-filter sitting's legs do not price what it claims, keyed
    by `<query>-<leg>`.

    **The legs of a row must return one answer**, byte for byte: they differ
    by a flag and a setting the spec says change no row, so a difference is
    the phase's correctness check failing under the timer, and a table beside
    it would price a wrong answer. An answer with no row proves nothing either
    way."""
    bad = []
    for name in shapes.DYNFILTER_QUERIES[figure]:
        legs = [reported.get(f"{name}-{leg}", {}) for leg in shapes.DYNFILTER_LEGS]
        digests = {leg.get("result_digest") for leg in legs}
        if None in digests or "" in digests or len(digests) != 1:
            bad.append(f"{name}: the legs answered differently, or not at all")
        elif int(legs[0].get("result_rows", "0")) == 0:
            bad.append(f"{name}: the query returned no row")
    return bad

#: What each leg's per-rep line calls it.
DYNFILTER_LEG_NAMES = {
    "off": "filter off",
    "on": "filter on",
    shapes.DYNFILTER_ROWS_LEG: "rows evaluated",
}


def _run_dynfilter(session: Session, figure: str, kind: str) -> str:
    """One dynamic-filter figure: each of `kind`'s queries under each of
    `DYNFILTER_LEGS`, warm, and the `dd` floor."""
    specs = _dynfilter_specs(kind)
    startup = runspec.RunSpec("dfcli", "dynfilter", shapes.DYNFILTER_STARTUP, "warm", "startup")
    session.sweep(figure, [*specs, startup], session.cfg.reps(6))
    answer = session.reported.get(startup.key(figure), {}).get("startup_answer")
    if answer != "1":
        raise RuntimeError(
            f"{figure}: the startup leg answered {answer!r} to `{shapes.STARTUP_SQL}`, so it "
            "timed something other than a startup"
        )
    floor = runspec.RunSpec("none", "dynfilter", "dd", "warm", "dd floor dynfilter")
    session.sweep(figure, [floor], session.cfg.reps(3))
    prefix = f"{shapes.DYNFILTER_FAMILY}{kind}-"
    reported = {
        spec.command.removeprefix(prefix): session.reported.get(spec.key(figure), {})
        for spec in specs
    }
    problems = dynfilter_problems(kind, reported)
    if problems:
        raise RuntimeError(f"{figure} does not price one answer: " + "; ".join(problems))
    rows, per_rep = [], []
    for name, (sql, label) in shapes.DYNFILTER_QUERIES[kind].items():
        walls = {
            leg: session.get(
                figure, runspec.RunSpec("dfcli", "dynfilter", f"{prefix}{name}-{leg}", "warm", "")
            )
            for leg in shapes.DYNFILTER_LEGS
        }
        answer = reported[f"{name}-on"]
        answered = (
            f"{int(answer['result_first']):,}" if kind == "join" else answer["result_rows"]
        )
        rows.append(
            [
                f"{label}: `{sql.removeprefix('SELECT ')}`",
                formatting.fmt_median_spread(walls["off"]),
                formatting.fmt_median_spread(walls["on"]),
                formatting.fmt_delta(formatting.median(walls["off"]), formatting.median(walls["on"])),
                formatting.fmt_median_spread(walls[shapes.DYNFILTER_ROWS_LEG]),
                formatting.fmt_delta(formatting.median(walls["on"]), formatting.median(walls[shapes.DYNFILTER_ROWS_LEG])),
                answered,
            ]
        )
        for leg in shapes.DYNFILTER_LEGS:
            per_rep.append(f"- {label}, {DYNFILTER_LEG_NAMES[leg]}: {formatting.fmt_readings(walls[leg])}")
    started = session.get(figure, startup)
    per_rep.append(f"- startup, `{shapes.STARTUP_SQL}`: {formatting.fmt_readings(started)}")
    per_rep.append(f"- `dd` → `/dev/null`: {formatting.fmt_readings(session.get(figure, floor))}")
    flag = shapes.DYNFILTER_FLAGS[kind]
    table = formatting.md_table(
        [
            "Query",
            "Filter off",
            "Filter on",
            "Δ, on against off",
            "Rows evaluated",
            "Δ, rows against on",
            "Rows the join matched" if kind == "join" else "Rows returned",
        ],
        rows,
    )
    profile = session.stager.profile("dynfilter")
    return (
        table
        + f"\n\nOne file — the control's rows with `u_key` and `bucket` appended, and three "
        f"small build tables, {profile['rows']:,} rows in all — queried warm by "
        f"`pgdt sql -c` at `{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS}`, against a cache one "
        f"untimed `pgdt parse` stating `{shapes.GATHER_STATISTICS}` wrote in the same container, and "
        "the legs of a row answer alike, byte for byte: `Filter off` and `Filter on` differ by "
        f"the producer's flag alone, `{flag}` `false` and `true`, the on leg's filter being "
        "whatever the scan makes of it at the provider's default, and `Rows evaluated` differs "
        f"from `Filter on` by `-c '{shapes.DYNFILTER_ROWS_SQL}'` alone, run ahead of the query in the "
        "same process. Every leg's reading carries the "
        "program's startup — loading it, starting its runtime and registering the dump — "
        f"which a leg answering `{shapes.STARTUP_SQL}` over the same cache, taken in the same "
        f"interleave, reads as {formatting.fmt_median_spread(started)}. "
        "`dd` → `/dev/null` on the same file: "
        f"**{formatting.fmt_s(formatting.median(session.get(figure, floor)))} s**.\n\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


def run_dynamic_filter_join(session: Session) -> str:
    """A selective join on a clustered key and on an unclustered one, and a
    join whose filter rejects nothing, each with DataFusion's join dynamic
    filter off, on, and on with its rows evaluated."""
    return _run_dynfilter(session, "dynamic-filter-join", "join")


def run_dynamic_filter_topk(session: Session) -> str:
    """An `ORDER BY … LIMIT` over an unsorted column, with DataFusion's TopK
    dynamic filter off, on, and on with its rows evaluated."""
    return _run_dynfilter(session, "dynamic-filter-topk", "topk")
