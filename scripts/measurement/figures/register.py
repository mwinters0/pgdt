"""The register: every figure's declaration, in run order -- a figure that shares a reading comes
after the figure that takes it -- and the queries over it by id."""

from __future__ import annotations

from generate_gzip_input import (
    GNU_GZIP_VERSION,
    ZLIB_VERSION,
    ZLIBNG_COMPAT_VERSION,
    ZLIBNG_VERSION,
)

import generate_long_lines as long_lines

from measurement import inputs
from measurement import runspec
from measurement import shapes
from measurement.figures import declaration
from measurement.figures import stand_ins


def subtraction_sweep(source: str, a: str, b: str) -> int:
    """The sweep of `source` in which a reading across `a` and `b` is taken:
    the one group holding both, for a pair some figure declares.

    **An undeclared pair is refused**, and so is a declared one no single
    group holds: either is a difference whose two sides may come from two
    sweeps, which is a session's drift published as a between-file one."""
    pair = {a, b}
    if not any(
        s.source == source and set(s.inputs) == pair
        for fig in EVERY_FIGURE
        for s in fig.subtracts
    ):
        raise ValueError(
            f"no figure declares a subtraction of {a!r} and {b!r} over {source}'s reps "
            "(`Figure.subtracts`)"
        )
    homes = [gi for gi, group in enumerate(EVERY_BY_ID[source].sweeps) if pair <= set(group)]
    if len(homes) != 1:
        raise ValueError(
            f"{source} measures {a!r} and {b!r} together in {len(homes)} sweeps, not one"
        )
    return homes[0]

#: The figure's two sweeps: each nested file with a control of its own, since
#: two of them and a control do not fit `warm_bound` and every reading across
#: files is against control (`Figure.subtracts`). The first sweep's control is
#: the one `cross-file-floor` and the allocator table read.
_NESTED_SWEEPS: tuple[tuple[str, str], ...] = (("control", "composite"), ("control", "arrays"))

# -- the per-block quadratic ------------------------------------------------

_BLOCK_COUNTS = (500, 1000, 2000, 4000)

#: The series, and the control that makes it readable as one: the same byte
#: count in **one** `COPY` block. Without it the table shows a cost rising with
#: block count and cannot say how much of the cost *is* block count — the
#: one-block run is what the 4000-block figure is a multiple of. A control the
#: harness does not run is a control that goes stale silently.
_QUADRATIC_ROWS: tuple[tuple[str, str], ...] = (
    ("one_block", "1 (control)"),
    *tuple((f"blocks{n}", f"{n}") for n in _BLOCK_COUNTS),
)

# -- what a scan holds resident ---------------------------------------------

#: The four rows of `peak-rss`, in table order: an input and what it varies.
#:
#: **Three readings, two controlled comparisons, one pivot.** `one_block` is
#: the pivot; `control` is the *same generator, same seed, same shape* at 1536x
#: the bytes, so the first two rows differ in bytes and nothing else; and the
#: block-count rows hold the byte count near the pivot's while multiplying the
#: block count by 500 and 4000. That is what lets one table answer both halves
#: of the claim it carries -- nothing accumulates per byte, nothing accumulates
#: per block -- which is precisely what koji, at 74 blocks over 784 GB, cannot:
#: a cost that scales with block count cannot express itself in it at all.
_RSS_ROWS: tuple[str, ...] = ("one_block", "control", "blocks500", "blocks4000")

#: The pivot every subtraction in this table is taken against.
_RSS_PIVOT = "one_block"

# -- what that growth is made of --------------------------------------------

#: The two block counts every leg of `rss-attribution` is taken at.
#:
#: **Two, not one.** A single absolute at 4,000 blocks folds in a baseline that
#: differs by 110 MiB between allocators, and jemalloc would read as
#: catastrophic on absolutes when its *slope* is within 35% of glibc's. Both
#: are `generate_block_count_bench.py` outputs, so the pair differs in block
#: count and in nothing else, and each reading the table publishes is a slope
#: in blocks rather than one number with a fixed baseline inside it.
_ATTRIBUTION_INPUTS: tuple[str, str] = ("blocks500", "blocks4000")

#: The nine legs, in table order: what the table calls the leg, which binary
#: runs it, and which command shape it is.
#:
#: **The two allocators besides the shipped one are named, never
#: re-specified.** What a leg's
#: binary *is* -- `--no-default-features`, its own target dir, `--version` read
#: back -- is the `allocator` figure's apparatus rule, so this figure calls
#: `ensure_allocator_binary` rather than carrying a second recipe for the same
#: builds, which is how two recipes for one binary come to differ. What is
#: genuinely this figure's own is the two shapes no other figure runs -- the
#: preamble prepass and `info` over a finished cache -- which are what separate
#: a cost paid per *table* from one paid per `COPY` block.
_ATTRIBUTION_LEGS: tuple[tuple[str, str, str], ...] = (
    ("`parse` — the `peak-rss` row", "pgdt", "parse-rss"),
    ("`parse`, system", "alloc:system", "parse-rss"),
    ("`parse`, jemalloc", "alloc:jemalloc", "parse-rss"),
    ("`parse --preamble-only`", "pgdt", "parse-preamble-rss"),
    ("`info --cache` over the finished cache", "pgdt", "info-cache-rss"),
    ("`query` (no match), cached", "pgdt", "query-nomatch-cached-rss"),
    ("`query` (no match), `--cache none`", "pgdt", "query-nomatch-rss"),
    ("`query` (no match), `--cache none`, system", "alloc:system", "query-nomatch-rss"),
    ("`query` (no match), `--cache none`, jemalloc", "alloc:jemalloc", "query-nomatch-rss"),
)


def _attribution_specs() -> list[tuple[str, runspec.RunSpec, runspec.RunSpec]]:
    """Each leg's label and its two specs, small block count first.

    A leg is identified by binary *and* shape, never by its label: `RunSpec.key`
    carries neither the label nor anything else, so two legs differing only in
    the words the table prints would silently share one reading. A test holds
    the nine apart."""
    return [
        (
            label,
            *(
                runspec.RunSpec(binary, name, command, "warm", f"{label} — {name}")
                for name in _ATTRIBUTION_INPUTS
            ),
        )
        for label, binary, command in _ATTRIBUTION_LEGS
    ]

