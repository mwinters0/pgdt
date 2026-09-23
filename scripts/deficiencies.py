#!/usr/bin/env python3
"""The deficiency register's reconciliation: `deficiencies.md`'s index against
the code marker that carries each entry's detail.

`docs/process.md` ("Known deficiencies") makes the register an index whose
detail lives elsewhere -- and "elsewhere" is **the code**, in the comment at a
`deficiency: KD<k>` marker on the mechanism itself. A session editing that
mechanism reads the detail without being sent anywhere, which is the whole
point; a paragraph in a design document is a second thing to maintain and the
first to go stale, so no marker may live under `docs/` at all. That buys
locality and pays for it with two places to drift, so the resolution is
mechanical, in both directions and with no discipline in the loop:

* every `KD<k>` in the index resolves to exactly one code marker, in the `.rs`
  file the index names;
* every code marker resolves back to an index entry, so one outliving its
  entry is an error rather than a slow lie;
* no `deficiency: KD<k>` marker appears anywhere under `docs/`, the index
  included -- a detail written there is the decay this rule exists to stop;
* every `(b)` entry is owned by a phase the roadmap's index lists as still
  running, and where that phase has been sliced the entry names a slice of it
  and that slice's checklist line names the entry back;
* every phase carrying a `## P<N> progress` checklist is `Current` in that
  index, and every `Current` phase carries one;
* every `KD<k>` a *ticked* checklist line cites falls inside the allocated
  range, which an `<!-- deficiency-watermark: KD<k> -->` marker carries.

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

**The two directions are asymmetric on purpose.** A checklist line is a record,
so its `KD<k>` is a *citation* and resolves against the allocated range -- it
may name a struck entry, and may not name a number nobody ever allocated. An
index entry is present tense, so it may name only live slices: **an entry naming
a ticked slice is stale by construction**, since partial closure rewrites the
entry in the same change that ticks the box. Making that an error is what
enforces the rewrite mechanically, and the rewrite is the rule most likely to be
skipped -- striking an entry is a visible ceremony, rewriting one because a
single row closed is quiet.

**The allocated range comes from a marker, not from the prose.** The watermark
sentence is rewritten at every strike, and again at the keystone that deletes
the named struck entries, so a regex over that wording is the fragility
`measurements.md` set its figure markers against. Deriving the mark instead as
`max(indexed)` was rejected: it breaks exactly when the highest-numbered entry
is struck, which is when it is needed.

**A `(b)` entry's owner is read from the roadmap's phase index**, because a
completed phase's checklist is deleted from `STATUS.md` at its wrap -- so an
entry owned by it would otherwise revert to "no checklist means not sliced yet"
and go quiet at the moment its pointer became most wrong. `Complete` and
`Struck` are not destinations, and neither is a phase the index does not list:
a stranded entry drops to `(c) unowned` unless a phase actually absorbs it, and
this says so rather than only reporting the contradiction.

**That cell is set by a person, so it is pinned at both ends.** A wrap sets
`Complete` and deletes the checklist; a slicing sets `Current` and writes one.
The check holds the rule that follows -- a phase carrying a `## P<N> progress`
checklist is `Current`, and a `Current` phase carries one -- so a transition
that half happened fails from whichever side it is short: a wrap that dropped
the checklist and left the state leaves `Current` with nothing under it, and a
slicing that wrote the checklist and left the state leaves `Specified` with a
checklist under it. Splitting `Specified` into `Specified` and `Sliced` is the
same rule under a new word, since `Current` already means "sliced and in
flight"; the cost either way is one cell edited at slicing time, which is what
the wrap already pays.

**An unreadable state cell fails the whole run, not just the entries that
resolve against it.** Consulting the index lazily -- parsing it, holding the
problems, and surfacing one only when a `(b)` entry actually lands on a bad row
-- was rejected: a malformed row is a defect in the index whether or not an
entry currently points at it, and a problem that appears only when something
else happens to point at it goes quiet exactly when the register is healthiest.
That is the failure `--stale`'s inert-entry rule was written against. The cost
is accepted knowingly: a roadmap edit that touches no deficiency can fail this
check, and the failure names the expected set so the fix is one word.

**An unrecognised state word is an error, not a guess in either direction.**
Reading it as "still running" fails open on the likeliest mistake, which is the
silence `Complete` was added to end. Reading it as *finished* fails closed and
keeps that property, but it invents a claim about a phase somebody deliberately
marked something else -- worse than refusing to read a word this does not know.
The vocabulary is duplicated in `process.md`'s prose and in `PHASE_STATES`, and
they can drift; that is the same duplication the stance words already accept,
where `(c)` must say "unowned" in that word.

This is `measure.py --check`'s idiom -- the index addresses an entry by a marker
comment, never by a heading, because a heading is rewritten whenever the thing
under it moves -- but it is not a measurement concern and shares nothing with
that harness but the shape.

**The marker is one token, `deficiency: KD<k>`, in a Rust comment**, and the
comment it sits in *is* the detail: a few lines stating the defect, what it
costs, the declared stance and what would close it. No measured numbers (a
`measurements.md` figure id instead, which is re-taken when the number moves)
and no phase history. Exactly one marker per entry, at the mechanism the entry
is about -- so an entry with nowhere to hang a marker is an entry whose
mechanism has not been found.

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

#: The index. Nothing under `docs/` may carry a marker, this file least of all:
#: a detail written here is the decay `process.md` names -- the index has become
#: the document, and the session editing the mechanism will not see it.
REGISTER = REPO / "docs" / "status" / "deficiencies.md"

#: Where the slice checklists are, which a `(b)` entry and its slice pair across.
STATUS = REPO / "docs" / "status" / "STATUS.md"

#: The phase index, which is where a `(b)` entry's owner is resolved: it is the
#: only place that can say a phase ran and finished, since the wrap deletes the
#: checklist that would otherwise stand in for one.
ROADMAP = REPO / "docs" / "design" / "roadmap.md"

#: Where no marker may live. Any Markdown under here is swept and must be clean.
DOC_ROOT = REPO / "docs"

#: Where a code marker may live -- and the only place a detail exists.
CODE_ROOTS = (
    REPO / "pgdump_query" / "src",
    REPO / "pgdt" / "src",
    REPO / "datafusion-pgdump" / "src",
    REPO / "datafusion-cli-pgdump" / "src",
)

SECTION_HEADING = "# Known deficiencies"

#: One token. Deliberately unanchored to any comment syntax, so the sweep over
#: `docs/` finds a stray one however it was written.
MARKER_RE = re.compile(r"deficiency:\s*(KD\d+)")

#: An index entry opens a bullet at column 0 and runs to the next one.
ENTRY_HEAD_RE = re.compile(r"^-\s+\*\*(KD\d+)\*\*\s+—\s*(.*)$")
BULLET_RE = re.compile(r"^-\s")
HEADING_RE = re.compile(r"^#{1,6}\s")

#: A phase's slice checklist. `process.md` fixes both the heading and the item
#: shape: `## P<N> progress`, then one `- [ ] **<N>.<M>**` per slice.
CHECKLIST_HEAD_RE = re.compile(r"^##\s+P(\d+)\s+progress\s*$")
CHECKLIST_ITEM_RE = re.compile(r"^-\s+\[([ xX])\]\s+\*\*(\d+(?:\.\d+)+)\*\*\s*(.*)$")

#: A reference to an entry, anywhere in prose. Bare, not the marker token: a
#: checklist line names an entry the way it names anything else.
REFERENCE_RE = re.compile(r"\bKD\d+\b")

#: The phase in a `(b)` entry's destination -- "P11, struck at 11.6" is P11.
DESTINATION_PHASE_RE = re.compile(r"\bP(\d+)\b")

#: The allocated range, carried by a marker rather than by the sentence that
#: states it, which is rewritten at every strike.
WATERMARK_RE = re.compile(r"<!--\s*deficiency-watermark:\s*(KD\d+)\s*-->")

#: The roadmap's phase index: the table whose header opens `| Phase | State |`.
PHASE_TABLE_HEAD_RE = re.compile(r"^\|\s*Phase\s*\|\s*State\s*\|")

#: A Phase cell names one phase, a comma-separated few, or a range: `P1–P5, P9`.
PHASE_RANGE_RE = re.compile(r"P(\d+)\s*[–—-]\s*P?(\d+)")
PHASE_ONE_RE = re.compile(r"P(\d+)")

#: The states a row may carry, lower-cased. `Complete` is set at the phase wrap,
#: because the index cannot otherwise say "this phase ran and finished" short of
#: striking it -- and a keystone may be years after the wrap.
PHASE_STATES = ("sketched", "specified", "current", "complete", "struck")

#: The two that mean the phase will absorb no more work, so a `(b)` entry naming
#: one names no destination.
FINISHED_STATES = ("complete", "struck")

#: The one that means the phase is sliced and in flight. It is set when the
#: checklist is written, exactly as `Complete` is set when it is deleted, and
#: the two artifacts pair with it: a phase carrying a checklist is this, and a
#: phase that is this carries one.
CURRENT_STATE = "current"


def _index(ident: str) -> int:
    """The numeric half of an identifier, whatever the sigil's length."""
    return int(re.sub(r"\D", "", ident))

