"""The document's markers: each figure's, each sitting's stamp and glibc, each outside section's,
and the headings a section label is held to."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Callable

from measurement import borrowing
from measurement import git
from measurement.figures import register
from measurement.staleness import outside_sections


#: The stamp is prose and prose wraps, so this spans lines — with a bounded
#: lazy run rather than an open one, so it cannot leap from the word
#: "measure.py" in one paragraph to a commit hash pages later.
STAMP_RE = re.compile(
    r"measure\.py.{0,200}?commit `([0-9a-f]{7,40})`", re.IGNORECASE | re.DOTALL
)

#: How the doc names a figure. A heading is free to quote a number and free to
#: change when the number moves -- so the harness must not address a section by
#: its title. The marker is the id, and it is what `--check` reconciles.
MARKER_RE = re.compile(r"<!--\s*figure:\s*([a-z0-9-]+)")


def markers_in(doc: Path) -> list[str]:
    """Every figure id `measurements.md` claims to carry, in order."""
    return MARKER_RE.findall(doc.read_text())

#: How a figure says it was taken **outside** the sitting the session stamp
#: names. The commit goes *inside* that figure's own marker rather than in a
#: sibling comment: a sibling can go missing on its own and its absence is
#: silent, and a figure missing one reads as stamp-sitting, which is the single
#: error this mechanism exists to make impossible. It is present only where the
#: sitting differs from the stamp -- the convention `outside-register` already
#: uses, where declaring nothing is the ordinary case -- so the stamp's commit
#: is never repeated once per figure in a document it could disagree with.
SITTING_RE = re.compile(r"<!--\s*figure:\s*([a-z0-9-]+)[^>]*?taken at `([0-9a-f]{7,40})`")


def figure_marker(
    fid: str,
    sitting: str | None = None,
    reproduce: str | None = None,
    glibc: str | None = None,
) -> str:
    """The marker a table is emitted under: the figure's id, the commit it was
    taken at and the glibc it ran under where the stamp does not speak for
    them, and how to reproduce it.

    Written in one place so that what `emit` puts above a table is by
    construction what `markers_in`, `figure_sittings` and `marker_glibcs` read
    back out of the doc after the paste."""
    reproduce = reproduce or f"--figure {fid}"
    taken = f" — taken at `{sitting}`" if sitting else ""
    under = f" — under glibc {glibc}" if glibc else ""
    return (
        f"<!-- figure: {fid}{taken}{under} — reproduce with "
        f"`cd scripts && uv run measure.py {reproduce}` -->"
    )

#: A glibc version as `glibc_named` writes one, and the two places it is read
#: back from: a figure's marker, and the session stamp's clause after its
#: commit — bounded, and barred from a full stop, so it cannot run on into the
#: accounting sentence or the next paragraph.
_GLIBC_VERSION = r"(\d+(?:\.\d+)+(?: and \d+(?:\.\d+)+)*)"

MARKER_GLIBC_RE = re.compile(rf"<!--\s*figure:\s*([a-z0-9-]+)[^>]*?under glibc {_GLIBC_VERSION}")

STAMP_GLIBC_RE = re.compile(
    rf"measure\.py.{{0,200}}?commit `[0-9a-f]{{7,40}}`[^.]{{0,120}}?glibc {_GLIBC_VERSION}",
    re.IGNORECASE | re.DOTALL,
)


def marker_glibcs(text: str) -> dict[str, str]:
    """Each figure whose marker names the glibc it ran under, and that glibc."""
    return dict(MARKER_GLIBC_RE.findall(text))


def stamp_glibc(text: str) -> str | None:
    """The glibc the session stamp names, read out of the doc's text."""
    match = STAMP_GLIBC_RE.search(text)
    return match.group(1) if match else None


def marker_glibc(whole_sweep: bool, stamped: str | None, ran_under: str | None) -> str | None:
    """The glibc a figure's marker names: its own, wherever the stamp does not
    speak for it.

    The stamp names the register image's, so a figure of the sweep whose
    program ran there names nothing — `taken at`'s convention, the datum
    present only where it differs. It differs for a program run in an image
    answering another glibc, and for every figure of a sitting of its own,
    whose marker already names the commit the stamp does not and names the
    glibc beside it."""
    if ran_under is None or (whole_sweep and ran_under == stamped):
        return None
    return ran_under


