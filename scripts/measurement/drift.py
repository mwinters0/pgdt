"""`--drift` and `--arms`: what two sweeps differ by, measured rather than asserted."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Mapping, Sequence

from measurement import config
from measurement import formatting
from measurement import git
from measurement.figures import register
from measurement.staleness import doc_markers


def drift_table(first: Path, second: Path) -> str:
    """Two sweeps of the same figures, differenced reading by reading.

    The standing rules assert session-to-session drift of "~10%" on the
    strength of history rather than a measurement, and the ninth rule's
    "under ~0.5 µs/row is apparatus" corollary leans on it. Two sweeps taken
    the same day, on identical inputs and binaries, are the only reading of the
    instrument itself this apparatus has ever had."""
    a = json.loads(first.read_text())
    b = json.loads(second.read_text())
    shared = sorted(set(a["readings"]) & set(b["readings"]))
    if not shared:
        raise ValueError("the two sweeps share no reading; they measured different figures")
    rows, deltas = [], []
    for key in shared:
        first_v, second_v = a["readings"][key], b["readings"][key]
        if not first_v or not second_v:
            continue
        m1, m2 = formatting.median(first_v), formatting.median(second_v)
        pct = (m2 - m1) / m1 * 100
        deltas.append(abs(pct))
        figure, _, reading = key.partition("/")
        rows.append([figure, f"`{reading}`", formatting.fmt_s(m1), formatting.fmt_s(m2), f"{pct:+.1f}%"])
    table = formatting.md_table(
        [
            "Figure",
            "Reading",
            f"sweep 1 ({a['date']})",
            f"sweep 2 ({b['date']})",
            "Δ",
        ],
        rows,
    )
    return (
        table
        + f"\n\nBoth sweeps ran against commit `{a['commit']}` and `{b['commit']}` on identical "
        f"inputs and binaries. Over {len(deltas)} shared readings the median absolute drift is "
        f"**{formatting.median(deltas):.1f}%** and the largest is **{max(deltas):.1f}%**.\n"
    )


def _fmt_fine(value: float) -> str:
    """Seconds to the microsecond, the most `TIME_FORMAT` asks for: a sitting
    whose image's bash clamps it to the millisecond prints three zeros after."""
    return f"{value:.6f}"


def relative_spread(values: Sequence[float]) -> float:
    """A leg's spread within one sitting, as a percentage of its median."""
    lo, hi = formatting.spread(values)
    return (hi - lo) / formatting.median(values) * 100


def _arm_readings(raw: dict, which: str) -> tuple[list[str], dict[str, dict[str, list[float]]]]:
    arms = raw.get("arms")
    if not arms or len(arms.get("names", ())) < 2:
        raise ValueError(
            f"{which} took one arm, so there is nothing to set beside it: take it with "
            "`--pin-cpus alternate` or `--stage-binaries alternate`"
        )
    return arms["names"], arms["readings"]


def _shared_keys(*tables: Mapping[str, Sequence[float]]) -> list[str]:
    keys = set(tables[0])
    for table in tables[1:]:
        keys &= set(table)
    return sorted(k for k in keys if all(table[k] for table in tables))


def arms_table(raw: dict) -> str:
    """One sitting's arms set beside each other, reading by reading: each
    arm's median, its spread within the sitting, and each later arm's move
    against the first.

    What it prices is a term both arms' readings share every minute of the
    machine with, which is why `alternate` takes them leg by leg in one
    sitting — the staged cold figure's Δ is what staging removes from a cold
    absolute. What it cannot price is drift, which only `--drift` across two
    sittings reads."""
    names, readings = _arm_readings(raw, "this sitting")
    keys = _shared_keys(*(readings.get(n, {}) for n in names))
    if not keys:
        raise ValueError("the arms share no reading")
    first = names[0]
    rows, wider = [], {n: 0 for n in names[1:]}
    moves: dict[str, list[float]] = {n: [] for n in names[1:]}
    for key in keys:
        figure, _, reading = key.partition("/")
        row = [figure, f"`{reading}`"]
        for n in names:
            vals = readings[n][key]
            lo, hi = formatting.spread(vals)
            row.append(
                f"{_fmt_fine(formatting.median(vals))} ({_fmt_fine(lo)}–{_fmt_fine(hi)}, "
                f"{relative_spread(vals):.1f}%)"
            )
        base = formatting.median(readings[first][key])
        for n in names[1:]:
            pct = (formatting.median(readings[n][key]) - base) / base * 100
            moves[n].append(pct)
            row.append(f"{pct:+.2f}%")
            if relative_spread(readings[n][key]) > relative_spread(readings[first][key]):
                wider[n] += 1
        rows.append(row)
    table = formatting.md_table(
        ["Figure", "Reading", *(f"{n}: median (range, spread)" for n in names),
         *(f"{n} against {first}" for n in names[1:])],
        rows,
    )
    summary = "".join(
        f"\n- **{n}** against **{first}**: median move {formatting.median(moves[n]):+.2f}%, "
        f"range {min(moves[n]):+.2f}% to {max(moves[n]):+.2f}%; spread wider than "
        f"{first}'s on {wider[n]} of {len(keys)} readings."
        for n in names[1:]
    )
    return f"{table}\n\nCommit `{raw['commit']}`, {raw['date']}, {len(keys)} readings.\n{summary}\n"


