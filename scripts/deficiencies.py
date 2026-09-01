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
  entry is an error rather than a slow lie;
* every `(b)` entry whose owning phase has been sliced names a slice of it, and
  that slice's checklist line names the entry back.

Failing on *either* half is the point. A one-directional check leaves the other
direction free to rot, which is exactly how a register stops being one.

**The fourth relation is the one whose failure is the feature.** A slice list is
rewritten as a phase is grilled, split and re-sliced, and an entry that named a
slice number quietly starts pointing at work that no longer exists. Nothing in
the pairing detects that on its own -- the entry still reads well, and the slice
that took over the work says nothing. So the re-slice is made to *fail* here
rather than to owe a re-target on discipline: a renumbered slice leaves the
entry naming an id the checklist does not list, and a split that moves the
closure leaves the named slice's line no longer naming the entry.

Two boundaries keep it from firing where there is no obligation:

* **A phase with no `## P<N> progress` checklist is not sliced yet**, so an
  entry owned by it names no slice and is not asked to. The obligation lands
  when the phase is sliced, and it lands automatically -- writing the checklist
  is what turns the check on.
* **A ticked line is a record, not a promise.** A landed slice's line still says
  which entry it closed, and the entry has already been rewritten to what is
  still true (or struck outright), so requiring it to name a landed slice back
  would force one of the two to lie. The reverse direction therefore reads
  unticked lines only.

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

#: A phase's slice checklist. `process.md` fixes both the heading and the item
#: shape: `## P<N> progress`, then one `- [ ] **<N>.<M>**` per slice.
CHECKLIST_HEAD_RE = re.compile(r"^##\s+P(\d+)\s+progress\s*$")
CHECKLIST_ITEM_RE = re.compile(r"^-\s+\[([ xX])\]\s+\*\*(\d+(?:\.\d+)+)\*\*\s*(.*)$")

#: A reference to an entry, anywhere in prose. Bare, not the marker token: a
#: checklist line names an entry the way it names anything else.
REFERENCE_RE = re.compile(r"\bKD\d+\b")

#: The phase in a `(b)` entry's destination -- "P11, struck at 11.6" is P11.
DESTINATION_PHASE_RE = re.compile(r"\bP(\d+)\b")


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


@dataclass(frozen=True)
class Slice:
    """One line of a phase's slice checklist, parsed."""

    #: `11.6`, `11.11.2` -- the identifier, not a position.
    id: str
    #: The phase it belongs to, from the heading above it.
    phase: int
    #: Ticked. A ticked line is a record of what the slice did; an unticked one
    #: is a promise, and only promises are held to the pairing.
    done: bool
    #: The whole item, joined, for reference-hunting and error messages.
    text: str


def slice_refs(text: str, phase: int) -> list[str]:
    """The slices of `phase` that `text` names, in order, deduplicated.

    Scoped to one phase's number on purpose: an unscoped "looks like `<n>.<m>`"
    would read a version, a section number or a figure as a slice. The lookbehind
    keeps `P11.3` and `roadmap-P11.3-…-notes.md` out, which name a doc rather
    than making a claim about the slice list.
    """
    pattern = re.compile(rf"(?<![\w.]){phase}\.\d+(?:\.\d+)?(?!\d)")
    out: list[str] = []
    for m in pattern.finditer(text):
        if m.group(0) not in out:
            out.append(m.group(0))
    return out


def entry_refs(text: str) -> list[str]:
    """The entries `text` names, in order, deduplicated."""
    out: list[str] = []
    for m in REFERENCE_RE.finditer(text):
        if m.group(0) not in out:
            out.append(m.group(0))
    return out


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


def parse_checklists(text: str) -> dict[int, list[Slice]]:
    """Every `## P<N> progress` checklist in the index, by phase.

    A phase absent from the result has not been sliced -- which is the fact the
    fourth relation turns on, so it is read rather than configured. Two phases
    can be in flight at once (`process.md`, "Two phases in flight"), each with
    its own checklist, so this is a mapping and not a single list.
    """
    out: dict[int, list[Slice]] = {}
    phase: int | None = None
    current: list[str] | None = None

    def close() -> None:
        nonlocal current
        if phase is not None and current:
            item = CHECKLIST_ITEM_RE.match(current[0])
            assert item is not None
            out.setdefault(phase, []).append(
                Slice(
                    id=item.group(2),
                    phase=phase,
                    done=item.group(1).strip().lower() == "x",
                    text=" ".join(part.strip() for part in current),
                )
            )
        current = None

    for line in text.splitlines():
        if line.startswith("## "):
            close()
            head = CHECKLIST_HEAD_RE.match(line.strip())
            phase = int(head.group(1)) if head else None
            if phase is not None:
                out.setdefault(phase, [])
            continue
        if phase is None:
            continue
        if CHECKLIST_ITEM_RE.match(line):
            close()
            current = [line]
        elif BULLET_RE.match(line) or not line.strip():
            close()
        elif current is not None:
            current.append(line)
    close()
    return out


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