def glibc_problems(text: str) -> list[str]:
    """Where the doc does not say which glibc a figure ran under, one line each:
    a stamp naming none, or a sitting marker naming none. A figure of the
    sweep run outside the register's image is the harness's to mark, and is not
    visible from the doc.

    **A stamp or sitting older than `glibc_of` is not exempt.** The ones
    published before it name a glibc read off the image records and
    `pacman.log` (`docs/status/history/2026-09-27.md`, "`M174`: the images are
    pinned, and each figure names its glibc"), which is as certain as asking: a
    digest is immutable and each tag's record is unmoved since before its
    sitting. The next sitting of each replaces them, so an exemption would be a
    cutoff kept here for a state the next sweep ends. A `--render` of one of
    those sittings names no glibc, its record never having held one, so this
    refuses the paste until the values are restored from that entry."""
    out = []
    if stamp_in(text) is not None and stamp_glibc(text) is None:
        out.append(
            "the session stamp names no glibc, so nothing says which one the figures it "
            "covers ran under"
        )
    named = marker_glibcs(text)
    for fid, sha in sorted(figure_sittings(text).items()):
        if fid not in named:
            out.append(
                f"{fid} declares a sitting of its own ({sha}) and names no glibc — write "
                f"`under glibc <version>` inside its marker"
            )
    return out


def figure_sittings(text: str) -> dict[str, str]:
    """Each figure the doc carries from a sitting of its own, and its commit.

    This is the datum every reader of the session stamp argues from: `--stale`
    ranges each figure from here, an acknowledgement is spent per figure
    against it, and `--verify-additive` regenerates a figure's inputs at it.
    Half-applying that -- one reader still reasoning from a commit the doc
    itself says is not the figure's -- is the same defect one level down."""
    return dict(SITTING_RE.findall(text))

#: How a section says it is *outside* the register. The symmetry with
#: `MARKER_RE` is the point: a figure announces itself and so does a section
#: that is not one, so "is this table one of ours" is read off the doc rather
#: than off a prose sentence some paragraphs away (`measurements.md`, "The
#: apparatus").
OUTSIDE_RE = re.compile(r"<!--\s*outside-register:\s*([a-z0-9-]+)")

#: And how it says when its readings were taken, which is what gives `--stale`
#: a range to argue a declared section's `depends` over. Same shape as
#: `SITTING_RE` and for the same reasons: inside the section's own marker, so
#: it cannot go missing on its own, and in the document rather than in this
#: file, which `session-drift` declares.
OUTSIDE_SITTING_RE = re.compile(
    r"<!--\s*outside-register:\s*([a-z0-9-]+)[^>]*?taken at `([0-9a-f]{7,40})`"
)


def outside_sittings(text: str) -> dict[str, str]:
    """Each declared section that names the commit its readings were taken at.

    Absent for a section that publishes no reading — `benches` — where there is
    no run to date and nothing for a range to be measured from."""
    return dict(OUTSIDE_SITTING_RE.findall(text))


def outside_bases(text: str, override: str | None = None) -> dict[str, str | None]:
    """The commit each declared section's readings are argued from.

    Only the sections that declare an invalidation edge: one that declares none
    publishes nothing a diff can falsify, so it has no staleness to have a base
    for. `override` is `--since`, which asks one question of the whole document
    and so is not per section — the same contract `figure_bases` has."""
    declared = outside_sittings(text)
    return {
        o.id: (override or declared.get(o.id)) for o in outside_sections.NOT_OURS.values() if o.depends
    }


def outside_sitting_problems(
    text: str,
    resolve: Callable[[str], str | None] = git.resolve_commit,
) -> list[str]:
    """Why a declared section's provenance line may not stand, one line each.

    The two halves of the field pair have to agree, and neither failure is
    visible from the document alone. A section declaring paths but no commit
    reads as a live edge and is inert — `--stale` has no range to intersect,
    which is the state koji was in before it declared anything, arrived at from
    the other side. A section declaring a commit but no paths asserts a
    provenance nothing reads, which is how a field stops being maintained."""
    declared = outside_sittings(text)
    out: list[str] = []
    for oid, outside in sorted(outside_sections.NOT_OURS.items()):
        sha = declared.get(oid)
        if outside.depends and sha is None:
            out.append(
                f"{oid} declares an invalidation edge and no commit its readings were taken "
                "at, so nothing can say what they have gone stale against — write "
                f"`taken at ` inside its `<!-- outside-register: {oid} … -->` marker"
            )
        elif sha is not None and not outside.depends:
            out.append(
                f"{oid} declares the commit {sha} and no invalidation edge, so nothing reads "
                "that commit; either give `outside_sections.NOT_OURS` the paths its readings depend on "
                "or drop the provenance from its marker"
            )
        elif sha is not None and resolve(sha) is None:
            out.append(f"{oid} declares {sha}, which is not a commit in this repository")
    for oid in sorted(set(declared) - set(outside_sections.NOT_OURS)):
        out.append(
            f"{oid} names the commit {declared[oid]} and is no section this harness disowns"
        )
    return out

