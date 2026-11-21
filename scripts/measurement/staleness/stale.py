"""`--stale`: the figures a diff has invalidated."""

from __future__ import annotations

import sys
from typing import Iterable
from typing import TYPE_CHECKING

from measurement import borrowing
from measurement import config
from measurement import git
from measurement.figures import register
from measurement.staleness import acknowledgements
from measurement.staleness import doc_markers
from measurement.staleness import outside_sections
from measurement.staleness import sitting_checks

if TYPE_CHECKING:
    from measurement.figures.declaration import Figure


def declared_hits(fig: Figure | outside_sections.Outside, changed: Iterable[str]) -> list[str]:
    """The paths in `changed` that `fig` declares. A declared path is a prefix:
    a directory matches everything under it.

    **One predicate, every caller.** `--stale` argues from it that a figure has
    gone stale, and the same way over a section the register does not hold.
    Writing either separately would make it a second authority over what can
    move a reading — and an `Outside` declares its edge in the same field a
    `Figure` does precisely so that one predicate still answers for both."""
    return sorted({c for c in changed for d in fig.depends if c == d or c.startswith(d)})


def figures_touched(changed: Iterable[str]) -> list[tuple[Figure, list[str]]]:
    """Figures whose declared paths a diff touches.

    **Every figure, not just the ones a sweep runs.** The derived
    `session-drift` declares the harness's source because the harness's own
    timing path is the apparatus it measures; iterating `FIGURES` here left
    that declaration inert, which is the failure mode a declared dependency
    exists to prevent.
    """
    changed = list(changed)
    out = []
    for fig in register.ALL_BY_ID.values():
        hits = declared_hits(fig, changed)
        if hits:
            out.append((fig, hits))
    return out


