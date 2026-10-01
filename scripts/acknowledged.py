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
    `--stale` red until a sweep re-stamps the doc — and a sweep is about two
    hours on a machine that has to be quiet, so the realistic outcome is that no sweep
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
    #: The ids this excuses: figures, and the declared sections `measure.NOT_OURS`
    #: holds, which the same register discharges. Empty means **every figure and
    #: no declared section** — right only for a change no figure's subject can
    #: see, and rare enough to be suspicious. A declared section is reached only
    #: by naming it: the blanket case is written about the readings its author
    #: had in mind, which are the durations a sweep takes, and a section's
    #: readings are something else. koji's are a byte-for-byte comparison of
    #: what a scan concludes, so excusing it is a claim about block offsets and
    #: row totals — one that has to be made deliberately rather than inherited
    #: from an entry about timings.
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
#:
#: **It is also the state comment-only entries no longer reach.** That oracle
#: is syntactic, so `measure.comment_only_commit` decides it per commit and per
#: path and `--stale` skips the commit without being told — which is what the
#: three entries that used to stand here were doing by hand, at one extra
#: commit apiece.
ACKNOWLEDGED: tuple[Acknowledged, ...] = (
    Acknowledged(
        commit="ce6ecb58",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "per-block-quadratic",
            "peak-rss",
            "map-only",
            "preamble-prepass",
            "projection-widths",
            "predicate-terms",
            "allocator",
            "parallel-scan-throughput",
            "parallel-peak-rss",
            "rss-attribution",
            "reserve",
            "statistics-gathering",
            "statistics-pruning",
            "dynamic-filter-join",
            "dynamic-filter-topk",
        ),
        why="outside comments and a #[cfg(test)] assert, only "
        "ParallelismBudgetLimited's message literal shortened, formatted at most "
        "once a plan; no scan, decode or allocation path changed",
        verified="git diff -U0 ce6ecb58^ ce6ecb58 -- pgdump_query/src/stream.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="8cffbac3",
        figures=(
            "chunk-size",
            "nested-end-to-end",
            "cross-file-floor",
            "peak-rss",
            "projection-widths",
            "predicate-terms",
            "allocator",
            "parallel-scan-throughput",
            "parallel-peak-rss",
            "rss-attribution",
            "reserve",
            "statistics-gathering",
            "statistics-pruning",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "session-drift",
        ),
        why="M191's lexer runs only on lines outside a COPY block (CopyScanner's "
        "State::Outside arm, map.rs's INSERT-run end), a few KB of preamble on "
        "these COPY-dominated inputs; the in-block per-row path is untouched, the "
        "scanner's state changes by a Lexer replacing a dollar tag, and "
        "measure.py only adds lex.rs to SCAN. Owed re-takes, not excused: "
        "scan-throughput-* (insert_run legs), preamble-prepass, "
        "per-block-quadratic, map-only",
        verified="git show 8cffbac3 -- pgdump_query/src/scan.rs pgdump_query/src/stream.rs "
        "scripts/measure.py | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
)