def reconcile_slices(
    entries: Sequence[Entry], checklists: dict[int, list[Slice]]
) -> list[str]:
    """The pairing between a `(b)` entry and the slice that will close it.

    Both ways, and neither half is redundant. Entry to slice catches the
    renumbering: the entry names an id the checklist no longer lists, or one
    whose line has stopped claiming it. Slice to entry catches the other
    half of a split -- a new slice that takes over the closure without the
    entry being re-aimed at it.
    """
    problems: list[str] = []
    indexed = {e.id: e for e in entries}

    for entry in entries:
        if entry.stance != "b":
            continue
        phase = DESTINATION_PHASE_RE.search(entry.destination)
        if not phase:
            # A destination that is not a phase -- there is no slice list to
            # reconcile against, and inventing one is not this check's job.
            continue
        n = int(phase.group(1))
        named = slice_refs(entry.text, n)
        listed = checklists.get(n)
        if listed is None:
            for ref in named:
                problems.append(
                    f"{entry.id} names slice {ref}, but STATUS carries no "
                    f'"## P{n} progress" checklist to resolve it against'
                )
            continue
        if not named:
            problems.append(
                f"{entry.id} is (b) owned by P{n}, which is sliced, and names no "
                f"slice of it — the entry and the slice that will close it name "
                f"each other"
            )
            continue
        by_id = {s.id: s for s in listed}
        for ref in named:
            found = by_id.get(ref)
            if found is None:
                problems.append(
                    f"{entry.id} names slice {ref}, which the P{n} checklist does "
                    f"not list — a re-slice re-targets every entry pointing at it"
                )
                continue
            if entry.id not in entry_refs(found.text):
                problems.append(
                    f"{entry.id} names slice {ref}, whose checklist line does not "
                    f"name {entry.id} back"
                )

    for n, slices in sorted(checklists.items()):
        for item in slices:
            if item.done:
                # A landed slice's line records what it closed; the entry it
                # named has since been rewritten or struck.
                continue
            for ref in entry_refs(item.text):
                entry = indexed.get(ref)
                if entry is None:
                    problems.append(
                        f"P{n} checklist line {item.id} names {ref}, which the "
                        "index does not list"
                    )
                    continue
                if item.id not in slice_refs(entry.text, n):
                    problems.append(
                        f"P{n} checklist line {item.id} names {ref}, which does "
                        f"not name {item.id} back"
                    )
    return problems


def report(
    entries: Sequence[Entry],
    details: Sequence[Marker],
    code: Sequence[Marker],
    problems: Sequence[str],
    checklists: dict[int, list[Slice]] | None = None,
    out=sys.stdout,
) -> None:
    checklists = checklists or {}
    by_code: dict[str, list[Marker]] = {}
    for m in code:
        by_code.setdefault(m.id, []).append(m)
    by_detail: dict[str, list[Marker]] = {}
    for m in details:
        by_detail.setdefault(m.id, []).append(m)

    paired: dict[str, list[str]] = {}
    for n, slices in sorted(checklists.items()):
        for item in slices:
            for ref in entry_refs(item.text):
                paired.setdefault(ref, []).append(item.id)

    stances = {"a": "deliberate tradeoff", "b": "owned", "c": "unowned"}
    sliced = ", ".join(f"P{n}" for n in sorted(checklists)) or "none"

    def stance_of(entry: Entry) -> str:
        text = f"({entry.stance}) {stances[entry.stance]}"
        return text + (f" by {entry.destination}" if entry.destination else "")

    # A (b) destination that names its slice is long, so the column is measured
    # rather than guessed -- a run-together line reads as a missing field.
    width = max((len(stance_of(e)) for e in entries), default=0) + 2

    print(
        f"{len(entries)} deficiencies indexed, {len(details)} detail entries, "
        f"{len(code)} code markers. Sliced phases: {sliced}.\n",
        file=out,
    )
    for entry in sorted(entries, key=lambda e: _index(e.id)):
        where = ", ".join(f"{m.path}:{m.line}" for m in by_detail.get(entry.id, []))
        stance = stance_of(entry)
        print(f"  {entry.id:<4}{stance:<{width}}{where or '(no detail entry)'}", file=out)
        for m in by_code.get(entry.id, []):
            print(f"        marked at {m.path}:{m.line}", file=out)
        if entry.id in paired:
            print(f"        paired with {', '.join(paired[entry.id])}", file=out)
    print(file=out)

    if problems:
        print("The index and its detail entries disagree:", file=out)
        for p in problems:
            print(f"  {p}", file=out)
    else:
        print(
            "Index, detail entries, code markers and slice pairings all resolve.",
            file=out,
        )


def check(repo: Path = REPO, out=sys.stdout) -> int:
    status = repo / "docs" / "status" / "STATUS.md"
    doc_root = repo / "docs"
    code_roots = tuple(repo / p for p in ("pgdump_query/src", "pgdump_query-cli/src"))

    text = status.read_text()
    entries, problems = parse_index(text)
    checklists = parse_checklists(text)
    details = markers_under((doc_root,), ".md", repo, skip=status)
    code = markers_under(code_roots, ".rs", repo)
    status_markers = markers_in(status, repo)
    problems = (
        list(problems)
        + reconcile(entries, details, code, status_markers, repo, status)
        + reconcile_slices(entries, checklists)
    )
    report(entries, details, code, problems, checklists, out=out)
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
