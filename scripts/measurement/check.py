"""`--check`: the register reconciled against the document -- markers, labels, the register's
boundary, partial sittings, consumers, and every shape's stated worker count."""

from __future__ import annotations

from pathlib import Path
from typing import Callable, Iterable, Mapping, Sequence

from acknowledged import Acknowledged

from measurement import borrowing
from measurement import config
from measurement import consumers
from measurement import git
from measurement import memory_model
from measurement import shapes
from measurement.figures import register
from measurement.figures import reserve
from measurement.staleness import acknowledgements
from measurement.staleness import doc_markers
from measurement.staleness import outside_sections
from measurement.staleness import sitting_checks


#: Lines `tables.md` carries that address the session folding a table in, never
#: a reader of `measurements.md`. Pasting a whole section drags them along --
#: which is what happened when the fold-in note landed under a figure and stood
#: there as though it were part of the table's own commentary.
SCAFFOLDING = ("**The fold-in must also re-read**",)


def scaffolding_in(text: str) -> list[str]:
    """Harness scaffolding that reached the document."""
    return [
        f"{marker} (line {i})"
        for i, line in enumerate(text.splitlines(), 1)
        for marker in SCAFFOLDING
        if line.startswith(marker)
    ]


def cmd_check(doc: Path) -> int:
    """Reconcile the register against the doc: which figures have landed a
    marker, which markers name nothing, which labels their headings left
    behind, where the register's boundary runs,
    which figures were taken outside the stamped sweep, which documents a
    fold-in must re-read because they repeat a figure's numbers, and whether
    every command shape states the worker count it is taken at."""
    text = doc.read_text()
    found = doc_markers.markers_in(doc)
    sittings = doc_markers.figure_sittings(text)
    outside = doc_markers.outside_register_sections(text)
    undeclared = [o.id for o in outside_sections.NOT_OURS.values() if o.id not in {i for i, _ in outside}]
    unknown_outside = sorted({i for i, _ in outside if i not in outside_sections.NOT_OURS})
    both = [(i, figs) for i, figs in outside if figs]
    unknown = [m for m in found if m not in register.ALL_BY_ID]
    dangling = [
        f"{fig.id} borrows from {shared.source}"
        for fig in register.EVERY_FIGURE
        for shared in fig.shares
        if shared.source not in register.EVERY_BY_ID
    ]
    duplicated = sorted({m for m in found if found.count(m) > 1})
    missing = [f.id for f in register.ALL_FIGURES if f.id not in found]
    unpinned = shapes.worker_count_problems()
    misspinned = shapes.pinned_count_problems()
    thin_axis = reserve.reserve_axis_problems()
    unargued_band = memory_model.charge_band_problems()
    gathering = shapes.statistics_flag_problems()
    scaffolding = scaffolding_in(text)
    mislabelled = doc_markers.section_label_problems(text)

    print(
        f"{doc.relative_to(config.REPO)} carries {len(set(found))} of {len(register.ALL_FIGURES)} figure markers.\n"
    )
    if missing:
        print("Not yet folded in (no `<!-- figure: <id> -->` under a heading):")
        for fid in missing:
            print(f"  {fid}")
        print()
    if unknown:
        print("Markers naming no figure — a rename that did not reach the register:")
        for m in sorted(set(unknown)):
            print(f"  {m}")
        print()
    if duplicated:
        print("Markers appearing more than once — one figure is one table:")
        for m in duplicated:
            print(f"  {m}")
        print()
    if dangling:
        print("Borrows naming no figure — a rename that did not reach the register:")
        for line in dangling:
            print(f"  {line}")
        print()
    if mislabelled:
        print(
            "Labels not beginning with their marker's heading — a heading rewritten when its\n"
            "number moved, and the figure's `section` in the register left behind:"
        )
        for line in mislabelled:
            print(f"  {line}")
        print()
    if unpinned:
        print(
            "Command shapes inheriting a worker count — a shape that states none measures\n"
            "whatever the CLI's `--jobs` happens to default to on the day, which has already\n"
            f"moved twice. State `--jobs {shapes.SWEEP_JOBS}`, and `{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS}` "
            f"on every run of `{shapes.SQL_SHELL.lstrip('/')}`:"
        )
        for command in unpinned:
            print(f"  {command}")
        print()
    elif misspinned:
        print(
            "Command shapes stating a worker count that is neither the apparatus's nor a\n"
            f"declared axis. State `--jobs {shapes.SWEEP_JOBS}`, or add the family to "
            "`measurement.shapes.JOBS_AXIS`\nand give it a figure whose rows are the counts:"
        )
        for line in misspinned:
            print(f"  {line}")
        print()
    else:
        print(
            f"No command shape inherits a worker count: each states `--jobs {shapes.SWEEP_JOBS}`, "
            f"or `--workers` for the\ndecode instrument, `PARALLEL_JOBS` for the "
            f"{len(shapes.JOBS_AXIS)} families whose axis it is, `GZPASS_JOBS` for the gzip first\n"
            "pass's, `GZIP_RSS_JOBS` for the gzip resident's, `LONG_LINE_JOBS` for the\n"
            f"long-line resident's, or `--jobs {shapes.RESERVE_JOBS}` for "
            "the reserve's stated legs; the reserve's flagless legs state none\nby "
            "declaration, and `dd` is not a run of ours. Every other run of "
            f"`{shapes.SQL_SHELL.lstrip('/')}`\nstates `{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS}` besides.\n"
        )
    if gathering:
        print(
            "Command shapes running `parse` with statistics gathered — the CLI's default,\n"
            f"which times the gathering rather than the scan. State `{shapes.NO_STATISTICS}`:"
        )
        for command in gathering:
            print(f"  {command}")
        print()
    if thin_axis:
        print(
            "Block sizes whose flagless axis is registered wrong — one whose limits cannot\n"
            f"reach the {shapes.RESERVE_FIT_MIN_COUNTS} distinct reader counts a two-term fit needs, "
            "so the family publishes a\nsecant however the sitting goes:"
        )
        for line in thin_axis:
            print(f"  {line}")
        print()
    if unargued_band:
        print(
            "Fault bands `BAND_STANCE` does not name — the rule is an enumeration so that a\n"
            "line added later refutes the model until somebody argues it out, which is only\n"
            "true while every band the code defines carries a stance:"
        )
        for line in unargued_band:
            print(f"  {line}")
        print()
    outside_dates = doc_markers.outside_sittings(text)
    if outside:
        print(
            "Declared outside the register — sections whose readings this harness did not\n"
            "take, and which the session stamp above therefore does not cover. Each still\n"
            "declares what invalidates it, so `--stale` can name it:"
        )
        for oid, _ in outside:
            known = outside_sections.NOT_OURS.get(oid)
            print(f"  {oid:<24} {known.section if known else '(unknown)'}")
            if known is None:
                continue
            edge = ", ".join(known.depends) or "(nothing — it publishes no reading)"
            print(f"  {'':<24}  invalidated by: {edge}")
            if known.depends:
                print(f"  {'':<24}  taken at: {outside_dates.get(oid, '(undeclared)')}")
        print()
    if undeclared:
        print(
            "Outside the register and saying so nowhere in the doc — add an\n"
            "`<!-- outside-register: <id> -->` marker under the section's heading:"
        )
        for oid in undeclared:
            print(f"  {oid:<24} {outside_sections.NOT_OURS[oid].section}")
        print()
    if unknown_outside:
        print("Declarations naming no section the harness disowns — see `--list`:")
        for oid in unknown_outside:
            print(f"  {oid}")
        print()
    if both:
        print(
            "Declared outside the register and carrying a figure marker — a section is one\n"
            "or the other, and the figure's apparatus line contradicts the declaration:"
        )
        for oid, figs in both:
            print(f"  {oid} carries {', '.join(sorted(set(figs)))}")
        print()
    partial = doc_markers.partial_sittings(text)
    if partial:
        print(
            "One reading published twice — a table below measured a reading it shares.\n"
            "The doc permits it while the harness's note is carried with the table; what it\n"
            "costs is that the set must be re-taken together before any of it moves:"
        )
        for fid, closure in partial:
            print(f"  {fid}")
            print(f"      re-take with: {', '.join(closure) or '(nothing — it shares no reading)'}")
        print()
    derived = borrowing.derivation_edges()
    if derived:
        # Reported rather than failed on, for the same reason a blessed partial
        # sitting is: nothing here is wrong, and a check that is permanently red
        # stops being read. What it buys is that the relationship is in front of
        # whoever is reading this to decide what a re-take drags.
        print(
            "Derived, not republished — one table's row is computed over another's reps.\n"
            "Not a closure edge: the number is not in the doc twice, so the two may come from\n"
            "different sittings. What it costs is that re-taking the source alone strands the\n"
            "consumer's row on reps the doc no longer holds:"
        )
        for consumer, source, what in derived:
            print(f"  {consumer} derives {what} from {source}")
        print()
    if register.UNTAKEN:
        print("Built, not taken (no marker expected — see `--list`):")
        for fig in register.UNTAKEN:
            print(f"  {fig.id}")
        print()
    stamp = doc_markers.stamped_commit(doc)
    if sittings:
        print(
            "Published outside the stamped sweep — each of these declares the commit it was\n"
            "taken at inside its own marker, and every reader of the stamp argues from that:"
        )
        for fid, sha in sorted(sittings.items()):
            print(f"  {fid:<24} {sha}")
        print()
    bad_sittings = sitting_checks.sitting_problems(sittings, stamp)
    if bad_sittings:
        print("Sittings the doc may not carry:")
        for line in bad_sittings:
            print(f"  {line}")
        print()
    unnamed_glibc = doc_markers.glibc_problems(text)
    if unnamed_glibc:
        print(
            "Figures whose glibc the doc does not name — the stamp names the register image's,\n"
            "and a sitting marker the one its own sitting ran under:"
        )
        for line in unnamed_glibc:
            print(f"  {line}")
        print()
    bad_outside = doc_markers.outside_sitting_problems(text)
    if bad_outside:
        print(
            "Declared sections whose provenance does not stand — an edge nothing can be\n"
            "measured from, or a commit nothing reads:"
        )
        for line in bad_outside:
            print(f"  {line}")
        print()
    # Generated, never reconciled: the count beside the stamp was hand-written
    # and was wrong for as long as one figure stood outside the sweep.
    accounting = sitting_checks.sitting_accounting(sorted(sittings.items()))
    miscounted = " ".join(accounting.split()) not in " ".join(text.split())
    if miscounted:
        print(
            "The session stamp's accounting sentence is not the one the harness generates.\n"
            "Replace it with this, verbatim:\n"
        )
        print(f"  {accounting}\n")
    acks = acknowledgements.resolved_acknowledgements()
    # A declared section is a name an acknowledgement may carry, and a base
    # spentness is computed against: `--stale` reds it the same way, and the
    # harness cannot re-take it, so an entry is the *only* discharge that does
    # not cost an hour on the HDD.
    unknown_ack, spent_ack = acknowledgement_problems(
        acks,
        {**register.ALL_BY_ID, **outside_sections.NOT_OURS},
        spent_acknowledgements(acks, {**sitting_checks.figure_bases(text), **doc_markers.outside_bases(text)}),
    )
    if acks:
        print("Acknowledged commits — a declared path changed and no reading moved:")
        for ack in acks:
            print(f"  {ack.commit[:7]}  {ack.why}")
        print()
    if unknown_ack:
        print("Acknowledgements naming no figure — a rename that did not reach the register:")
        for line in unknown_ack:
            print(f"  {line}")
        print()
    if spent_ack:
        print(
            "Spent acknowledgements — every figure they cover has moved past them; delete\n"
            "these entries:"
        )
        for line in spent_ack:
            print(f"  {line}")
        print()
    if scaffolding:
        print(
            "Harness scaffolding pasted into the document — these lines address the\n"
            "session folding a table in, not a reader of the document; delete them:"
        )
        for line in scaffolding:
            print(f"  {line}")
        print()
    print("What else a moved figure invalidates:")
    for fig in register.ALL_FIGURES:
        print(f"  {fig.id}")
        for q in consumers.consumers(fig):
            print(f"      {q}")
    return (
        1
        if (
            scaffolding
            or unknown
            or duplicated
            or dangling
            or mislabelled
            or unpinned
            or misspinned
            or gathering
            or thin_axis
            or unargued_band
            or undeclared
            or unknown_outside
            or both
            or bad_sittings
            or unnamed_glibc
            or bad_outside
            or miscounted
            or unknown_ack
            or spent_ack
        )
        else 0
    )