#: The three stances, spelled exactly. `(c)` must say "unowned" in that word --
#: the whole point of the stance is that unowned is a resting state a reader can
#: recognise, not an omission.
STANCE_RE = re.compile(r"\*\*\(([abc])\)\s+([^*]+?)\*\*")
OWNED_RE = re.compile(r"^owned by\s+(\S.*)$")

#: `Detail: `pgdump_query/src/io.rs``. A repo-relative path in backticks, not a
#: link: the target is source, and a Markdown link into it resolves nowhere a
#: reader of the rendered index can follow anyway.
DETAIL_RE = re.compile(r"Detail:\s*`([^`]+)`")


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
    #: The `.rs` file the index says carries this entry's marker, repo-relative.
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
    """The lines under `heading`, up to the next heading of any level.

    Returns nothing when the section is absent, which the caller reports --
    a missing section is a register that has been deleted, not an empty one.
    """
    out: list[str] = []
    inside = False
    for line in text.splitlines():
        if HEADING_RE.match(line):
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


def parse_watermark(text: str) -> tuple[str | None, list[str]]:
    """The allocated range's high-water mark, and what is wrong with it.

    One marker, or the register has lost the promise that a spent identifier
    stays resolvable -- which is what a ticked line's citation leans on.
    """
    marks = WATERMARK_RE.findall(text)
    if not marks:
        return None, [
            "no `<!-- deficiency-watermark: KD<k> -->` marker — a ticked line's "
            "`KD<k>` is a citation, and it has nothing to resolve against"
        ]
    if len(marks) > 1:
        return None, [
            f"{len(marks)} deficiency-watermark markers ({', '.join(marks)}) — "
            "the allocated range is one number"
        ]
    return marks[0], []