def arm_drift_table(a: dict, b: dict) -> str:
    """Two sittings' arms, each differenced across the pair as
    `drift_table` differences one.

    **The criterion it is read against is `M178`'s, written before any
    sitting** (`docs/status/history/2026-09-28.md`): pinning is adopted only
    if its median absolute move between sittings is **at most half** the
    unpinned arm's, over the pairs of three sittings, each gap recorded rather
    than set, **and no leg's spread widens** — a pinned leg whose spread within a sitting exceeds
    the unpinned leg's in that same sitting counts against it. Either half
    failing refutes it; the table prints both, and the ratio, rather than
    deciding over pairs it sees one of."""
    names_a, ra = _arm_readings(a, "the first sitting")
    names_b, rb = _arm_readings(b, "the second sitting")
    names = [n for n in names_a if n in names_b]
    if len(names) < 2:
        raise ValueError("the two sittings share fewer than two arms")
    first = names[0]
    rows, drift = [], {}
    for n in names:
        keys = _shared_keys(ra.get(n, {}), rb.get(n, {}))
        deltas = [
            abs((formatting.median(rb[n][k]) - formatting.median(ra[n][k])) / formatting.median(ra[n][k]) * 100) for k in keys
        ]
        if not deltas:
            raise ValueError(f"arm {n} shares no reading across the two sittings")
        drift[n] = formatting.median(deltas)
        rows.append([n, str(len(keys)), f"**{formatting.median(deltas):.2f}%**", f"{max(deltas):.2f}%"])
    table = formatting.md_table(["Arm", "Readings", "Median absolute move", "Largest"], rows)
    lines = []
    for n in names[1:]:
        ratio = drift[n] / drift[first] if drift[first] else float("inf")
        wider = []
        for which, raw in (("sitting 1", ra), ("sitting 2", rb)):
            keys = _shared_keys(raw.get(first, {}), raw.get(n, {}))
            count = sum(
                relative_spread(raw[n][k]) > relative_spread(raw[first][k]) for k in keys
            )
            wider.append(f"{count} of {len(keys)} in {which}")
        lines.append(
            f"- **{n}**: its drift is {ratio:.2f}× **{first}**'s (at most 0.50 is the "
            f"criterion); legs whose spread is wider than {first}'s: {', '.join(wider)} "
            "(none is the criterion)."
        )
    return (
        f"{table}\n\nSittings at `{a['commit']}` ({a['date']}) and `{b['commit']}` "
        f"({b['date']}).\n\n" + "\n".join(lines) + "\n"
    )


def cmd_arms(run_dir: str) -> int:
    print(arms_table(json.loads((Path(run_dir) / "raw.json").read_text())))
    return 0


def cmd_drift(first: str, second: str) -> int:
    fig = register.ALL_BY_ID["session-drift"]
    # A derived figure is computed across two sweeps of one commit, and that
    # commit is usually the stamp's -- the sweep pair is what stamps the doc.
    # Where it is not, this table is published outside the stamp like any
    # other and declares where it came from. A pair whose legs disagree
    # declares nothing: nothing was to be committed between them, so the table
    # is already invalid and its own closing sentence names both commits.
    raws = [json.loads((Path(d) / "raw.json").read_text()) for d in (first, second)]
    commits = {raw["commit"] for raw in raws}
    stamp = doc_markers.stamped_commit(config.REPO / "docs/design/measurements.md")
    taken = commits.pop() if len(commits) == 1 else None
    sitting = (
        taken
        if taken and stamp and git.resolve_commit(taken) != git.resolve_commit(stamp)
        else None
    )
    # Named beside the sitting, as any sitting marker names it, where the two
    # sweeps agree on the register image's glibc — which one sweep pair
    # re-taking one apparatus does unless the image was re-pinned between them.
    glibcs = {raw.get("glibc") for raw in raws}
    glibc = glibcs.pop() if sitting and len(glibcs) == 1 else None
    print(f"## {fig.section}\n")
    print(doc_markers.figure_marker(fig.id, sitting, reproduce="--drift <sweep> <sweep>", glibc=glibc) + "\n")
    print(drift_table(Path(first) / "raw.json", Path(second) / "raw.json"))
    # A pair of experiment sittings is read arm by arm as well; `drift_table`
    # above is the first arm's, which is what every table renders.
    if all(len(raw.get("arms", {}).get("names", ())) > 1 for raw in raws):
        print(arm_drift_table(*raws))
    return 0