def cmd_stale(since: str | None) -> int:
    """Which figures a diff has invalidated, each argued from its own base —
    and, in a stanza of its own, which section the register does not hold.

    A figure published outside the stamped sweep declares the commit it was
    taken at, and its range starts there: reading the stamp for every figure
    reported one stale against three commits it *postdates*, which is a red
    nobody can clear — not by a measurement, which would spend a sitting to
    conceal that the range was the defect, and not by an acknowledgement, which
    would assert "this commit moved no reading" about a commit that ran before
    the reading was taken."""
    doc = config.REPO / "docs/design/measurements.md"
    text = doc.read_text()
    bases = sitting_checks.figure_bases(text, since)
    if any(rev is None for rev in bases.values()):
        print(
            "no --since given and measurements.md carries no session stamp naming a commit; "
            "pass --since <rev>",
            file=sys.stderr,
        )
        return 2
    # A declared section is argued from its own marker, never from the stamp:
    # the stamp is scoped to the register and says nothing about a section
    # outside it, so reading it here would date koji's readings to a sweep that
    # did not take them.
    obases = doc_markers.outside_bases(text, since)
    undated = sorted(oid for oid, rev in obases.items() if rev is None)
    obases = {oid: rev for oid, rev in obases.items() if rev is not None}
    everything = {**bases, **obases}
    changed_since = {rev: git.changed_paths(rev) for rev in sorted({*everything.values()})}
    for rev, changed in changed_since.items():
        who = sorted(fid for fid, base in everything.items() if base == rev)
        scope = (
            "every figure and declared section"
            if len(who) == len(everything)
            else ", ".join(who)
        )
        print(f"{len(changed)} path(s) changed since {rev} — {scope}")
    print()
    if undated:
        print(
            "Declared outside the register with an invalidation edge and no commit its\n"
            "readings were taken at — nothing can say what these have gone stale against\n"
            "(`--check` names it too):"
        )
        for oid in undated:
            print(f"  {oid}")
        print()
    touched = [
        (fig, hits)
        for fig in register.ALL_BY_ID.values()
        if (hits := declared_hits(fig, changed_since[bases[fig.id]]))
    ]
    outside_touched = [
        (outside_sections.NOT_OURS[oid], hits)
        for oid, rev in sorted(obases.items())
        if (hits := declared_hits(outside_sections.NOT_OURS[oid], changed_since[rev]))
    ]
    if not touched and not outside_touched:
        print("no figure's declared paths were touched, and no declared section's.")
        return 0

    acks = acknowledgements.resolved_acknowledgements()
    dirty = git.dirty_paths()
    # Per base, because a commit's position in a range depends on where the
    # range starts: the same path is touched by different commits for a figure
    # read from the stamp and one read from its own sitting.
    by_base = {
        rev: git.commits_touching(
            {
                p
                for who, hits in (*touched, *outside_touched)
                if everything[who.id] == rev
                for p in hits
            },
            rev,
        )
        for rev in changed_since
    }

    stale: list[tuple[Figure, list[str]]] = []
    outside_stale: list[tuple[outside_sections.Outside, list[str]]] = []
    excused_by: dict[str, list[str]] = {}
    # Keyed by commit and path rather than by commit: the oracle is answered
    # per path, and a commit that is comments in `io.rs` and code in
    # `measure.py` is exactly the shape three of the register's own entries
    # had.
    comment_by: dict[tuple[str, str], list[str]] = {}
    # One loop over both, because an acknowledgement excuses a *commit* and
    # says nothing about what kind of reading is on the other side of it: a
    # declared section is discharged by the same entry, in the same register,
    # and splitting the walk would have been the second authority over that
    # question.
    for who, hits in (*touched, *outside_touched):
        by_path = by_base[everything[who.id]]
        ok = acknowledgements.excused_paths(who.id, hits, by_path, dirty, acks)
        left = [h for h in hits if h not in ok]
        if left:
            (outside_stale if isinstance(who, outside_sections.Outside) else stale).append((who, left))
        for path in ok:
            for commit in by_path[path]:
                if acknowledgements.excuses(acks, commit, who.id):
                    excused_by.setdefault(commit, []).append(who.id)
                else:
                    comment_by.setdefault((commit, path), []).append(who.id)

    if comment_by:
        print(
            "Comment-only, skipped — every hunk these commits make inside the path falls\n"
            "within a comment, so nothing there can move a reading and no entry is owed:\n"
        )
        for (commit, path), ids in comment_by.items():
            print(f"  {commit[:7]}  {path}")
            print(f"           skipped for: {', '.join(sorted(set(ids)))}")
        print()

    if excused_by:
        print("Acknowledged — the commit is recorded as moving no reading:\n")
        for commit, ids in excused_by.items():
            ack = next(a for a in acks if a.commit == commit)
            short = commit[:7]
            print(f"  {short}  {ack.why}")
            print(f"           verified: {ack.verified or '(read by hand; nothing re-checks it)'}")
            print(f"           excuses: {', '.join(sorted(set(ids)))}")
        print()

    stamp = doc_markers.stamp_in(text)

    def report_outside() -> None:
        """The declared sections whose readings a diff has moved.

        Printed apart from the figures because the two ask different things of
        the reader. A stale figure names a sweep; a stale section names a run
        nobody here can make — koji is an hour on a different medium — so what
        this stanza buys is the choice between making that run by hand and
        writing down why the change cannot have moved it. Silence, which is
        what this printed before the edge existed, was not one of the two."""
        if not outside_stale:
            return
        print(
            "\nDeclared outside the register and moved anyway — the harness does not re-take\n"
            "these, so each is discharged by a run somebody makes by hand or by an\n"
            "acknowledgement, exactly as a figure's red is:"
        )
        for outside, hits in outside_stale:
            print(f"  {outside.id:<24} stale — {', '.join(hits)} (since {obases[outside.id]})")
            for path, excused, blocking in acknowledgements.inert_excuses(
                outside.id, hits, by_base[obases[outside.id]], acks
            ):
                names = ", ".join(c[:7] for c in excused)
                held = ", ".join(c[:7] for c in blocking) or "an uncommitted change"
                print(f"      {path}: {names} excused here but inert — held red by {held}")

    if not stale:
        if not outside_stale:
            print("nothing is stale: every touched path is accounted for.")
            return 0
        print("no figure is stale: every touched path of one is accounted for.")
        report_outside()
        return 1

    for fig, hits in stale:
        base = bases[fig.id]
        own = "" if base == stamp or since else f" (since its own sitting {base})"
        print(f"  {fig.id:<24} stale — {', '.join(hits)}{own}")
        by_path = by_base[base]
        for path, excused, blocking in acknowledgements.inert_excuses(fig.id, hits, by_path, acks):
            names = ", ".join(c[:7] for c in excused)
            held = ", ".join(c[:7] for c in blocking) or "an uncommitted change"
            print(f"      {path}: {names} excused here but inert — held red by {held}")
    if all(f.stage == "derived" for f, _ in stale):
        # A derived figure is computed from two sweeps' `raw.json`, so it is
        # re-taken in seconds and forces no sweep on anything else.
        print(
            "\nEvery stale figure here is derived: re-take it with "
            "`uv run measure.py --drift <sweep> <sweep>`, which measures nothing."
        )
        report_outside()
        return 1
    # `--figure` is what takes one on its own, so a derived figure is not in
    # this list however few edges it stands in: `--drift` is how it moves.
    alone = sorted(
        fig.id for fig, _ in stale if fig.id in register.SELECTABLE_BY_ID and not borrowing.entangled_with(fig.id)
    )
    print(
        "\nA stale figure is re-taken with the whole doc: one sweep replaces every table "
        "(`uv run measure.py --all`), because the doc differences across tables."
    )
    if alone:
        print(
            "These stand in no borrow edge, so each may instead be re-taken on its own and "
            f"declare the sitting in its marker: {', '.join(alone)}."
        )
    report_outside()
    return 1
