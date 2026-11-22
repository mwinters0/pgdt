"""What a figure declares (`Figure`), what a dry run of its legs reports (`StandIn`), the cache its
legs read built ahead of them (`Prebuilt`), and the library paths its `depends` names."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Mapping
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from measurement.runspec import RunSpec, Shared, Subtraction


@dataclass(frozen=True)
class StandIn:
    """What one dry-run leg reports in place of a run (`Figure.stand_in`)."""

    #: The key-value lines the leg's own streams would carry, which the
    #: session keeps as `Session.reported`.
    reported: Mapping[str, str]
    #: The keys an instrument leg's report carries beyond the session's
    #: generic stand-in report; read only where the spec declares the
    #: instrument.
    instrument: Mapping[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class Prebuilt:
    """A cache some of a figure's legs read, built by `Session.prebuild` in a
    container of its own, untimed, once a sitting for each input and regime
    those legs read, and mounted read-only beside the dump in their timed
    containers (`Figure.prebuilt`)."""

    #: The command-shape prefix of the legs that read it.
    reads: str
    #: The builder's whole script, writing the cache to `{cache}`, which the
    #: session fills with a path in the directory it mounts writable.
    builder: str


@dataclass
class Figure:
    id: str
    #: A human label for the measurements.md section this table belongs under.
    #: **Not an address**: the doc addresses a figure by its `<!-- figure: id -->`
    #: marker, so a heading may quote a number and may be rewritten when the
    #: number moves without losing its table. The label is what a sitting
    #: renders as that heading, so `--check` holds it to begin with the heading
    #: its marker sits under (`section_label_problems`): a heading rewritten
    #: takes its label with it. Not read from the heading instead: `--list`,
    #: the sweep log and a sitting's grouping print it without the doc.
    section: str
    #: The regimes this figure is taken in, `+`-separated — every token a key
    #: of `REGIMES`, or one of `NON_REGIME_STAGES` for a figure that reads no
    #: staged input. `--stage` selects on it, and `registered_regimes` is what
    #: the harness's regime vocabulary is reconciled against, so a token here
    #: that nothing declares is an error rather than a silent extra regime.
    stage: str
    #: Repo-relative paths whose change invalidates this figure. `--stale`
    #: intersects these with a diff. A figure that cannot say what invalidates
    #: it is one nobody has thought about.
    depends: tuple[str, ...]
    #: Caption for this figure's table, when its section holds more than one.
    #: The cold and warm throughput tables share a section on purpose: the
    #: `INSERT` path's per-byte CPU is then a division within one place rather
    #: than across two regimes, which is the defect that allocated the warm
    #: table in the first place.
    table_label: str = ""
    cold_inputs: tuple[str, ...] = ()
    warm_inputs: tuple[str, ...] = ()
    #: A figure whose warm set outgrows `warm_bound`, split: each group is
    #: staged and swept on its own, in this order, so `Session.sweep` runs a
    #: spec only beside the specs over its own group. Inputs share a group
    #: wherever a declared `Subtraction` pairs them, and an input two groups'
    #: subtractions both need is measured in each (`RunSpec.sweep`). Empty is
    #: one sweep over `warm_inputs`.
    warm_groups: tuple[tuple[str, ...], ...] = ()
    #: Inputs this figure reads cold off the NVMe. A third list rather than a
    #: flag on `cold_inputs`, because the two name different *devices* and a
    #: figure is free to want both -- which is what the throughput section
    #: does, one table per regime over the same three files.
    nvme_inputs: tuple[str, ...] = ()
    #: Readings this figure takes from another rather than measuring them.
    #: The borrow graph, declared: `requires` and `sharing_closure` are both
    #: read off it, and `share_readings` writes the table's provenance
    #: paragraph from it.
    shares: tuple[Shared, ...] = ()
    #: The cross-input readings this figure reads, its own reps' or another
    #: figure's.
    subtracts: tuple[Subtraction, ...] = ()
    #: The container memory limit this figure's runs are given, where the
    #: recorded 512 MB is not what it needs. It is an **apparatus** departure,
    #: so a figure that sets it says so in its own table: the register's one
    #: line is "3.00 GiB inputs read by a `glibc` binary in a 512 MB
    #: container", and a figure holding N decoded 24 MiB blocks
    #: at once cannot be one of them at 24 workers. Held under
    #: `contained.MAX_FRACTION` of the machine (`memory_preflight`).
    memory: str | None = None
    #: Consumers that repeat this figure's numbers **without naming it** — the
    #: residue the scan cannot find. `depends` is the edge into a figure; the
    #: edge out is `consumers()`, computed from who spells the figure's id, and
    #: declaring it beside each figure is what left every tuple pointing at a
    #: document a retarget had already replaced. What stays here is the manual
    #: and the README: they state the claim to a reader who will never see a
    #: figure id, so nothing in their text can be matched against one.
    also_quoted_by: tuple[str, ...] = ()
    #: Whether every `pgdt` leg of this figure runs under the instruction
    #: counter (`COUNT_INSTRUCTIONS`), its count kept beside the wall clock
    #: (`Session.instructions`). A figure declaring it declares
    #: `COUNTER_INSTRUMENT` too, and its containers carry
    #: `COUNT_INSTRUCTIONS_CAPS`; `emit` refuses it before the first reading
    #: where the counter does not count (`counter_preflight`).
    counts_instructions: bool = False
    #: What a dry run of a leg reports in place of the run's own report, given
    #: the spec and the rep's pseudo-random bytes (`figures.stand_ins`): a
    #: renderer refusing a leg whose report it cannot read, a dry run must
    #: reach every division it performs, as `test_stand_ins` holds every
    #: selectable figure's to. `None` reports nothing.
    stand_in: Callable[[RunSpec, bytes], StandIn] | None = None
    #: The cache this figure's legs read built ahead of the sweep, where a
    #: cold leg cannot build it in its own timed container.
    prebuilt: Prebuilt | None = None

    @property
    def requires(self) -> tuple[str, ...]:
        """The figures whose readings this one uses, pulled in automatically.

        Derived from `shares` rather than declared beside it: two lists of the
        same fact drift, and the one that drifts is the one no run function
        reads."""
        return tuple(dict.fromkeys(s.source for s in self.shares))

    @property
    def staging_groups(self) -> tuple[tuple[str, ...], ...]:
        """The warm sets this figure stages one at a time: its `warm_groups`,
        or its whole warm set as one, or none."""
        if self.warm_groups:
            return self.warm_groups
        return (self.warm_inputs,) if self.warm_inputs else ()

    @property
    def sweeps(self) -> tuple[tuple[str, ...], ...]:
        """The input sets this figure measures together: its `warm_groups`, or
        every input it reads as one sweep."""
        if self.warm_groups:
            return self.warm_groups
        return (tuple(dict.fromkeys((*self.warm_inputs, *self.cold_inputs, *self.nvme_inputs))),)

#: The paths behind each mechanism a figure can depend on. Declared narrowly
#: on purpose: a blanket `crates/` would mark every figure stale on
#: every commit, which is a `--stale` nobody reads. The cost of narrowness is
#: that a change *outside* these paths that moves a figure goes unannounced —
#: which is why the doc carries a session stamp as well, so "are these figures
#: from before or after my change" has a second answer.
#: The streams: the mapping pass, `pgdt-mapping`'s, a cold query's table
#: settled over it and planned, and the replay, a module of one file a
#: concern, each named, since a declared path is matched by prefix and
#: `stream.rs` is not a prefix of `stream/`; their tests are not declared.
STREAM = (
    *(
        f"crates/pgdt-mapping/src/{name}.rs"
        for name in ("backfill", "context", "forward", "forward/save", "whole_file")
    ),
    "crates/pgdt-query/src/dump.rs",
    "crates/pgdt-query/src/dump/settle.rs",
    "crates/pgdt-query/src/stream.rs",
    *(
        f"crates/pgdt-query/src/stream/{name}.rs"
        for name in (
            "merge",
            "note",
            "partitions",
            "replay",
            "replay/dynamic",
            "replay/partition",
            "replay/plan",
            "replay/segment",
            "resume",
            "table",
        )
    ),
)

#: The interior split: a block's interior scanned in pieces and folded, and
#: the scheduler handing them out.
INTERIOR = (
    "crates/pgdt-mapping/src/interior.rs",
    "crates/pgdt-mapping/src/interior/piece.rs",
)

SCAN = (
    "crates/pgdt-dump/src/scan.rs",
    "crates/pgdt-dump/src/scan/chunked.rs",
    "crates/pgdt-core/src/copy.rs",
    "crates/pgdt-core/src/identifier.rs",
    "crates/pgdt-dump/src/lex.rs",
    *STREAM,
)

#: Every figure that times a `pgdt` run over a file reads its bytes through
#: this one crate, whatever else the figure is about, so it is its own
#: mechanism rather than part of `SCAN`: `nested-end-to-end` and
#: `cross-file-floor` declare no scanner path and are still moved by it.
#: The read path was undeclared until the buffer pool landed, which made a
#: change to the largest single term in a warm `parse`'s user time read green
#: against every table it moved.
READ = tuple(
    f"crates/pgdt-io/src/{name}.rs"
    for name in (
        "arrangement",
        "lib",
        "memory",
        "parallelism",
        "partition",
        "source",
        "source/local",
        "source/origin",
        "source/pool",
        "source/remote",
        "source/compressed",
        "source/compressed/blocks",
        "source/compressed/fetched",
        "source/compressed/file",
        "source/compressed/index",
        "source/compressed/xz",
    )
)

MAP = (
    "crates/pgdt-dump/src/map.rs",
    "crates/pgdt-dump/src/map/statement.rs",
    "crates/pgdt-dump/src/map/tiling.rs",
    "crates/pgdt-dump/src/map/toc.rs",
    "crates/pgdt-model/src/span.rs",
    "crates/pgdt-model/src/persisted/span.rs",
)

#: The map's own per-block cost is two modules, not one: `map::Builder::snapshot`
#: clones the span list and `forward::splice` rebuilds the index from it,
#: under the save throttle's gate. A figure that prices the map declares both,
#: or a change to the gate reads green against a table it just moved.
MAP_BUILD = (*MAP, *STREAM)

#: Where a filter term is resolved and evaluated, and the ordering keys it
#: compares by: the key's dispatch, and each family's key type beside its
#: reader (which `DECODE` declares too). Its own mechanism because
#: until `predicate-terms` no registered figure passed a filter at all, so
#: nothing in the register was moved by this file and nothing declared it --
#: which is the same shape of blindness the read path had before `READ`.
PREDICATE = (
    "crates/pgdt-query/src/predicate.rs",
    *(
        f"crates/pgdt-query/src/predicate/{name}.rs"
        for name in ("eval", "notes", "resolve", "truths")
    ),
    "crates/pgdt-codec/src/key.rs",
    "crates/pgdt-codec/src/key/form.rs",
    "crates/pgdt-codec/src/key/order.rs",
    "crates/pgdt-codec/src/value.rs",
    "crates/pgdt-codec/src/value/datetime.rs",
    "crates/pgdt-codec/src/value/int.rs",
    "crates/pgdt-codec/src/value/json.rs",
    "crates/pgdt-codec/src/value/nested/key.rs",
    "crates/pgdt-codec/src/value/network.rs",
    "crates/pgdt-codec/src/value/numeric.rs",
)

#: Per-type scalar decode and render-back. Its own mechanism rather than part
#: of `NESTED`, because the figures it moves are not the ones `NESTED` moves:
#: every figure that runs a *typed* query pays it on every scalar column,
#: including `projection-widths`, whose narrow rows have no nested column in
#: them at all. Undeclared until the scalar decoders lost their per-field
#: allocations, which is the third instance of the blindness `READ` and
#: `PREDICATE` record: a file nothing declared because no slice had yet
#: touched it.
DECODE = (
    "crates/pgdt-codec/src/decode.rs",
    "crates/pgdt-codec/src/value.rs",
    "crates/pgdt-codec/src/value/bit.rs",
    "crates/pgdt-codec/src/value/bool.rs",
    "crates/pgdt-codec/src/value/bytea.rs",
    "crates/pgdt-codec/src/value/datetime.rs",
    "crates/pgdt-codec/src/value/digits.rs",
    "crates/pgdt-codec/src/value/float.rs",
    "crates/pgdt-codec/src/value/geometric.rs",
    "crates/pgdt-codec/src/value/int.rs",
    "crates/pgdt-codec/src/value/json.rs",
    "crates/pgdt-codec/src/value/libc.rs",
    "crates/pgdt-codec/src/value/network.rs",
    "crates/pgdt-codec/src/value/numeric.rs",
    "crates/pgdt-codec/src/value/text.rs",
    "crates/pgdt-codec/src/value/uuid.rs",
)

CACHE = (
    "crates/pgdt-dump/src/cache.rs",
    "crates/pgdt-dump/src/cache/file.rs",
    "crates/pgdt-dump/src/cache/identity.rs",
    "crates/pgdt-dump/src/cache/mode.rs",
    "crates/pgdt-dump/src/cache/store.rs",
)

#: The type register, the comparison register its built-in table answers
#: beside it, the declared type's catalog walk both read, and the input
#: grammar the comparison register's unordered positions carry.
PGTYPE = (
    "crates/pgdt-codec/src/pgtype.rs",
    "crates/pgdt-codec/src/declared.rs",
    "crates/pgdt-codec/src/compare.rs",
    "crates/pgdt-codec/src/compare/walk.rs",
    "crates/pgdt-codec/src/grammar.rs",
    "crates/pgdt-codec/src/grammar/check.rs",
    "crates/pgdt-codec/src/grammar/input.rs",
)

#: The unrepresentable count, which every data-level mapping pass takes beside
#: the census (`docs/design/decisions.md`, "D96"), and the fold of a row into
#: both, in `block.rs`, beside the count's persisted shape.
UNREPRESENTABLE = (
    "crates/pgdt-codec/src/unrepresentable.rs",
    "crates/pgdt-model/src/block.rs",
    "crates/pgdt-model/src/persisted/block.rs",
)

#: The nested literal codec, and the batch it is built into and read back
#: from: what a query asks for, the batcher, its column builders and retained
#: chunks, and the render back to text; their tests are not declared.
NESTED = (
    "crates/pgdt-codec/src/value/nested.rs",
    "crates/pgdt-codec/src/value/nested/output.rs",
    "crates/pgdt-codec/src/value/nested/input.rs",
    *(
        f"crates/pgdt-query/src/{name}.rs"
        for name in ("batch", "batch/builder", "batch/chunks", "options", "render")
    ),
)

PREAMBLE = (
    "crates/pgdt-dump/src/preamble.rs",
    "crates/pgdt-dump/src/ddl.rs",
    "crates/pgdt-dump/src/ddl/column.rs",
    "crates/pgdt-dump/src/ddl/metadata.rs",
    "crates/pgdt-dump/src/ddl/refs.rs",
    "crates/pgdt-dump/src/ddl/syntax.rs",
    "crates/pgdt-dump/src/ddl/table.rs",
    "crates/pgdt-dump/src/ddl/types.rs",
    "crates/pgdt-dump/src/lex.rs",
    "crates/pgdt-model/src/catalog.rs",
    "crates/pgdt-model/src/persisted/catalog.rs",
)

#: A query figure also reads through the CLI's own row rendering. The whole
#: `src/` directory rather than `main.rs`: a declared path is matched by
#: prefix, so naming the one file leaves every other module in that crate as a
#: staleness edge nobody declared -- and the crate is a module a subcommand,
#: the rendering in `query.rs`.
QUERY_CLI = ("crates/pgdt/src/",)

#: The resident-set instrument every resident figure reads through
#: (`PEAK_RSS`): a figure running a shape under it declares it, since a change
#: to what it reads moves every such reading.
RSS_INSTRUMENT = ("crates/peak-rss/src/",)

#: The instruction counter a counted figure reads through
#: (`COUNT_INSTRUCTIONS`), declared by it for `RSS_INSTRUMENT`'s reason.
COUNTER_INSTRUMENT = ("crates/count-instructions/src/",)

GEN_PERF = ("scripts/generate_perf_data.py",)

GEN_BLOCKS = ("scripts/generate_block_count_bench.py",)

GEN_SHAPES = (
    "scripts/generate_perf_data.py",
    "scripts/generate_large_object_bench.py",
    "scripts/generate_insert_run_bench.py",
)

#: The pruning input's generator, and the perf generator whose rows it writes.
GEN_PRUNING = ("scripts/generate_pruning_bench.py", *GEN_PERF)

#: The dynamic-filter input's generator, and the perf generator likewise.
GEN_DYNFILTER = ("scripts/generate_dynamic_filter_bench.py", *GEN_PERF)

#: `pgdt sql`: the provider it reads the dump through, and DataFusion's CLI
#: around it, the library `pgdt` calls.
DATAFUSION = ("crates/datafusion-pgdump/src/", "crates/datafusion-cli-pgdump/src/")

#: Where statistics are gathered, stored and read back. `pgdt-mapping`'s
#: `statistics` is the gathering, `model/persisted/statistics.rs` what is
#: stored and `model/statistics.rs` how it is read;
#: `PGTYPE`, `resolve.rs` and `PREDICATE` are the comparison a bound is
#: keyed and ordered under.
STATISTICS = (
    "crates/pgdt-mapping/src/statistics.rs",
    *(
        f"crates/pgdt-mapping/src/statistics/{name}.rs"
        for name in (
            "account",
            "density",
            "gather",
            "gather/block",
            "gather/bounds",
            "gather/charge",
            "gather/checking",
            "gather/clip",
            "gather/column",
            "gather/gatherer",
            "gather/order",
            "gather/piece",
            "gather/read",
            "gather/strict",
            "request",
            "stored",
        )
    ),
    "crates/pgdt-model/src/statistics.rs",
    "crates/pgdt-model/src/persisted/statistics.rs",
    *PGTYPE,
    "crates/pgdt-codec/src/resolve.rs",
    *PREDICATE,
)

#: What a query figure's reading carries beyond its rows, its query timed over
#: a data-level cache (`DATA_LEVEL_QUERIES`): the cache, decoded whole inside
#: the timer, and the census and statistics the untimed builder left in it.
#: Merged into a figure's own paths with `_declare`, which drops a repeat.
CACHED_QUERY = (*MAP, *CACHE, *STATISTICS)


def _declare(*paths: str) -> tuple[str, ...]:
    """`paths` in order, each once: a path declared twice is printed twice by
    `--list`."""
    return tuple(dict.fromkeys(paths))