def acknowledgement_problems(
    acks: Sequence[Acknowledged], known: Iterable[str], spent: Iterable[str]
) -> tuple[list[str], list[str]]:
    """Entries naming a figure that does not exist, and entries already spent.

    Spent is the one that silts up. An acknowledgement covers a commit inside
    one figure's `--stale` range; once every figure it names is argued from a
    commit past it, no range can reach it again and the entry excuses nothing.
    Left standing, the register accumulates permanent excuses whose diffs
    nobody will ever re-read — which is how a mechanism that exists to keep a
    signal honest turns into the thing dulling it. Which entries those are is
    `spent_acknowledgements`, which reads each figure's own base.
    """
    known = set(known)
    spent = set(spent)
    unknown = sorted(
        f"{ack.commit[:7]} names {fid}" for ack in acks for fid in ack.figures if fid not in known
    )
    return unknown, sorted(ack.commit[:7] for ack in acks if ack.commit in spent)


def spent_acknowledgements(
    acks: Sequence[Acknowledged],
    bases: Mapping[str, str | None],
    ancestor: Callable[[str, str], bool] | None = None,
) -> list[str]:
    """The commits no figure they cover can reach any more.

    **Spentness is per figure**, because the range each figure is argued from
    is. An entry recorded against a commit between the stamp and a figure taken
    later is spent *for that figure* — its `--stale` range starts after the
    commit — and still live for every figure read from the stamp. So an entry
    goes only when every figure it covers has moved past it; going on the stamp
    alone would delete an excuse that is still doing work, and the next diff
    under that path would read stale with the reason gone.

    **`bases` carries the declared sections too, and a blanket entry covers
    none of them.** The mapping is the union of both, so that an entry naming
    `koji` is spent against koji's own marker rather than against the stamp —
    but an empty `figures` tuple means every figure and no declared section
    (`excuses`), and reading it as the whole mapping would have made a section
    hold a blanket entry alive that never reached it."""
    ancestor = ancestor or git.is_ancestor
    out = []
    for ack in acks:
        covered = ack.figures or tuple(fid for fid in bases if fid not in outside_sections.NOT_OURS)
        revs = [bases.get(fid) for fid in covered]
        if revs and all(rev is not None and ancestor(ack.commit, rev) for rev in revs):
            out.append(ack.commit)
    return out
