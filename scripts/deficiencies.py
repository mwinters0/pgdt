#!/usr/bin/env python3
"""The deficiency register's reconciliation: STATUS.md's index against the
detail paragraphs beside each mechanism, and against the markers in the source.

`docs/process.md` ("Known deficiencies") makes the register an index whose
detail lives elsewhere -- beside the mechanism, where CLAUDE.md's read-triggers
already send a session that is about to touch it. That buys locality and pays
for it with a second place to drift, and an index that has drifted from its
detail is worse than either alone. So the resolution is mechanical, in both
directions and with no discipline in the loop:

* every `KD<k>` in the index resolves to exactly one detail paragraph, in the
  file the index names;
* every detail paragraph resolves back to an index entry;
* every source-code marker resolves to an index entry, so one outliving its
  entry is an error rather than a slow lie.

Failing on *either* half is the point. A one-directional check leaves the other
direction free to rot, which is exactly how a register stops being one.

This is `measure.py --check`'s idiom -- the doc addresses an entry by a marker
comment, never by a heading, because a heading is rewritten whenever the thing
under it moves -- but it is not a measurement concern and shares nothing with
that harness but the shape.

**The marker is one token, `deficiency: KD<k>`, in both file kinds.** In Markdown
it goes in an HTML comment (`<!-- deficiency: KD3 -->`) beside the paragraph; in
Rust it goes in the doc comment of the item that would otherwise mislead. A code
marker is *not* wanted per entry -- only where a line reads as a complete,
deliberate choice and gives no sign that a limitation hangs off it. Most
entries are visible in their own doc section and need none.

Usage:

    cd scripts
    uv run deficiencies.py
    uv run python -m unittest test_deficiencies
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

REPO = Path(__file__).resolve().parent.parent

#: The index. Nothing else in the tree may carry a marker: a detail paragraph
#: written here is the decay `process.md` names -- the index has become the
#: document, and the session editing the mechanism will not see it.
STATUS = REPO / "docs" / "status" / "STATUS.md"

#: Where a detail paragraph may live. Any Markdown under here.
DOC_ROOT = REPO / "docs"

#: Where a code marker may live.
CODE_ROOTS = (REPO / "pgdump_query" / "src", REPO / "pgdump_query-cli" / "src")

SECTION_HEADING = "## Known deficiencies"

#: One token, both file kinds. Deliberately not anchored to `<!--`, so the Rust
#: comments and the Markdown ones are found by the same rule.
MARKER_RE = re.compile(r"deficiency:\s*(KD\d+)")

#: An index entry opens a bullet at column 0 and runs to the next one.
ENTRY_HEAD_RE = re.compile(r"^-\s+\*\*(KD\d+)\*\*\s+—\s*(.*)$")
BULLET_RE = re.compile(r"^-\s")


def _index(ident: str) -> int:
    """The numeric half of an identifier, whatever the sigil's length."""
    return int(re.sub(r"\D", "", ident))

#: The three stances, spelled exactly. `(c)` must say "unowned" in that word --
#: the whole point of the stance is that unowned is a resting state a reader can
#: recognise, not an omission.
STANCE_RE = re.compile(r"\*\*\(([abc])\)\s+([^*]+?)\*\*")
OWNED_RE = re.compile(r"^owned by\s+(\S.*)$")
DETAIL_RE = re.compile(r"Detail:\s*\[[^\]]*\]\(([^)]+)\)")


@dataclass(frozen=True)
class Entry:
    """One line of the index, parsed."""

    id: str
    #: "a", "b" or "c".
    stance: str
    #: The stance's own words: "deliberate tradeoff", "owned by P7", "unowned".
    label: str
    #: For (b), who is going to fix it. Empty otherwise.
    destination: str
    #: The file the index says the detail paragraph is in, repo-relative.
    detail: str
    #: The whole bullet, joined, for error messages.
    text: str


@dataclass(frozen=True)
class Marker:
    """One `deficiency: KD<k>` occurrence."""

    id: str
    #: Repo-relative.
    path: str
    line: int


