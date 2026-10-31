#!/usr/bin/env python3
"""The `timescaledb: TS<n>` markers resolved against the invariants they name.

`docs/design/timescaledb-invariants.md` is walked at each TimescaleDB release,
and the walk finds every site a release may break only if each is held to its
entry. A site relying on an entry -- a function in a `.rs` file or in the
fixture generator -- carries a `timescaledb: TS<n>` comment, and the entry's
"Relied on by" field names it. This holds the two halves to each other, both
ways:

* every marker sits in a `.rs` or `.py` file, inside or attached to a function;
* every marker names an entry of `timescaledb-invariants.md`;
* every marker's site is named in that entry's "Relied on by", as a code span
  holding an identifier or a path ending in one (`` `write_answers` ``,
  `` `TimescaleDb::of` ``);
* every such code span in an entry's "Relied on by" is the site of a marker
  naming that entry. A code span holding anything else -- a file, a document,
  a qualified column (`` `chunk.dropped` ``) -- names no site, so the field
  spells what is not a site as no bare identifier.

**The site is the function**, resolved as `pg_refuses.py` resolves its
markers' (`pg_refuses.site_of`), a Python `def` as a Rust `fn`.

Usage:

    cd scripts
    uv run timescaledb_invariants.py
    uv run python -m unittest test_timescaledb_invariants
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

import pg_refuses
import upstream

REPO = Path(__file__).resolve().parent.parent
INVARIANTS = REPO / "docs" / "design" / "timescaledb-invariants.md"

MARKER_RE = re.compile(r"\btimescaledb: (TS\d+)\b")
HEADING_RE = re.compile(r"^## (TS\d+) — ")
#: A code span naming a site: an identifier, or a path ending in one.
SITE_SPAN_RE = re.compile(r"^(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)$")
#: Files scanned for markers. The checker and its tests quote the marker.
SKIP = ("scripts/timescaledb_invariants.py", "scripts/test_timescaledb_invariants.py")
SUFFIXES = (".rs", ".py")


@dataclass(frozen=True)
class Marker:
    id: str
    path: str
    line: int
    #: The function the marker marks, or `None` where none was found.
    site: str | None


def sites(text: str) -> dict[str, set[str]]:
    """Each entry's id to the functions its "Relied on by" field names."""
    out: dict[str, set[str]] = {}
    for id, spans in pg_refuses.relied_on_spans(text, HEADING_RE).items():
        out[id] = {m.group(1) for m in map(SITE_SPAN_RE.match, spans) if m}
    return out


def markers(repo: Path, paths: Sequence[str]) -> list[Marker]:
    found: list[Marker] = []
    for rel in paths:
        if rel in SKIP or not rel.endswith(SUFFIXES):
            continue
        path = repo / rel
        if not path.is_file():
            continue
        lines = path.read_text().splitlines()
        for n, line in enumerate(lines):
            for m in MARKER_RE.finditer(line):
                site = pg_refuses.site_of(lines, n, python=rel.endswith(".py"))
                found.append(Marker(m.group(1), rel, n + 1, site))
    return found


def reconcile(entries: dict[str, set[str]], found: Sequence[Marker]) -> list[str]:
    problems: list[str] = []
    for m in found:
        where = f"{m.path}:{m.line}"
        if m.id not in entries:
            problems.append(f"{where}: marker names {m.id}, which timescaledb-invariants.md does not carry")
        elif m.site is None:
            problems.append(f"{where}: no function holds the marker, so it names no site")
        elif m.site not in entries[m.id]:
            problems.append(f"{where}: {m.id}'s \"Relied on by\" does not name `{m.site}`, the site it marks")
    for id, named in sorted(entries.items()):
        marked = {m.site for m in found if m.id == id}
        for site in sorted(named - marked):
            problems.append(f"{id}'s \"Relied on by\" names `{site}`, which no `timescaledb: {id}` marker marks")
    return problems


def check(repo: Path = REPO, out=sys.stdout) -> int:
    register = repo / INVARIANTS.relative_to(REPO)
    entries = sites(register.read_text() if register.exists() else "")
    found = markers(repo, upstream.tracked_files(repo))
    problems = reconcile(entries, found)
    for id in sorted({m.id for m in found}, key=lambda i: int(i[2:])):
        listed = sorted({f"{m.path}::{m.site}" for m in found if m.id == id})
        print(f"{id}  {', '.join(listed)}", file=out)
    if problems:
        for p in problems:
            print(f"error: {p}", file=out)
        print(f"timescaledb: {len(problems)} problem(s)", file=out)
        return 1
    print(f"timescaledb: {len(found)} marker(s), every one resolved both ways", file=out)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--repo", type=Path, default=REPO, help="repository root (tests use this)")
    args = parser.parse_args(argv)
    return check(args.repo)


if __name__ == "__main__":
    raise SystemExit(main())