def phase_numbers(cell: str) -> list[int]:
    """The phases a Phase cell names: `P11`, `P1–P5, P9`, `P1-P5`."""
    out: list[int] = []
    for m in PHASE_RANGE_RE.finditer(cell):
        lo, hi = int(m.group(1)), int(m.group(2))
        if lo <= hi:
            out.extend(range(lo, hi + 1))
    for m in PHASE_ONE_RE.finditer(PHASE_RANGE_RE.sub(" ", cell)):
        out.append(int(m.group(1)))
    return sorted(set(out))


def phase_state(cell: str) -> str:
    """A State cell's state word: `**Specified**; open` is "specified"."""
    plain = re.sub(r"[*`]", " ", cell).strip()
    if not plain:
        return ""
    return re.split(r"[\s;,.]+", plain, maxsplit=1)[0].lower()


def parse_phase_index(text: str) -> tuple[dict[int, str], list[str]]:
    """The roadmap's phase index, as phase number to state word.

    The vocabulary is closed on purpose: a state the check does not know reads
    as "still running" by default, which is exactly the silence a `Complete`
    state was added to end, so an unrecognised word is an error instead.
    """
    problems: list[str] = []
    states: dict[int, str] = {}
    rows: list[str] = []
    inside = False
    for line in text.splitlines():
        if not inside:
            if PHASE_TABLE_HEAD_RE.match(line):
                inside = True
            continue
        if not line.startswith("|"):
            break
        rows.append(line)
    if not inside:
        return {}, [
            "the roadmap carries no `| Phase | State | … |` index table — a (b) "
            "entry's owning phase has nothing to resolve against"
        ]

    for row in rows:
        cells = [c.strip() for c in row.strip().strip("|").split("|")]
        if not cells or not cells[0] or set(cells[0]) <= set("-: "):
            continue
        numbers = phase_numbers(cells[0])
        if not numbers:
            problems.append(
                f"phase index row names no `P<k>`: {cells[0][:60]}"
            )
            continue
        cell = cells[1] if len(cells) > 1 else ""
        state = phase_state(cell)
        if state not in PHASE_STATES:
            problems.append(
                f"phase index row {cells[0]} carries state {cell!r} — expected "
                f"one of {', '.join(PHASE_STATES)}"
            )
            continue
        for n in numbers:
            if n in states:
                problems.append(f"P{n} appears twice in the phase index")
            states[n] = state
    return states, problems


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
    code: Sequence[Marker],
    doc_markers: Sequence[Marker],
    repo: Path,
) -> list[str]:
    """Every problem the register can have, in both directions."""
    problems: list[str] = []
    indexed = {e.id: e for e in entries}

    by_id: dict[str, list[Marker]] = {}
    for m in code:
        by_id.setdefault(m.id, []).append(m)

    for entry in entries:
        found = by_id.get(entry.id, [])
        named = (repo / entry.detail).resolve()
        if not entry.detail.endswith(".rs"):
            problems.append(
                f"{entry.id} names a detail that is not source: {entry.detail} — "
                "the detail is the comment at the code marker"
            )
        elif not named.exists():
            problems.append(
                f"{entry.id} names a detail file that does not exist: {entry.detail}"
            )
        if not found:
            problems.append(
                f"{entry.id} is indexed and no code marker carries its detail — "
                f"expected a `deficiency: {entry.id}` comment in {entry.detail}"
            )
            continue
        if len(found) > 1:
            where = ", ".join(f"{m.path}:{m.line}" for m in found)
            problems.append(
                f"{entry.id} has {len(found)} code markers and must have one: {where}"
            )
        for m in found:
            if (repo / m.path).resolve() != named:
                problems.append(
                    f"{entry.id}'s marker is at {m.path}:{m.line} but the index "
                    f"names {entry.detail}"
                )

    for m in code:
        if m.id not in indexed:
            problems.append(
                f"{m.path}:{m.line} marks {m.id}, which the index does not list"
            )
    for m in doc_markers:
        problems.append(
            f"{m.path}:{m.line} carries a `deficiency: {m.id}` marker — a document "
            "is not where a detail lives; the comment at the code marker is"
        )
    return problems


