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
        commit="8b97956",
        figures=(
            "census-brace-free",
            "census-arrays",
            "scan-throughput-cold",
            "scan-throughput-warm",
            "nested-end-to-end",
            "census-attribution",
            "cross-file-floor",
            "per-block-quadratic",
            "projection-widths",
        ),
        why=(
            "deleted composite-isolated's generator support -- the "
            "--weak-composite flag and the composite_text input -- which no "
            "surviving input's row logic reaches; all five inputs a published "
            "figure is taken on are byte-identical across the change"
        ),
        verified="cd scripts && uv run measure.py --verify-additive",
    ),
    Acknowledged(
        commit="9ed21d4",
        figures=(
            "nested-end-to-end",
            "census-attribution",
            "cross-file-floor",
            "map-only",
            "projection-widths",
        ),
        why=(
            "the filter term grammar, which no registered command shape "
            "executes: none passes --filter, so parse_filter and everything "
            "under it is unreachable in every timed run. The one added "
            "function a shape does reach is quoted_name_note, called once on "
            "query-nomatch's not-found path, where it compares the first and "
            "last byte of `public.nosuchtable` and returns None -- against a "
            "timed leg measured in seconds"
        ),
        verified="test 0 -eq \"$(grep -c -- --filter scripts/measure.py)\"",
    ),
    Acknowledged(
        commit="a6e713f",
        figures=(
            "census-brace-free",
            "census-arrays",
            "scan-throughput-cold",
            "scan-throughput-warm",
        ),
        why=(
            "the comparison register moved to L2, which edited stream.rs -- "
            "one more Copy vector cut per block in `project` -- and batch.rs "
            "under #[cfg(test)]. These four figures are parse-shaped: every "
            "timed run of theirs is `parse` or `dd`, and `pgdq parse` enters "
            "map_file, which reaches neither `resolve_block` nor `project` "
            "(their only non-test call sites are inside table_stream, "
            "stream.rs:915/1204/1248 and 925/1223/1260). The three "
            "query-shaped figures over the same paths are NOT excused here: "
            "they run one comparison_for per column per block, and "
            "projection-widths runs the extra cut too"
        ),
        verified=(
            "jq -r '.readings|keys[]' runs/measure-20260830T191415/raw.json | "
            "awk -F/ '$1~/^(census-brace-free|census-arrays|"
            "scan-throughput-cold|scan-throughput-warm)$/{print $4}' | "
            "sort -u  # dd, parse -- nothing query-shaped"
        ),
    ),
    Acknowledged(
        commit="682819d",
        figures=("preamble-prepass",),
        why=(
            "M28 keyed DumpMetadata::types on the type name, which puts a "
            "find over the types so far in front of every CREATE TYPE in "
            "preamble.rs -- a declared path of this figure. The new code is "
            "`record_type` and its one call site, inside the SpanBody::TypeDef "
            "arm, and this figure's every timed run is on blocks4000, which "
            "generate_block_count_bench.py builds out of CREATE TABLEs alone: "
            "no type DDL, so no TypeDef span, so `record_type` is never called "
            "in a timed run. Reachability, the same oracle a6e713f's entry uses"
        ),
        verified=(
            "test 0 -eq \"$(grep -c 'CREATE TYPE\\|CREATE DOMAIN' "
            "scripts/generate_block_count_bench.py)\""
        ),
    ),
)
