"""`map-only` and `preamble-prepass`: what building the map and reading the preamble alone cost."""

from __future__ import annotations

import os
import subprocess
from typing import TYPE_CHECKING

from measurement import borrowing
from measurement import formatting
from measurement import runspec
from measurement import staging

if TYPE_CHECKING:
    from measurement.timing import Session


# -- the map's own quadratic ------------------------------------------------


def run_map_only(session: Session) -> str:
    figure = "map-only"
    counts = (1000, 2000, 4000)
    specs = [
        runspec.RunSpec("pgdt", f"blocks{n}", "query-nomatch", "warm", f"map only blocks{n}")
        for n in counts
    ]
    session.sweep(figure, specs, session.cfg.reps(3))
    medians = [f"{formatting.fmt_s(formatting.median(session.get(figure, s)))} s" for s in specs]
    table = formatting.md_table(
        ["blocks", *[str(n) for n in counts]], [["map only, no saving", *medians]]
    )
    per_rep = "\n".join(
        f"- {n} blocks: {formatting.fmt_readings(session.get(figure, s))}" for n, s in zip(counts, specs)
    )
    return table + "\n\nPer-rep readings (s):\n" + per_rep + "\n"


# -- the preamble prepass ---------------------------------------------------


def run_preamble_prepass(session: Session) -> str:
    figure = "preamble-prepass"
    preamble = runspec.RunSpec("pgdt", "blocks4000", "parse-preamble", "warm", "preamble-only blocks4000")
    session.sweep(figure, [preamble], session.cfg.reps(5))
    note = borrowing.share_readings(session, figure)
    full_spec = runspec.RunSpec("pgdt", "blocks4000", "parse-cache-out", "warm", "full parse blocks4000")
    if full_spec.key(figure) not in session.readings:
        session.sweep(figure, [full_spec], session.cfg.reps(2))
    full = session.get(figure, full_spec)
    pv = session.get(figure, preamble)
    dump = session.input_path("blocks4000", "warm")
    first_copy = 0
    if dump.exists():
        header = subprocess.run(
            ["grep", "-m1", "-b", "-a", "-E", "^COPY .* FROM stdin;", str(dump)],
            text=True, stdout=subprocess.PIPE, env={**os.environ, "LC_ALL": "C"},
        ).stdout
        first_copy = int(header.split(":", 1)[0]) if ":" in header else 0
    total = staging.file_size(session.cfg, dump, "blocks4000")
    table = formatting.md_table(
        ["", "Wall"],
        [
            ["`parse --preamble-only`, 4000-table dump", f"**{formatting.fmt_s(formatting.median(pv))} s**"],
            ["full `parse` of the same file", f"{formatting.fmt_s(formatting.median(full))} s"],
        ],
    )
    share = (
        f"\n\nThe first `COPY` header sits at byte {first_copy:,} of {total:,} — "
        f"{first_copy / total:.0%} of the file is preamble, which is the most "
        "preamble-heavy shape available here.\n"
    )
    return table + share + note + f"\nPer-rep readings (s): {formatting.fmt_readings(pv)}\n"