def reconcile_slices(
    entries: Sequence[Entry],
    checklists: dict[int, list[Slice]],
    phases: dict[int, str],
    watermark: str | None,
) -> list[str]:
    """The pairing between a `(b)` entry and the slice that will close it.

    Both ways, and neither half is redundant. Entry to slice catches the
    renumbering: the entry names an id the checklist no longer lists, one whose
    line has stopped claiming it, or one that has landed. Slice to entry catches
    the other half of a split -- a new slice that takes over the closure without
    the entry being re-aimed at it.

    Ahead of both sits the phase index: a checklist and a `Current` cell are the
    two halves of a phase in flight, so each is held to the other, and a `(b)`
    entry whose phase the index calls finished, or does not list, has no
    destination at all -- the slice pairing beneath it would be noise.
    """
    problems: list[str] = []
    indexed = {e.id: e for e in entries}
    mark = _index(watermark) if watermark else None

    if mark is not None:
        for entry in entries:
            if _index(entry.id) > mark:
                problems.append(
                    f"{entry.id} is indexed above the watermark ({watermark}) — "
                    "the marker is what records that a number is allocated"
                )

    for n in sorted(set(phases) | set(checklists)):
        state = phases.get(n)
        sliced = n in checklists
        if state in FINISHED_STATES and sliced:
            problems.append(
                f'P{n} is {state} in the roadmap\'s phase index and still carries '
                f'a "## P{n} progress" checklist — the wrap deletes the checklist '
                "and sets the state, in one change"
            )
        elif sliced and state is None:
            problems.append(
                f'P{n} carries a "## P{n} progress" checklist and the roadmap\'s '
                "phase index does not list it — a phase carrying a checklist is "
                "current"
            )
        elif sliced and state != CURRENT_STATE:
            problems.append(
                f'P{n} carries a "## P{n} progress" checklist and the roadmap\'s '
                f"phase index calls it {state} — slicing a phase sets the state to "
                "current, in the change that writes the checklist"
            )
        elif state == CURRENT_STATE and not sliced:
            problems.append(
                f"P{n} is current in the roadmap's phase index and carries no "
                f'"## P{n} progress" checklist — slicing a phase writes the '
                "checklist and sets the state, in one change"
            )

    for entry in entries:
        if entry.stance != "b":
            continue
        phase = DESTINATION_PHASE_RE.search(entry.destination)
        if not phase:
            # A destination that is not a phase -- there is no slice list to
            # reconcile against, and inventing one is not this check's job.
            continue
        n = int(phase.group(1))
        state = phases.get(n)
        if state is None:
            problems.append(
                f"{entry.id} is (b) owned by P{n}, which the roadmap's phase "
                "index does not list — the entry drops to (c) unowned unless a "
                "phase actually absorbs it"
            )
            continue
        if state in FINISHED_STATES:
            problems.append(
                f"{entry.id} is (b) owned by P{n}, which is {state} — a finished "
                "phase is not a named destination, so the entry drops to (c) "
                "unowned unless another phase absorbs it"
            )
            continue
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
            if found.done:
                problems.append(
                    f"{entry.id} names slice {ref}, which has landed — closing a "
                    "part rewrites the entry in the change that ticks the box, so "
                    "an entry naming a ticked slice is stale either way"
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
                # named has since been rewritten or struck. So its `KD<k>` is a
                # citation, held only to resolving against the allocated range.
                for ref in entry_refs(item.text):
                    if mark is not None and _index(ref) > mark:
                        problems.append(
                            f"P{n} checklist line {item.id} cites {ref}, which "
                            f"was never allocated — the register is allocated "
                            f"through {watermark}"
                        )
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
    code: Sequence[Marker],
    problems: Sequence[str],
    checklists: dict[int, list[Slice]] | None = None,
    watermark: str | None = None,
    phases: dict[int, str] | None = None,
    out=sys.stdout,
) -> None:
    checklists = checklists or {}
    phases = phases or {}
    by_code: dict[str, list[Marker]] = {}
    for m in code:
        by_code.setdefault(m.id, []).append(m)

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

    by_state: dict[str, list[int]] = {}
    for n, state in sorted(phases.items()):
        by_state.setdefault(state, []).append(n)
    index_summary = "; ".join(
        f"{state} {', '.join(f'P{n}' for n in ns)}" for state, ns in sorted(by_state.items())
    ) or "not read"

    print(
        f"{len(entries)} deficiencies indexed, {len(code)} code markers "
        f"carrying their detail. Sliced phases: {sliced}.",
        file=out,
    )
    print(
        f"Allocated through {watermark or '(no watermark)'}. "
        f"Phase index: {index_summary}.\n",
        file=out,
    )
    for entry in sorted(entries, key=lambda e: _index(e.id)):
        where = ", ".join(f"{m.path}:{m.line}" for m in by_code.get(entry.id, []))
        stance = stance_of(entry)
        print(f"  {entry.id:<4}{stance:<{width}}{where or '(no code marker)'}", file=out)
        if entry.id in paired:
            print(f"        paired with {', '.join(paired[entry.id])}", file=out)
    print(file=out)

    if problems:
        print("The register does not reconcile:", file=out)
        for p in problems:
            print(f"  {p}", file=out)
    else:
        print(
            "Index, code markers, slice pairings and phase states all resolve.",
            file=out,
        )


