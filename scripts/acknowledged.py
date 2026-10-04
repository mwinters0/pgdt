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
    This is the other one: a change *inside* a declared path that provably
    moves nothing leaves `--stale` red until a sweep re-stamps the doc, and a
    sweep is about two hours on a machine that has to be quiet.

    So a commit can be excused, per figure, with its evidence attached -- by a
    reader about to reason from that figure, as the alternative to re-taking
    it. No commit owes an entry when it lands; red is the resting state
    (`measurements.md`, "A stale figure does not oblige a sweep").

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
    commit. So writing an entry does not mean the figure goes green: check
    `--stale`, which names an inert entry under the figure it failed to clear,
    and excuse every commit it names or re-take the figure.

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
        commit="a3058df5",
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
        why="adds one integer comparison per plan (span_on_chunk) and "
        "changes ParallelismBudgetLimited's lever list and message only where a "
        "span is stated below the chunk on a source the chunk does not size, which "
        "no figure's plain-file or .xz-block run reaches; outside that, "
        "#[cfg(test)] code only, no scan, decode or allocation path changed",
        verified="git diff -U0 a3058df5^ a3058df5 -- pgdump_query/src/stream.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
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
        why="the lexer runs only on lines outside a COPY block (CopyScanner's "
        "State::Outside arm, map.rs's INSERT-run end), a few KB of preamble on "
        "these COPY-dominated inputs; the in-block per-row path is untouched, the "
        "scanner's state changes by a Lexer replacing a dollar tag, and "
        "measure.py only adds lex.rs to SCAN. Owed re-takes, not excused: "
        "scan-throughput-* (insert_run legs), preamble-prepass, "
        "per-block-quadratic, map-only",
        verified="git show 8cffbac3 -- pgdump_query/src/scan.rs pgdump_query/src/stream.rs "
        "scripts/measure.py | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="1bd3d789",
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
        why="rewords four messages (an error's Display, an option's help, "
        "pgdt's statistics line when MemAvailable is unreadable, a plan note's "
        "text), each built once per plan or only when printed, and adds "
        "#[cfg(test)] code; no scan, decode or map path changes, so no timing or "
        "RSS reading can move",
        verified="git show 1bd3d789 -- pgdump_query/src pgdt/src "
        "datafusion-pgdump/src | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="e6ebe406",
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
        why="moves map.rs's Arc::new(statistics) inside the StatisticsScope "
        "on_copy_end already entered; without introspect that scope is an empty "
        "struct with no Drop, so a timed build does the same work, and an "
        "introspect build only attributes one small allocation per block to "
        "statistics, a line no table reads",
        verified="git show e6ebe406 -- pgdump_query/src/map.rs; "
        "grep -n -A16 'pub(crate) struct StatisticsScope' pgdump_query/src/instrument.rs",
    ),
    Acknowledged(
        commit="f6ef4636",
        figures=("session-drift",),
        why="joins two untimed builders to their timed command by && where "
        "they had ;, which times the same command whenever the builder succeeds, "
        "and widens _PARSE_RUN, a --check pattern no sitting runs; no shape "
        "session-drift times changes",
        verified="git show f6ef4636 -- scripts/measure.py | grep '^[-+]' "
        "| grep -v '^[-+][[:space:]]*#'",
    ),
    Acknowledged(
        commit="79917ef8",
        figures=("session-drift",),
        why="adds section_label_problems to --check and corrects one figure's "
        "section label, a heading string a sitting renders; no command shape, "
        "staging or timed path changes",
        verified="git show 79917ef8 -- scripts/measure.py | grep '^[-+]' "
        "| grep -v '^[-+][[:space:]]*#'",
    ),
    Acknowledged(
        commit="c112e3e4",
        figures=("session-drift",),
        why="extends section_label_problems' docstring; no code changes",
        verified="git show c112e3e4 -- scripts/measure.py",
    ),
    Acknowledged(
        commit="0112e9da",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "peak-rss",
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
        why="a repoint: comments only in lex.rs and preamble.rs, which "
        "comment_only_commit cannot place, so it reads neither file's change as "
        "comment-only; every other path the commit touched it does",
        verified="git show 0112e9da -- pgdump_query/src/lex.rs pgdump_query/src/preamble.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="7d722fa4",
        figures=("preamble-prepass", "rss-attribution"),
        why="admits KD61 and KD62 to their out-of-band rows: two deficiency "
        "markers in preamble.rs lose their promotion clause, comments only, which "
        "comment_only_commit cannot place in that file",
        verified="git show 7d722fa4 -- pgdump_query/src/preamble.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="4ecb705d",
        figures=(
            "projection-widths",
            "allocator",
            "parallel-scan-throughput",
            "statistics-pruning",
            "dynamic-filter-join",
            "dynamic-filter-topk",
        ),
        why="decode.rs changes only the branch a negative scale reaches "
        "with fewer digits than its zeros, and render_decimal's scale <= 0 arm; "
        "every measured input's numeric has a positive scale, so neither runs",
        verified="git show 4ecb705d -- pgdump_query/src/decode.rs; "
        "grep -n 'numeric(' scripts/generate_*.py",
    ),
    Acknowledged(
        commit="c68568ea",
        figures=("preamble-prepass", "rss-attribution"),
        why="files KD73: one deficiency marker added in preamble.rs above "
        "the Connect arm, comments only, which comment_only_commit cannot place "
        "in that file",
        verified="git show c68568ea -- pgdump_query/src/preamble.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="28fb62ae",
        figures=(
            "scan-throughput-cold",
            "scan-throughput-warm",
            "scan-throughput-nvme",
            "chunk-size",
            "peak-rss",
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
        why="lex.rs only widens ident_cont's visibility; preamble.rs's "
        "strip_kw adds one byte check after a keyword match, allocating nothing, "
        "and re-classifies only CREATE TABLESPACE, which no measured input holds",
        verified="git show 28fb62ae -- pgdump_query/src/lex.rs pgdump_query/src/preamble.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="1014ac5c",
        figures=("preamble-prepass", "rss-attribution"),
        why="dump_metadata_from_spans changes only its Connect and "
        "VersionHeader arms, a handful of spans per file, with the same clones "
        "and no new allocation; no per-line or per-row path moves",
        verified="git show 1014ac5c -- pgdump_query/src/preamble.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="a5b5f99f",
        figures=("preamble-prepass", "rss-attribution"),
        why="parse_table_element adds at most seven keyword-prefix checks "
        "per CREATE TABLE column-list entry, and the comma split two byte arms; "
        "both run on the preamble's table statements only, allocate nothing new, "
        "and keep fewer column definitions",
        verified="git show a5b5f99f -- pgdump_query/src/preamble.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="f6aedf59",
        figures=(
            "allocator",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "parallel-scan-throughput",
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "projection-widths",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="CACHE_FORMAT_VERSION's value moves 33 -> 34, a fixed-width u32 "
        "written and compared in the cache header alike; the rest is rustdoc and "
        "GOLDEN_ORDER, a constant inside a #[cfg(test)] module",
        verified="git show f6aedf59 -- pgdump_query/src/cache.rs pgdump_query/src/predicate.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="5d99d7a9",
        figures=(
            "allocator",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "parallel-scan-throughput",
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "projection-widths",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="map_numeric runs once per column as a schema resolves, never per "
        "row, and every generated input declares numeric(20,6) or numeric(12,2), "
        "which map to the type they did; cache.rs moves CACHE_FORMAT_VERSION's u32 "
        "value and predicate.rs a #[cfg(test)] constant",
        verified="git show 5d99d7a9 -- pgdump_query/src/pgtype.rs pgdump_query/src/cache.rs "
        "pgdump_query/src/predicate.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'; "
        "grep -n 'numeric(' scripts/generate_*.py",
    ),
    Acknowledged(
        commit="6418a001",
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
            "predicate-terms",
            "projection-widths",
            "reserve",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="no measured input declares an INHERITS or OF table, so every column "
        "resolves as before; classify tries one more keyword prefix per statement, and "
        "gather, prune and summary look a column up per block and column, never per row",
        verified="git show 6418a001 -- pgdump_query/src/gather.rs pgdump_query/src/map.rs "
        "pgdump_query/src/stream.rs pgdump_query/src/prune.rs pgdump_query/src/summary.rs "
        "pgdump_query/src/resolve.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="8bae73d7",
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
            "predicate-terms",
            "projection-widths",
            "reserve",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="two keyword prefixes per non-table CREATE, one parse per ALTER TYPE "
        "that adds no value and one prefix per \\connect, all per statement; no "
        "generated input holds an unlogged or foreign table, a DROP ATTRIBUTE or a "
        "connection-string \\connect; the rest is a constant, a test pin and a --map label",
        verified="git show 8bae73d7 -- pgdump_query/src/map.rs pgdump_query/src/preamble.rs "
        "pgdt/src/main.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="db949970",
        figures=("reserve",),
        why="in reserve's set only cache.rs changed, and there only "
        "CACHE_FORMAT_VERSION's u32 value, written and compared in the header alike",
        verified="git show db949970 -- pgdump_query/src/cache.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="d1caf819",
        figures=(
            "allocator",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "nested-end-to-end",
            "parallel-scan-throughput",
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "projection-widths",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="is_box runs once per array column as its schema resolves and changes "
        "the answer only for a quoted box element, which no generated input holds; the "
        "rest is CACHE_FORMAT_VERSION's u32 value and a #[cfg(test)] pin",
        verified="git show d1caf819 -- pgdump_query/src/pgtype.rs pgdump_query/src/cache.rs "
        "pgdump_query/src/predicate.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="4d946f8e",
        figures=(
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
        ),
        why="in these figures' sets only cache.rs and predicate.rs changed, "
        "CACHE_FORMAT_VERSION's u32 value and a #[cfg(test)] pin",
        verified="git show 4d946f8e -- pgdump_query/src/cache.rs pgdump_query/src/predicate.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="39466c97",
        figures=(
            "allocator",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "nested-end-to-end",
            "parallel-scan-throughput",
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "projection-widths",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="one match guard on a \\connect span that follows another, which no "
        "generated input holds; the rest is CACHE_FORMAT_VERSION's u32 value, a "
        "#[cfg(test)] pin and unit tests",
        verified="git show 39466c97 -- pgdump_query/src/cache.rs pgdump_query/src/predicate.rs "
        "pgdump_query/src/preamble.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="4d946f8e",
        figures=(
            "allocator",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "nested-end-to-end",
            "parallel-scan-throughput",
            "projection-widths",
            "statistics-pruning",
        ),
        why="acknowledged by the maintainer without a re-take: one is_infinite "
        "test per float decoded, changing the value only for a digit spelling past the "
        "type's largest finite value, which no generated input holds",
        verified="git show 4d946f8e -- pgdump_query/src/decode.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="0911d0fd",
        figures=(
            "allocator",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "nested-end-to-end",
            "parallel-scan-throughput",
            "predicate-terms",
            "projection-widths",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="a float literal's reader runs once per literal as a term resolves; "
        "on a row the only change is a bool guard on nested_key's leaf arm, and the field "
        "reader, the stored keys and GOLDEN_ORDER's field order are unchanged; the rest "
        "is accepted_form's wording and unit tests",
        verified="git show 0911d0fd -- pgdump_query/src/predicate.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="cb7ef7a0",
        figures=(
            "allocator",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "nested-end-to-end",
            "parallel-scan-throughput",
            "predicate-terms",
            "projection-widths",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="an integer literal's width check runs once per literal as a term "
        "resolves; on a row order_key's integer arm is the same i64 parse, CompareKind::Int "
        "carrying its width built once per column; the rest is accepted_form's wording "
        "and tests",
        verified="git show cb7ef7a0 -- pgdump_query/src/predicate.rs pgdump_query/src/pgtype.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="d9019a1a",
        figures=(
            "predicate-terms",
            "reserve",
        ),
        why="in these figures' sets only cache.rs and predicate.rs changed, "
        "CACHE_FORMAT_VERSION's u32 value, accepted_form's wording reached only on an "
        "error, and tests",
        verified="git show d9019a1a -- pgdump_query/src/cache.rs pgdump_query/src/predicate.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="79462282",
        figures=(
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
        ),
        why="in these figures' sets only cache.rs, predicate.rs and "
        "unrepresentable.rs changed: CACHE_FORMAT_VERSION's u32 value, an interval key's "
        "narrower integers with fewer checked operations, the interval tier test as one "
        "checked_mul, accepted_form's error wording, and tests",
        verified="git show 79462282 -- pgdump_query/src/cache.rs pgdump_query/src/predicate.rs "
        "pgdump_query/src/unrepresentable.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="6fef2f8c",
        figures=(
            "allocator",
            "cross-file-floor",
            "dynamic-filter-join",
            "dynamic-filter-topk",
            "nested-end-to-end",
            "parallel-scan-throughput",
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "projection-widths",
            "reserve",
            "rss-attribution",
            "statistics-gathering",
            "statistics-pruning",
        ),
        why="the per-row bound runs only on a bare numeric field or a jsonb number, "
        "and every generated input declares numeric(20,6) or numeric(12,2), read by the "
        "unchanged Decimal arm, and no jsonb; a literal's bound runs once per term; the rest "
        "is CACHE_FORMAT_VERSION's u32 value, accepted_form's error wording and tests",
        verified="git show 6fef2f8c -- pgdump_query/src/decode.rs pgdump_query/src/predicate.rs "
        "pgdump_query/src/cache.rs | grep '^[-+]' | grep -v '^[-+][[:space:]]*//'; "
        "grep -n 'numeric\\|jsonb' scripts/generate_*.py",
    ),
    Acknowledged(
        commit="74eea90d",
        figures=(
            "peak-rss",
            "preamble-prepass",
            "predicate-terms",
            "reserve",
            "rss-attribution",
        ),
        why="in these figures' sets only cache.rs and predicate.rs changed, "
        "CACHE_FORMAT_VERSION's u32 value, accepted_form's error wording and tests",
        verified="git show 74eea90d -- pgdump_query/src/cache.rs pgdump_query/src/predicate.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="100e377a",
        figures=(
            "nested-decode-micro",
            "reserve",
        ),
        why="reserve's set changed only CACHE_FORMAT_VERSION's u32 value; "
        "nested-decode-micro runs the decoders bench filtered to its nested group, and the "
        "bench changed only inside float_family's numeric case",
        verified="git show 100e377a -- pgdump_query/src/cache.rs pgdump_query/benches/decoders.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
    Acknowledged(
        commit="e318c3e5",
        figures=(
            "chunk-size",
            "map-only",
            "parallel-peak-rss",
            "peak-rss",
            "per-block-quadratic",
            "preamble-prepass",
            "reserve",
            "rss-attribution",
            "scan-throughput-cold",
            "scan-throughput-nvme",
            "scan-throughput-warm",
        ),
        why="these are metadata-level scans, which gather no statistics; in their "
        "sets on_copy_end returns a Result whose Refused arm only a gathering observer "
        "reaches, eager_pass and close_copy_block propagate it, the back-fill and re-read "
        "paths a metadata scan never takes, pgdt's error match gains one arm, "
        "CACHE_FORMAT_VERSION's u32 value, and leader.rs's change is test-only",
        verified="git show e318c3e5 -- pgdump_query/src/map.rs pgdump_query/src/stream.rs "
        "pgdump_query/src/cache.rs pgdump_query/src/leader.rs pgdt/src/main.rs "
        "| grep '^[-+]' | grep -v '^[-+][[:space:]]*//'",
    ),
)
