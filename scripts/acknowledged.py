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
        commit="d6b3660",
        figures=("session-drift",),
        why=(
            "27.11's instrument adds a rows leg to the two dynamic-filter figures and "
            "moves `--profile-recipe`'s pair; every command shape the stamped pair ran "
            "builds a byte-identical script, and no drift, emit or timing path moved"
        ),
        verified=(
            "git show --format= -U0 d6b3660 -- scripts/measure.py  # dfcli, dynfilter "
            "and profile hunks only; each of the 87 prior command_shapes() _script() equal"
        ),
    ),
    Acknowledged(
        commit="186399e",
        figures=("dynamic-filter-join", "dynamic-filter-topk"),
        why=(
            "28.2's levels: the figures' builder still parses at the data level with "
            "census and statistics, so the query path gains only the provider's "
            "planning-time check of the table's blocks for a missing census, a "
            "census_metadata_level pass that is a no-op over a data-level cache, and "
            "about a byte per block in the cache encoding"
        ),
        verified=(
            "git show 186399e -- datafusion-pgdump/src/table.rs datafusion-pgdump/src/dump.rs  "
            "# build() adds one blocks_of() scan before planning; nothing on the scan path"
        ),
    ),
    Acknowledged(
        commit="8b4db51",
        figures=("session-drift",),
        why=(
            "M180's bar: measure.py gains a table of barred figures, a refusal in main() "
            "before any figure is taken, and text in --list and --stale; which figures a "
            "sitting may select changes, and nothing a timed run executes does"
        ),
        verified=(
            "git show 8b4db51 -- scripts/measure.py  "
            "# hunks: usage docstring, BARRED, cmd_list, cmd_stale, main()'s selection guard"
        ),
    ),
    Acknowledged(
        commit="4c0399d",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "preamble-prepass",
            "parallel-scan-throughput",
            "per-block-quadratic",
            "peak-rss",
            "parallel-peak-rss",
            "reserve",
        ),
        why=(
            "28.3's count: each figure times only `parse --statistics-level metadata` (or "
            "`--preamble-only`), where map_forward and the leader build no counter since both "
            "gate it on the census, and the eager pass that counts unconditionally is reached "
            "only by build_index/build_map, which only tests call; what is left is about a byte "
            "per block and four per file in the cache encoding"
        ),
        verified=(
            "git show 4c0399d -- pgdump_query/src/stream.rs  "
            "# map_forward: header = census.then(..); leader plan = header.map(..); "
            "graft callers build_index  # tests only"
        ),
    ),
    Acknowledged(
        commit="179cce7",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "preamble-prepass",
            "parallel-scan-throughput",
            "per-block-quadratic",
            "peak-rss",
            "parallel-peak-rss",
            "reserve",
        ),
        why=(
            "28.4's views: each figure times only `parse --statistics-level metadata` (or "
            "`--preamble-only`), where the gatherer is reached only through a data-level "
            "request (map_forward's observer_for, backfill_statistics behind "
            "StatisticsRequest::gathers) and nothing is pruned; what is left is the cache "
            "format version"
        ),
        verified=(
            "graft callers observer_tracking  # map_forward's observer_for and reread_block; "
            "sed -n 1500,1520p pgdump_query/src/stream.rs  # back-fill only if statistics.gathers()"
        ),
    ),
    Acknowledged(
        commit="3d49806",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "nested-end-to-end",
            "cross-file-floor",
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
        why=(
            "M182's rename: ComparisonSemantics::Arrow becomes ::DataFusion, with the "
            "identifiers, test names and rustdoc built on it and rustfmt's reflow of the "
            "longer lines; it compiles to the same code, and the one runtime difference is "
            "the nested-column refusal's text, which no figure's workload reaches"
        ),
        verified=(
            "git show --format= --word-diff=porcelain 3d49806 -- pgdump_query/src "
            "datafusion-pgdump/src  # every changed word is Arrow->DataFusion or a reflow"
        ),
    ),
    Acknowledged(
        commit="e1f05d3",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "nested-end-to-end",
            "cross-file-floor",
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
        why=(
            "M183's rename: arrow_order and the arrow_divergence family, BoundsSet::Arrow, "
            "ColumnStatistics::arrow_bounds and NestedArrowOrder named for DataFusion "
            "semantics, with rustdoc and rustfmt's reflow; it compiles to the same code, "
            "bincode does not serialize field names, and the one output difference is "
            "pgdt info --json's key, which no figure's workload reads"
        ),
        verified=(
            "git show --format= --word-diff=porcelain e1f05d3 -- '*/src/*.rs'  "
            "# every changed word is arrow->datafusion, A->F or a reflow"
        ),
    ),
    Acknowledged(
        commit="6648707",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "per-block-quadratic",
            "peak-rss",
            "map-only",
            "preamble-prepass",
            "parallel-peak-rss",
            "rss-attribution",
            "reserve",
        ),
        why=(
            "28.6's refusal at planning: its only change to these figures' paths is "
            "stream.rs's ReplayPlan::new, which a parse never builds and a query matching "
            "no table enters with no blocks, returning before the new per-column walk; "
            "every leg here is a parse, an info or a no-match query"
        ),
        verified=(
            "git show 6648707 -- pgdump_query/src/stream.rs  # materialized_unrepresentable "
            "returns on matches.first() == None; measure.py's legs for these figures are "
            "parse*, info-cache-rss and query-nomatch*"
        ),
    ),
    Acknowledged(
        commit="3294e24",
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
        why=(
            "28.7's untyped mode: stream.rs computes the widened set only under "
            "UnrepresentableMode::Text, and read_as_text returns before touching the schema "
            "when no column is marked; predicate.rs adds a variant to a matches!; pgdt's "
            "main.rs adds a CLI value, its help and a match arm. No figure's leg chooses an "
            "unrepresentable mode, so every one runs the default null mode"
        ),
        verified=(
            "git show 3294e24 -- pgdump_query/src/stream.rs pgdump_query/src/predicate.rs "
            "pgdt/src/main.rs  # TableColumns::settle's text is Vec::new() off (Typed, Text); "
            "read_as_text returns on !text.contains(&true); grep -i unrepresentable "
            "scripts/measure.py names no mode"
        ),
    ),
    Acknowledged(
        commit="caa3bab",
        figures=(
            "allocator",
            "chunk-size",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "map-only",
            "nested-end-to-end",
            "parallel-peak-rss",
            "parallel-scan-throughput",
            "peak-rss",
            "per-block-quadratic",
            "preamble-prepass",
            "predicate-terms",
            "projection-widths",
            "reserve",
            "rss-attribution",
            "scan-throughput-cold",
            "scan-throughput-nvme",
            "scan-throughput-warm",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why=(
            "28.8's unrepresentable test: per row, eval_value only moves as_read below the "
            "compared check and adds an arm on the path of operators taking no value; per "
            "row group, one tests_unrepresentable branch per term; per block, query_tests "
            "returns on a filter holding no test; batch.rs and statistics.rs are a rename and "
            "a visibility; the provider's pushdown answers every other filter as before, and "
            "install rebuilds the session from its own state, config kept, walking each "
            "logical plan once. No figure's filter holds the test"
        ),
        verified=(
            "git show caa3bab -- pgdump_query/src/predicate.rs pgdump_query/src/stream.rs "
            "pgdump_query/src/batch.rs pgdump_query/src/prune.rs pgdump_query/src/statistics.rs "
            "datafusion-pgdump/src/table.rs datafusion-pgdump/src/pushdown.rs  # "
            "install uses SessionStateBuilder::new_from_existing; grep -i unrepresentable "
            "scripts/measure.py names no mode or test"
        ),
    ),
    Acknowledged(
        commit="949fb2f",
        figures=(
            "dynamic-filter-join",
            "dynamic-filter-topk",
        ),
        why=(
            "28.11's embedder guard: registration now calls register_udf where it rebuilt "
            "the session, doing strictly less; the shell's state is the one "
            "new_with_config_rt builds (SessionStateBuilder with_config, with_runtime_env, "
            "with_default_features) plus one physical optimizer rule walking each plan's "
            "expressions once at planning, touching no scan code. Neither figure's query "
            "names pgdump_unrepresentable"
        ),
        verified=(
            "git show 949fb2f -- datafusion-cli-pgdump/src/main.rs datafusion-pgdump/src/lib.rs "
            "datafusion-pgdump/src/factory.rs datafusion-pgdump/src/unrepresentable.rs  # "
            "SessionContext::new_with_config_rt in DataFusion 55.1.0 "
            "core/src/execution/context/mod.rs builds the same state; grep -i unrepresentable "
            "scripts/measure.py names no test"
        ),
    ),
)