def repo_rel(path: Path, repo: Path) -> str:
    try:
        return str(path.relative_to(repo))
    except ValueError:
        return str(path)


# --------------------------------------------------------------------------
# Parsing
# --------------------------------------------------------------------------


def section_lines(text: str, heading: str = SECTION_HEADING) -> list[str]:
    """The lines under `heading`, up to the next heading of the same level.

    Returns nothing when the section is absent, which the caller reports --
    a missing section is a register that has been deleted, not an empty one.
    """
    out: list[str] = []
    inside = False
    for line in text.splitlines():
        if line.startswith("## "):
            if inside:
                break
            inside = line.strip() == heading
            continue
        if inside:
            out.append(line)
    return out


def bullets(lines: Iterable[str]) -> list[list[str]]:
    """Split a section's lines into top-level bullets, each with its wrapped
    continuation lines."""
    out: list[list[str]] = []
    for line in lines:
        if BULLET_RE.match(line):
            out.append([line])
        elif out and line.strip():
            out[-1].append(line)
        elif line.strip() == "":
            # A blank line ends a bullet without starting one; the next
            # non-bullet paragraph is prose, not a continuation.
            if out:
                out.append([])
                out.pop()
    return [b for b in out if b]


def parse_index(text: str) -> tuple[list[Entry], list[str]]:
    """The index entries, and what is wrong with the ones that do not parse."""
    problems: list[str] = []
    lines = section_lines(text)
    if not lines:
        return [], [f'no "{SECTION_HEADING}" section — the register is gone']

    entries: list[Entry] = []
    seen: dict[str, int] = {}
    for bullet in bullets(lines):
        joined = " ".join(part.strip() for part in bullet)
        head = ENTRY_HEAD_RE.match(bullet[0])
        if not head:
            problems.append(
                f"entry does not open `- **KD<k>** — `: {bullet[0].strip()[:70]}"
            )
            continue
        did = head.group(1)
        seen[did] = seen.get(did, 0) + 1

        stance = STANCE_RE.search(joined)
        if not stance:
            problems.append(
                f"{did} declares no stance — expected one of "
                "`**(a) deliberate tradeoff**`, `**(b) owned by <destination>**`, "
                "`**(c) unowned**`"
            )
            continue
        kind, label = stance.group(1), stance.group(2).strip()

        destination = ""
        if kind == "a" and not label.startswith("deliberate tradeoff"):
            problems.append(f'{did} is stance (a) but does not say "deliberate tradeoff"')
        if kind == "b":
            owned = OWNED_RE.match(label)
            if not owned:
                problems.append(
                    f"{did} is stance (b) and names no destination — "
                    "expected `**(b) owned by <destination>**`"
                )
            else:
                destination = owned.group(1).strip()
        if kind == "c" and label != "unowned":
            problems.append(
                f'{did} is stance (c) but does not say "unowned" in that word '
                f"(it says {label!r})"
            )

        detail = DETAIL_RE.search(joined)
        if not detail:
            problems.append(
                f"{did} names no detail entry — expected `Detail: [<name>](<path>)`"
            )
            continue

        entries.append(
            Entry(
                id=did,
                stance=kind,
                label=label,
                destination=destination,
                detail=detail.group(1).strip(),
                text=joined,
            )
        )

    for did, count in sorted(seen.items()):
        if count > 1:
            problems.append(f"{did} is indexed {count} times — an identifier is one entry")
    return entries, problems


def markers_in(path: Path, repo: Path) -> list[Marker]:
    rel = repo_rel(path, repo)
    out: list[Marker] = []
    for n, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
        for m in MARKER_RE.finditer(line):
            out.append(Marker(id=m.group(1), path=rel, line=n))
    return out


def markers_under(roots: Sequence[Path], suffix: str, repo: Path, skip: Path | None = None):
    out: list[Marker] = []
    for root in roots:
        if not root.exists():
            continue
        for path in sorted(root.rglob(f"*{suffix}")):
            if skip is not None and path == skip:
                continue
            out.extend(markers_in(path, repo))
    return out


# --------------------------------------------------------------------------
# Reconciliation
# --------------------------------------------------------------------------


