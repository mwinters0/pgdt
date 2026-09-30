#!/usr/bin/env python3
"""The upstream register's reconciliation: `docs/status/upstream.md`'s entries
against the `upstream: UF<k>` comments marking each site to revisit.

`.claude/skills/upstream-issue/SKILL.md` is the rule: an entry records a defect or
limit whose fix belongs to a dependency, what we do meanwhile, and what to change
once the fix ships. An upgrade reads the entry and then visits the sites, so the
sites are marked in the code and the resolution is mechanical, both ways:

* every entry carries its six fields, in order -- the issue, upstream, fixed
  when, workaround, watch, when it lands;
* its upstream field links an issue, PR or discussion, or says none was found:
  a blank there cannot be told from nobody having looked;
* every `KD<k>` it names is live in `docs/status/deficiencies.md`, so striking
  a deficiency obliges rewriting the entry that leans on it;
* every entry resolves to at least one marker, and every marker to an entry;
* no marker sits under `docs/`, where it would mark nothing to change;
* every entry falls inside the allocated range an
  `<!-- upstream-watermark: UF<k> -->` marker carries, none twice.

Usage:

    cd scripts
    uv run upstream.py
    uv run python -m unittest test_upstream
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

import deficiencies

REPO = Path(__file__).resolve().parent.parent
REGISTER = REPO / "docs" / "status" / "upstream.md"

#: The fields an entry holds, in order: `- **<field>.** …`.
FIELDS = ("The issue", "Upstream", "Fixed when", "Workaround", "Watch", "When it lands")

HEADING_RE = re.compile(r"^## (UF\d+) — (\S.*)$")
FIELD_RE = re.compile(r"^- \*\*([^*]+?)\.\*\*")
MARKER_RE = re.compile(r"\bupstream: (UF\d+)\b")
WATERMARK_RE = re.compile(r"<!--\s*upstream-watermark:\s*(UF\d+)\s*-->")
LINK_RE = re.compile(r"https?://")
NONE_FOUND_RE = re.compile(r"\bno\b[^.]*\bfound\b", re.IGNORECASE)
#: Files scanned for markers. The checker and its tests quote the marker.
MARKED_SUFFIXES = (".rs", ".py", ".toml", ".sh", ".md")
SKIP = ("scripts/upstream.py", "scripts/test_upstream.py")


@dataclass(frozen=True)
class Entry:
    id: str
    title: str
    #: Field name to its text, joined.
    fields: dict[str, str]
    #: The field names in the order written.
    order: tuple[str, ...]


@dataclass(frozen=True)
class Marker:
    id: str
    path: str
    line: int


def parse(text: str) -> tuple[list[Entry], str | None, list[str]]:
    """The entries, the watermark and what is malformed."""
    problems: list[str] = []
    marks = WATERMARK_RE.findall(text)
    if len(marks) != 1:
        problems.append(f"expected one upstream-watermark marker, found {len(marks)}")
    watermark = marks[0] if len(marks) == 1 else None

    entries: list[Entry] = []
    current: tuple[str, str] | None = None
    fields: dict[str, list[str]] = {}
    order: list[str] = []
    field: str | None = None

    def close() -> None:
        if current is not None:
            joined = {k: " ".join(v) for k, v in fields.items()}
            entries.append(Entry(current[0], current[1], joined, tuple(order)))

    for line in text.splitlines():
        if line.startswith("## "):
            close()
            m = HEADING_RE.match(line)
            if not m:
                problems.append(f"heading is not `## UF<k> — <title>`: {line!r}")
                current = None
            else:
                current = (m.group(1), m.group(2))
            fields, order, field = {}, [], None
            continue
        if current is None:
            continue
        m = FIELD_RE.match(line)
        if m:
            field = m.group(1)
            order.append(field)
            fields[field] = [line[m.end():].strip()]
        elif field is not None and line.strip():
            fields[field].append(line.strip())
    close()
    return entries, watermark, problems


def check_entries(entries: Sequence[Entry], watermark: str | None, live_kds: set[str]) -> list[str]:
    problems: list[str] = []
    seen: set[str] = set()
    for e in entries:
        if e.id in seen:
            problems.append(f"{e.id} appears twice")
        seen.add(e.id)
        if e.order != FIELDS:
            problems.append(
                f"{e.id}'s fields are {list(e.order)}, not {list(FIELDS)} in that order"
            )
        up = e.fields.get("Upstream", "")
        if not LINK_RE.search(up) and not NONE_FOUND_RE.search(up):
            problems.append(
                f"{e.id}'s upstream field neither links an issue, PR or discussion "
                "nor says none was found"
            )
        for kd in deficiencies.entry_refs(" ".join(e.fields.values())):
            if kd not in live_kds:
                problems.append(f"{e.id} names {kd}, which the deficiency register does not carry")
        if watermark is not None and deficiencies._index(e.id) > deficiencies._index(watermark):
            problems.append(f"{e.id} is past the watermark {watermark}")
    return problems


def reconcile(entries: Sequence[Entry], markers: Sequence[Marker]) -> list[str]:
    problems: list[str] = []
    ids = {e.id for e in entries}
    for m in markers:
        where = f"{m.path}:{m.line}"
        if m.path.startswith("docs/"):
            problems.append(f"{where}: an upstream marker under docs/ marks nothing to change")
        elif m.id not in ids:
            problems.append(f"{where}: marker names {m.id}, which the register does not carry")
    marked = {m.id for m in markers if not m.path.startswith("docs/")}
    for e in entries:
        if e.id not in marked:
            problems.append(f"{e.id} has no `upstream: {e.id}` marker at a site to revisit")
    return problems


def tracked_files(repo: Path) -> list[str]:
    """Tracked and untracked-but-not-ignored files, repo-relative."""
    out = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
        cwd=repo, capture_output=True, text=True, check=True,
    ).stdout
    return sorted({p for p in out.splitlines() if p})


def markers(repo: Path, paths: Sequence[str]) -> list[Marker]:
    found: list[Marker] = []
    for rel in paths:
        if rel in SKIP or not rel.endswith(MARKED_SUFFIXES):
            continue
        path = repo / rel
        if not path.is_file():
            continue
        for n, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            for m in MARKER_RE.finditer(line):
                found.append(Marker(m.group(1), rel, n))
    return found


def check(repo: Path = REPO, out=sys.stdout) -> int:
    register = repo / REGISTER.relative_to(REPO)
    kd_register = repo / deficiencies.REGISTER.relative_to(deficiencies.REPO)
    text = register.read_text() if register.exists() else ""
    entries, watermark, problems = parse(text)
    live, _ = deficiencies.parse_index(kd_register.read_text() if kd_register.exists() else "")
    found = markers(repo, tracked_files(repo))
    problems = (
        problems
        + check_entries(entries, watermark, {e.id for e in live})
        + reconcile(entries, found)
    )
    for e in entries:
        sites = sorted({m.path for m in found if m.id == e.id})
        print(f"{e.id}  {len(sites)} site(s)  {e.title}", file=out)
    if problems:
        for p in problems:
            print(f"error: {p}", file=out)
        print(f"upstream: {len(problems)} problem(s)", file=out)
        return 1
    print(f"upstream: {len(entries)} entries, every marker resolved", file=out)
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