# -- the allocator ----------------------------------------------------------

#: The three headline shapes, over the control file, warm, each paired with
#: the figure whose reading of it is shared rather than retaken.
#:
#: These are the shapes the phase's baseline table quotes and the shapes the
#: profile was taken on: `parse` is discovery, and the two `query` modes are
#: row extraction with and without typing. Nothing narrower would answer the
#: question, because the allocator's share of each is different -- the
#: allocator is ~8% of a `typed` query's user time and does not appear in
#: `parse`'s profile at all, and it is `parse` where the answer turned out to
#: be decided.
_ALLOCATOR_SHAPES: tuple[tuple[str, str, str], ...] = (
    ("parse", "`pgdt parse` — structure discovery", "scan-throughput-warm"),
    (
        "query-strings",
        "`query --schema-mode strings` — zero-copy extraction",
        "nested-end-to-end",
    ),
    ("query-typed", "`query --schema-mode typed`", "nested-end-to-end"),
)

#: The same three shapes as a borrow graph the register can read. Built from
#: `_ALLOCATOR_SHAPES` rather than written out again, so a shape added there
#: cannot be one the harness forgets to share.
ALLOCATOR_SHARES: tuple[runspec.Shared, ...] = tuple(
    runspec.Shared(
        source,
        f"the reference column's `{command}` row",
        (runspec.RunSpec("pgdt", "control", command, "warm", ""),),
    )
    for command, _, source in _ALLOCATOR_SHAPES
)

#: The one reading this figure does not take on the budget axis: the shipped
#: serial arrangement, at the library's own `DEFAULT_MEMORY_BUDGET`.
#:
#: **It is `peak-rss`'s `control` row, spec for spec**, which is why this
#: figure declares it as its one `Shared` edge — the same binary, command,
#: input and regime, so measuring it twice would put two numbers in the doc for
#: one measurement. A sitting that took `peak-rss` borrows it; one that did not
#: measures it here and says so in the table's own provenance paragraph.
_RESERVE_BASELINE = runspec.RunSpec("pgdt", "control", "parse-rss", "warm", "plain, serial default")

#: `gzip-resident`'s three legs: the registered input, what the table calls
#: it, and which charge its `parse` runs under.
#:
#: **Two charges, because a gzip `parse` runs under two.** A single
#: member's first `parse` is its first pass, charged per worker with no pool
#: (`gzip_first_pass_charge`); a BGZF file is walked at open, so its first
#: `parse` block-decodes as an `.xz` file's does (`gzip_block_charge`). Those
#: two are compressions of `control` at `pg_dump`'s level, so neither's
#: plaintext or zlib differs from the other's.
#:
#: **The third is the first pass's falsifier**: a member whose chunks reach
#: the speculation limit (`repeated_gz`), the only file filling the plaintext
#: terms most of a task's charge is (`gzip_task_bytes`) on every chunk, so the
#: only one on which each added worker can hold more than it was charged
#: (`docs/design/decisions.md`, "D124"). The same zlib, level
#: and length of plaintext as `control_gz`, packed tighter.
GZIP_RSS_LEGS: tuple[tuple[str, str, str], ...] = (
    ("control_gz", "single member, first pass", "pass"),
    ("repeated_gz", "single member at the limit, first pass", "pass"),
    ("control_bgzf", "BGZF, block path", "block"),
)

#: The read path a gzip `parse` adds to `READ`: the deflate adapter, its first
#: pass and walk, and the crate beneath them. Kept apart from `READ` so a
#: change here leaves the `.xz` and plain figures current.
GZIP_READ = (
    "crates/pgdt-io/src/source/compressed/deflate.rs",
    "crates/pgdt-io/src/source/compressed/deflate/",
    "crates/pgdt-gzip-seek/src/",
    "crates/pgdt-gzip-seek/Cargo.toml",
)

# -- what a parallel gzip first pass buys -----------------------------------

#: `gzip-first-pass`'s single members, in its table's column order: the
#: registered input, the writer that wrote it, and what that file is to a
#: reader. **Every one is `control`**, so a leg's plaintext and its rate's
#: denominator are one file's and the legs differ by their writer's blocks
#: alone, which is what a speculation finds a chunk's start by.
GZPASS_WRITERS: tuple[tuple[str, str, str], ...] = (
    ("control_gz", f"zlib {ZLIB_VERSION}, level 6", "`pg_dump -Z` over stock zlib"),
    (
        "control_gz_ng",
        f"zlib-ng-compat {ZLIBNG_COMPAT_VERSION}, level 6",
        "`pg_dump -Z` on Fedora 44, whose zlib is zlib-ng's compatibility build",
    ),
    ("control_gz_gnu", f"GNU gzip {GNU_GZIP_VERSION}, level 6", "`pg_dump | gzip`"),
    (
        "control_gz_quick",
        f"zlib-ng {ZLIBNG_VERSION}, level 1",
        "a writer ending no block before its last, whose first pass the frontier carries alone",
    ),
)

# -- what the data level costs, and what its statistics buy ----------------

#: `statistics-gathering`'s inputs, in the table's row order: the three
#: scan-throughput inputs, and the `--arrays --composite` file. **The control
#: and the arrays file bracket the census**: a control row holds no array, so
#: the census reads only its counted columns, and every arrays row holds three
#: nested values it inspects as well, so the two rows' profiles of the `data`
#: leg attribute the census on the shape most dumps have and on the one it
#: exists for. **Two of the inputs hold no `COPY` row**, and a data-level
#: `parse` over them observes nothing: their rows are what the level costs
#: where it records nothing, which is the control's row read from the other
#: side.
_STATISTICS_ROWS: tuple[tuple[str, str], ...] = (
    ("control", "`COPY` block"),
    ("arrays", "`COPY` block, arrays in every row"),
    ("large_object", "Large-object region"),
    ("insert_run", "`INSERT` run"),
)


def long_line_groups(kind: str) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """`kind`'s sweeps, as the lengths' tokens: every length but the bound,
    which the secant and its residuals subtract; then the lengths read at one
    worker alone beside the secant's longer end, their anchor.

    **Two sweeps because the bound's cluster outgrows the tmpfs room beside
    the ladder** (`warm_bound`): an anchor of the secant's longer end, measured
    in each, lets the bound's residual subtract within its own sweep and take
    from the other only the secant, a per-byte rate, never a reading
    (`Subtraction`)."""
    ladder = tuple(
        length for length in inputs.long_line_lengths(kind) if length not in inputs.LONG_LINE_SERIAL_ONLY
    )
    return ladder, (inputs.LONG_LINE_SECANT[1], *inputs.LONG_LINE_SERIAL_ONLY)


