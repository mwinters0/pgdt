"""What a sitting owes: the commit each figure was taken at, and the sittings that cannot stand
together."""

from __future__ import annotations

from typing import Callable, Mapping, Sequence

from measurement import borrowing
from measurement import git
from measurement.figures import register
from measurement.staleness import doc_markers


def figure_bases(text: str, override: str | None = None) -> dict[str, str | None]:
    """The commit each figure is argued from: its own sitting where its marker
    declares one, the session stamp otherwise.

    **One function, every reader of the stamp.** `--stale`, acknowledgement
    spentness and `--verify-additive` all used to read the stamp directly, and
    each of them was then wrong for a figure taken elsewhere -- reporting it
    stale against commits it *postdates*, spending an acknowledgement its range
    cannot reach, and regenerating its inputs at a revision it was not taken at.

    `override` is `--since`, which asks a deliberate question of every figure at
    once ("what has moved since X") and so is not per figure. `None` for a
    figure means nothing says where to argue from: no marker and no stamp."""
    if override:
        return {fid: override for fid in register.ALL_BY_ID}
    stamp = doc_markers.stamp_in(text)
    sittings = doc_markers.figure_sittings(text)
    return {fid: sittings.get(fid, stamp) for fid in register.ALL_BY_ID}


def sitting_problems(
    sittings: Mapping[str, str],
    stamp: str | None,
    resolve: Callable[[str], str | None] = git.resolve_commit,
    ancestor: Callable[[str, str], bool] | None = None,
) -> list[str]:
    """Why a figure may not carry the sitting its marker declares, one line each.

    Three refusals, and the third is the one that looks entirely plausible in
    the doc:

    - **A figure that shares or is derived from may not be published outside
      the sweep at all.** That is the doc asserting a differencing that crosses
      sittings, and it is the same class of error as a section declared outside
      the register while carrying a figure marker.
    - **A sitting that names the stamp's own commit is a marker that should not
      be there.** A figure the stamped sweep took declares nothing, so a marker
      repeating the stamp is a second copy of one fact that can go stale
      against it.
    - **A sitting must descend from the stamp.** An older one cannot
      legitimately exist -- a sweep replaces every table at once, so a figure
      the sweep took carries no marker and a figure it did not take was folded
      in later -- which makes a non-descendant either a marker a sweep left
      behind or a hand edit, both of which republish a fresh table under a lying
      provenance and put `--stale` back on the wrong commit. It costs one
      `is_ancestor` call."""
    ancestor = ancestor or git.is_ancestor
    out: list[str] = []
    for fid, sha in sorted(sittings.items()):
        if fid not in register.EVERY_BY_ID:
            # `--check`'s unknown-marker report already names it; saying it
            # twice would make one rename look like two problems.
            continue
        entangled = borrowing.entangled_with(fid)
        if entangled:
            out.append(
                f"{fid} declares a sitting of its own ({sha}) and is read beside "
                f"{', '.join(entangled)} — a figure published outside the stamped sweep must "
                "share no reading and stand in no derivation, since one sitting is what lets "
                "these tables be differenced against each other"
            )
            continue
        if stamp is None:
            out.append(
                f"{fid} declares a sitting ({sha}) in a document whose session stamp names no "
                "commit, so there is nothing for it to be outside of"
            )
            continue
        got, base = resolve(sha), resolve(stamp)
        if got is None:
            out.append(f"{fid} declares {sha}, which is not a commit in this repository")
            continue
        if base is None:
            out.append(
                f"{fid} declares {sha}, but the session stamp's {stamp} is not a commit in this "
                "repository, so nothing can say whether the sitting descends from it"
            )
            continue
        if got == base:
            out.append(
                f"{fid} declares the stamp's own commit ({sha}); a figure the stamped sweep took "
                "carries no sitting marker, and one that carries none is read from the stamp"
            )
            continue
        if not ancestor(base, got):
            out.append(
                f"{fid} declares {sha}, which does not descend from the stamp's {stamp} — a "
                "sitting older than the stamp cannot legitimately exist, so this is a marker a "
                "sweep left behind or a hand edit, and either republishes a fresh table under a "
                "lying provenance"
            )
    return out


def sitting_accounting(outside: Sequence[tuple[str, str]], total: int | None = None) -> str:
    """The stamp's accounting of which figures came from the sitting it names.

    **Generated, not reconciled.** `session_stamp` already computes the harder
    half of the sentence, and the count beside it was hand-maintained — which
    is exactly the thing that goes wrong silently: the sentence claiming all
    eighteen markers was false for as long as one figure stood outside the
    sweep and said so only in prose three paragraphs away."""
    total = len(register.ALL_FIGURES) if total is None else total
    if not outside:
        return f"**All {total} figures below come from that sitting.**"
    named = ", ".join(f"`{fid}` (`{sha}`)" for fid, sha in outside)
    rest = (
        "The other carries its own sitting commit inside its marker, and every reader of this "
        "stamp argues from that instead"
        if len(outside) == 1
        else f"The other {len(outside)} carry their own sitting commits inside their markers, "
        "and every reader of this stamp argues from those instead"
    )
    return f"**{total - len(outside)} of the {total} figures below come from that sitting.** {rest}: {named}."
