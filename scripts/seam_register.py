#!/usr/bin/env python3
"""The seam register's reconciliation: `docs/status/seam-findings.md`'s entries
against the `seam: S<k>` comments marking each site of a finding.

The register's own header is the rule: an entry is an antipattern a read of
the tree for the `seams` skill found, ruled a defect and left for a later pass,
stated as its symptom, its detail at the markers. What is mechanical is here:

* every entry is one indexed line, `- **S<k>** — *<antipattern>.* <symptom>`,
  wrapped to at most `ENTRY_LINES` lines;
* its antipattern is one the skill's "Antipatterns" names today, so striking
  one from the skill obliges rewriting the entries leaning on it;
* it is not one of the antipatterns another register already holds
  (`ELSEWHERE`);
* every entry resolves to at least one marker in a `.rs` file, and every
  marker to an entry;
* every entry falls inside the allocated range an
  `<!-- seam-watermark: S<k> -->` marker carries, none twice.

Usage:

    cd scripts
    uv run seam_register.py
    uv run python -m unittest test_seam_register
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

REPO = Path(__file__).resolve().parent.parent
REGISTER = REPO / "docs" / "status" / "seam-findings.md"
SKILL = REPO / ".claude" / "skills" / "seams" / "SKILL.md"

#: An entry's index line, wrapped, at most this many lines.
ENTRY_LINES = 4
#: Antipatterns another register holds, and where they go instead.
ELSEWHERE = {
    "One concept, several files": "a **Not in one place** line in docs/design/concepts.md",
    "Sibling cycles": "an OWED or ACCEPTED entry in scripts/module_graph.py",
}

ENTRY_RE = re.compile(r"^- \*\*(S\d+)\*\* — \*([^*]+?)\.\*\s+\S")
ENTRY_START_RE = re.compile(r"^- \*\*S\d")
ANTIPATTERN_RE = re.compile(r"^- \*\*([^*]+?)\.?\*\*")
MARKER_RE = re.compile(r"\bseam: (S\d+)\b")
WATERMARK_RE = re.compile(r"<!--\s*seam-watermark:\s*(S\d+)\s*-->")


@dataclass(frozen=True)
class Entry:
    id: str
    antipattern: str
    lines: int


@dataclass(frozen=True)
class Marker:
    id: str
    path: str
    line: int


def _index(ident: str) -> int:
    return int(ident[1:])


def antipatterns(skill: str) -> list[str]:
    """The names the skill's "### Antipatterns" section gives, in order."""
    names: list[str] = []
    inside = False
    for line in skill.splitlines():
        if line.startswith("#"):
            inside = line.strip() == "### Antipatterns"
            continue
        m = ANTIPATTERN_RE.match(line) if inside else None
        if m:
            names.append(m.group(1))
    return names


def parse(text: str) -> tuple[list[Entry], str | None, list[str]]:
    """The entries, the watermark and what is malformed."""
    problems: list[str] = []
    marks = WATERMARK_RE.findall(text)
    if len(marks) != 1:
        problems.append(f"expected one seam-watermark marker, found {len(marks)}")
    watermark = marks[0] if len(marks) == 1 else None

    entries: list[Entry] = []
    current: tuple[str, str] | None = None
    lines = 0

    def close() -> None:
        if current is not None:
            entries.append(Entry(current[0], current[1], lines))

    for line in text.splitlines():
        if line.startswith("- "):
            close()
            current, lines = None, 0
            m = ENTRY_RE.match(line)
            if m:
                current, lines = (m.group(1), m.group(2)), 1
            elif ENTRY_START_RE.match(line):
                problems.append(f"entry is not `- **S<k>** — *<antipattern>.* <symptom>`: {line!r}")
        elif current is not None and line.startswith("  ") and line.strip():
            lines += 1
        else:
            close()
            current = None
    close()
    return entries, watermark, problems


def check_entries(entries: Sequence[Entry], watermark: str | None, names: Sequence[str]) -> list[str]:
    problems: list[str] = []
    if not names:
        problems.append("the seams skill's \"Antipatterns\" names none")
    seen: set[str] = set()
    for e in entries:
        if e.id in seen:
            problems.append(f"{e.id} appears twice")
        seen.add(e.id)
        if e.lines > ENTRY_LINES:
            problems.append(f"{e.id} runs {e.lines} lines, past {ENTRY_LINES}")
        if e.antipattern in ELSEWHERE:
            problems.append(f"{e.id} is {e.antipattern!r}, which is {ELSEWHERE[e.antipattern]}")
        elif names and e.antipattern not in names:
            problems.append(f"{e.id} names {e.antipattern!r}, which the seams skill does not")
        if watermark is not None and _index(e.id) > _index(watermark):
            problems.append(f"{e.id} is past the watermark {watermark}")
    return problems


def reconcile(entries: Sequence[Entry], markers: Sequence[Marker]) -> list[str]:
    problems: list[str] = []
    ids = {e.id for e in entries}
    for m in markers:
        if m.id not in ids:
            problems.append(f"{m.path}:{m.line}: marker names {m.id}, which the register does not carry")
    marked = {m.id for m in markers}
    for e in entries:
        if e.id not in marked:
            problems.append(f"{e.id} has no `seam: {e.id}` marker at a site")
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
        if not rel.endswith(".rs"):
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
    skill = repo / SKILL.relative_to(REPO)
    entries, watermark, problems = parse(register.read_text() if register.exists() else "")
    names = antipatterns(skill.read_text() if skill.exists() else "")
    found = markers(repo, tracked_files(repo))
    problems = problems + check_entries(entries, watermark, names) + reconcile(entries, found)
    for e in entries:
        sites = sorted({m.path for m in found if m.id == e.id})
        print(f"{e.id}  {e.antipattern}: {', '.join(sites) or 'no site'}", file=out)
    if problems:
        for p in problems:
            print(f"error: {p}", file=out)
        print(f"seam register: {len(problems)} problem(s)", file=out)
        return 1
    print(f"seam register: {len(entries)} entries, every marker resolved", file=out)
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