def check(repo: Path = REPO, out=sys.stdout) -> int:
    register = repo / REGISTER.relative_to(REPO)
    status = repo / STATUS.relative_to(REPO)
    roadmap = repo / ROADMAP.relative_to(REPO)
    doc_root = repo / "docs"
    code_roots = tuple(repo / p.relative_to(REPO) for p in CODE_ROOTS)

    # A missing file parses as a register that is gone, which is what it is.
    text = register.read_text() if register.exists() else ""
    entries, problems = parse_index(text)
    watermark, watermark_problems = parse_watermark(text)
    checklists = parse_checklists(status.read_text()) if status.exists() else {}
    if roadmap.exists():
        phases, phase_problems = parse_phase_index(roadmap.read_text())
    else:
        phases, phase_problems = {}, [
            f"{repo_rel(roadmap, repo)} does not exist — the phase index is "
            "where a (b) entry's owner is resolved"
        ]
    code = markers_under(code_roots, ".rs", repo)
    doc_markers = markers_under((doc_root,), ".md", repo)
    problems = (
        list(problems)
        + watermark_problems
        + phase_problems
        + reconcile(entries, code, doc_markers, repo)
        + reconcile_slices(entries, checklists, phases, watermark)
    )
    report(entries, code, problems, checklists, watermark, phases, out=out)
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
