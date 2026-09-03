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
        commit="7545dc6",
        figures=("nested-end-to-end", "cross-file-floor", "projection-widths", "allocator"),
        why=(
            "P7's staging retargeted one doc comment in `batch.rs` at the phase "
            "spec, the inbox it cited having been drained. Comment-only: no "
            "item, signature or expression changed, so no code is generated "
            "differently and no timed path is reached."
        ),
        verified="git show 7545dc6 -- pgdump_query/src/batch.rs",
    ),
    Acknowledged(
        commit="7545dc6",
        figures=("session-drift",),
        why=(
            "The same retarget in `measure.py`, inside `quoted_by` metadata "
            "and a comment. `quoted_by` is read by `--check` alone and names "
            "no command shape a sweep executes, so no figure's input, "
            "invocation or timing changed."
        ),
        verified="git show 7545dc6 -- scripts/measure.py",
    ),
    Acknowledged(
        commit="fbaaa49",
        figures=("session-drift",),
        why=(
            "7.1 added `--profile-recipe` to `measure.py`: three new functions "
            "(`profile_argv`, `profile_recipe`, `cmd_profile`) and one "
            "subcommand. Reachability -- no sweep command shape reaches any of "
            "them, and no existing figure's input, invocation or timing "
            "changed."
        ),
        verified="git show fbaaa49 -- scripts/measure.py",
    ),
    Acknowledged(
        commit="a6bf6cd",
        figures=("session-drift",),
        why=(
            "The symbol-fetch entry's closure rewrote `PGDQ_PROFILE_DEBUGINFOD`'s "
            "comment and the recipe's printed prose inside `profile_recipe`. "
            "Comment and string-literal only, on the same unreachable "
            "subcommand."
        ),
        verified="git show a6bf6cd -- scripts/measure.py",
    ),
    Acknowledged(
        commit="305af4b",
        figures=("session-drift",),
        why=(
            "`M47`'s first half re-pointed four printed lines of "
            "`profile_recipe` at CONTRIBUTING.md. String literals inside the "
            "same unreachable subcommand."
        ),
        verified="git show 305af4b -- scripts/measure.py",
    ),
    Acknowledged(
        commit="360e144",
        figures=("session-drift",),
        why=(
            "`M47`'s second half rewrote the symbol step: `DEBUGINFOD`'s "
            "docstring, `profile_recipe`'s docstring, and the shell lines it "
            "prints. Docstrings and string literals inside the same "
            "unreachable subcommand -- the register, the stage functions and "
            "every command shape a sweep executes are untouched."
        ),
        verified="git show 360e144 -- scripts/measure.py",
    ),
)