def long_line_warm_groups() -> tuple[tuple[str, ...], ...]:
    """Every sweep, in order: each kind's (`long_line_groups`)."""
    return tuple(
        tuple(inputs.long_line_input(kind, length) for length in group)
        for kind in long_lines.KINDS
        for group in long_line_groups(kind)
    )


# -- what each purge delay leaves resident -----------------------------------


def purge_delay_resident_groups() -> tuple[tuple[str, ...], ...]:
    """`purge-delay-resident`'s sweeps: each kind's ladder, as
    `long-line-resident` stages it, then its lines at the bound alone. No
    anchor beside the bound: nothing here subtracts across the two."""
    return tuple(
        tuple(inputs.long_line_input(kind, length) for length in group)
        for kind in long_lines.KINDS
        for group in (long_line_groups(kind)[0], inputs.LONG_LINE_SERIAL_ONLY)
    )

FIGURES: list[declaration.Figure] = [
    declaration.Figure(
        id="scan-throughput-cold",
        also_quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run cold, on the SSD",
        stage="cold",
        depends=(*declaration.SCAN, *declaration.MAP, *declaration.READ, *declaration.GEN_SHAPES),
        cold_inputs=("control", "large_object", "insert_run"),
    ),
    declaration.Figure(
        id="scan-throughput-warm",
        also_quoted_by=(
            "docs/design/pg-dump-compatibility.md",
            "docs/design/roadmap.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run warm, on tmpfs",
        stage="warm",
        depends=(*declaration.SCAN, *declaration.MAP, *declaration.READ, *declaration.GEN_SHAPES),
        warm_inputs=("control", "large_object", "insert_run"),
        warm_groups=(("control",), ("large_object",), ("insert_run",)),
    ),
    # The third device class, and the only one that can price the I/O
    # defaults: on the HDD and the SATA SSD the device is the whole cost and
    # on tmpfs there is no device at all, so a readahead, `fadvise` or
    # chunk-size change has nowhere to show. Like the other two, it borrows
    # nothing: its `COPY` row is its own reading.
    declaration.Figure(
        id="scan-throughput-nvme",
        also_quoted_by=(
            "docs/design/pg-dump-compatibility.md",
        ),
        section="Scan throughput by input shape",
        table_label="Every run cold, on the NVMe",
        stage="cold-nvme",
        depends=(*declaration.SCAN, *declaration.MAP, *declaration.READ, *declaration.GEN_SHAPES),
        nvme_inputs=("control", "large_object", "insert_run"),
    ),
    # The one of the three I/O defaults that is a value rather than a scheme,
    # and the only one that can be swept without a second build: `--chunk-size`
    # is a flag, so every row here is the same binary over the same file.
    # It borrows nothing -- the default row is a `parse` under a flag the
    # throughput tables do not pass, so it is its own reading even where the
    # number would look interchangeable with theirs.
    declaration.Figure(
        id="chunk-size",
        section="What the read chunk size is worth",
        stage="warm+cold+cold-nvme",
        depends=(*declaration.SCAN, *declaration.MAP, *declaration.READ, *declaration.QUERY_CLI, *declaration.GEN_PERF),
        warm_inputs=("control",),
        cold_inputs=("control",),
        nvme_inputs=("control",),
    ),
    declaration.Figure(
        id="nested-end-to-end",
        section="A typed query over nested columns costs 6.6 µs a row more than a string one",
        stage="warm",
        depends=declaration._declare(*declaration.NESTED, *declaration.DECODE, *declaration.MAP, *declaration.READ, *declaration.QUERY_CLI, *declaration.GEN_PERF, *declaration.CACHED_QUERY),
        warm_inputs=("control", "composite", "arrays"),
        warm_groups=_NESTED_SWEEPS,
        subtracts=(
            runspec.Subtraction(
                "nested-end-to-end", ("control", "arrays"), "the headline, paired per rep"
            ),
            *(
                runspec.Subtraction(
                    "nested-end-to-end",
                    ("control", nested),
                    "each nested file's `strings` leg against its control's",
                )
                for _, nested in _NESTED_SWEEPS
            ),
        ),
    ),
    declaration.Figure(
        id="cross-file-floor",
        section="The cross-file subtraction bottoms out at about half a microsecond a row",
        stage="warm",
        depends=declaration._declare(*declaration.NESTED, *declaration.DECODE, *declaration.READ, *declaration.QUERY_CLI, *declaration.GEN_PERF, *declaration.CACHED_QUERY),
        warm_inputs=("control", "control43"),
        shares=(
            #: Consumed, not republished: row 1 is `_per_row_diffs` over the
            #: nested sweep's own reps, which publishes a per-row difference
            #: rather than either of the readings it is taken from. So it
            #: orders the run and pulls the source in, and is not an edge of
            #: the sharing closure.
            runspec.Shared("nested-end-to-end", "row 1's per-rep differences"),
        ),
        subtracts=(
            runspec.Subtraction(
                "nested-end-to-end", ("control", "composite"), "row 1, the composite's share"
            ),
            runspec.Subtraction("cross-file-floor", ("control", "control43"), "row 2, the seed floor"),
        ),
    ),
    declaration.Figure(
        id="per-block-quadratic",
        section="Per-block cache saving is quadratic in block count, and so is the map",
        stage="warm",
        depends=(*declaration.MAP_BUILD, *declaration.READ, *declaration.CACHE, *declaration.GEN_BLOCKS, *declaration.GEN_PERF),
        warm_inputs=tuple(name for name, _ in _QUADRATIC_ROWS),
        #: The prose reads each block count's `parse` against the one-block
        #: control of the same bytes.
        subtracts=tuple(
            runspec.Subtraction(
                "per-block-quadratic",
                (_QUADRATIC_ROWS[0][0], name),
                "a block count against the control",
            )
            for name, _ in _QUADRATIC_ROWS[1:]
        ),
    ),
    # A figure whose reading is not a time. It is registered rather than left a
    # koji row because a claim outside the register has no `depends` edge to go
    # red when the read path moves, and koji's run captures no resident figure
    # a correction could come from. `depends` therefore carries the read path first — that is the
    # mechanism the claim is about — and the map and the cache, which are what
    # a per-block cost would accumulate in.
    declaration.Figure(
        id="peak-rss",
        also_quoted_by=(
            "docs/manual/dump-inspection.md",
            "README.md",
        ),
        #: The manual and the README state the *claim* this figure licenses to
        #: a reader who cannot check it against the code, and neither names the
        #: figure — which is what keeps them declared while the read path (`READ`), which
        #: quotes the reading and says `peak-rss` doing it, is computed.
        #:
        #: **Only half of the manual's sentence is this figure's.** "Does not
        #: grow with the size of the dump" is these rows; "grows with the
        #: number of tables, by roughly 10 KB each" is a per-*table* claim this
        #: figure's inputs cannot license, since `blocks4000` gives every table
        #: exactly one `COPY` block and the per-table and per-block axes
        #: coincide in it. That half is `rss-attribution`'s, which separates
        #: them by stopping a leg at the preamble.
        section="What a scan holds resident, per byte and per block",
        stage="warm",
        # `MAP` rather than `MAP_BUILD`: what the latter adds is `STREAM`,
        # which `SCAN` already names, and a path declared twice is printed
        # twice by `--list`.
        depends=(*declaration.RSS_INSTRUMENT, *declaration.READ, *declaration.SCAN, *declaration.MAP, *declaration.CACHE, *declaration.GEN_PERF, *declaration.GEN_BLOCKS),
        warm_inputs=_RSS_ROWS,
        subtracts=tuple(
            runspec.Subtraction("peak-rss", (_RSS_PIVOT, name), "a row against the pivot")
            for name in _RSS_ROWS
            if name != _RSS_PIVOT
        ),
    ),
    declaration.Figure(
        id="map-only",
        section="Per-block cache saving is quadratic in block count, and so is the map (map alone)",
        stage="warm",
        depends=(*declaration.MAP_BUILD, *declaration.READ, *declaration.QUERY_CLI, *declaration.GEN_BLOCKS),
        warm_inputs=("blocks1000", "blocks2000", "blocks4000"),
        #: The prose reads the series' growth per doubling, row by row.
        subtracts=(
            runspec.Subtraction("map-only", ("blocks1000", "blocks2000"), "the first doubling"),
            runspec.Subtraction("map-only", ("blocks2000", "blocks4000"), "the second doubling"),
        ),
    ),
    declaration.Figure(
        id="preamble-prepass",
        section="The preamble prepass is bounded by the schema, not by the dump",
        stage="warm",
        #: Its second row is `per-block-quadratic`'s 4000-block `parse` reading,
        #: borrowed rather than re-measured (`requires`, below) -- so this
        #: figure inherits that one's staleness edges as well as its own, or a
        #: change to the map moves a row here that reads green.
        depends=(*declaration.PREAMBLE, *declaration.MAP_BUILD, *declaration.READ, *declaration.CACHE, *declaration.GEN_BLOCKS),
        warm_inputs=("blocks4000",),
        shares=(
            runspec.Shared(
                "per-block-quadratic",
                "the full-`parse` row, which is the quadratic table's 4000-block `parse` cell",
                (runspec.RunSpec("pgdt", "blocks4000", "parse-cache-out", "warm", ""),),
            ),
        ),
    ),
    declaration.Figure(
        id="nested-decode-micro",
        section="Nested decode costs what it copies, and an element is now a borrowed slice",
        stage="criterion",
        depends=(
            "crates/pgdt-codec/src/value/nested.rs",
            "crates/pgdt-codec/src/value/nested/output.rs",
            "crates/pgdt-codec/benches/decoders.rs",
        ),
    ),
    # Which form a resumable reader takes: one reader as today's pull walk, an
    # `async` walk and a hand-written push machine, over `nested-decode-micro`'s
    # literals and a scalar, one run and a run a byte.
    declaration.Figure(
        id="resumable-reader-micro",
        section="Which form a resumable reader takes",
        stage="criterion",
        depends=(
            "crates/pgdt-codec/benches/resumable/main.rs",
            "crates/pgdt-codec/benches/resumable/pull.rs",
            "crates/pgdt-codec/benches/resumable/walk.rs",
            "crates/pgdt-codec/benches/resumable/push.rs",
        ),
    ),
    # `depends` carries the scan path as well as the decode one, which no other
    # query figure does: the zero-column row is an absolute reading of replay
    # with nothing decoded, so a change in what replay costs moves it directly
    # rather than cancelling out of a difference.
    declaration.Figure(
        id="projection-widths",
        section="What a column costs: five projection widths over one file",
        stage="warm",
        depends=declaration._declare(*declaration.SCAN, *declaration.NESTED, *declaration.DECODE, *declaration.READ, *declaration.QUERY_CLI, *declaration.GEN_PERF, *declaration.CACHED_QUERY),
        warm_inputs=("arrays",),
    ),
    # The only figure in the register that passes a filter, and therefore the
    # only one a change to `predicate.rs` or `predicate/` can move. `depends` carries the scan
    # path for the same reason `projection-widths` does — every row is an
    # absolute reading of replay with nothing decoded — and `QUERY_CLI`
    # because `--where` is parsed there. `batch.rs` is deliberately absent:
    # no row survives any of these predicates, so `push_row` never runs.
    declaration.Figure(
        id="predicate-terms",
        section="What a filter term costs, and how much of it is the walk to its field",
        stage="warm",
        depends=declaration._declare(*declaration.PREDICATE, *declaration.SCAN, *declaration.READ, *declaration.QUERY_CLI, *declaration.GEN_PERF, *declaration.CACHED_QUERY),
        warm_inputs=("control",),
    ),
    # `depends` is the union of what moves the three shapes it times, because
    # a table of *ratios* between allocators is invalidated by a change in
    # what any of the three shapes allocates -- which is most of the row path.
    # The CLI's own manifest is in there too: that is where the legs are
    # declared and where the shipped default lives, so a change to it changes
    # what this figure is a figure of.
    declaration.Figure(
        id="allocator",
        section="Which allocator a figure was taken under",
        stage="warm",
        depends=declaration._declare(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.READ,
            *declaration.QUERY_CLI,
            *declaration.GEN_PERF,
            # Its two `query` shapes read a data-level cache (`CACHED_QUERY`).
            *declaration.CACHE,
            *declaration.STATISTICS,
            # Where the legs are declared and where the shipped default lives,
            # so a change to it changes what this figure is a figure of.
            # `src/alloc.rs` needs no line of its own: `QUERY_CLI` is the
            # directory.
            "crates/pgdt/Cargo.toml",
        ),
        warm_inputs=("control",),
        shares=ALLOCATOR_SHARES,
    ),
    # What link-time optimization buys the shipped build, read once on the
    # one-crate tree and once on the split one. `depends` is `allocator`'s,
    # whose three shapes it times, plus where the legs are declared — the root
    # manifest's profiles — and `crates/pgdt/build.rs`, which names a leg's
    # profile, and what moves its fourth, the provider's typed scan: the
    # DataFusion crates, and the pruning its filter consults; and the counter
    # every leg runs under.
    declaration.Figure(
        id="release-profile",
        stand_in=stand_ins.parallel_scan,
        section="What link-time optimization buys the shipped build",
        stage="warm",
        depends=declaration._declare(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.READ,
            *declaration.QUERY_CLI,
            *declaration.GEN_PERF,
            *declaration.CACHE,
            *declaration.STATISTICS,
            "crates/pgdt-query/src/prune.rs",
            *declaration.DATAFUSION,
            "crates/pgdt/Cargo.toml",
            "crates/pgdt/build.rs",
            "Cargo.toml",
            *declaration.COUNTER_INSTRUMENT,
        ),
        warm_inputs=("control",),
        counts_instructions=True,
    ),
    declaration.Figure(
        id="xz-decode-scaling",
        stand_in=stand_ins.decode,
        section="What a second decode worker buys, and what the twenty-fourth does not",
        stage="warm-parallel",
        # Not the library's read path: no `pgdt` runs here at all. What can move
        # this figure is the decoder, the instrument that drives it, and the
        # generators behind the two files — including the perf generator, which
        # the control leg's bytes are a compression of.
        depends=(
            "vendor/xz-seek/src/",
            "crates/pgdt-io/examples/xz_decode.rs",
            "scripts/generate_xz_input.py",
            *declaration.GEN_PERF,
        ),
        warm_inputs=("control_xz", "koji_xz"),
        memory=shapes.DECODE_MEMORY,
    ),
    # A first pass's serial inflate against the plain parse, on every device.
    # What can move it is the instrument and the backends it pins, the
    # generators behind both files, and everything the `parse` row runs
    # through — the scanner, the map, the read path and the CLI.
    declaration.Figure(
        id="inflate-throughput",
        section="What serial inflate costs against the plain parse",
        stage="warm+cold+cold-nvme",
        depends=declaration._declare(
            "crates/gzip-inflate/src/",
            "crates/gzip-inflate/Cargo.toml",
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.QUERY_CLI,
        ),
        warm_inputs=("control", "control_gz"),
        cold_inputs=("control", "control_gz"),
        nvme_inputs=("control", "control_gz"),
        subtracts=(
            runspec.Subtraction(
                "inflate-throughput",
                ("control", "control_gz"),
                "each inflate row's time against the plain `parse`'s",
            ),
        ),
    ),
    # `pgdt-gzip-seek`'s first pass and resumed unit decodes on each engine it builds
    # over. What can move it is the crate and its example, the engines its
    # manifest pins, the inflate instrument behind the reference rows, and the
    # generators behind the file.
    declaration.Figure(
        id="gzseek-backend",
        section="Which engine `pgdt-gzip-seek` reads gzip through",
        stage="warm",
        depends=declaration._declare(
            "crates/pgdt-gzip-seek/src/",
            "crates/pgdt-gzip-seek/examples/gz_read.rs",
            "crates/pgdt-gzip-seek/Cargo.toml",
            "crates/gzip-inflate/src/",
            "crates/gzip-inflate/Cargo.toml",
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
        ),
        warm_inputs=("control_gz",),
    ),
    # gzip's own costs at each candidate checkpoint spacing, on the engine
    # `pgdt-gzip-seek` ships. What can move it is the crate and its example, the
    # engine its manifest pins, the inflate instrument behind the reference
    # rows, and the generators behind the file.
    declaration.Figure(
        id="gzseek-spacing",
        stand_in=stand_ins.gzip_spacing,
        section="What a checkpoint's spacing costs gzip",
        stage="warm",
        depends=declaration._declare(
            "crates/pgdt-gzip-seek/src/",
            "crates/pgdt-gzip-seek/examples/gz_read.rs",
            "crates/pgdt-gzip-seek/Cargo.toml",
            "crates/gzip-inflate/src/",
            "crates/gzip-inflate/Cargo.toml",
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
        ),
        warm_inputs=("control_gz",),
    ),
    # The parallel scan's throughput claim, and the first figure in the register
    # whose axis is the worker count of `pgdt` itself. `depends` is the union of
    # everything a parallel scan runs through — the scanner, the map, the read
    # path, the leader, the decoder, the CLI where `--jobs` is parsed, and the
    # provider its typed legs scan through — plus both generators behind its
    # inputs. It is wide on purpose: this figure is the one that would be
    # quietly wrong if any of them changed.
    declaration.Figure(
        id="parallel-scan-throughput",
        stand_in=stand_ins.parallel_scan,
        prebuilt=declaration.Prebuilt(
            f"{shapes.PARALLEL_SCAN_PREBUILT}-jobs-", shapes.PREBUILT_BUILDER
        ),
        section="What a second scan worker buys, and where the plain path stops",
        stage="warm-parallel+cold-nvme-parallel",
        depends=declaration._declare(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.QUERY_CLI,
            *declaration.INTERIOR,
            "vendor/xz-seek/src/",
            "scripts/generate_xz_input.py",
            *declaration.GEN_PERF,
            # Its provider legs read a data-level cache (`CACHED_QUERY`), and
            # prune by its statistics under their filter.
            *declaration.CACHE,
            *declaration.STATISTICS,
            "crates/pgdt-query/src/prune.rs",
            *declaration.DATAFUSION,
        ),
        warm_inputs=("control", "control_xz"),
        nvme_inputs=("control",),
        memory=shapes.PARALLEL_MEMORY,
    ),
    # The parallel scan's *memory* claim: one stated number bounds the read
    # path. `depends` is narrower than its sibling's — nothing here decodes a
    # field or renders a row, the shape being `parse` — but it carries the same
    # read path, leader and decoder, which is where a resident set is decided.
    declaration.Figure(
        id="parallel-peak-rss",
        section="What a parallel scan holds resident, at two block sizes",
        stage="warm-parallel",
        depends=(
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.QUERY_CLI,
            *declaration.INTERIOR,
            "vendor/xz-seek/src/",
            "scripts/generate_xz_input.py",
            *declaration.GEN_PERF,
            # The gate's twins are the `system` build, which these features
            # select (`GATE_LEG`).
            "crates/pgdt/Cargo.toml",
        ),
        # **The query path's sub-stream sizing does not reach this figure.** It
        # is in `stream::replay::partition::plan_partitions`, and every leg here is
        # `pgdt parse`, which reaches `worker_count` through
        # `pgdt_mapping`'s `scan_region` instead — so this figure is excused by
        # reachability where its sibling is not, though both declare the same
        # read path.
        warm_inputs=("control_xz", "control_xz128"),
        memory=shapes.PARALLEL_MEMORY,
    ),
    # The attribution's instrument.
    #
    # **It cannot be taken alone, and the `Shared` edge is what says so.** Its
    # `parse` reference row runs `peak-rss`'s `blocks500` and `blocks4000`
    # shapes — same binary, same command, same apparatus — and a reading is
    # keyed by figure *and* spec, so without the edge the two are separate
    # measurements of one run: the standalone readings and `peak-rss`'s
    # disagreed at 9.58/44.26 MiB against 9.73/43.78, two numbers for one
    # measurement a section apart. Standing in that share is also what confines
    # this figure to a stamped sweep (`sitting_problems`; `measurements.md`,
    # "A figure may be published outside the sweep").
    declaration.Figure(
        id="rss-attribution",
        #: The manual's per-*table* claim is declared here rather than on
        #: `peak-rss`, which cannot tell per-table from per-block on its own
        #: inputs: `blocks4000` gives every table exactly one `COPY` block, so
        #: the two coincide in it and only these legs separate them. The
        #: manual's sentence carries a second claim — that nothing accumulates
        #: per byte — which is `peak-rss`'s, so both figures declare that file.
        also_quoted_by=("docs/manual/dump-inspection.md",),
        section="What the per-block resident growth is made of",
        stage="warm",
        # What `peak-rss` declares, plus the two mechanisms only this figure's
        # own legs reach: the preamble prepass, which is where the per-*table*
        # structure is paid, and the CLI's `query` path, which the four
        # no-match legs run. The CLI manifest is here for the reason the
        # `allocator` figure carries it — three of the nine legs are its legs,
        # and that file is where they are declared.
        depends=(
            *declaration.RSS_INSTRUMENT,
            *declaration.READ,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.CACHE,
            *declaration.PREAMBLE,
            *declaration.QUERY_CLI,
            "crates/pgdt/Cargo.toml",
            *declaration.GEN_BLOCKS,
        ),
        #: Read off `_attribution_specs` rather than respelling the two
        #: `RunSpec`s: a leg is identified by its binary and shape, and a second
        #: spelling of one is what drifts.
        shares=(
            runspec.Shared(
                "peak-rss",
                "the `parse` reference row at both block counts",
                _attribution_specs()[0][1:],
            ),
        ),
        warm_inputs=_ATTRIBUTION_INPUTS,
        #: Every leg's slope, its `parse` row's over `peak-rss`'s reps where
        #: this sitting took that figure.
        subtracts=(
            runspec.Subtraction("rss-attribution", _ATTRIBUTION_INPUTS, "each leg's per-block slope"),
            runspec.Subtraction("peak-rss", _ATTRIBUTION_INPUTS, "the `parse` row's slope, borrowed"),
        ),
    ),
    # The budget rule's one number, the compressed path's account, and the third
    # resident-set instrument.
    #
    # **It stands in one edge, and the second is a stated non-edge.** Onto
    # `peak-rss`: its serial-default row is that figure's `control` row spec for
    # spec, so the two share a reading rather than take one each. Onto
    # `rss-attribution`: **none**, and that is a reading of the leg set rather
    # than an omission — that figure's every leg runs over
    # `blocks500`/`blocks4000`, the two block-count shapes whose axis it is,
    # where every leg here runs over `control`, `control_xz` or `control_xz128`,
    # so no two of their runs are the same run. *Rejected:* manufacturing an
    # edge by adding a block-count leg here — it would re-measure the plain path
    # on the compressed path's figure, to buy a borrow nothing reads. What the
    # three do share is a *sitting*: all three read resident, so one sweep takes
    # them together, and that collapse is the closure the harness computes
    # rather than an edge any one of them declares.
    declaration.Figure(
        id="reserve",
        stand_in=stand_ins.reserve,
        # The manual's `--memory` guidance and its claim that a
        # flagless scan stays inside its allocation are read off this table
        # without naming it, so they are declared rather than computed.
        also_quoted_by=("docs/manual/dump-inspection.md",),
        section="What a scan holds above the budget it was given",
        # Two regimes: every axis here occupies every hardware thread and is
        # gated as `warm-parallel`, while the serial-default row is the shape
        # `peak-rss` takes and is gated as that figure gates it.
        stage="warm+warm-parallel",
        # What `parallel-peak-rss` declares, which is where a resident set
        # under a stated budget is decided, plus the map and the cache — a
        # per-block cost accumulates in those and would read here as reserve.
        depends=(
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.CACHE,
            *declaration.QUERY_CLI,
            *declaration.INTERIOR,
            "vendor/xz-seek/src/",
            "scripts/generate_xz_input.py",
            *declaration.GEN_PERF,
            # The gate's twins are the `system` build, which these features
            # select (`GATE_LEG`).
            "crates/pgdt/Cargo.toml",
        ),
        # Every family's inputs, deduplicated: the stated axis's two, the
        # flagless axis's two block sizes, and the baseline's — which is one of
        # the stated axis's already and is named anyway, since the day it is not,
        # a staging list built off `RESERVE_INPUTS` alone leaves that row's file
        # unstaged.
        warm_inputs=tuple(
            dict.fromkeys(
                (
                    *(name for name, _ in shapes.RESERVE_INPUTS),
                    *(name for name, _, _ in shapes.RESERVE_FLAGLESS_INPUTS),
                    _RESERVE_BASELINE.input,
                )
            )
        ),
        #: The shipped serial arrangement, which is `peak-rss`'s `control` row
        #: spec for spec.
        shares=(
            runspec.Shared(
                "peak-rss",
                "the shipped serial arrangement's peak resident set",
                (_RESERVE_BASELINE,),
            ),
        ),
        memory=shapes.PARALLEL_MEMORY,
    ),
    # The data level's two figures, each standing in no sharing edge, so each
    # publishes outside a sweep from its own commit. `depends` for the first is
    # what a data-level scan runs through — the census in the map, the count
    # beside it, the gathering — and what writes what it recorded; its
    # `metadata` leg is the scan-throughput shape and declares that figure's
    # paths.
    declaration.Figure(
        id="statistics-gathering",
        section="What the data level costs a parse",
        stage="warm",
        depends=(
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.CACHE,
            *declaration.STATISTICS,
            *declaration.UNREPRESENTABLE,
            *declaration.QUERY_CLI,
            *declaration.GEN_SHAPES,
        ),
        warm_inputs=tuple(name for name, _ in _STATISTICS_ROWS),
        warm_groups=tuple((name,) for name, _ in _STATISTICS_ROWS),
        memory=shapes.STATISTICS_MEMORY,
    ),
    # Everything the timed query reads through — the replay, the cache it
    # loads, the pruning plan, the filter, the typed decode and batch build of
    # the rows that pass — plus the gathering, which decides what the untimed
    # builder leaves for the query to prune with.
    declaration.Figure(
        id="statistics-pruning",
        stand_in=stand_ins.pruning,
        section="What row-group statistics buy a query",
        stage="warm",
        depends=(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.CACHE,
            *declaration.STATISTICS,
            "crates/pgdt-query/src/prune.rs",
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.QUERY_CLI,
            *declaration.GEN_PRUNING,
        ),
        warm_inputs=("pruning",),
    ),
    # The spec's two figures for DataFusion's dynamic filters, each standing in
    # no sharing edge. Everything the timed query reads through — the provider
    # and the binary, the replay, the cache, the pruning plan, the filter, the
    # typed decode — plus the gathering, which decides what the untimed builder
    # leaves the scan to prune with.
    declaration.Figure(
        id="dynamic-filter-join",
        stand_in=stand_ins.dynamic_filter,
        section="What DataFusion's dynamic filters buy a query",
        table_label="A join's filter, over its probe table",
        stage="warm",
        depends=(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.CACHE,
            *declaration.STATISTICS,
            "crates/pgdt-query/src/prune.rs",
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.DATAFUSION,
            # `pgdt sql` is `pgdt`'s: its dispatch and allocator are the binary's.
            *declaration.QUERY_CLI,
            *declaration.GEN_DYNFILTER,
        ),
        warm_inputs=("dynfilter",),
    ),
    declaration.Figure(
        id="dynamic-filter-topk",
        stand_in=stand_ins.dynamic_filter,
        section="What DataFusion's dynamic filters buy a query",
        table_label="A TopK's filter, over the table it sorts",
        stage="warm",
        depends=(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.CACHE,
            *declaration.STATISTICS,
            "crates/pgdt-query/src/prune.rs",
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.DATAFUSION,
            # `pgdt sql` is `pgdt`'s: its dispatch and allocator are the binary's.
            *declaration.QUERY_CLI,
            *declaration.GEN_DYNFILTER,
        ),
        warm_inputs=("dynfilter",),
    ),
    # gzip's resident against its charge, at three worker counts over two
    # single members' first passes, one whose chunks reach the speculation
    # limit, and a BGZF file's block path. `depends` is
    # `parallel-peak-rss`'s read path with gzip's in place of xz's, plus the
    # map and the cache, where a per-block cost would read as remainder.
    declaration.Figure(
        id="gzip-resident",
        section="What a gzip scan holds against its charge",
        stage="warm-parallel",
        depends=declaration._declare(
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *GZIP_READ,
            *declaration.CACHE,
            *declaration.QUERY_CLI,
            *declaration.INTERIOR,
            "scripts/generate_gzip_input.py",
            "scripts/generate_repeated_rows.py",
            *declaration.GEN_PERF,
            # The gate's twins are the `system` build, which these features
            # select (`GATE_LEG`).
            "crates/pgdt/Cargo.toml",
        ),
        warm_inputs=tuple(leg for leg, _, _ in GZIP_RSS_LEGS),
        memory=shapes.GZPASS_MEMORY,
    ),
    # A gzip first pass's wall clock against the worker count, over four
    # writers' single members, and at two chunk lengths. `depends` is
    # `gzip-resident`'s read path less its resident instrument, plus the chunk
    # leg's feature, declared in both manifests.
    declaration.Figure(
        id="gzip-first-pass",
        stand_in=stand_ins.gzip_first_pass,
        section="What a parallel gzip first pass buys",
        stage="warm-parallel",
        depends=declaration._declare(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *GZIP_READ,
            *declaration.CACHE,
            *declaration.QUERY_CLI,
            *declaration.INTERIOR,
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
            "crates/pgdt/Cargo.toml",
            "crates/pgdt-io/Cargo.toml",
        ),
        warm_inputs=("control", *(leg for leg, _, _ in GZPASS_WRITERS)),
        warm_groups=tuple(("control", leg) for leg, _, _ in GZPASS_WRITERS),
        # The physical count read against the floor beside it, file by file.
        subtracts=tuple(
            runspec.Subtraction(
                "gzip-first-pass",
                ("control", leg),
                "the physical count's time over the plain `parse` read in its sweep",
            )
            for leg, _, _ in GZPASS_WRITERS
        ),
        memory=shapes.GZPASS_MEMORY,
    ),
    # What a pass holds while it holds a long line, by value kind, line length
    # and count: the parse and its gathering, the strict check, the
    # provider's query and replay, which builds the column it reads, and
    # `pgdt query`'s ordering filter, each query's factor read off its plan.
    declaration.Figure(
        id="long-line-resident",
        stand_in=stand_ins.long_line,
        section="What a held line holds resident",
        stage="warm-parallel",
        depends=declaration._declare(
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.INTERIOR,
            *declaration.CACHE,
            *declaration.STATISTICS,
            *declaration.UNREPRESENTABLE,
            *declaration.DECODE,
            *declaration.NESTED,
            *declaration.DATAFUSION,
            # `pgdt sql` is `pgdt`'s: its dispatch and allocator are the binary's.
            *declaration.QUERY_CLI,
            "scripts/generate_long_lines.py",
        ),
        warm_inputs=inputs.LONG_LINE_INPUTS,
        # A kind's sweeps (`long_line_groups`), so the tmpfs holds one
        # kind's lengths at a time, and the bound's cluster beside the
        # secant's longer end alone.
        warm_groups=long_line_warm_groups(),
        # The secant, each other length's residual against its shorter end,
        # and the bound's against its longer end, measured beside it.
        subtracts=tuple(
            runspec.Subtraction(
                "long-line-resident",
                (inputs.long_line_input(kind, a), inputs.long_line_input(kind, b)),
                what,
            )
            for kind in long_lines.KINDS
            for a, b, what in (
                (*inputs.LONG_LINE_SECANT, "a pass's secant between two line lengths"),
                *(
                    (inputs.LONG_LINE_SECANT[0], length, "a pass's residual against its secant")
                    for length in long_line_groups(kind)[0]
                    if length not in inputs.LONG_LINE_SECANT
                ),
                *(
                    (inputs.LONG_LINE_SECANT[1], length, "a pass's residual at the bound, in its sweep")
                    for length in inputs.LONG_LINE_SERIAL_ONLY
                    if length in inputs.long_line_lengths(kind)
                ),
            )
        ),
        memory=shapes.LONG_LINE_MEMORY,
    ),
    # What each purge delay costs a scan in wall clock, the shipped binary
    # under each leg's environment: `release-profile`'s paths, the parallel
    # scan's, and where the allocator and its delay are decided — mimalloc's
    # version is the lock file's.
    declaration.Figure(
        id="purge-delay",
        stand_in=stand_ins.parallel_scan,
        section="What mimalloc's purge delay costs, and what it leaves resident",
        stage="warm+warm-parallel",
        table_label="Wall clock",
        depends=declaration._declare(
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.NESTED,
            *declaration.DECODE,
            *declaration.READ,
            *declaration.QUERY_CLI,
            *declaration.INTERIOR,
            *declaration.GEN_PERF,
            *declaration.CACHE,
            *declaration.STATISTICS,
            "crates/pgdt-query/src/prune.rs",
            *declaration.DATAFUSION,
            "crates/pgdt/Cargo.toml",
            "Cargo.lock",
        ),
        warm_inputs=("control",),
    ),
    # What each purge delay leaves resident over `long-line-resident`'s legs:
    # that figure's paths, and the allocator's as above.
    declaration.Figure(
        id="purge-delay-resident",
        stand_in=stand_ins.long_line,
        section="What mimalloc's purge delay costs, and what it leaves resident",
        stage="warm-parallel",
        table_label="Peak resident over `long-line-resident`'s legs",
        depends=declaration._declare(
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.MAP,
            *declaration.READ,
            *declaration.INTERIOR,
            *declaration.CACHE,
            *declaration.STATISTICS,
            *declaration.UNREPRESENTABLE,
            *declaration.DECODE,
            *declaration.NESTED,
            *declaration.DATAFUSION,
            *declaration.QUERY_CLI,
            "scripts/generate_long_lines.py",
            "crates/pgdt/Cargo.toml",
            "Cargo.lock",
        ),
        warm_inputs=inputs.LONG_LINE_INPUTS,
        warm_groups=purge_delay_resident_groups(),
        memory=shapes.LONG_LINE_MEMORY,
    ),
]

FIGURES_BY_ID = {f.id: f for f in FIGURES}

#: Instruments that are **built but whose figure has not been taken**.
#:
#: A sweep does not run these and the doc carries no table *of this harness's*
#: for them, which is why they sit outside `ALL_FIGURES`: the marker
#: reconciliation would otherwise demand a section with no numbers under it.
#: `--figure <id>` still selects one, which is how the reading gets taken — and
#: taking it moves the entry into `FIGURES`, where the doc-side checks start
#: applying.
#:
#: The distinction is worth a list rather than a comment because *built* and
#: *taken* fail differently. An instrument nobody built is work; an instrument
#: built and never run is a claim nobody checked, and it is invisible unless
#: something names it.
#:
#: **Empty is the healthy state, not a disused mechanism.** A figure published
#: outside a stamped sweep declares inside its own
#: marker the commit it was taken at, and a sitting run from a working tree
#: carrying its own uncommitted apparatus has no such commit to name — so an
#: instrument built ahead of that commit waits here rather than in `FIGURES`.
#: *Rejected:* `composite-isolated`, which isolated one column by declaring it
#: two ways over byte-identical rows — `projection-widths` makes the same
#: isolation a subtraction between two adjacent rows of one table over one
#: file.
UNTAKEN: list[declaration.Figure] = []

#: A figure that no sweep produces, because it is computed *across* two of
#: them. It still gets a section, a marker and both declared edges — it is one
#: table like any other, and the register is what `--check` reconciles against
#: the doc.
DERIVED: list[declaration.Figure] = [
    declaration.Figure(
        id="session-drift",
        section="What a session's own drift costs, measured rather than asserted",
        stage="derived",
        # Nothing in the library invalidates this one: it measures the
        # apparatus, not the code. What can move it is the harness's own timing
        # path.
        depends=("scripts/measure.py", "scripts/measurement/"),
    )
]

ALL_FIGURES = FIGURES + DERIVED

ALL_BY_ID = {f.id: f for f in ALL_FIGURES}

#: Everything `--figure` may name: the sweep's figures plus the untaken
#: instruments. Not `ALL_FIGURES`, which is the set the *doc* must carry.
SELECTABLE = FIGURES + UNTAKEN

SELECTABLE_BY_ID = {f.id: f for f in SELECTABLE}

#: Every figure the register knows, whether or not a sweep takes it.
EVERY_FIGURE = FIGURES + UNTAKEN + DERIVED

EVERY_BY_ID = {f.id: f for f in EVERY_FIGURE}