def reconcile(
    entries: Sequence[Entry],
    details: Sequence[Marker],
    code: Sequence[Marker],
    status_markers: Sequence[Marker],
    repo: Path,
    status: Path,
) -> list[str]:
    """Every problem the register can have, in both directions."""
    problems: list[str] = []
    indexed = {e.id: e for e in entries}

    by_id: dict[str, list[Marker]] = {}
    for m in details:
        by_id.setdefault(m.id, []).append(m)

    for entry in entries:
        found = by_id.get(entry.id, [])
        named = (status.parent / entry.detail).resolve()
        if not named.exists():
            problems.append(
                f"{entry.id} names a detail file that does not exist: {entry.detail}"
            )
        if not found:
            problems.append(
                f"{entry.id} is indexed and nothing carries its detail — "
                f"expected `<!-- deficiency: {entry.id} -->` in {entry.detail}"
            )
            continue
        if len(found) > 1:
            where = ", ".join(f"{m.path}:{m.line}" for m in found)
            problems.append(
                f"{entry.id} has {len(found)} detail entries and must have one: {where}"
            )
        for m in found:
            if (repo / m.path).resolve() != named:
                problems.append(
                    f"{entry.id}'s detail is at {m.path}:{m.line} but the index "
                    f"names {entry.detail}"
                )

    for m in details:
        if m.id not in indexed:
            problems.append(
                f"{m.path}:{m.line} carries a detail entry for {m.id}, which the "
                "index does not list"
            )
    for m in code:
        if m.id not in indexed:
            problems.append(
                f"{m.path}:{m.line} marks {m.id}, which the index does not list"
            )
    for m in status_markers:
        problems.append(
            f"{m.path}:{m.line} carries a `deficiency: {m.id}` marker — the index "
            "is not where a detail entry lives"
        )
    return problems


def report(
    entries: Sequence[Entry],
    details: Sequence[Marker],
    code: Sequence[Marker],
    problems: Sequence[str],
    out=sys.stdout,
) -> None:
    by_code: dict[str, list[Marker]] = {}
    for m in code:
        by_code.setdefault(m.id, []).append(m)
    by_detail: dict[str, list[Marker]] = {}
    for m in details:
        by_detail.setdefault(m.id, []).append(m)

    stances = {"a": "deliberate tradeoff", "b": "owned", "c": "unowned"}
    print(
        f"{len(entries)} deficiencies indexed, {len(details)} detail entries, "
        f"{len(code)} code markers.\n",
        file=out,
    )
    for entry in sorted(entries, key=lambda e: _index(e.id)):
        stance = f"({entry.stance}) {stances[entry.stance]}"
        if entry.destination:
            stance += f" by {entry.destination}"
        where = ", ".join(f"{m.path}:{m.line}" for m in by_detail.get(entry.id, []))
        print(f"  {entry.id:<4}{stance:<28}{where or '(no detail entry)'}", file=out)
        for m in by_code.get(entry.id, []):
            print(f"        marked at {m.path}:{m.line}", file=out)
    print(file=out)

    if problems:
        print("The index and its detail entries disagree:", file=out)
        for p in problems:
            print(f"  {p}", file=out)
    else:
        print("Index, detail entries and code markers all resolve.", file=out)


def check(repo: Path = REPO, out=sys.stdout) -> int:
    status = repo / "docs" / "status" / "STATUS.md"
    doc_root = repo / "docs"
    code_roots = tuple(repo / p for p in ("pgdump_query/src", "pgdump_query-cli/src"))

    entries, problems = parse_index(status.read_text())
    details = markers_under((doc_root,), ".md", repo, skip=status)
    code = markers_under(code_roots, ".rs", repo)
    status_markers = markers_in(status, repo)
    problems = list(problems) + reconcile(
        entries, details, code, status_markers, repo, status
    )
    report(entries, details, code, problems, out=out)
    return 1 if problems else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo", type=Path, default=REPO, help="repository root (tests use this)"
    )
    args = parser.parse_args(argv)
    return check(args.repo)


if __name__ == "__main__":
    raise SystemExit(main())
