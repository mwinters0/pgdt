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
        commit="c151c64",
        figures=("preamble-prepass", "rss-attribution"),
        why=(
            "`M138`'s fold rewrote one doc comment in `preamble.rs` to the "
            "re-taken reading; that file holds `\"/*\"` beside code, which defeats "
            "the syntactic oracle, so the claim is made by hand"
        ),
        verified=(
            "git show --format= -U0 c151c64 -- pgdump_query/src/preamble.rs | "
            "grep -E '^[+-]' | grep -vE '^(\\+\\+\\+|---)' | "
            "grep -vE '^[+-][[:space:]]*(///|//!|//|#)'  # empty"
        ),
    ),
    Acknowledged(
        commit="c151c64",
        figures=("session-drift",),
        why=(
            "`M138`'s fold changed `measure.py`'s rendered sentences, a step-pair "
            "label, two docstrings and a comment; no command shape, input, regime "
            "or gate moved, so no reading can"
        ),
        verified="git show --format= -U0 c151c64 -- scripts/measure.py  # read every hunk",
    ),
    Acknowledged(
        commit="fd082b2",
        figures=(),
        why=(
            "`M140` renamed the statistics group to the row group — three "
            "constants, `--row-group-size` and one log message's text — with "
            "rustfmt's reflow of the lines they shortened; no code path, input "
            "or command's meaning moved"
        ),
        verified=(
            "git show --format= -U0 fd082b2 -- pgdt/src pgdump_query/src "
            "scripts/measure.py | grep -E '^[+-]' | grep -vE '^(\\+\\+\\+|---)' | "
            "grep -viE 'row[ _-]group|statistics[ _-]group'  # only rustfmt's reflows"
        ),
    ),
    Acknowledged(
        commit="f593a99",
        figures=(),
        why=(
            "`M141` renamed `--statistics-min-rows` and `--statistics-max-rows` "
            "to `--row-group-min-rows` and `--row-group-max-rows`, with their "
            "fields, doc comments and refusal text; no code path, input or "
            "command's meaning moved"
        ),
        verified=(
            "git show --format= -U0 f593a99 -- pgdt/src | grep -E '^[+-]' | "
            "grep -vE '^(\\+\\+\\+|---)' | "
            "grep -viE 'row[_-]group|statistics[_-](min|max)[_-]rows'  # empty"
        ),
    ),
    Acknowledged(
        commit="bb0909b",
        figures=("session-drift",),
        why=(
            "the repoint findings' record correction changed two of "
            "`measure.py`'s rendered sentences, folded from a `--render`; no "
            "command shape, input, regime or gate moved, so no reading can"
        ),
        verified="git show --format= -U0 bb0909b -- scripts/measure.py  # read every hunk",
    ),
    Acknowledged(
        commit="9d885cb",
        figures=("preamble-prepass", "rss-attribution"),
        why=(
            "the repoint rewrote two comments in `preamble.rs`, whose `\"/*\"` "
            "beside code defeats the syntactic oracle, so the claim is made by hand"
        ),
        verified=(
            "git show --format= -U0 9d885cb -- pgdump_query/src/preamble.rs | "
            "grep -E '^[+-]' | grep -vE '^(\\+\\+\\+|---)' | "
            "grep -vE '^[+-][[:space:]]*(///|//!|//|#)'  # empty"
        ),
    ),
    Acknowledged(
        commit="9d885cb",
        figures=("predicate-terms", "statistics-gathering", "statistics-pruning"),
        why=(
            "the repoint's one non-comment line in `predicate.rs` is `accepted_form`'s "
            "`cidr` sentence, read only to word a literal that failed to decode"
        ),
        verified=(
            "git show --format= -U0 9d885cb -- pgdump_query/src/predicate.rs | "
            "grep -E '^[+-]' | grep -vE '^(\\+\\+\\+|---)' | "
            "grep -vE '^[+-][[:space:]]*(///|//!|//|#)'  # the cidr sentence alone"
        ),
    ),
    Acknowledged(
        commit="86b921d",
        figures=("preamble-prepass", "rss-attribution"),
        why=(
            "the repoint rewrote comments in `preamble.rs` alone, whose `\"/*\"` "
            "beside code defeats the syntactic oracle, so the claim is made by hand"
        ),
        verified=(
            "git show --format= -U0 86b921d -- pgdump_query/src/preamble.rs | "
            "grep -E '^[+-]' | grep -vE '^(\\+\\+\\+|---)' | "
            "grep -vE '^[+-][[:space:]]*(///|//!|//|#)'  # empty"
        ),
    ),
    Acknowledged(
        commit="4d9fb4b",
        figures=("session-drift",),
        why=(
            "P27.1 added `datafusion-cli-pgdump` as a second timed program: eight "
            "new shapes, a `time_run` branch taken only for the `dfcli` binary, "
            "and a dry-run stand-in; every existing shape's script, mounts and "
            "image are unchanged, so no reading of `pgdt` can move"
        ),
        verified=(
            "every `command_shapes()` entry's `_script` at 788fc7d against HEAD: "
            "78 byte-identical, 8 `dfcli-dynamic-filter-*` added; "
            "git show --format= -U0 4d9fb4b -- scripts/measure.py  # read the time_run hunks"
        ),
    ),
    Acknowledged(
        commit="2f94f14",
        figures=("session-drift",),
        why=(
            "the dynamic-filter join queries answer `count(*)` first; only "
            "`dfcli-dynamic-filter-join-*` shapes changed"
        ),
        verified="git show --format= -U0 2f94f14 -- scripts/measure.py  # the three query strings and a comment",
    ),
    Acknowledged(
        commit="b326165",
        figures=("session-drift",),
        why=(
            "27.1's fold-in moved the two dynamic-filter figures from `UNTAKEN` "
            "into `FIGURES`; no shape, input, regime or gate moved"
        ),
        verified="git show --format= -U0 b326165 -- scripts/measure.py  # the registry move alone",
    ),
    Acknowledged(
        commit="b470ab6",
        figures=("session-drift",),
        why=(
            "`M174` pinned both images to the digests their tags already resolved "
            "to, and asks each place's glibc by one untimed `getconf` run per "
            "session, before that place's first figure; every shape's script, "
            "mounts and image content are unchanged, so no reading can move"
        ),
        verified=(
            "git show --format= -U0 b470ab6 -- scripts/measure.py  # read every hunk; "
            "`sudo nerdctl image inspect --mode=native docker.io/library/postgres:16` "
            "and `…/archlinux:base`, `.[0].Image.Target.digest`, against `Config`'s pins"
        ),
    ),
    Acknowledged(
        commit="2f75ff4",
        figures=("session-drift",),
        why=(
            "the review of `M174`'s glibc call added to `glibc_problems`'s "
            "docstring; no command shape, input, regime or gate moved, so no "
            "reading can"
        ),
        verified="git show --format= -U0 2f75ff4 -- scripts/measure.py  # one docstring",
    ),
    Acknowledged(
        commit="7f5002d",
        figures=(
            "census-brace-free",
            "census-arrays",
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "per-block-quadratic",
            "peak-rss",
            "preamble-prepass",
            "parallel-peak-rss",
            "statistics-gathering",
            "reserve",
            "map-only",
            "rss-attribution",
        ),
        why=(
            "27.3 changed the query replay (`replay`, `TablePartitions`, "
            "`TableStream`, `StreamShared`) and `prune.rs`, which only a query's "
            "plan reaches; `pgdt parse` runs `map_file` → `map_file_watched` → "
            "`map_forward`/`splice`, none touched, so no parse timing moves; "
            "`query-nomatch` replays no segment, gaining one `Arc<AtomicU64>` "
            "per stream"
        ),
        verified=(
            "git show --format= -U0 7f5002d -- pgdump_query/src/stream.rs | grep '^@@'  "
            "# no hunk inside splice, map_forward, map_file or map_file_watched"
        ),
    ),
)
