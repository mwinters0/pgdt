#!/usr/bin/env python3
"""Whether the release image's Debian pin is inside the window a release allows.

`docs/design/roadmap-P29-releases.md`, "The Linux libc", is what this
implements: **a new Debian stable gets a grace period of at least six
months**, during which the pin stays on its predecessor, and **twelve months
after Debian's release the pin must have moved**. So, for the codename
`release/Dockerfile`'s `FROM` names:

* a codename Debian has not released, or released under six months ago, is
  refused: the floor moved before the grace period was served;
* a codename whose successor Debian released twelve months ago or more is
  refused: the move is overdue;
* between six and twelve months after the successor's release the pin may
  stay or move, and the day is the maintainer's: reported, not refused;
* otherwise it holds.

Release days are `distro-info-data`'s `debian.csv`, fetched from its upstream
repository at each run, since a copy installed with a distribution is as stale
as the distribution. A source that cannot be read refuses rather than passes:
nothing here guesses a date.

Usage:

    python3 scripts/debian_window.py                  # the Dockerfile's pin, against today
    python3 scripts/debian_window.py --csv debian.csv # a file in place of the fetch
    python3 scripts/debian_window.py --today 2027-03-01
    cd scripts && uv run python -m unittest test_debian_window
"""

from __future__ import annotations

import argparse
import csv
import datetime
import io
import re
import sys
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

import release
from release import ReleaseError

#: `distro-info-data`'s own repository: the one place Debian's release days
#: are recorded and kept current.
CSV_URL = "https://salsa.debian.org/debian/distro-info-data/-/raw/main/debian.csv"
GRACE_MONTHS = 6
LIMIT_MONTHS = 12
BASE_RE = re.compile(r"^debian:([a-z]+)@sha256:[0-9a-f]{64}$")


@dataclass(frozen=True)
class Series:
    """One Debian codename and the day it was released (`None` until it is)."""

    codename: str
    released: datetime.date | None


@dataclass(frozen=True)
class Verdict:
    ok: bool
    message: str


def add_months(day: datetime.date, months: int) -> datetime.date:
    """`day` plus whole calendar months, clamped to the target month's last day."""
    index = day.year * 12 + (day.month - 1) + months
    year, month = divmod(index, 12)
    month += 1
    for d in (day.day, 30, 29, 28):
        try:
            return datetime.date(year, month, d)
        except ValueError:
            continue
    raise AssertionError("unreachable: every month has 28 days")


def parse_csv(text: str) -> list[Series]:
    """Every row's codename and release day, in file order."""
    rows = csv.DictReader(io.StringIO(text))
    if not rows.fieldnames or not {"series", "release"} <= set(rows.fieldnames):
        raise ReleaseError("debian.csv has no `series` and `release` columns")
    series = []
    for row in rows:
        released = (row["release"] or "").strip()
        try:
            day = datetime.date.fromisoformat(released) if released else None
        except ValueError as e:
            raise ReleaseError(f"debian.csv: {row['series']}'s release day {released!r} is not a date") from e
        series.append(Series(row["series"].strip(), day))
    return series


def pinned_codename(dockerfile: str) -> str:
    """The codename of the Dockerfile's one pinned `debian:` base."""
    m = BASE_RE.match(release.pinned_base(dockerfile))
    if not m:
        raise ReleaseError("the Dockerfile's base is not a `debian:<codename>@sha256:…` reference")
    return m.group(1)


def assess(series: Sequence[Series], pin: str, today: datetime.date) -> Verdict:
    """The pin against the window, as the module docstring states it."""
    released = sorted((s for s in series if s.released and s.released <= today), key=lambda s: s.released)
    names = [s.codename for s in released]
    if pin not in names:
        return Verdict(False, f"{pin} is not a Debian release as of {today}: the pin moved too early")
    here = released[names.index(pin)]
    earliest = add_months(here.released, GRACE_MONTHS)
    if today < earliest:
        return Verdict(
            False,
            f"{pin} was released {here.released}: the grace period runs to {earliest}, "
            "so the floor stays on its predecessor until then",
        )
    successors = released[names.index(pin) + 1 :]
    if not successors:
        return Verdict(True, f"{pin} is Debian stable (released {here.released}): the pin holds")
    nxt = successors[0]
    may = add_months(nxt.released, GRACE_MONTHS)
    must = add_months(nxt.released, LIMIT_MONTHS)
    if today >= must:
        return Verdict(
            False,
            f"{nxt.codename} was released {nxt.released}: the pin must have moved to it by {must}",
        )
    if today >= may:
        return Verdict(
            True,
            f"{pin} holds, but its successor {nxt.codename} was released {nxt.released}: "
            f"the pin may move now and must by {must}",
        )
    return Verdict(
        True,
        f"{pin} holds: its successor {nxt.codename} was released {nxt.released}, "
        f"and the grace period runs to {may}",
    )


def fetch(url: str = CSV_URL) -> str:
    """The release-day table, or a refusal naming why it could not be read."""
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            return response.read().decode("utf-8")
    except (OSError, ValueError) as e:
        raise ReleaseError(f"cannot read Debian's release days from {url}: {e}") from e


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="debian_window.py", description=__doc__.split("\n\n")[0])
    p.add_argument("--csv", type=Path, help="a `debian.csv` to read in place of fetching one")
    p.add_argument("--today", type=datetime.date.fromisoformat, help="a day in place of today (YYYY-MM-DD)")
    return p


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        text = args.csv.read_text() if args.csv else fetch()
        verdict = assess(
            parse_csv(text),
            pinned_codename(release.DOCKERFILE.read_text()),
            args.today or datetime.date.today(),
        )
    except (ReleaseError, OSError) as e:
        print(f"debian_window.py: {e}", file=sys.stderr)
        return 2
    print(("ok: " if verdict.ok else "refused: ") + verdict.message)
    return 0 if verdict.ok else 1


if __name__ == "__main__":
    sys.exit(main())