#: An ATX heading. Matched per line rather than with `re.MULTILINE` over the
#: whole text, because the doc's shell blocks contain comment lines that start
#: with `#` and would otherwise cut a section in half.
HEADING_RE = re.compile(r"^(#{1,6}) +\S")


def headings(text: str) -> list[tuple[int, int]]:
    """Every heading's offset and level, code fences excluded.

    The fence skip is load-bearing: `measurements.md` publishes a `sh` block
    whose first line is `# maps to EOF (the table never matches) and never
    saves`, which reads as a level-1 heading and would end the section it is
    printed inside."""
    out: list[tuple[int, int]] = []
    pos = 0
    fenced = False
    for line in text.splitlines(keepends=True):
        if line.startswith("```"):
            fenced = not fenced
        elif not fenced:
            match = HEADING_RE.match(line)
            if match:
                out.append((pos, len(match.group(1))))
        pos += len(line)
    return out


def section_label_problems(text: str) -> list[str]:
    """Each figure whose `section` label does not begin with the heading its
    marker sits under, one line each.

    The label renders as a sitting's heading, so a label left quoting the
    number its heading was rewritten from would print that number at the next
    sweep. "Begin with" rather than "equal" leaves room for a suffix telling
    two figures under one heading apart, as `map-only`'s does. A marker under
    no heading has no heading to match, and says so.

    **A declared section's label is not held.** `Outside.section` is read only
    by `--list`, beside its id, and never renders as a heading, so the stale
    number this guards against cannot reach print through one; `benches`'
    label names its files instead, which is what `--list` wants of it."""
    marks = headings(text)
    out = []
    for match in MARKER_RE.finditer(text):
        fig = register.ALL_BY_ID.get(match.group(1))
        if fig is None:
            continue
        above = [pos for pos, _ in marks if pos < match.start()]
        if not above:
            out.append(f"{fig.id} sits under no heading")
            continue
        line = text[above[-1] :].split("\n", 1)[0]
        heading = line.lstrip("#").strip()
        if not fig.section.startswith(heading):
            out.append(f"{fig.id}: label {fig.section!r}, heading {heading!r}")
    return out


def outside_register_sections(text: str) -> list[tuple[str, list[str]]]:
    """Each section the doc declares outside the register, and the figure
    markers found inside it — which must be none.

    A section runs from its heading to the next heading at or above its own
    level, so a declared section covers its own subheadings. A marker under no
    heading at all takes the text above the first one, which is the preamble:
    the sentence scoping the session stamp lives there, and it is not a
    section anybody may declare."""
    marks = headings(text)
    found = []
    for match in OUTSIDE_RE.finditer(text):
        above = [(pos, level) for pos, level in marks if pos < match.start()]
        if above:
            start, level = above[-1]
            below = [pos for pos, lvl in marks if pos > start and lvl <= level]
        else:
            start = 0
            below = [pos for pos, _ in marks]
        end = below[0] if below else len(text)
        found.append((match.group(1), MARKER_RE.findall(text[start:end])))
    return found

#: The lead-in `share_readings` writes when a borrow could not be satisfied,
#: and the one the doc carries verbatim in substance when such a table is
#: published (`measurements.md`, "The apparatus").
PARTIAL_RE = re.compile(r"\*\*Partial sweep\*\*")


def partial_sittings(text: str) -> list[tuple[str, list[str]]]:
    """Tables the doc publishes that measured a reading they share.

    One reading, two numbers: the figure named and the figures it shares with
    each carry a measurement of the same run. The doc **permits** this, so long
    as the harness's note is carried with the table -- which is why `--check`
    reports it rather than failing on it. What it buys is that the set to
    re-take is named at the moment someone is reading the doc to decide, rather
    than recomputed by hand from the harness's source.

    Attribution is by position: a note belongs to the nearest figure marker
    above it, which is how the emitted table is laid out."""
    marks = [(m.start(), m.group(1)) for m in MARKER_RE.finditer(text)]
    seen: list[str] = []
    for note in PARTIAL_RE.finditer(text):
        above = [fid for pos, fid in marks if pos < note.start()]
        if above and above[-1] not in seen:
            seen.append(above[-1])
    return [(fid, borrowing.sharing_closure(fid)) for fid in seen]


def stamp_in(text: str) -> str | None:
    """The commit a session stamp names, read out of the doc's text."""
    match = STAMP_RE.search(text)
    return match.group(1) if match else None


def stamped_commit(doc: Path) -> str | None:
    """The commit the doc's session stamp names, so `--stale` has a default."""
    return stamp_in(doc.read_text())
