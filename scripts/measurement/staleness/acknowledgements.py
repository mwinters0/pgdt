"""Acknowledged commits: a declared path changed, and no reading moved."""

from __future__ import annotations

import dataclasses
from typing import Callable, Sequence

from acknowledged import ACKNOWLEDGED, Acknowledged

from measurement import config
from measurement import host
from measurement.staleness import comments
from measurement.staleness import outside_sections


# Both names live in `acknowledged.py` and are imported at the top of this
# file. They are there, not here, because `session-drift` declares this path:
# an entry added to this file would mark stale the figure it is excusing, and
# no entry can name its own sha. That module's docstring has the full reason.


def resolved_acknowledgements(
    acks: Sequence[Acknowledged] = ACKNOWLEDGED,
) -> list[Acknowledged]:
    """`ACKNOWLEDGED` with every commit resolved to a full sha, so comparison
    against `git log` output is not a string-length accident."""
    out = []
    for ack in acks:
        sha = host.run(["git", "rev-parse", f"{ack.commit}^{{commit}}"], cwd=config.REPO, capture=True).strip()
        out.append(dataclasses.replace(ack, commit=sha))
    return out


def excuses(acks: Sequence[Acknowledged], commit: str, figure_id: str) -> Acknowledged | None:
    """The acknowledgement covering this commit for this id, if any.

    **A blanket entry reaches every figure and no declared section.** An empty
    `figures` tuple is the claim that no figure's subject can see the change,
    and the readings a figure publishes are durations a sweep takes; a declared
    section's are not, so the blanket claim is not about them. koji's readings
    are a byte-for-byte comparison of what a scan concludes, and excusing those
    is a claim about block offsets and row totals — reachable only by naming
    `koji`, so that its author decided it rather than inheriting it from an
    entry written about timings. The permission itself is unchanged: an entry
    that *names* a declared section still excuses it, which is the discharge
    `measurements.md`, "A commit can be acknowledged" settled, since the
    harness cannot re-take a section it does not own."""
    for ack in acks:
        if ack.commit != commit:
            continue
        if figure_id in ack.figures:
            return ack
        if not ack.figures and figure_id not in outside_sections.NOT_OURS:
            return ack
    return None


def excused_paths(
    figure_id: str,
    hits: Sequence[str],
    commits_by_path: dict[str, Sequence[str]],
    dirty: set[str],
    acks: Sequence[Acknowledged],
    comment_only: Callable[[str, str], bool] = comments.comment_only_commit,
) -> list[str]:
    """Which of a figure's touched paths are settled — by an acknowledgement,
    or by the commit changing nothing but comments inside that path.

    The two settlements are not interchangeable and only one of them is
    written by hand. `comment_only` is passed in so a test can hold the
    register's half on its own, and its default is the real oracle so that a
    caller cannot get the weaker answer by forgetting it.

    Two refusals, and both are the conservative direction:

    - **An uncommitted path is never excused.** There is no commit to point
      at, so nobody has reviewed the diff — a dirty tree under a measured path
      is exactly what `git_head` already calls unpublishable.
    - **Every commit touching the path must be excused, not just one.** A path
      changed by an excused commit and an unexamined one is stale on the
      strength of the second.
    """
    out = []
    for path in hits:
        if path in dirty:
            continue
        commits = commits_by_path.get(path) or ()
        if commits and all(
            excuses(acks, c, figure_id) or comment_only(c, path) for c in commits
        ):
            out.append(path)
    return out


def inert_excuses(
    figure_id: str,
    hits: Sequence[str],
    commits_by_path: dict[str, Sequence[str]],
    acks: Sequence[Acknowledged],
    comment_only: Callable[[str, str], bool] = comments.comment_only_commit,
) -> list[tuple[str, list[str], list[str]]]:
    """Per still-red path: the entries that excuse it, and the commits that do not.

    An entry excuses a commit, so a later unexamined commit on the same path
    leaves the earlier entry correct and doing nothing. `excused_paths` drops
    that path, and the entry then appears in no output at all — which reads
    exactly like a missing entry, and has once been mistaken for one. So a
    stale path that carries at least one excused commit says so, naming the
    commits that are actually holding it red.

    Returns `(path, excused shas, blocking shas)`, only for paths where the
    first list is non-empty; a path nothing excuses is red for the ordinary
    reason and needs no commentary. A comment-only commit is in neither list:
    it is settled without an entry, so naming it as what holds the path red
    would send the reader to a diff that changes nothing.
    """
    out = []
    for path in hits:
        excused, blocking = [], []
        for commit in commits_by_path.get(path) or ():
            if excuses(acks, commit, figure_id):
                excused.append(commit)
            elif not comment_only(commit, path):
                blocking.append(commit)
        if excused:
            out.append((path, excused, blocking))
    return out
