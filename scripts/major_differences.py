#!/usr/bin/env python3
"""The `VD<n>` register held to its rules: differences nothing depends on.

`docs/design/postgres-major-differences.md` holds the differences between
PostgreSQL majors that no decision here depends on yet. A difference a decision
does depend on is an invariant instead, the entry moving there in the change
that makes the decision. This holds the register to that:

* every entry is headed `## VD<n> — <title>`, its number unique and at or
  below the `<!-- difference-watermark: VD<k> -->` marker, so a struck
  number is never reissued;
* every entry carries its **Claim**, **Proof**, **Observed** and
  **Re-verify** fields;
* **no `VD<n>` is cited outside the register**, a dated history entry, or
  this checker and its tests. A citation from code, a `D<k>` entry, an
  invariant, the manual or a spec is something depending on the difference,
  which is what makes it an invariant; the entry moves there instead.

Usage:

    cd scripts
    uv run major_differences.py
    uv run python -m unittest test_major_differences
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence

import upstream

REPO = Path(__file__).resolve().parent.parent
REGISTER = "docs/design/postgres-major-differences.md"

HEADING_RE = re.compile(r"^## (VD(\d+)) — \S")
LOOSE_HEADING_RE = re.compile(r"^#+ .*\bVD\d+\b")
WATERMARK_RE = re.compile(r"<!-- difference-watermark: VD(\d+) -->")
CITATION_RE = re.compile(r"\bVD\d+\b")
FIELDS = ("Claim", "Proof", "Observed", "Re-verify")
#: Where a `VD<n>` may be named without depending on it.
ALLOWED = (
    REGISTER,
    "scripts/major_differences.py",
    "scripts/test_major_differences.py",
)
ALLOWED_PREFIXES = ("docs/status/history/",)


def entries(text: str) -> tuple[dict[str, set[str]], int | None, list[str]]:
    """Each entry's id to the bold fields it carries, the watermark, and
    the problems found reading them."""
    found: dict[str, set[str]] = {}
    problems: list[str] = []
    marks = [int(m) for m in WATERMARK_RE.findall(text)]
    if len(marks) > 1:
        problems.append(f"{REGISTER}: {len(marks)} watermarks, where one is the allocation authority")
    current: str | None = None
    for n, line in enumerate(text.splitlines(), 1):
        m = HEADING_RE.match(line)
        if m:
            current = m.group(1)
            if current in found:
                problems.append(f"{REGISTER}:{n}: {current} is headed twice")
            found[current] = set()
            continue
        if LOOSE_HEADING_RE.match(line):
            problems.append(f"{REGISTER}:{n}: a heading naming a VD is not `## VD<n> — <title>`")
            current = None
            continue
        if line.startswith("#"):
            current = None
            continue
        if current is not None:
            for field in FIELDS:
                if line.startswith(f"**{field}.**") or line.startswith(f"**{field}:**"):
                    found[current].add(field)
    return found, (marks[0] if marks else None), problems


def citations(repo: Path, paths: Sequence[str]) -> list[str]:
    """Every `VD<n>` named outside the places allowed to name one."""
    out: list[str] = []
    for rel in paths:
        if rel in ALLOWED or rel.startswith(ALLOWED_PREFIXES):
            continue
        path = repo / rel
        if not path.is_file():
            continue
        try:
            text = path.read_text()
        except UnicodeDecodeError:
            continue
        for n, line in enumerate(text.splitlines(), 1):
            for m in CITATION_RE.finditer(line):
                out.append(f"{rel}:{n}: cites {m.group(0)}, so something depends on it; "
                           "make it an invariant and strike the entry")
    return out


def check(repo: Path = REPO, out=sys.stdout) -> int:
    register = repo / REGISTER
    problems: list[str] = []
    if not register.exists():
        problems.append(f"{REGISTER} is missing")
        found, watermark = {}, None
    else:
        found, watermark, problems = entries(register.read_text())
        if watermark is None:
            problems.append(f"{REGISTER}: no `difference-watermark` marker")
        for id, fields in found.items():
            missing = [f for f in FIELDS if f not in fields]
            if missing:
                problems.append(f"{REGISTER}: {id} lacks {', '.join(missing)}")
            if watermark is not None and int(id[2:]) > watermark:
                problems.append(f"{REGISTER}: {id} is past the watermark, VD{watermark}")
    problems += citations(repo, upstream.tracked_files(repo))
    if problems:
        for p in problems:
            print(f"error: {p}", file=out)
        print(f"major-differences: {len(problems)} problem(s)", file=out)
        return 1
    print(f"major-differences: {len(found)} entr{'y' if len(found) == 1 else 'ies'}, "
          f"none cited outside the register", file=out)
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
