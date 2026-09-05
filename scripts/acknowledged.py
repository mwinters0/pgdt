#!/usr/bin/env python3
"""The acknowledgement register: commits that touched a figure's declared paths
without moving a reading.

**This file exists so that acknowledging a commit is not self-defeating.** The
register used to live in `measure.py`, which `session-drift` declares — so the
very commit that added an entry marked that figure stale, and no entry can name
its own sha. Chasing it with a follow-up commit moves the problem by one and
never terminates. Nothing here is on any timing path, so no figure declares
this file, and an ack-only commit now touches no measured path.

`measure.py` re-exports both names, so `measure.ACKNOWLEDGED` and
`measure.Acknowledged` still resolve.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Acknowledged:
    """One commit that touched a figure's declared paths without moving it.

    `depends` is coarse on purpose (see `SCAN` and the constants beside it),
    and coarseness costs something in *both* directions. The cost designed for
    is the false negative: a change outside the declared paths moves a figure
    and nothing says so, which is why the doc also carries a session stamp.
    This is the other one, and it is the one that decays the mechanism. A
    change *inside* a declared path that provably moves nothing leaves
    `--stale` red until a sweep re-stamps the doc — and a sweep is an hour on a
    machine that has to be quiet, so the realistic outcome is that no sweep
    runs and `--stale` becomes a light that is always on. A signal that is
    always on is the same thing as no signal, which is the decay the register
    was built against, arriving from the other side.

    So a commit can be excused, per figure, with its evidence attached.

    **A command shape names an entry point, not the code that runs.** A
    figure's shape says which command was timed; whether the changed code
    executes is the entry point *and* the input together, and the input is the
    half that is easy to forget. A change behind a conditional the figure's
    input never triggers does not run in it, however squarely the shape lands
    on the edited file -- so an entry claiming reachability names the input and
    the reason it cannot reach, and its `verified` reads the generator rather
    than a staging directory that may be gone.

    **Per commit, never per path.** Excusing a path would silently cover every
    future change to it, which is precisely the escape hatch that turns this
    into a way to wave away real staleness. A commit is a fixed diff that
    someone — or `--verify-additive` — actually looked at.

    **An entry excuses a commit, and a path is accounted for only when every
    commit that touched it is excused.** So one unexamined commit on a path
    makes every earlier entry on that path *inert* — the excuse is still
    correct and still does nothing, and the figure reads red for the new
    commit. Two consequences, and both have already cost a session. Writing an
    entry does not mean the figure goes green: check `--stale`, which names an
    inert entry under the figure it failed to clear. And a slice landing on an
    already-red declared path still owes an entry or a stated reason not to
    write one: "the path was red before I touched it" is not one, because the
    figure's colour is not what the register is tracking.

    **An acknowledgement lives inside one stamp's range.** Once the doc is
    re-stamped past it, the commit is no longer in any `--stale` range and the
    entry is spent; `--check` names spent entries so they are deleted rather
    than kept as sediment.
    """

    #: Any spelling `git rev-parse` resolves. Normalised before comparison, so
    #: the short sha that `--stale` prints is what gets recorded.
    commit: str
    #: The figure ids this excuses. Empty means every figure — right only for a
    #: change no figure's subject can see, and rare enough to be suspicious.
    figures: tuple[str, ...]
    #: What changed and why no reading moves, in one line.
    why: str
    #: The command that re-checks the claim, so the entry is not a session's
    #: word. Empty is allowed and is the weaker kind of entry: it says someone
    #: read the diff and nothing mechanical can confirm it.
    verified: str = ""


#: The excused commits, live for the current session stamp only. Empty is the
#: state a fresh stamp leaves behind: every entry a sweep re-stamps past is
#: spent, and `--check` names it so it is deleted rather than kept as sediment.
ACKNOWLEDGED: tuple[Acknowledged, ...] = (
    Acknowledged(
        commit="2f986a0",
        figures=(
            "nested-end-to-end",
            "cross-file-floor",
            "projection-widths",
            "allocator",
            "session-drift",
        ),
        why=(
            "P7's wrap. Doc-comment and docstring text only in batch.rs, decode.rs "
            "and measure.py, retargeting citations out of the slice notes the same "
            "commit deleted; every other file it touches is a document no figure "
            "declares."
        ),
        verified=(
            "git show --stat 2f986a0 -- pgdump_query/src pgdump_query-cli/src "
            "scripts/measure.py, then git diff 2f986a0^ 2f986a0 -- those paths: "
            "every hunk is inside a /// or //! comment or a Python docstring."
        ),
    ),
    Acknowledged(
        commit="6b90905",
        # Every figure: nine declared paths, and the union of what declares
        # them is the whole register.
        figures=(
            "census-brace-free",
            "census-arrays",
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "nested-end-to-end",
            "census-attribution",
            "cross-file-floor",
            "per-block-quadratic",
            "map-only",
            "preamble-prepass",
            "nested-decode-micro",
            "projection-widths",
            "predicate-terms",
            "allocator",
            "session-drift",
        ),
        why=(
            "P7's keystone. The same retargeting one level out: comment text in "
            "batch.rs, index.rs, alloc.rs, both benches, both Cargo manifests "
            "and generate_perf_data.py's module docstring, plus measure.py's "
            "seventeen quoted_by edges into the deleted spec. A quoted_by tuple "
            "is read by --check and --stale and by nothing that times a run. "
            "generate_perf_data.py is the one to look at twice, since a "
            "generator change moves the input rather than the code: its diff is "
            "three lines of the docstring above the first import."
        ),
        verified=(
            "git diff 6b90905^ 6b90905 -- pgdump_query pgdump_query-cli scripts "
            "Cargo.toml: every hunk is inside a ///, //!, # or \"\"\" comment, "
            "except measure.py's removed quoted_by entries. No run function, "
            "Figure spec, input list, command shape or generator output "
            "changes; scripts/test_measure.py's 232 assertions pass unchanged."
        ),
    ),
)
