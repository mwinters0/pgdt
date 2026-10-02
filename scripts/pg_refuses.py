#!/usr/bin/env python3
"""The `pg-refuses: I<n>` markers resolved against the invariants they name.

`docs/design/roadmap.md`, "A literal is guaranteed in `*_out`'s form and never
read past `*_in`'s" is the rule: a value this build refuses because PostgreSQL's
input function refuses it carries a `pg-refuses: I<n>` comment at the refusal,
and the invariant `I<n>` lists that site under "Relied on by", so walking
`docs/design/postgres-invariants.md` at a new major finds every refusal the
release may have lifted. This holds the two halves to each other:

* every marker sits in a `.rs` file, where a refusal can be;
* every marker names an entry of `postgres-invariants.md`;
* every marker's site -- the function holding it, or the one it is attached
  to -- is named in that entry's "Relied on by" field, as an identifier inside
  a code span (`` `parse_array` ``, `` `decode::decode_f64` ``).

**The site is the function**, found from the marker's own line: a marker whose
next line past comments and attributes declares a `fn` is attached to that
function, and any other marker sits in the innermost function declared above it
at a shallower indent. A function and not a line, because the record names
functions and a line number would move with every edit above it.

**What counts as a refusal on PostgreSQL's terms**: a value -- a filter's
literal or a dump's field -- refused because the type's `*_in` refuses the
text, so a release widening that function would lift it. Not one: a spelling
refused because this build does not read it (`docs/design/decisions.md`,
"D55", a shortfall); a value our front end cannot hold (`D96`); a declaration
resolved to text. A function porting an `*_in` grammar whole, whose every
refusal is the server's, carries one marker; a function mixing a refusal of the
server's with shortfalls of ours marks the check.

**No operator refusal is one**, because none is made on the server's terms. A
comparison the server lacks is answered where this build can (`json`'s, as text,
`ComparisonDivergence::AsText`), and every operator `predicate.rs` refuses is
this build's own: a type the register models no order for (a `D55` shortfall)
or a server-side function it cannot run (`UnanswerableReason`, whose `=` the
server answers). A release adding an operator therefore lifts none of them, and
a marker on one would name an invariant no release can falsify. A
`ComparisonPlan` variant that is the server's refusal would reopen this.

Usage:

    cd scripts
    uv run pg_refuses.py
    uv run python -m unittest test_pg_refuses
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

import upstream

REPO = Path(__file__).resolve().parent.parent
INVARIANTS = REPO / "docs" / "design" / "postgres-invariants.md"

MARKER_RE = re.compile(r"\bpg-refuses: (I\d+)\b")
HEADING_RE = re.compile(r"^## (I\d+) — ")
RELIED_RE = re.compile(r"^\*\*Relied on by[:.]\*\*")
#: A line ending the "Relied on by" field: the next bold field, a rule, a heading.
FIELD_END_RE = re.compile(r"^(\*\*[A-Z][^*]*[:.]\*\*|---|## )")
CODE_SPAN_RE = re.compile(r"`([^`]+)`")
IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
FN_RE = re.compile(
    r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"[^\"]*\"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
)
COMMENT_RE = re.compile(r"^\s*(//|#\[|#!\[)")
#: Files scanned for markers. The checker and its tests quote the marker.
SKIP = ("scripts/pg_refuses.py", "scripts/test_pg_refuses.py")


@dataclass(frozen=True)
class Marker:
    id: str
    path: str
    line: int
    #: The function the marker marks, or `None` where none was found.
    site: str | None


def relied_on_by(text: str) -> dict[str, set[str]]:
    """Each entry's id to the identifiers its "Relied on by" field's code spans hold."""
    out: dict[str, set[str]] = {}
    current: str | None = None
    field: list[str] | None = None

    def close() -> None:
        if current is not None and field is not None:
            joined = " ".join(field)
            idents = out.setdefault(current, set())
            for span in CODE_SPAN_RE.findall(joined):
                idents.update(IDENT_RE.findall(span))

    for line in text.splitlines():
        m = HEADING_RE.match(line)
        if m:
            close()
            current, field = m.group(1), None
            out.setdefault(current, set())
            continue
        if current is None:
            continue
        if RELIED_RE.match(line):
            close()
            field = [line]
        elif field is not None:
            if FIELD_END_RE.match(line):
                close()
                field = None
            else:
                field.append(line.strip())
    close()
    return out


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip())


def site_of(lines: Sequence[str], index: int) -> str | None:
    """The function the marker on `lines[index]` marks."""
    for line in lines[index + 1:]:
        if not line.strip() or COMMENT_RE.match(line):
            continue
        m = FN_RE.match(line)
        if m:
            return m.group(2)
        break
    depth = _indent(lines[index])
    for line in reversed(lines[:index]):
        if not line.strip() or COMMENT_RE.match(line):
            continue
        m = FN_RE.match(line)
        if m and len(m.group(1)) < depth:
            return m.group(2)
    return None


def markers(repo: Path, paths: Sequence[str]) -> list[Marker]:
    found: list[Marker] = []
    for rel in paths:
        if rel in SKIP or rel.startswith("docs/"):
            continue
        path = repo / rel
        if not path.is_file():
            continue
        try:
            text = path.read_text()
        except UnicodeDecodeError:
            continue
        lines = text.splitlines()
        for n, line in enumerate(lines):
            for m in MARKER_RE.finditer(line):
                site = site_of(lines, n) if rel.endswith(".rs") else None
                found.append(Marker(m.group(1), rel, n + 1, site))
    return found


def reconcile(entries: dict[str, set[str]], found: Sequence[Marker]) -> list[str]:
    problems: list[str] = []
    for m in found:
        where = f"{m.path}:{m.line}"
        if not m.path.endswith(".rs"):
            problems.append(f"{where}: a pg-refuses marker outside Rust source marks no refusal")
        elif m.id not in entries:
            problems.append(f"{where}: marker names {m.id}, which postgres-invariants.md does not carry")
        elif m.site is None:
            problems.append(f"{where}: no function holds the marker, so it names no site")
        elif m.site not in entries[m.id]:
            problems.append(
                f"{where}: {m.id}'s \"Relied on by\" does not name `{m.site}`, the site it marks"
            )
    return problems


def check(repo: Path = REPO, out=sys.stdout) -> int:
    register = repo / INVARIANTS.relative_to(REPO)
    entries = relied_on_by(register.read_text() if register.exists() else "")
    found = markers(repo, upstream.tracked_files(repo))
    problems = reconcile(entries, found)
    for id in sorted({m.id for m in found}, key=lambda i: int(i[1:])):
        sites = sorted({f"{m.path}::{m.site}" for m in found if m.id == id})
        print(f"{id}  {', '.join(sites)}", file=out)
    if problems:
        for p in problems:
            print(f"error: {p}", file=out)
        print(f"pg-refuses: {len(problems)} problem(s)", file=out)
        return 1
    print(f"pg-refuses: {len(found)} marker(s), every one resolved", file=out)
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
