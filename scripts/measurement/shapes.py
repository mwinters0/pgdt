"""The command shapes: every named shape's in-container shell (`_script`), the parameters each
family's shapes are built from, and the checks holding every shape to the harness's rules -- a
stated worker count, a stated statistics level, a pinned budget."""

from __future__ import annotations

import math
import re
from fractions import Fraction
from typing import Mapping

import generate_perf_data as perf

import generate_long_lines as long_lines

from measurement import config
from measurement import inputs
from measurement import memory_model
from measurement import parsing


#: The perf table's columns, split the way the generator writes them: the 16
#: scalars, then the two arrays under `--arrays`, then the composite under
#: `--composite`. Taken from the generator so a rename there reaches the
#: projections below.
_PERF_SCALARS = tuple(name for name, _ in perf.COLUMNS)

_PERF_ARRAYS = tuple(name for name, _ in perf.ARRAY_COLUMNS)

_PERF_COMPOSITE = tuple(name for name, _ in perf.COMPOSITE_COLUMNS)

#: The five projection widths `projection-widths` is taken at, over the
#: `--arrays --composite` file's 19 columns. Every list is a *subsequence* of
#: the file's own column order, so no row introduces a reordering as a second
#: variable, and each is a superset of the row above it -- which is what makes
#: the difference between two adjacent rows the cost of exactly the columns
#: they differ by.
#:
#: Each key is that width's column count, and a test is what holds it so.
#: Writing `len(_PERF_SCALARS)` as the key instead would keep the mapping
#: consistent with itself while quietly renaming the rows the doc's table
#: carries.
PROJECTION_WIDTHS: dict[int, tuple[str, ...]] = {
    0: (),
    1: ("v_smallint",),
    16: _PERF_SCALARS,
    17: _PERF_SCALARS + _PERF_COMPOSITE,
    19: _PERF_SCALARS + _PERF_ARRAYS + _PERF_COMPOSITE,
}


def projection_flags(width: int) -> str:
    """The `pgdt query` flags that ask for one width.

    Zero columns is `--no-columns` rather than an empty repetition of
    `--column`, which is the CLI this phase specifies; every other width
    repeats `--column`, because a comma-separated list cannot express a column
    name containing a comma."""
    if width not in PROJECTION_WIDTHS:
        raise ValueError(f"no projection registered at width {width}")
    names = PROJECTION_WIDTHS[width]
    if not names:
        return "--no-columns"
    return " ".join(f"--column {name}" for name in names)

#: The predicate shapes `predicate-terms` is taken at: a column of the control
#: file, and how many terms are asked against it.
#:
#: **Every term is false for every row, and the terms are OR'd.** `Or`
#: evaluates its children until one is `True`, so an all-false disjunction
#: evaluates every one of them on every row -- and no row survives, so the
#: decode, the Arrow build and the render are identically absent from every
#: row of the table. That is what makes the difference between two rows here
#: the predicate and nothing else. A conjunction would do the opposite: `And`
#: stops at the first non-`True` conjunct, so an N-term one costs what a
#: 1-term one does.
#:
#: **Two depths, because what a shared split would remove is the walk.**
#: `ResolvedTerm::eval` takes its field through the row's shared `RowSplit`,
#: which walks from the front of the row once, as far as the deepest term
#: asks -- so terms against the control's thirteenth column walk thirteen
#: fields and terms against its first walk one, whatever their count. `v_bool` is the deep column rather than the last one because the two
#: depths must differ by the walk and not by what decoding the field costs,
#: and its values are one byte; `v_escaped` and `v_long_text` are the two the
#: row order otherwise recommends and both are the wrong shape for that.
PREDICATE_SHAPES: dict[str, tuple[str, int]] = {
    "deep-1": ("v_bool", 1),
    "deep-2": ("v_bool", 2),
    "deep-3": ("v_bool", 3),
    "deep-5": ("v_bool", 5),
    "shallow-5": ("id", 5),
    "shallow-1": ("id", 1),
}


def predicate_expr(shape: str) -> str:
    """The `--where` expression one shape asks for.

    Distinct literals rather than one repeated, so the tree really is N terms:
    nothing folds two terms (`decisions.md`, "Predicates"), but a table
    whose rows differ only in how often one term is repeated invites the
    reader to wonder."""
    if shape not in PREDICATE_SHAPES:
        raise ValueError(f"no predicate registered as {shape!r}")
    column, terms = PREDICATE_SHAPES[shape]
    if terms < 1:
        raise ValueError(f"predicate {shape!r} asks for no terms")
    return " OR ".join(f"{column}=zzz{i}" for i in range(1, terms + 1))

#: The read chunk sizes `chunk-size` is taken at, in bytes, smallest first.
#: `1 << 20` is the shipped default (`pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES`) and every
#: other row is read against it.
#:
#: **The range brackets the read path's own buffer-pool ceiling.** 8 MiB is
#: the largest buffer `source::pool::BufferPool::give` keeps for a length nobody
#: announced; a read loop announces its chunk size, so a 16 MiB chunk is
#: pooled like the rest. The 16 MiB row is in the table on purpose either way:
#: a sweep that stopped at the ceiling would leave a reader to assume the curve
#: continues, and this row is what says whether it does.
CHUNK_SIZES: tuple[int, ...] = (64 << 10, 256 << 10, 1 << 20, 4 << 20, 8 << 20, 16 << 20)

#: The worker counts `xz-decode-scaling` is taken at, and the row order of its
#: table. One is the probe this figure replaces; 24 is every hardware thread on
#: the recorded apparatus; 12 is its physical cores, which is where SMT stops
#: adding a core and starts sharing one.
DECODE_WORKERS: tuple[int, ...] = (1, 2, 4, 8, 12, 16, 24)

#: The `inflate-throughput` figure's command family: one serial inflate of a
#: whole gzip member by the `gzip-inflate` instrument, a shape a backend.
INFLATE_FAMILY = "inflate-"

#: The deflate backends that figure is taken over, in its table's row order,
#: each spelled as the instrument's `--backend` spells it, zlib-ng first as
#: the reference `pgdt-gzip-seek`'s throughput floor is stated against
#: (`docs/design/decisions.md`, "D127").
INFLATE_BACKENDS: tuple[str, ...] = ("zlib-ng", "zlib-rs", "miniz-oxide")

#: Every inflate shape, `INFLATE_FAMILY` and a backend.
INFLATE_SHAPES: tuple[str, ...] = tuple(f"{INFLATE_FAMILY}{b}" for b in INFLATE_BACKENDS)

#: The `gzseek-backend` figure's command family: `pgdt-gzip-seek`'s `gz_read`
#: example over one gzip member, a shape a stage and an engine.
GZSEEK_FAMILY = "gzseek-"

#: The engines `pgdt-gzip-seek` builds over, each spelled as the crate's feature, in
#: the table's row order, **the crate's default first** — `crates/pgdt-gzip-seek/
#: Cargo.toml`'s `default`, which a test holds this to.
GZSEEK_ENGINES: tuple[str, ...] = ("zlib-rs", "zlib-ng", "zlib")

#: The instrument's `--shape`s: the checkpointing first pass alone, and that
#: pass followed by every unit decoded alone from its checkpoint.
GZSEEK_STAGES: tuple[str, ...] = ("forward", "units")

#: Every `pgdt-gzip-seek` shape, `GZSEEK_FAMILY`, a stage and an engine.
GZSEEK_SHAPES: tuple[str, ...] = tuple(
    f"{GZSEEK_FAMILY}{stage}-{engine}" for engine in GZSEEK_ENGINES for stage in GZSEEK_STAGES
)

#: The checkpoint spacing the backend figure reads every engine at. **Not the
#: spacing that ships**, which `gzseek-spacing` set on the engine this one
#: picked (`docs/design/decisions.md`, "D113"): one within xz's block sizes,
#: so each engine's per-unit fixed cost is in its unit rows at a count the
#: shipped spacing could give, and the decoder stops at every block's end
#: whatever the spacing (`crates/pgdt-gzip-seek/src/forward.rs`).
GZSEEK_SPACING = 32 * config.MIB

#: The `gzseek-spacing` figure's command family: `pgdt-gzip-seek`'s `gz_read`
#: example on the engine that ships, a shape a stage and a spacing in MiB.
GZSPACING_FAMILY = "gzspacing-"

#: Its `--shape`s: the first pass alone; that pass and every unit begun and
#: brought to its first plaintext byte, whose excess over the first is a
#: resume's fixed cost times the unit count; and that pass and every unit
#: decoded whole.
GZSPACING_STAGES: tuple[str, ...] = ("forward", "starts", "units")

#: The candidate spacings, in MiB of plaintext, and the table's row order:
#: powers of two across xz's block sizes — from the 1 MiB floor of `xz -T`'s
#: default block (three times the dictionary, at least 1 MiB: `xz(1)`,
#: `--block-size`) through the 24 and 128 MiB blocks the xz figures read —
#: since a unit is also the decode unit, the block-cache unit and the
#: partition grain (`docs/design/decisions.md`, "D113"). The smallest is
#: where a resume's fixed cost is multiplied most, so it is the row that
#: resolves it.
GZSPACING_MIB: tuple[int, ...] = (1, 2, 4, 8, 16, 32, 64, 128)

#: Every spacing shape, `GZSPACING_FAMILY`, a stage and a spacing.
GZSPACING_SHAPES: tuple[str, ...] = tuple(
    f"{GZSPACING_FAMILY}{stage}-{mib}" for mib in GZSPACING_MIB for stage in GZSPACING_STAGES
)

#: The `gzip-first-pass` figure's command family: `pgdt parse` of a single
#: gzip member, its first pass, at a worker count its own axis carries
#: (`GZPASS_JOBS`) under `GZPASS_BUDGET`. **Not a `JOBS_AXIS` family**, whose
#: counts and budget are the `parallel-*` figures': exempted from `SWEEP_JOBS`
#: by its own prefix, as the decode instrument's is, and still stating its
#: count.
GZPASS_FAMILY = "parse-gzpass-jobs-"

#: The `gzip-resident` figure's command family: `pgdt parse` of a gzip file
#: under the peak-resident wrapper, at a count its own axis carries
#: (`GZIP_RSS_JOBS`) under `GZPASS_BUDGET`. `parallel-peak-rss`'s shape over
#: gzip, but for the budget, which is why it is a family of its own rather
#: than that figure's `JOBS_AXIS` rows: exempted from `SWEEP_JOBS` by its own
#: prefix, as `GZPASS_FAMILY` is.
GZRSS_FAMILY = "parse-gzrss-jobs-"

#: The `long-line-resident` figure's command family, `<family><pass>-<jobs>`:
#: one pass over a cluster of long lines under the peak-resident wrapper, at a
#: count of `LONG_LINE_JOBS`, under `long_line_allowance`. Its own
#: prefix, exempted from `SWEEP_JOBS` as `GZRSS_FAMILY` is, its counts being
#: its axis; a gathering family (`GATHERING_FAMILIES`), a default parse being
#: one of its passes.
LONG_RSS_FAMILY = "long-rss-"

#: `long-line-resident`'s instrument legs, `<family><pass>`: a query or filter
#: pass of `LONG_LINE_PLANNED` on the introspection build, once at
#: `SWEEP_JOBS`, stating no allowance, read for the factor its plan settled
#: (`planned_factor`, `crates/pgdt/src/introspect.rs`) and never for a
#: resident. A gathering family as `LONG_RSS_FAMILY` is, its builder the same.
LONG_FACTOR_FAMILY = "long-factor-"

#: The passes, in the table's column order, each with what its column is
#: called: a parse at the data level, the request stated as every gathering
#: figure states it (`GATHER_STATISTICS`); the same parse told to check every
#: field (`--postgres-invalid-values strict`); a query decoding every value,
#: the provider's (`long_line_invocation`); and `pgdt query`'s ordering filter
#: over the long column, whose key only the library's semantics hold, the
#: provider comparing as DataFusion does (`long_line_filter`).
LONG_LINE_PASSES: tuple[tuple[str, str], ...] = (
    ("parse", "default parse"),
    ("strict", "strict parse"),
    ("query", "query"),
    ("filter", "ordering filter"),
)

#: The passes a query's factor is planned by, each read off the instrument
#: (`LONG_FACTOR_FAMILY`).
LONG_LINE_PLANNED: tuple[str, ...] = ("query", "filter")

#: The counts each pass is read at: the serial path and the optimization
#: baseline (`roadmap.md`, "Four workers is the optimization baseline;
#: twenty-four is the guard"), as the phase's spec names them.
LONG_LINE_JOBS: tuple[int, ...] = (1, 4)

#: The figure's container. **Headroom, not a contract**: no held line is
#: charged on today's code, so nothing here is held to the allowance it states;
#: what will be is a held line's draw on the ledger
#: (`docs/design/decisions.md`, "D157"), and a re-take under it is where a
#: gate twin belongs. **Held under
#: `contained.MAX_FRACTION` of the machine** (`memory_preflight`), the one
#: ceiling every leg's resident set must fit.
LONG_LINE_MEMORY = "15g"

#: What a `long-line-resident` timed leg's shape states for `--memory` and
#: `pgdump.memory`, replaced by its spec's own allowance as it runs
#: (`RunSpec.allowance`): the number is the sitting's, sized from the factors
#: its instrument legs read (`long_line_allowance`), so no shape restates one.
ALLOWANCE_SLOT = "@allowance@"


def long_line_passes(kind: str) -> tuple[str, ...]:
    """The passes `kind` is read by: the two parses and the query, or for a
    term's kind (`LONG_LINE_TERM_KINDS`) the one holding its term — the
    provider's query over an array, the ordering filter over `jsonb`."""
    if kind not in long_lines.KINDS:
        raise ValueError(f"{kind!r} is no value kind of long-line-resident")
    if kind == "jsonb":
        return ("filter",)
    if kind in inputs.LONG_LINE_TERM_KINDS:
        return ("query",)
    return ("parse", "strict", "query")


def long_line_jobs(kind: str, length: str) -> tuple[int, ...]:
    """The counts a line of `length` is read at: `LONG_LINE_JOBS`, or one
    alone for a length in `LONG_LINE_SERIAL_ONLY` or a term's kind
    (`LONG_LINE_TERM_KINDS`)."""
    if length not in inputs.long_line_lengths(kind):
        raise ValueError(f"{length!r} is no line length of long-line-resident's {kind}")
    if length in inputs.LONG_LINE_SERIAL_ONLY or kind in inputs.LONG_LINE_TERM_KINDS:
        return (1,)
    return LONG_LINE_JOBS


def long_line_allowance(factors: Mapping[str, Fraction]) -> int:
    """The `--memory` and `pgdump.memory` every `long-line-resident` timed leg
    states: `PARALLEL_BUDGET`'s readers, plus the widest leg's lines held at
    once at its charge — a line a worker, at every length and count the figure
    reads its kind at.

    **A charge is the ledger's** (`docs/design/decisions.md`, "D154",
    "D158"): a query's or a filter's its line once plus its kind's factor,
    `factors` holding each planned kind's as its instrument leg read it
    (`LONG_FACTOR_FAMILY`); a parse's, which streams its line and draws only
    for the scalar text it collects, at most its line. So **it is sized from
    the largest factor, read rather than restated**.

    **Sized off the cluster so that it never binds on the lines.** Inert on
    today's code, which charges no held line. Once the ledger charges one, an
    allowance with nothing for the lines — `PARALLEL_BUDGET`'s alone — leaves
    four 512 MiB lines past what its carve leaves, and the re-take would price
    waiting and parking, which the floor's tests and the acceptance's hand run
    own, rather than the charge factors. **One allowance for every leg, stated
    on both takes**, so the two differ by the code alone where the factors do
    not move, and two lengths of one pass differ by the line alone."""
    held = Fraction(0)
    for kind in long_lines.KINDS:
        for name in long_line_passes(kind):
            if name in LONG_LINE_PLANNED and kind not in factors:
                raise ValueError(f"no factor was read for {kind}'s {name}")
            charge = 1 + factors[kind] if name in LONG_LINE_PLANNED else Fraction(1)
            for length in inputs.long_line_lengths(kind):
                for jobs in long_line_jobs(kind, length):
                    held = max(held, jobs * charge * inputs.long_line_bytes(length))
    return memory_model.stated_allowance(PARALLEL_BUDGET + math.ceil(held))

#: This machine's hardware threads and physical cores, which the figure's
#: rows are named by and `gzpass_topology_problem` holds the machine to
#: before a reading: written down rather than read, so a table names the
#: counts it was taken at, and refused on any other machine rather than
#: retaken at counts that mean something else there.
GZPASS_LOGICAL = 24

GZPASS_PHYSICAL = 12

#: The `--jobs` the figure is taken at, and its table's row order: the serial
#: pass, the optimization baseline (`roadmap.md`, "Four workers is the
#: optimization baseline; twenty-four is the guard"), the physical count, the
#: logical count less one and the logical count. **The upper two are the
#: challengers the default, the physical count, is read against**
#: (`gzpass_outcome`), and the third the falsifier naming the cost they add
#: (`docs/design/decisions.md`, "D2"): `--jobs` counts the
#: frontier and not the scan's thread, so at the logical count less one the
#: process runs no more threads than there are CPUs.
GZPASS_JOBS: tuple[int, ...] = (1, 4, GZPASS_PHYSICAL, GZPASS_LOGICAL - 1, GZPASS_LOGICAL)

#: The read-buffer budget every leg of `gzip-first-pass` and of
#: `gzip-resident` states (`stated_allowance`), and so a first pass's
#: allowance: one that affords the widest row a task each at either chunk
#: length under `margin_allowance` as well as under the budget, which
#: `PARALLEL_BUDGET` does not. A row whose run resolves another count refuses
#: either figure.
GZPASS_BUDGET = 4 << 30

#: The `--jobs` values the two `parallel-*` figures are taken at, and the row
#: order of their tables.
#:
#: **The same counts as `DECODE_WORKERS`, deliberately.** The decode figure is
#: the floor these two are read against — it says what the decoder alone does
#: with N workers — so a row of one that has no counterpart in the other would
#: be a comparison nobody can make. The endpoints carry the same meaning here:
#: 1 is the serial path this project ships, 12 is the machine's physical cores,
#: 24 is every hardware thread.
#:
#: **The range deliberately runs past the count a plain source delivers.**
#: `POOL_DEPTH` clamps `BufferPool::slots()` to four and the interior split
#: grants `MayWait`, so a fifth fused worker on a plain file waits; the rows
#: above four keep that arrangement in the table. What the wait costs them is
#: not separated from anything else they pay (`docs/design/decisions.md`,
#: "D25").
PARALLEL_JOBS: tuple[int, ...] = (1, 2, 4, 8, 12, 16, 24)

#: The row every other parallel row is a ratio against: `--jobs 1`, which the
#: CLI turns into `Parallelism::Serial` — the serial code path this project
#: ships, not a pool of one.
PARALLEL_BASELINE = 1

#: The command-shape families whose worker count is a figure's **axis** rather
#: than the apparatus's constant, spelled as the prefixes `_script` dispatches
#: on. Each is an existing shape's name with `-jobs-<n>` appended, so the shape
#: a parallel row measures is nameable as "the warm throughput table's row,
#: under N workers".
#:
#: Declared as a constant because three things read it and a fourth would
#: otherwise have to guess: `_script` dispatches on it, `command_shapes`
#: enumerates it, and `pinned_count_problems` exempts it from `SWEEP_JOBS` —
#: beside the decode instrument's `decode-` shapes, the reserve figure's two
#: stated families (`RESERVE_JOBS`) and its flagless one, each by its own
#: prefix. A family added to `_script` and not here states a count nothing
#: reconciles.
#:
#: **The provider families' count is `target_partitions`, not `--jobs`**
#: (`PARALLEL_SCAN`, `PARALLEL_SCAN_PREBUILT`): their rows run `pgdt sql`, and
#: the `--jobs` their untimed builder states is `SWEEP_JOBS` on every row.
JOBS_AXIS: tuple[str, ...] = (
    "parse-jobs-",
    "parse-rss-jobs-",
    "dfcli-query-typed-jobs-",
    "dfcli-query-typed-prebuilt-jobs-",
)

#: The provider family's shape, `JOBS_AXIS`' third prefix less its `-jobs-`.
#: **Unsuffixed, it is a shape of its own**: the same query at `SWEEP_JOBS`
#: partitions, stating no allowance, as `parse` is `parse-jobs-<n>` at the
#: apparatus's count (`typed_scan_invocation`).
PARALLEL_SCAN = "dfcli-query-typed"

#: The same query over a cache built **in a container of its own** ahead of the
#: sweep (`Session.prebuild`), `JOBS_AXIS`' fourth prefix less its `-jobs-`:
#: the timed container mounts that cache read-only where `--source` looks for it
#: and runs no builder.
#:
#: **A cold leg cannot build its cache in the timed container**, as
#: `PARALLEL_SCAN` does: the regime drops the page cache before the container
#: starts, and a builder there reads the whole dump back in before the timer,
#: so the query would read it warm under a cold heading. Built once a sitting
#: rather than once a rep, so each rep's drop leaves both the dump and the
#: cache to come off the device. What stands in for the `&&` that keeps a
#: failed builder from timing a cold map is the provider itself: `pgdt sql`
#: refuses a dump with no complete cache beside it rather than mapping it.
#:
#: *Rejected: evicting the dump inside the container after the builder*, by
#: `posix_fadvise(DONTNEED)` (`dd iflag=nocache`): it is a second eviction
#: mechanism beside the regime's `drop_caches`, scoped to the files it names,
#: so whatever else the builder pulled in stays warm and nothing says so.
PARALLEL_SCAN_PREBUILT = "dfcli-query-typed-prebuilt"

#: The **read-buffer budget** every row of both `parallel-*` figures runs at,
#: stated as the `--memory` allowance that leaves it (`stated_allowance`).
#:
#: **One value for every row, because the axis is the worker count.** A budget
#: that grew with `--jobs` would make each row a different apparatus, and the
#: table's ratios would be over two variables at once.
#:
#: **2 GiB, because it must admit the widest row's partitions on the coarsest
#: input.** A block-decoding `CompressedFileSource` charges each reader its block unit, the
#: chunk buffer and the decoder's own retention
#: (`xz_seek::Reader::decode_footprint`, 9,471,776 B on this shape's 8 MiB
#: dictionary), and the readers together the block pool's retention list,
#: `(POOL_DEPTH.max(workers) − 1)` units (`WorkerMemory`) — so 24 workers over
#: 24 MiB blocks want `24 x 34.03 + 23 x 24 MiB` = 1.34 GiB, and 24 over 128 MiB
#: blocks want more than any budget this machine would state; the 128 MiB leg
#: is bound by its own block size and says so, which is the whole point of
#: taking `parallel-peak-rss` at two of them. Below this the widest rows would be silently clamped by
#: `worker_count`, and a clamped row is a lower count wearing a higher label.
#:
#: **`--jobs 1` states it too**: `Parallelism::workers(1, _)` is `Serial`
#: carrying the budget, so the baseline row runs one block-decoding reader at
#: this budget rather than the streaming fallback, and a speedup is a speedup
#: over that. Each table says so.
#:
#: **A budget clamp and a `POOL_DEPTH` clamp are not the same kind of thing,
#: which is why this table annotates one and sizes around the other.** Both
#: deliver fewer workers than a row's label asks for, and the table already
#: carries a note for the second. The difference is whose ceiling it is.
#: `POOL_DEPTH` is the shipped library's own, so a reader of the published
#: figure meets it too and the annotated row states a real property of the
#: thing measured. This constant is the *harness's* choice; a row clamped by it
#: publishes an apparatus decision wearing a library ceiling's clothes, and
#: nothing in the table distinguishes the two. So the annotation that suffices
#: for `POOL_DEPTH` does not discharge a budget clamp — the budget is sized so
#: that the clamp does not happen, and where it cannot be, what the figure
#: measures has to be restated rather than footnoted.
PARALLEL_BUDGET = 2 << 30

#: The fixed room the two `parallel-*` figures' containers hold *above* the
#: budget they state: the decoder dictionaries, the batches in flight and the
#: allocator's retention, none of which the stated budget covers.
#:
#: **It is a constant, and that is the whole principle.** What is refused is
#: sizing this off the `--jobs` axis, because what sits above a stated budget is
#: nothing the library states: the terms no charge bills and the allocator's
#: retention, which on the platform allocator is glibc seeding an arena for
#: every thread that allocates (`docs/status/history/2026-09-09.md`, "`M76`: the arena cap is not the
#: runtime's") — a number picked from the allocator on the machine that took
#: the figure rather than from anything the library asks for, and
#: `measurements.md`'s contract is that a figure carries the command that
#: re-takes it. Sizing off the *budget* is the opposite case: the budget is a
#: number the library is handed and promises to bound its pools by, so
#: `budget + headroom` states the library's own contract and travels to any
#: machine.
PARALLEL_HEADROOM = 2 << 30

#: What the two `parallel-*` figures' containers are given, against the
#: register's 512 MB — an apparatus departure, and each figure's own table says
#: so. **Derived, never typed**: it is the stated budget plus
#: `PARALLEL_HEADROOM`, so raising the budget cannot leave the container behind.
#:
#: **Never a literal.** A literal `3g` is this rule against a 1 GiB budget —
#: budget plus 2 GiB — and nothing recomputes it when the budget moves: raising
#: the budget to 2 GiB under it halved the headroom instead of moving the
#: container. The block pool's slot ceiling is
#: `clamp((budget - chunk_held) / unit, 1, POOL_DEPTH.max(jobs))`, a function of
#: the *budget* and the announced count rather than of the readers delivered
#: (`KD21`), so at `control_xz128`'s 128 MiB unit it went from seven slots to
#: fifteen: `parallel-peak-rss`'s widest row measured 2110 MiB at 1 GiB and
#: **3067 MiB at 2 GiB**, five megabytes under a 3072 MiB limit ([`../../docs/status/history/2026-09-10.md`](../../docs/status/history/2026-09-10.md),
#: "The container was sized off a number that moved"). The 24 MiB leg rose
#: 89 MiB over the same step, being depth-bound rather than budget-bound, which
#: is why the shared constant hid it.
PARALLEL_MEMORY = f"{(PARALLEL_BUDGET + PARALLEL_HEADROOM) // config.GIB}g"

#: `gzip-first-pass`'s and `gzip-resident`'s container: their budget plus
#: `PARALLEL_HEADROOM`, sized as `PARALLEL_MEMORY` is and for its reason.
GZPASS_MEMORY = f"{(GZPASS_BUDGET + PARALLEL_HEADROOM) // config.GIB}g"

#: The worker count every `pgdt` invocation this harness makes states, and the
#: one every registered figure is taken at **except the five whose axis it is**
#: (`parallel-scan-throughput`, `parallel-peak-rss`, `gzip-resident`,
#: `gzip-first-pass`, `xz-decode-scaling`) **and
#: `reserve`**, whose stated legs hold `RESERVE_JOBS` and whose flagless legs
#: state nothing.
#:
#: **The exemption is by axis, and it is what `JOBS_AXIS` names.** A figure
#: measuring what the second worker buys cannot state one count for every row —
#: its rows *are* the counts — so it states `PARALLEL_JOBS` instead, exactly as
#: the decode instrument states `DECODE_WORKERS` in its own vocabulary. What the
#: exemption is not is a licence to inherit: every one of those shapes still
#: states a count, `worker_count_problems` still refuses a shape that pins none,
#: and `pinned_count_problems` refuses a shape that pins something *other* than
#: this constant without declaring itself an axis. So the failure this whole
#: reconciliation exists against — a shape whose count moved because a default
#: did — is closed on both sides rather than opened by the exemption.
#:
#: **A worker count is apparatus, on the same argument the allocator is.** A
#: shape that states nothing measures whatever the CLI's `--jobs` defaults to
#: that day — and that default has moved underneath the published figures
#: more than once and now depends on the source, one worker on a plain file and
#: `available_parallelism()` on an `.xz` one, without a single shape changing,
#: which is a figure whose apparatus nothing in the document can name. Nothing
#: about this constant follows the flag. So no invocation
#: here inherits it: `_script` states it, `profile_argv` states it, the koji
#: recipe takes it as a parameter, and `--check` refuses a shape that pins no
#: count (`measurements.md`, "The apparatus").
#:
#: **It is 1 because that is the arrangement the published sitting measured**,
#: not because serial is preferred: every published shape outside the declared
#: exemptions — the `JOBS_AXIS` families, the decode instrument's `decode-`
#: shapes, the reserve's two stated families at `RESERVE_JOBS` and its flagless
#: legs — states `--jobs 1`, so a re-take at this value reproduces that
#: apparatus rather than replacing it. Raising it is an apparatus change
#: and obliges a re-sweep, exactly as changing the allocator would.
SWEEP_JOBS = 1

#: **Every `pgdt parse` this harness runs gathers no statistics**, outside the
#: figures whose subject is the gathering or what it buys (`GATHER_STATISTICS`). `parse`
#: records at the data level by default, reading every value of every column,
#: so a shape that inherited the default would re-time what its figure
#: measures the day the default moved — the failure `SWEEP_JOBS` is stated
#: against. `statistics_flag_problems` refuses a shape that runs `parse`
#: without it; `--preamble-only` reads no row and takes no statistics flag, so
#: it states none. The metadata level records no census either, so a query
#: over a cache it wrote reads its table once more for one, and no figure
#: queries such a cache.
NO_STATISTICS = "--statistics-level metadata"

#: `pgdt_mapping::statistics::ROW_GROUP_DEFAULT_SIZE_BYTES`, mirrored, and
#: held to the library's by a test.
ROW_GROUP_SIZE = 1 << 20

#: **What the statistics figures state instead, where they gather**, and the
#: untimed builders of the dynamic-filter figures and the query figures
#: (`DATA_LEVEL_QUERIES`) with them. Their subject is the
#: data level the rule above keeps out of every other figure, what its
#: statistics buy a query, or a query over the cache it writes, so they are its one exemption, and they state the request rather
#: than inherit it for the rule's own reason: the selection and the group size
#: are both defaults that can move, and a shape inheriting either would re-time
#: its figure the day one did. The size stated is the default's base size, and a
#: stated size is gathered exactly, so what is priced is gathering at a
#: mebibyte, which a flagless `parse` coarsens where rows are wide.
GATHER_STATISTICS = f"--statistics-level data --row-group-size {ROW_GROUP_SIZE}"

#: **The query figures query a data-level cache one untimed `parse` wrote in
#: the same container**, as `statistics-pruning` does, stating `--statistics
#: none` so no pruning enters the reading. A query over no cache
#: (`--cache none`) maps inside the timer, timing the census
#: and the count with the rows, and one over a metadata-level cache reads its
#: table once more for a census, inside the timer (`NO_STATISTICS`). These are the
#: prefixes `_script` dispatches on. `query-nomatch*` is not here: it times
#: the map and its saves, which a built cache would leave nothing of.
#:
#: **Every row of a figure reads one cache**: the builder states `SWEEP_JOBS`
#: whatever its query states, as `PARALLEL_SCAN`'s does on every row of its
#: axis. **Builder and query are joined by `&&`**, as every shape's untimed
#: builder joins its timed command (`test_shapes.py`, `Scripts`): a builder
#: that failed would leave the query to map the table cold and save it inside
#: the timer, published as a read over a cache. What the
#: query reads carries decoding the whole cache, statistics included
#: (`PRUNING_LEGS`), which a query after a default `parse` pays too, and the
#: builder discovers its statistics allowance from the figure's container
#: (D85), so the container sizes the cache the query decodes.
DATA_LEVEL_QUERIES: tuple[str, ...] = (
    "query-typed",
    "query-strings",
    "query-project-",
    "query-where-",
)

#: That builder, ahead of the timer.
DATA_LEVEL_BUILDER = (
    f"/pgdt parse --source /dump.sql --cache /tmp/x.pgdt --jobs {SWEEP_JOBS} "
    f"{GATHER_STATISTICS} >/dev/null && "
)

#: What the timed query states: the builder's cache, and no use of its
#: statistics.
DATA_LEVEL_QUERY = "--cache /tmp/x.pgdt --statistics none"

#: `statistics-gathering`'s shapes: one whole-file `parse` under the resident
#: wrapper, at the metadata level and at the data level, `<family><leg>-rss`.
#: **The figure prices the data level whole**: the census, the unrepresentable
#: count and the statistics together, the only two levels a `parse` offers
#: (`docs/design/decisions.md`, "D77"), and which of the three costs what is
#: an attribution, read off a profile of the `data` leg (`PROFILE_SHAPES`).
#: That is the shipped default `parse`, which no scan figure times. **No leg
#: states a column override** (`metadata,<table>.<column>=data`): it censuses
#: and counts the whole table with one column's statistics, so subtracted from
#: these it would difference the census back out, which `D35` retired for
#: the profile. The id stays
#: `statistics-gathering`, `data` being a value of `--statistics-level`.
#: **Both legs are the family's own**, the `metadata` leg included, though
#: its argv is `parse-rss`'s: the figure's container is not the register's, so
#: it is not that run, and a leg borrowed from `peak-rss` would stand this
#: figure in a sharing edge that confines it to a stamped sweep.
STATISTICS_FAMILY = "parse-statistics-"

#: The two legs, in the table's column order, each with the flags it states.
STATISTICS_LEGS: tuple[tuple[str, str], ...] = (
    ("metadata", NO_STATISTICS),
    ("data", GATHER_STATISTICS),
)

#: `statistics-gathering`'s container limit, against the register's 512 MB.
#: **Chosen generously, so that no kill interrupts the figure**: statistics are
#: held per group per tracked column, and the arithmetic's worst case — every
#: one of a 3.00 GiB input's 3,072 groups holding a full dictionary of 256-byte
#: entries and two capped bounds on each of the control's 16 columns — is about
#: 800 MiB above the scan. The figure records resident beside the time, so the
#: headroom this leaves is read rather than assumed. What statistics may hold is
#: bounded (`docs/design/decisions.md`, "D85"); this limit sits above that bound
#: rather than standing in for it.
STATISTICS_MEMORY = "2g"

#: The strict parse `--count-pair` counts: the data level's unwrapped `parse`
#: (`STATISTICS_FAMILY`'s `data`) told to check every field, as
#: `long-line-resident`'s strict pass is, so the strict check's path is a row
#: of its own. No figure times it.
STRICT_PARSE = "parse-strict"

#: What it states beside the data level's request.
STRICT_FLAG = "--postgres-invalid-values strict"

#: `statistics-pruning`'s shapes, `<family><filter>-<leg>`: a `pgdt query`
#: timed against a cache one untimed gathering `parse` wrote in the same
#: container, as `info-cache-rss` builds its own — so the cache is by
#: construction this input's, and the two legs differ by `--statistics` alone.
PRUNING_FAMILY = "query-pruning-"

#: The filters, in the table's row order: the `--where` each states and what
#: the row calls it. **The range is the one the spec names**, a selective range
#: on a sorted column, which statistics answer twice over — groups wholly
#: outside it are skipped by their bounds, and the block's ascending order stops
#: the read at the first row past its upper bound. **The equality is answered by
#: a dictionary alone**, `v_category` being text under no stated collation, so
#: `pgdt`'s semantics reads no bounds for it (`generate_pruning_bench.py`). Both select 3,000 rows of the
#: 3.00 GiB input: one run of `id`, and three runs of one label spread across
#: the file — which is what the table's rows-returned column reads back.
#: **The third is one its statistics cannot narrow** (`PRUNING_UNNARROWED`):
#: `v_smallint` is drawn uniformly per row, so each group's few hundred values
#: have bounds spanning the column's range, holding the midrange `0`, and more
#: distinct values than a dictionary keeps. Every group's bounds are read and
#: none is skipped, so its legs differ by what consulting them costs. A
#: `smallint` rather than a wider integer so the filter still returns rows:
#: 13 of the 3.00 GiB input's, where one value of a `bigint` would match none.
PRUNING_FILTERS: dict[str, tuple[str, str]] = {
    "range": ("id > 400000 AND id <= 403000", "Range on the sorted `id`"),
    "dictionary": ("v_category=category-200", "Equality on the low-cardinality `v_category`"),
    "unnarrowed": ("v_smallint=0", "Equality on the uniformly drawn `v_smallint`"),
}

#: The filter whose pruned leg must consult statistics and skip nothing, where
#: the others' must skip.
PRUNING_UNNARROWED = "unnarrowed"

#: The legs, in the table's column order: what `query --statistics` states.
PRUNING_LEGS = ("none", "all")

#: **No leg prices carrying statistics.** The cache is decoded whole whatever
#: the query states (`cache::file::read_cache_file`), so every leg pays the
#: statistics' decode, and no `parse` writes a cache that holds its table's
#: census without its statistics: a metadata-level one leaves the query to
#: re-read the table for its census inside the timer. A column override's
#: cache, carrying one column's statistics, would time nearly the subtraction
#: the retired leg took, whose reading the spreads left unresolved; what would
#: resolve it is the query timing its own cache load, which no decision asks
#: for. The pruned legs bound the decode from above, each decoding the whole
#: cache inside its wall, and the figure's prose states that bound.

#: DataFusion's CLI, as the binary the figures time carries it: `pgdt`'s
#: `sql`, run after a `pgdt parse` that builds the cache it reads.
SQL_SHELL = "/pgdt sql"

#: The catalog `--source` registers the input under: a generated dump names no
#: database, so one is given.
DFCLI_CATALOG = "bench"

#: **The worker count, stated as `datafusion-cli` takes it**: the session's
#: `target_partitions`, which is what the provider plans a scan's partitions
#: against, set from the environment `ConfigOptions::from_env` reads. Stated at
#: `SWEEP_JOBS` on every run of the binary, for that constant's reason, and
#: held there by `worker_count_problems` and `pinned_count_problems`.
DFCLI_PARTITIONS = "DATAFUSION_EXECUTION_TARGET_PARTITIONS"

#: `dynamic-filter-join` and `dynamic-filter-topk`'s shapes,
#: `<family><figure>-<query>-<leg>`: one untimed `pgdt parse` stating
#: `GATHER_STATISTICS` writes the cache where `--source` looks for it, beside
#: the dump, and the timed `pgdt sql -c` runs one query under
#: one of `DYNFILTER_LEGS`. What the query returned is hashed outside the
#: timer, so the three legs are held to one answer.
#:
#: **One binary, two levers, three legs**: DataFusion's own producer flag
#: (`optimizer.enable_{join,topk}_dynamic_filter_pushdown`) and the
#: provider's `pgdump.dynamic_filter_rows`, stated by a `SET` run ahead of the
#: query in the same process. So each table prices what ships — the filter on
#: against off — and what the setting buys and costs — rows evaluated against
#: the filter on, which `decisions.md`, "D93" reads.
DYNFILTER_FAMILY = "dfcli-dynamic-filter-"

#: Each figure's queries, in its table's row order: the SQL and what the row
#: calls it. **Every join counts one payload column over the probe's rows it
#: matched**, so a row the filter would drop is one whose `v_text` need not be
#: decoded, and the output is one line however many rows matched. The build
#: side is the small table, which is what the planner collects from the
#: provider's row counts (`generate_dynamic_filter_bench.py`):
#:
#: - **`clustered`** joins the ascending `id` against a run of consecutive ids:
#:   the bounds rule out every group but the run's, where group pruning pays.
#: - **`unclustered`** joins `u_key`, a bijection of `id`, against as many
#:   scattered values: no group's bounds narrow anything, and only a row-level
#:   check drops a row.
#: - **`costing`** joins `bucket` against every value it holds, the largest
#:   `IN` list a join publishes: the filter rejects no row, and what checking
#:   it costs is pure overhead.
#:
#: **A join answers `count(*)` first, and `count(p.v_text)` beside it**: the
#: first is the rows it matched, which its table reports; the second keeps the
#: payload decoded, what row-level evaluation exists to skip, and is not that
#: count, `v_text` being nullable.
#:
#: **The TopK orders by the unsorted `u_key`**, projecting a payload column
#: beside it; `u_key` has no ties, so the answer is one answer.
DYNFILTER_QUERIES: dict[str, dict[str, tuple[str, str]]] = {
    "join": {
        "clustered": (
            f"SELECT count(*), count(p.v_text) FROM {DFCLI_CATALOG}.public.perf p "
            f"JOIN {DFCLI_CATALOG}.public.near b ON p.id = b.k",
            "A selective join on the clustered `id`",
        ),
        "unclustered": (
            f"SELECT count(*), count(p.v_text) FROM {DFCLI_CATALOG}.public.perf p "
            f"JOIN {DFCLI_CATALOG}.public.scattered b ON p.u_key = b.k",
            "A selective join on the unclustered `u_key`",
        ),
        "costing": (
            f"SELECT count(*), count(p.v_text) FROM {DFCLI_CATALOG}.public.perf p "
            f"JOIN {DFCLI_CATALOG}.public.every b ON p.bucket = b.k",
            "A join on `bucket` rejecting no row",
        ),
    },
    "topk": {
        "unsorted": (
            f"SELECT p.u_key, p.v_text FROM {DFCLI_CATALOG}.public.perf p "
            "ORDER BY p.u_key LIMIT 10",
            "`ORDER BY` the unsorted `u_key`, `LIMIT 10`",
        ),
    },
}

#: The producer flag each figure's legs state, as `ConfigOptions::from_env`
#: reads `optimizer.enable_{join,topk}_dynamic_filter_pushdown`.
DYNFILTER_FLAGS = {
    "join": "DATAFUSION_OPTIMIZER_ENABLE_JOIN_DYNAMIC_FILTER_PUSHDOWN",
    "topk": "DATAFUSION_OPTIMIZER_ENABLE_TOPK_DYNAMIC_FILTER_PUSHDOWN",
}

#: The legs, in the table's column order, and the producer flag's value in
#: each: `off`, the filter off; `on`, the filter on at the provider's default;
#: `rows`, on with `DYNFILTER_ROWS_SQL` run first.
DYNFILTER_LEGS = {"off": "false", "on": "true", "rows": "true"}

#: The leg stating the setting, and the statement it runs ahead of its query:
#: its own `-c`, which prints nothing under `--format csv`, so the answer
#: read back is the query's alone.
DYNFILTER_ROWS_LEG = "rows"

DYNFILTER_ROWS_SQL = "SET pgdump.dynamic_filter_rows = true"

#: **The startup leg**: the builder as every leg runs it, then
#: `pgdt sql` registering the dump and answering `STARTUP_SQL`
#: under the timer, so what loading the program, its runtime and the
#: registration cost a leg — and how much that varies — sits in each table
#: beside the legs it is inside. Taken in each figure's own interleave rather
#: than borrowed between the two.
DYNFILTER_STARTUP = f"{DYNFILTER_FAMILY}startup"

STARTUP_SQL = "SELECT 1"


def dfcli_invocation(figure: str, name: str, leg: str, dump: str) -> tuple[list[str], list[str]]:
    """The environment and the arguments one run of `SQL_SHELL` over a
    dynamic-filter shape states, over the dump at `dump`, its SQL last.

    One function for the figure (`_script`) and for the account's
    instruments (`profile_recipe`), so a profiled or introspected leg is the
    timed one by construction: a flag moved in one and not the other would
    profile something no figure measures."""
    if name not in DYNFILTER_QUERIES.get(figure, {}) or leg not in DYNFILTER_LEGS:
        raise ValueError(f"unknown dynamic-filter shape {figure}-{name}-{leg}")
    env = [
        f"{DFCLI_PARTITIONS}={SWEEP_JOBS}",
        f"{DYNFILTER_FLAGS[figure]}={DYNFILTER_LEGS[leg]}",
    ]
    argv = ["--source", f"{DFCLI_CATALOG}={dump}", "--format", "csv", "-q"]
    if leg == DYNFILTER_ROWS_LEG:
        argv += ["-c", DYNFILTER_ROWS_SQL]
    return env, [*argv, "-c", DYNFILTER_QUERIES[figure][name][0]]


def dfcli_shell(env: list[str], program: str, argv: list[str]) -> str:
    """`dfcli_invocation`'s run as one shell line, each statement
    single-quoted."""
    words = (f"'{a}'" if " " in a else a for a in argv)
    return f"{' '.join(env)} {program} {' '.join(words)}"

#: What a timed run of `SQL_SHELL` answered, read back off `/tmp/result.csv`
#: outside the timer as `key=value` lines `parse_reported` takes: its rows
#: under the header, its first row's first field, and a digest of the whole.
DFCLI_ANSWER = (
    "echo result_rows=$(($(wc -l </tmp/result.csv) - 1)) && "
    "echo result_first=$(sed -n 2p /tmp/result.csv | cut -d, -f1) && "
    "echo result_digest=$(sha256sum </tmp/result.csv | cut -d' ' -f1)"
)

#: `parallel-scan-throughput`'s provider legs' query: every column of the
#: control's table counted, under a filter every row passes.
#:
#: **Every column, so every column is decoded typed**, as the `pgdt query`
#: legs it replaced decoded them (`KD57` is why it replaced them); **one row
#: out**, so no printing serializes what the partitions decode in parallel.
#: **The filter is what keeps the counts from being answered**: unfiltered,
#: the plan node's exact NULL counts answer every `count(<column>)` with no
#: row read, where a pushed filter hands them over as estimates
#: (`docs/design/decisions.md`, "D89"). `id IS NOT NULL` keeps every row,
#: since the generator writes an `id` on each, so its statistics prune
#: nothing.
PARALLEL_SCAN_SQL = (
    f"SELECT {', '.join(f'count({name})' for name, _ in perf.COLUMNS)} "
    f"FROM {DFCLI_CATALOG}.{perf.TABLE} WHERE id IS NOT NULL"
)


def parallel_scan_invocation(partitions: int, dump: str) -> tuple[list[str], list[str]]:
    """The environment and the arguments one provider leg of
    `parallel-scan-throughput` states over the dump at `dump`, at
    `partitions` as its session's `target_partitions`.

    **The allowance is the `pgdt` legs' own**, stated by a `SET` ahead of the
    query in the same process, so each column of the table runs under one
    stated allowance (`PARALLEL_BUDGET`)."""
    if partitions not in PARALLEL_JOBS:
        raise ValueError(f"{partitions} is a partition count the figure does not carry")
    env = [f"{DFCLI_PARTITIONS}={partitions}"]
    argv = [
        "--source", f"{DFCLI_CATALOG}={dump}", "--format", "csv", "-q",
        "-c", f"SET pgdump.memory = {memory_model.stated_allowance(PARALLEL_BUDGET)}",
        "-c", PARALLEL_SCAN_SQL,
    ]
    return env, argv


def typed_scan_invocation(dump: str) -> tuple[list[str], list[str]]:
    """The environment and the arguments of `PARALLEL_SCAN` unsuffixed:
    `PARALLEL_SCAN_SQL` over the dump at `dump`, at `SWEEP_JOBS` partitions.

    **No allowance is stated**, so the provider carves the container's limit
    as the `pgdt query` shapes beside it in `release-profile` do; that figure's
    shapes run in one container, which a stated `PARALLEL_BUDGET` would
    overrun."""
    env = [f"{DFCLI_PARTITIONS}={SWEEP_JOBS}"]
    argv = ["--source", f"{DFCLI_CATALOG}={dump}", "--format", "csv", "-q", "-c", PARALLEL_SCAN_SQL]
    return env, argv

#: The untimed builder every `PARALLEL_SCAN` shape but the prebuilt one runs
#: ahead of its query: the cache, written beside the dump where `--source`
#: looks, at `SWEEP_JOBS` whatever partition count the query states, so every
#: row of a figure reads one cache.
PROVIDER_BUILDER = (
    f"/pgdt parse --source /dump.sql --cache /dump.sql.pgdt --jobs {SWEEP_JOBS} "
    f"{GATHER_STATISTICS} >/dev/null && "
)

#: `PARALLEL_SCAN_PREBUILT`'s builder (`Prebuilt`): `PROVIDER_BUILDER`'s
#: flags, writing to `{cache}` in a directory the session mounts writable
#: rather than beside the read-only dump. Untimed and run once a sitting, so
#: it is not a command shape; `figures/test_parallel.py` holds it to the warm
#: builder's flags.
PREBUILT_BUILDER = (
    f"/pgdt parse --source /dump.sql --cache {{cache}} "
    f"--jobs {SWEEP_JOBS} {GATHER_STATISTICS} >/dev/null"
)

#: `long-line-resident`'s query: every value of the table built, one row out.
#: **The provider's scan, not `pgdt query`'s**, whose printing
#: renders each value back into a buffer of its own — a CLI's term, which a
#: charge set for the library must not carry (`measurements.md`, "The
#: apparatus", on a CLI number sizing no library change), and the reason
#: `parallel-scan-throughput`'s query legs are the provider's.
#: **The filter is what keeps the count from being answered**, as
#: `PARALLEL_SCAN_SQL`'s is: unfiltered, the plan node's exact NULL count
#: answers `count(v)` with no row read (`docs/design/decisions.md`, "D89"),
#: and every row holds an `id`. It holds only while `id` is nullable, a
#: `NOT NULL` column letting the filter be simplified away
#: (`test_generate_long_lines.py`'s `test_the_id_is_nullable_and_every_row_holds_one`).
#: A count rather than a length, DataFusion measuring no `Binary` value.
LONG_LINE_SQL = (
    f"SELECT count(v) FROM {DFCLI_CATALOG}.{long_lines.TABLE} WHERE id IS NOT NULL"
)

#: The untimed builder of `long-line-resident`'s query legs: `PROVIDER_BUILDER`
#: over the figure's cluster.
LONG_LINE_BUILDER = (
    f"/pgdt parse --source /dump.sql --cache /dump.sql.pgdt --jobs {SWEEP_JOBS} "
    f"{GATHER_STATISTICS} >/dev/null && "
)


def long_line_invocation(
    partitions: int, dump: str, allowance: str | None = ALLOWANCE_SLOT
) -> tuple[list[str], list[str]]:
    """The environment and arguments one query leg of `long-line-resident`
    states over the dump at `dump`, at `partitions` as its session's
    `target_partitions`: the `pgdt` legs' allowance, a `SET` ahead of the
    query in the same process — no allowance for an
    instrument leg reading the plan's factor, which no allowance moves
    (`LONG_FACTOR_FAMILY`)."""
    if partitions not in LONG_LINE_JOBS:
        raise ValueError(f"{partitions} is a partition count the figure does not carry")
    env = [f"{DFCLI_PARTITIONS}={partitions}"]
    argv = [
        "--source", f"{DFCLI_CATALOG}={dump}", "--format", "csv", "-q",
        *(("-c", f"SET pgdump.memory = {allowance}") if allowance is not None else ()),
        "-c", LONG_LINE_SQL,
    ]  # fmt: skip
    return env, argv

#: `long-line-resident`'s ordering filter: the long `jsonb` column ordered
#: against an empty object, which every row's object passes, a pair or more
#: being greater than none (I41's `compareJsonbContainers`), so it keeps every
#: row and reads every field's key. Only `id` is projected, so the batch
#: decodes nothing the key's term does not.
LONG_LINE_FILTER = "v>{}"


def long_line_filter(jobs: int, allowance: str | None = ALLOWANCE_SLOT) -> str:
    """`pgdt query`'s ordering filter over the cluster at `jobs`, under the
    figure's allowance, over the cache the untimed builder
    wrote; no allowance for an instrument leg (`long_line_invocation`).

    **`pgdt query`'s, not the provider's**: a pushed filter compares as
    DataFusion does over the emitted value (`docs/design/decisions.md`,
    "D88"), `jsonb` as its text, so only the library's semantics hold a
    `jsonb` key. Its printing renders `id` alone."""
    if jobs not in LONG_LINE_JOBS:
        raise ValueError(f"{jobs} is a job count the figure does not carry")
    memory = f"--memory {allowance} " if allowance is not None else ""
    return (
        f"/pgdt query --source /dump.sql --cache /dump.sql.pgdt --table {long_lines.TABLE} "
        f"--column id --filter '{LONG_LINE_FILTER}' --jobs {jobs} {memory}>/dev/null"
    )

#: What the decode figure's container is given, against the register's 512 MB.
#: At 24 workers over 24 MiB blocks the decoder holds 24 decoded slots, 26
#: compressed windows and 24 LZMA2 dictionaries — around 950 MB on the
#: generated leg, whose compressed windows are the larger of the two. It is an
#: apparatus departure and the figure's own table says so.
DECODE_MEMORY = "2g"

#: The stated budgets `reserve` reads a resident set across: its one axis.
#:
#: **Four, spanning an order of magnitude either side of the library's own
#: `DEFAULT_MEMORY_BUDGET`**, because the quantity this axis publishes is a
#: *difference* from the number stated and one reading of it cannot say whether
#: the difference is a constant or a fraction
#: (`docs/design/decisions.md`, "I/O, memory and parallelism").
#:
#: **What the axis shows needs all four.** A plain source reads flat across
#: them, the budget binding almost nothing there; a block-decoding `.xz` rises
#: with the reader count each budget affords at `RESERVE_JOBS`, since what a
#: stated budget buys there is readers. One reading could have said neither.
RESERVE_BUDGETS: tuple[int, ...] = (64 << 20, 128 << 20, 256 << 20, 512 << 20)

#: The worker count every reserve reading states.
#:
#: The top of `PARALLEL_JOBS`, which is this machine's `available_parallelism()`
#: and so the count an `.xz` source recommends here (`default_workers`): the
#: stated legs pin that count and vary only the budget.
#:
#: **This is a declared axis for `pinned_count_problems`, not an inherited
#: count.** The family states it on every shape; what makes it exempt from
#: `SWEEP_JOBS` is that the figure holds the count fixed and varies the budget,
#: which is the same exemption `JOBS_AXIS` takes with the two swapped.
RESERVE_JOBS = PARALLEL_JOBS[-1]

#: The two arena settings each budget is read at: token, `MALLOC_ARENA_MAX`
#: value (empty = set nothing), and what the table calls the leg.
#:
#: **Two.** `unset` is the operator who capped nothing, which is the case that
#: kills the process and the arrangement `MEMORY_RESERVE` was chosen under.
#: `two` is the tightest value an operator would plausibly set — the floor the
#: manual publishes and what the koji probes used — so it bounds how much of the
#: resident set is arena retention at all, which the uncapped leg cannot say on
#: its own. **Neither can be dropped**: a one-leg figure cannot report a null,
#: and an arena claim here has more than once outlived the build it was
#: measured on.
#:
#: **No leg caps at the worker count plus one.** A cap at or above the arena
#: count cannot bind, but the arena count is not the worker count: the
#: instrument reads `readers + 2` arenas at every limit that resolved two readers
#: or more, so a `readers + 1` cap sits below it there. At one reader it reads 2
#: arenas, where a cap of 2 is already inert. No reading prices such a cap; the leg is left out as one not
#: worth a sitting, not as one inert by construction.
RESERVE_ARENAS: tuple[tuple[str, str, str], ...] = (
    ("unset", "", "arenas uncapped"),
    ("two", "2", "`MALLOC_ARENA_MAX=2`"),
)

#: The two sources each leg is read over, and what the table calls each.
#:
#: **Both, because the rule states one reserve for every source.** The budget
#: binds the pools on a block-decoding `.xz` and barely binds anything on a
#: plain source, so that one number has to be read against the shape where it
#: is loosest as well as the one where it is tightest.
RESERVE_INPUTS: tuple[tuple[str, str], ...] = (
    ("control_xz", "`.xz`"),
    ("control", "plain"),
)

#: The command-shape prefix the reserve family answers to. It carries `rss`
#: because `Session.time_run` reads the wrapper's report only for a shape whose
#: name says it has one.
RESERVE_FAMILY = "parse-rss-reserve-"

#: The command-shape prefix of the legs that state **nothing** — no `--jobs`,
#: no `--memory` — so that the arrangement under test is the one a
#: flagless invocation resolves for itself.
#:
#: **This is the one family in the register that states no worker count, and it
#: is declared at both ends rather than slipping past the reconciliation.**
#: `worker_count_problems` and `pinned_count_problems` skip it by this prefix,
#: and a test asserts that what it states is *neither* flag — the exemption is
#: from stating a count, never a licence to pin one quietly. What earns it is
#: that the count is the **reading**: under discovery `Discovered::resolve`
#: lowers the source's recommendation to what the allocation affords, so the
#: `jobs=` a run reports is the reader count its resident set is a resident set
#: *for*, and a shape that pinned one would measure an arrangement the shipped
#: default never produces.
#:
#: **It does not reopen the figure the register refuses** (`measurements.md`,
#: "The apparatus"): a *throughput* table off unpinned shapes, the throughput of
#: the default being already measured at a stated count by
#: `parallel-scan-throughput`. This is a resident reading of an arrangement no
#: stated shape can express, and the count it runs at is recorded per leg from
#: the run's own report rather than assumed.
RESERVE_FLAGLESS = "parse-rss-discover-"

#: The command-shape prefix of the **path step**: the same `parse` at a stated
#: budget either side of what one block-decoding reader costs, so that block
#: decode is afforded on one leg and declined on the other
#: (`BlockCache::affordable`).
#:
#: A family of its own rather than two more entries in `RESERVE_BUDGETS`, whose
#: four values are crossed with both arena settings and both sources: the step's
#: two budgets are neither whole mebibytes nor an axis anything else is read
#: across, and adding them there would multiply eight legs out of a pair.
RESERVE_STEP_FAMILY = "parse-rss-step-"

#: The two compressed inputs the flagless axis is read over: the registered
#: input, what the table calls it, and its block size.
#:
#: **Both block sizes, because the charge is a multiple of the unit.** A
#: unit-shaped error in that charge is multiplied by the reader count, and a
#: fit taken at one block size cannot tell a term that scales with the unit
#: from one that does not.
#:
#: The block size is declared here rather than read off the file because it is
#: what `reader_bytes` and `pool_bytes` are functions of, and the renderer
#: needs it to say which legs took the block path at all — `parse` emits no
#: decline note, that being a `PlanNote` on a query's `TableStream`, so
#: `block_path_afforded` over the run's own reported budget is what answers it.
RESERVE_FLAGLESS_INPUTS: tuple[tuple[str, str, int], ...] = (
    ("control_xz", "24 MiB blocks", 24 << 20),
    ("control_xz128", "128 MiB blocks", 128 << 20),
)

#: The container limits the flagless axis is read at: the `-m` value and the
#: bytes it states.
#:
#: **The limit is the axis, because under discovery it is the only input.** A
#: flagless run reads `limit − MEMORY_RESERVE`, fits the source's recommended
#: count inside it and spends exactly what that many readers cost, so one
#: number decides both terms of the arrangement — which is why these legs carry
#: a **per-spec** container limit where every other figure takes its own.
#:
#: **512 MiB is the bottom of the range**, where the rule's own worst headroom
#: is. The 128 MiB leg reads the streaming fallback there — 128 MiB granted
#: against a 522.03 MiB line — and the 24 MiB leg takes the block path at one
#: reader, which each cell says for itself (`block_path_afforded`). A fallback
#: leg is still a leg the allocation has to hold. 1 GiB and 1.5 GiB put the
#: middle of the curve in the fit rather than only its worst end; and 2 GiB is
#: where the 24 MiB leg's count saturates at the source's own recommendation,
#: which is what separates "the allowance ran out" from "the recommendation
#: did".
#:
#: **544 MiB and 1088 MiB are there for the side of the charge below
#: `POOL_DEPTH`**, where the retention list is a constant `(POOL_DEPTH − 1)`
#: units rather than growing with the count. `544m` resolves two readers of
#: 24 MiB blocks, a below-depth count no other limit gives that family.
#: `1088m` resolves one reader of 128 MiB blocks — the count `1g` resolves too,
#: so it adds a second cell of that arrangement rather than a count — and
#: eleven of 24 MiB blocks. The 128 MiB family's below-depth regime is
#: therefore one distinct count, and a line through it alone would be an
#: intercept asserted as a measurement; the family's fit is over all its
#: counts, which is what `RESERVE_FIT_MIN_COUNTS` holds.
#:
#: **Additive, so nothing already read moves**: `512m` keeps the axis's worst
#: headroom and the other three keep theirs. `544m` is also close to the
#: smallest arrangement the mechanism has, which is where a fit over the 24 MiB
#: family is checked rather than extrapolated
#: (`.claude/skills/evidence/SKILL.md`, rule 2).
#:
#: **A leg may be OOM-killed, and that is a reading rather than an apparatus
#: failure** — see `KILL_TOLERANT`, which is where that licence is granted and
#: bounded. The rule aims resident at the limit by construction, so every
#: flagless leg that takes the block path sits close to its own ceiling.
RESERVE_LIMITS: tuple[tuple[str, int], ...] = (
    ("512m", 512 << 20),
    ("544m", 544 << 20),
    ("1g", 1 << 30),
    ("1088m", 1088 << 20),
    ("1536m", 1536 << 20),
    ("2g", 2 << 30),
)

RESERVE_MECHANISM_INPUT = "control_xz"

#: The block size of the input the mechanism legs run over, read out of
#: `RESERVE_FLAGLESS_INPUTS` rather than written again, so the unit the step's
#: budgets are computed from and the unit the flagless table prints cannot
#: disagree.
RESERVE_MECHANISM_UNIT = next(
    unit for name, _, unit in RESERVE_FLAGLESS_INPUTS if name == RESERVE_MECHANISM_INPUT
)

#: The path step's two stated budgets: exactly what one block-decoding reader of
#: that file costs the budget rule, and **one byte less**.
#:
#: `block_path_afforded` is the comparison, so the pair straddles it and nothing
#: else differs between the two runs — which is what makes the difference
#: between them the whole block path against the streaming fallback rather than
#: a budget change with a path change inside it.
#:
#: **It is `charge_bytes(unit, 1)` and not `reader_bytes(unit)`**:
#: `BlockCache::affordable` charges that one reader's share of the retention
#: list, so a pair straddling the per-reader term alone would run the streaming
#: decoder on *both* legs and publish their difference as the cost of a path
#: neither took.
#:
#: It earns its place whatever the attribution finds: it is what an operator
#: needs in order to decide whether `--memory` is worth setting, and no
#: figure states it.
RESERVE_STEP_BUDGETS: tuple[int, ...] = (
    memory_model.charge_bytes(RESERVE_MECHANISM_UNIT, 1),
    memory_model.charge_bytes(RESERVE_MECHANISM_UNIT, 1) - 1,
)

#: How many distinct reader counts a two-term fit must cover before its
#: **intercept** is published.
#:
#: **Three, because the model has two terms.** `resident = fixed + readers ×
#: per_reader` passes exactly through two points, so at two distinct counts the
#: residual the table prints is **`±0 MiB` by construction** and cannot be told
#: from a two-term model that happens to describe the mechanism. Three is the
#: smallest number of points at which the residual is a reading.
#:
#: **It is a property of the model, not of one family.** Every reader of a
#: fitted line here is reading `_least_squares`' two terms, so the guard lives
#: at that boundary — `_fit_or_secant`, which all three call sites cross — and
#: not in whichever renderer happened to notice it first.
#:
#: **Below it the slope survives and the intercept does not.** Both traps the
#: rule exists for (`.claude/skills/evidence/SKILL.md`, rule 2) are statements
#: about the intercept: curvature outside the window folds into it, and a
#: clamped term reads as a constant of the process. A difference between two
#: distinct reader counts is a measured **secant** and carries no model claim,
#: so it is published — named endpoints, no intercept, no residual — where
#: refusing the whole line would withhold a number that is sound and silently
#: withdraw the instrument's comparison against `reader_bytes` in a censored
#: sitting.
#:
#: `_least_squares` keeps its own floor of two, which is where the arithmetic
#: stops being defined; this is the *publication* rule above it, and it is the
#: one a reader of the table is relying on.
RESERVE_FIT_MIN_COUNTS = 3

#: Where a shape that writes its cache out of the container's own layer writes
#: it: `Session.time_run` mounts the sitting's warm directory here for every
#: shape in `CACHE_OUT_SHAPES`, and for no other.
CACHE_OUT_MOUNT = "/out"

#: The shapes writing their cache under `CACHE_OUT_MOUNT`.
CACHE_OUT_SHAPES: tuple[str, ...] = ("parse-cache-out",)


def _script(command: str) -> str:
    """The in-container shell for one command shape.

    The timer is a bash builtin inside the container. Nothing redirects stderr
    inside a timed command -- some shells route `time`'s own report through the
    timed command's redirection, which deletes the figure and leaves a labelled
    run with no number under it.

    **Every `pgdt` shape states its worker count**, because a shape that
    inherits the CLI's default measures whatever that default is on the day --
    see `SWEEP_JOBS`. `--check` refuses a shape that pins none."""
    q = "time /pgdt"
    j = f"--jobs {SWEEP_JOBS}"
    ns = NO_STATISTICS
    if command == "parse":
        return f"{q} parse --source /dump.sql --cache /tmp/x.pgdt {j} {ns} >/dev/null"
    if command == "parse-rss":
        # The same `parse` as above, wrapped so the run reports its own peak
        # resident set as well as its wall clock. The redirection is outside
        # the wrapper and takes pgdt's stdout with it; the reading goes to
        # stderr, where bash's `time` report already goes.
        return (
            f"time {parsing.PEAK_RSS} /pgdt parse --source /dump.sql "
            f"--cache /tmp/x.pgdt {j} {ns} >/dev/null"
        )
    if command == "parse-preamble":
        return (
            f"{q} parse --preamble-only --source /dump.sql --cache /tmp/x.pgdt "
            f"{j} >/dev/null"
        )
    if command == "parse-preamble-rss":
        # `rss-attribution`'s per-*table* leg: the same prepass as above,
        # wrapped so what it holds resident is the reading. It stops at the end
        # of the schema section, before a data block is read, which is what
        # separates a cost paid per table from one paid per `COPY` block --
        # `peak-rss`'s own inputs cannot, since `blocks4000` gives every table
        # exactly one block and the two coincide in it.
        return (
            f"time {parsing.PEAK_RSS} /pgdt parse --preamble-only "
            f"--source /dump.sql --cache /tmp/x.pgdt {j} >/dev/null"
        )
    if command == "info-cache-rss":
        # An index *deserialized* rather than built: the second route to the
        # per-table structures, with no scanner, no census and no splice
        # anywhere in the timed process.
        #
        # **The cache is built in the same container, outside the timer and
        # outside the wrapper.** A prebuilt one mounted from the host would
        # have to be staged per input and kept in step with the input's own
        # stamp; built here it is by construction this input's cache. `time`
        # and the wrapper both sit on the `info` alone, so `parse_bash_time`
        # and `parse_maxrss_kib` each still see exactly one report -- and the
        # wrapper is a fresh process, so its `RUSAGE_CHILDREN` cannot carry the
        # builder's peak.
        #
        # **The worker count is stated on the builder, and `info` states
        # none**: `info` takes no `--jobs` because it starts no workers, so
        # there is no default for it to inherit.
        return (
            f"/pgdt parse --source /dump.sql --cache /tmp/x.pgdt {j} {ns} >/dev/null && "
            f"time {parsing.PEAK_RSS} /pgdt info --cache /tmp/x.pgdt "
            ">/dev/null"
        )
    if command in ("query-nomatch-cached-rss", "query-nomatch-rss"):
        # The pair that isolates the un-throttled splice, one flag apart. A
        # table that never matches maps to EOF and renders no row, so what
        # differs between them is the save throttle and nothing else: with a
        # cache path `parse`'s throttle governs, with `--cache none` the
        # whole-list rebuild is paid per block (`KD5`).
        #
        # `query` rather than `parse` because `parse` refuses `--cache none`
        # outright -- "cache is disabled, but `parse` requires a cache file" --
        # so the un-throttled shape is reachable only through `query`.
        cache = "/tmp/x.pgdt" if command.endswith("cached-rss") else "none"
        return (
            f"time {parsing.PEAK_RSS} /pgdt query --source /dump.sql "
            f"--table public.nosuchtable --cache {cache} {j} >/dev/null"
        )
    if command == "parse-cache-out":
        # The cache goes to the mounted tmpfs, not the container's own layer,
        # and the removal is outside the timer.
        cache = f"{CACHE_OUT_MOUNT}/measure.pgdt"
        return f"rm -f {cache}; {q} parse --source /dump.sql --cache {cache} {j} {ns} >/dev/null"
    if command in ("query-typed", "query-strings"):
        # Over a data-level cache built ahead of the timer (`DATA_LEVEL_QUERIES`).
        mode = command.split("-")[1]
        return (
            f"{DATA_LEVEL_BUILDER}{q} query --source /dump.sql --table public.perf "
            f"{DATA_LEVEL_QUERY} --schema-mode {mode} {j} >/dev/null"
        )
    if command.startswith("query-project-"):
        # Typed, always: the figure is about what building a column costs, and
        # `strings` builds every column the same cheap way.
        width = command.rpartition("-")[2]
        if not width.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        return (
            f"{DATA_LEVEL_BUILDER}{q} query --source /dump.sql --table public.perf "
            f"{DATA_LEVEL_QUERY} --schema-mode typed {projection_flags(int(width))} {j} "
            ">/dev/null"
        )
    if command.startswith("query-where-"):
        # `strings`, always, for two reasons that agree. The typed `=` decodes
        # the literal once against the column's own type, so `zzz1` on an
        # `integer` column is `Error::PredicateValueDecode` before the first
        # row; and the zero-copy path is the one this lever is read against,
        # since `strings` is where the field split and the walk are most of
        # what the library does (`decisions.md`, "D29", whose split-and-walk and decode rows do not move with the
        # mode).
        expr = predicate_expr(command.removeprefix("query-where-"))
        return (
            f"{DATA_LEVEL_BUILDER}{q} query --source /dump.sql --table public.perf "
            f"{DATA_LEVEL_QUERY} --schema-mode strings --where '{expr}' {j} >/dev/null"
        )
    if command == "query-nomatch":
        # Maps to EOF (the table never matches) and never saves.
        return (
            f"{q} query --source /dump.sql --table public.nosuchtable --cache none "
            f"{j} >/dev/null"
        )
    if command.startswith(STATISTICS_FAMILY):
        # `statistics-gathering`: the same wrapped `parse` as `parse-rss`, its
        # level stated by the leg. Unsuffixed, the same `parse` unwrapped,
        # which is what `--count-pair` counts: the counter must sit directly
        # around the timed `/pgdt` (`counted_script`).
        leg, _, suffix = command.removeprefix(STATISTICS_FAMILY).partition("-")
        flags = dict(STATISTICS_LEGS)
        if leg not in flags or suffix not in ("rss", ""):
            raise ValueError(f"unknown command shape {command!r}")
        if not suffix:
            return (
                f"{q} parse --source /dump.sql --cache /tmp/x.pgdt {j} {flags[leg]} "
                ">/dev/null"
            )
        return (
            f"time {parsing.PEAK_RSS} /pgdt parse --source /dump.sql "
            f"--cache /tmp/x.pgdt {j} {flags[leg]} >/dev/null"
        )
    if command == STRICT_PARSE:
        # `--count-pair`'s strict row: the unwrapped data-level `parse` above,
        # checking every field.
        return (
            f"{q} parse --source /dump.sql --cache /tmp/x.pgdt {j} {GATHER_STATISTICS} "
            f"{STRICT_FLAG} >/dev/null"
        )
    if command.startswith(PRUNING_FAMILY):
        # `statistics-pruning`. The builder is outside the timer and states the
        # gathering request; the timed `query` states which use it makes of it.
        # `typed`, stated: the range compares `id` as an integer.
        name, _, leg = command.removeprefix(PRUNING_FAMILY).rpartition("-")
        if name not in PRUNING_FILTERS or leg not in PRUNING_LEGS:
            raise ValueError(f"unknown command shape {command!r}")
        expr = PRUNING_FILTERS[name][0]
        return (
            f"/pgdt parse --source /dump.sql --cache /tmp/x.pgdt {j} "
            f"{GATHER_STATISTICS} >/dev/null && "
            f"{q} query --source /dump.sql --table public.perf --cache /tmp/x.pgdt "
            f"--schema-mode typed --where '{expr}' --statistics {leg} {j} >/dev/null"
        )
    if command == DYNFILTER_STARTUP:
        # The builder and the registration exactly as a leg has them, and a
        # query whose answer is read back outside the timer, so a startup that
        # did not answer is refused rather than timed.
        env = [f"{DFCLI_PARTITIONS}={SWEEP_JOBS}"]
        argv = ["--source", f"{DFCLI_CATALOG}=/dump.sql", "--format", "csv", "-q", "-c", STARTUP_SQL]
        return (
            f"/pgdt parse --source /dump.sql --cache /dump.sql.pgdt {j} "
            f"{GATHER_STATISTICS} >/dev/null && "
            f"time {dfcli_shell(env, SQL_SHELL, argv)} "
            ">/tmp/result.csv && "
            "echo startup_answer=$(sed -n 2p /tmp/result.csv)"
        )
    if command.startswith(DYNFILTER_FAMILY):
        # `dynamic-filter-join` and `-topk`. The builder is outside the timer,
        # as `statistics-pruning`'s is, and writes the cache beside the dump,
        # where `--source` looks — the root of the container's own layer, since
        # the dump is mounted read-only. The answer is read back outside the
        # timer too, as `key=value` lines `parse_reported` takes.
        figure, _, rest = command.removeprefix(DYNFILTER_FAMILY).partition("-")
        name, _, leg = rest.rpartition("-")
        if name not in DYNFILTER_QUERIES.get(figure, {}) or leg not in DYNFILTER_LEGS:
            raise ValueError(f"unknown command shape {command!r}")
        env, argv = dfcli_invocation(figure, name, leg, "/dump.sql")
        return (
            f"/pgdt parse --source /dump.sql --cache /dump.sql.pgdt {j} "
            f"{GATHER_STATISTICS} >/dev/null && "
            f"time {dfcli_shell(env, SQL_SHELL, argv)} "
            f">/tmp/result.csv && {DFCLI_ANSWER}"
        )
    if command.startswith("parse-chunk-"):
        # The read chunk, the one lever of the three I/O defaults that is a
        # value rather than a scheme. `parse` rather than `query`: this is
        # about the bytes arriving, and the row machinery above it is what the
        # rest of the register measures.
        size = command.rpartition("-")[2]
        if not size.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(size) not in CHUNK_SIZES:
            raise ValueError(f"{command!r} names a chunk size the figure does not carry")
        return (
            f"{q} parse --source /dump.sql --cache /tmp/x.pgdt "
            f"--chunk-size {size} {j} {ns} >/dev/null"
        )
    if command.startswith(RESERVE_FAMILY):
        # The reserve family: one stated budget, one stated worker count, and
        # an arena setting in front of the wrapper rather than on `nerdctl
        # run`, so the whole leg is visible in the recorded argv the way every
        # other apparatus choice is.
        #
        # **The assignment goes before `peak-rss`, not before `/pgdt`.** The
        # wrapper starts its arguments with its own environment, so the child
        # inherits it; an assignment written on the inner command would be a
        # further argument to `peak-rss` and would set nothing.
        token, _, budget = command.removeprefix(RESERVE_FAMILY).rpartition("-")
        arenas = {name: value for name, value, _ in RESERVE_ARENAS}
        if token not in arenas:
            raise ValueError(f"{command!r} names an arena setting the figure does not carry")
        if not budget.isdigit() or int(budget) not in RESERVE_BUDGETS:
            raise ValueError(f"{command!r} names a budget the figure does not carry")
        arena = f"MALLOC_ARENA_MAX={arenas[token]} " if arenas[token] else ""
        return (
            f"time {arena}{parsing.PEAK_RSS} /pgdt parse "
            f"--source /dump.sql --cache /tmp/x.pgdt "
            f"--jobs {RESERVE_JOBS} --memory {memory_model.stated_allowance(int(budget))} {ns} >/dev/null"
        )
    if command.startswith(RESERVE_FLAGLESS):
        # The flagless family: the same `parse` under the same wrapper, with
        # **neither** flag, so what runs is what `Discovered::resolve`
        # resolves from the container's own limit. The arena setting is still a
        # token, because the arena-cap mechanism leg is this shape with
        # `MALLOC_ARENA_MAX` set and nothing else changed.
        #
        # **The container limit is not here**, and it cannot be: it is a
        # `nerdctl run` argument rather than an argv one, so it rides on the
        # `RunSpec` instead (`RunSpec.memory`) and is what two legs of this
        # family differ by.
        token = command.removeprefix(RESERVE_FLAGLESS)
        arenas = {name: value for name, value, _ in RESERVE_ARENAS}
        if token not in arenas:
            raise ValueError(f"{command!r} names an arena setting the figure does not carry")
        arena = f"MALLOC_ARENA_MAX={arenas[token]} " if arenas[token] else ""
        return (
            f"time {arena}{parsing.PEAK_RSS} /pgdt parse "
            f"--source /dump.sql --cache /tmp/x.pgdt {ns} >/dev/null"
        )
    if command.startswith(RESERVE_STEP_FAMILY):
        # The path step: a stated budget either side of `charge_bytes(unit, 1)`
        # — what `BlockCache::affordable` charges one reader, pool list
        # included, and not `reader_bytes` alone — one byte
        # apart, so the two runs differ by whether `BlockCache::affordable`
        # admits a block-decoding reader and by nothing else.
        budget = command.removeprefix(RESERVE_STEP_FAMILY)
        if not budget.isdigit() or int(budget) not in RESERVE_STEP_BUDGETS:
            raise ValueError(f"{command!r} names a budget the figure does not carry")
        return (
            f"time {parsing.PEAK_RSS} /pgdt parse "
            f"--source /dump.sql --cache /tmp/x.pgdt "
            f"--jobs {RESERVE_JOBS} --memory {memory_model.stated_allowance(int(budget))} {ns} >/dev/null"
        )
    if command == PARALLEL_SCAN:
        # The provider's typed scan at the apparatus's count, read as
        # `PARALLEL_SCAN`'s axis rows are read (`typed_scan_invocation`).
        env, argv = typed_scan_invocation("/dump.sql")
        return (
            f"{PROVIDER_BUILDER}time {dfcli_shell(env, SQL_SHELL, argv)} "
            f">/tmp/result.csv && {DFCLI_ANSWER}"
        )
    if command.startswith(GZPASS_FAMILY):
        # A gzip first pass at a count `gzip-first-pass` carries, and the plain
        # `parse` it falls toward at one, the shape `parallel-scan-throughput`'s
        # `parse` rows are under the figure's own budget.
        jobs = command.removeprefix(GZPASS_FAMILY)
        if not jobs.isdigit() or int(jobs) not in GZPASS_JOBS:
            raise ValueError(f"{command!r} names a job count the figure does not carry")
        return (
            f"{q} parse --source /dump.sql --cache /tmp/x.pgdt --jobs {jobs} "
            f"--memory {memory_model.stated_allowance(GZPASS_BUDGET)} {ns} >/dev/null"
        )
    if command.startswith(GZRSS_FAMILY):
        # `parse-rss-jobs-<n>` over a gzip file under `gzip-resident`'s
        # budget, which affords its widest row a first pass's task each.
        jobs = command.removeprefix(GZRSS_FAMILY)
        if not jobs.isdigit() or int(jobs) not in GZIP_RSS_JOBS:
            raise ValueError(f"{command!r} names a job count the figure does not carry")
        return (
            f"time {parsing.PEAK_RSS} /pgdt parse --source /dump.sql --cache /tmp/x.pgdt "
            f"--jobs {jobs} --memory {memory_model.stated_allowance(GZPASS_BUDGET)} {ns} >/dev/null"
        )
    if command.startswith(LONG_RSS_FAMILY):
        # `long-line-resident`: one pass at one count under the wrapper, every
        # leg stating its sitting's allowance (`ALLOWANCE_SLOT`). A query's or filter's builder is outside the
        # timer and the wrapper, a process of its own, so neither reads its peak.
        name, _, jobs = command.removeprefix(LONG_RSS_FAMILY).rpartition("-")
        if name not in dict(LONG_LINE_PASSES) or not jobs.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(jobs) not in LONG_LINE_JOBS:
            raise ValueError(f"{command!r} names a job count the figure does not carry")
        if name == "query":
            env, argv = long_line_invocation(int(jobs), "/dump.sql")
            return (
                f"{LONG_LINE_BUILDER}time {dfcli_shell(env, f'{parsing.PEAK_RSS} {SQL_SHELL}', argv)} "
                f">/tmp/result.csv && {DFCLI_ANSWER}"
            )
        if name == "filter":
            return f"{LONG_LINE_BUILDER}time {parsing.PEAK_RSS} {long_line_filter(int(jobs))}"
        strict = f" {STRICT_FLAG}" if name == "strict" else ""
        return (
            f"time {parsing.PEAK_RSS} /pgdt parse --source /dump.sql --cache /tmp/x.pgdt "
            f"--jobs {jobs} --memory {ALLOWANCE_SLOT} {GATHER_STATISTICS}{strict} >/dev/null"
        )
    if command.startswith(LONG_FACTOR_FAMILY):
        # `long-line-resident`'s instrument legs: the timed legs' query or
        # filter at `SWEEP_JOBS`, under no wrapper and no allowance, for the
        # report its process writes at exit.
        name = command.removeprefix(LONG_FACTOR_FAMILY)
        if name == "query":
            env, argv = long_line_invocation(SWEEP_JOBS, "/dump.sql", None)
            return (
                f"{LONG_LINE_BUILDER}time {dfcli_shell(env, SQL_SHELL, argv)} "
                f">/tmp/result.csv && {DFCLI_ANSWER}"
            )
        if name == "filter":
            return f"{LONG_LINE_BUILDER}time {long_line_filter(SWEEP_JOBS, None)}"
        raise ValueError(f"unknown command shape {command!r}")
    if command.startswith(JOBS_AXIS):
        # The three shapes whose worker count is a figure's axis rather than the
        # apparatus's constant. Each `pgdt` one is otherwise the shape it is
        # named after, so a `parse` row of `parallel-scan-throughput` and the
        # corresponding row of `scan-throughput-warm` differ in `--jobs` and
        # `--memory` and nothing else.
        #
        # **The budget is stated on every row, including the first, where it is
        # inert**: `--jobs 1` is `Parallelism::Serial` and `Serial` carries no
        # budget, so the baseline runs at the library's own default. Writing the
        # flag anyway keeps the argv one shape rather than two, and the table is
        # what says the baseline is the serial path.
        shape, _, jobs = command.rpartition("-jobs-")
        if not jobs.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(jobs) not in PARALLEL_JOBS:
            raise ValueError(f"{command!r} names a job count the figure does not carry")
        p = f"--jobs {jobs} --memory {memory_model.stated_allowance(PARALLEL_BUDGET)}"
        if shape == "parse":
            return f"{q} parse --source /dump.sql --cache /tmp/x.pgdt {p} {ns} >/dev/null"
        if shape == "parse-rss":
            return (
                f"time {parsing.PEAK_RSS} /pgdt parse --source /dump.sql "
                f"--cache /tmp/x.pgdt {p} {ns} >/dev/null"
            )
        if shape == PARALLEL_SCAN:
            # The dynamic-filter figures' builder and read-back around
            # `PARALLEL_SCAN_SQL`: the builder states `SWEEP_JOBS`, not the
            # row's count, so every row reads one cache, written beside the
            # dump where `--source` looks; the count is the provider's
            # `target_partitions`, and `--memory` its `SET pgdump.memory`.
            env, argv = parallel_scan_invocation(int(jobs), "/dump.sql")
            return (
                f"{PROVIDER_BUILDER}time {dfcli_shell(env, SQL_SHELL, argv)} "
                f">/tmp/result.csv && {DFCLI_ANSWER}"
            )
        if shape == PARALLEL_SCAN_PREBUILT:
            # The same query with no builder: `Session.prebuild` wrote the
            # cache in its own container (`PREBUILT_BUILDER`) and `time_run`
            # mounts it read-only beside the dump (`PARALLEL_SCAN_PREBUILT`).
            env, argv = parallel_scan_invocation(int(jobs), "/dump.sql")
            return (
                f"time {dfcli_shell(env, SQL_SHELL, argv)} "
                f">/tmp/result.csv && {DFCLI_ANSWER}"
            )
        raise ValueError(f"unknown command shape {command!r}")
    if command.startswith("decode-"):
        # The `xz_decode` example, not `pgdt`: nothing in the library decodes
        # concurrently yet, so this figure reaches the decoder's own bulk entry
        # point directly. `/pgdt` is the harness's fixed mount point for
        # whichever binary a spec names, and `/dump.sql` its fixed mount point
        # for the input, whatever that input actually is.
        #
        # **Nothing is redirected.** The instrument's stdout is where the
        # decoded byte count and the admitted worker count come back, which is
        # what the table's rate is computed from and what says the worker count
        # was not clamped.
        workers = command.rpartition("-")[2]
        if not workers.isdigit():
            raise ValueError(f"unknown command shape {command!r}")
        if int(workers) not in DECODE_WORKERS:
            raise ValueError(f"{command!r} names a worker count the figure does not carry")
        return f"time /pgdt --source /dump.sql --workers {workers}"
    if command.startswith(INFLATE_FAMILY):
        # The `gzip-inflate` instrument, mounted where every figure's binary is:
        # one serial inflate of the whole member. **Nothing is redirected**,
        # for the decode instrument's reason — its stdout is where the
        # plaintext it delivered comes back.
        backend = command.removeprefix(INFLATE_FAMILY)
        if backend not in INFLATE_BACKENDS:
            raise ValueError(f"{command!r} names a backend the figure does not carry")
        return f"time /pgdt --source /dump.sql --backend {backend}"
    if command.startswith(GZSEEK_FAMILY):
        # `pgdt-gzip-seek`'s `gz_read` example, one build an engine, mounted where
        # every figure's binary is. **Nothing is redirected**, for the inflate
        # instrument's reason.
        stage, _, engine = command.removeprefix(GZSEEK_FAMILY).partition("-")
        if stage not in GZSEEK_STAGES or engine not in GZSEEK_ENGINES:
            raise ValueError(f"{command!r} names a stage or engine the figure does not carry")
        return f"time /pgdt --source /dump.sql --spacing {GZSEEK_SPACING} --shape {stage}"
    if command.startswith(GZSPACING_FAMILY):
        # The same example, the shipped engine's build, at one candidate
        # spacing. **Nothing is redirected**, for the inflate instrument's
        # reason.
        stage, _, mib = command.removeprefix(GZSPACING_FAMILY).partition("-")
        if stage not in GZSPACING_STAGES or not mib.isdigit() or int(mib) not in GZSPACING_MIB:
            raise ValueError(f"{command!r} names a stage or spacing the figure does not carry")
        return f"time /pgdt --source /dump.sql --spacing {int(mib) * config.MIB} --shape {stage}"
    if command == "dd":
        return "time dd if=/dump.sql of=/dev/null bs=4M"
    raise ValueError(f"unknown command shape {command!r}")


def dynfilter_shapes(figure: str | None = None) -> tuple[str, ...]:
    """The dynamic-filter figures' command shapes, or one figure's, in its
    table's order: each query, in `DYNFILTER_LEGS`' order."""
    return tuple(
        f"{DYNFILTER_FAMILY}{fig}-{name}-{leg}"
        for fig, queries in DYNFILTER_QUERIES.items()
        if figure in (None, fig)
        for name in queries
        for leg in DYNFILTER_LEGS
    )


def command_shapes() -> tuple[str, ...]:
    """Every command shape `_script` builds, enumerated.

    `_script` parses its argument rather than matching it, so the
    parameterized families are expanded from the registries that define them
    — one list, not two. What this exists for is the worker-count
    reconciliation below, which has to iterate the shapes to check them;
    `test_shapes.py` holds it against `_script`'s own branches, so a shape
    added there and not here is an error rather than a silent exemption."""
    return (
        "parse",
        "parse-rss",
        "parse-preamble",
        "parse-preamble-rss",
        "info-cache-rss",
        "parse-cache-out",
        "query-typed",
        "query-strings",
        "query-nomatch",
        "query-nomatch-cached-rss",
        "query-nomatch-rss",
        *(f"query-project-{w}" for w in PROJECTION_WIDTHS),
        *(f"query-where-{s}" for s in PREDICATE_SHAPES),
        *(f"parse-chunk-{n}" for n in CHUNK_SIZES),
        *(f"{STATISTICS_FAMILY}{leg}{rss}" for leg, _ in STATISTICS_LEGS for rss in ("-rss", "")),
        STRICT_PARSE,
        *(f"{PRUNING_FAMILY}{name}-{leg}" for name in PRUNING_FILTERS for leg in PRUNING_LEGS),
        *dynfilter_shapes(),
        DYNFILTER_STARTUP,
        PARALLEL_SCAN,
        *(f"{family}{n}" for family in JOBS_AXIS for n in PARALLEL_JOBS),
        *(f"{GZPASS_FAMILY}{n}" for n in GZPASS_JOBS),
        *(f"{GZRSS_FAMILY}{n}" for n in GZIP_RSS_JOBS),
        *(f"{LONG_RSS_FAMILY}{name}-{n}" for name, _ in LONG_LINE_PASSES for n in LONG_LINE_JOBS),
        *(f"{LONG_FACTOR_FAMILY}{name}" for name in LONG_LINE_PLANNED),
        *(
            f"{RESERVE_FAMILY}{token}-{budget}"
            for token, _, _ in RESERVE_ARENAS
            for budget in RESERVE_BUDGETS
        ),
        *(f"{RESERVE_FLAGLESS}{token}" for token, _, _ in RESERVE_ARENAS),
        *(f"{RESERVE_STEP_FAMILY}{budget}" for budget in RESERVE_STEP_BUDGETS),
        *(f"decode-{w}" for w in DECODE_WORKERS),
        *INFLATE_SHAPES,
        *GZSEEK_SHAPES,
        *GZSPACING_SHAPES,
        "dd",
    )

#: A worker count stated on a command line: `pgdt`'s `--jobs`, or the decode
#: instrument's own `--workers`. Either spelling pins the count; what fails is
#: a shape carrying neither.
_WORKER_COUNT = re.compile(r"--(?:jobs|workers) \d+")

#: The shapes that state no worker count and are right not to, being no run
#: of `pgdt`: `dd`, the device floor, and the inflate and `pgdt-gzip-seek`
#: instruments', each of which has one thread by construction and no default a
#: count could be inherited from. Named rather than inferred, so another
#: non-`pgdt` shape has to be admitted here on purpose.
_NO_WORKERS = ("dd", *INFLATE_SHAPES, *GZSEEK_SHAPES, *GZSPACING_SHAPES)

#: The one `pgdt` family that states no worker count and is right not to: the
#: reserve figure's flagless legs, whose reading *is* the count a flagless run
#: resolves. Declared as its own prefix rather than appended to `_NO_WORKERS`
#: above, because that tuple's rule is "no run of `pgdt`" and this is the
#: opposite — it is exactly a run of `pgdt`, stating nothing on purpose
#: (`RESERVE_FLAGLESS`).
_NO_FLAGS = (RESERVE_FLAGLESS,)


def worker_count_problems() -> list[str]:
    """Command shapes that inherit a worker count instead of stating one.

    The mechanical half of "a worker count is apparatus" (`SWEEP_JOBS`): a
    shape that pins nothing measures whatever the CLI's `--jobs` defaults to
    that day, and no table can say which arrangement it read. That is how the
    default has moved underneath the published figures with no shape changing
    and nothing noticing.

    **The flagless family is exempt and declares itself** (`_NO_FLAGS`): its
    reading is the count a flagless run resolves, so a shape that pinned one
    would measure an arrangement the shipped default never produces.
    `flagless_flag_problems` is the other side of that exemption — it holds
    those shapes to stating *neither* flag, so the exemption cannot become a
    quiet pin.

    **A shape running the SQL shell states its count as its partitions**
    (`_dfcli_partitions`), and each `pgdt parse` beside it its own `--jobs`,
    so one running no builder — `PARALLEL_SCAN_PREBUILT`, whose builder runs
    outside it — needs no `--jobs` to pin it."""

    def inherits(script: str) -> bool:
        partitions = _dfcli_partitions(script)
        if partitions is None:
            return not _WORKER_COUNT.search(script)
        return partitions == set() or any(
            not _WORKER_COUNT.search(run) for run in _PARSE_RUN.findall(script)
        )

    return [
        command
        for command in command_shapes()
        if command not in _NO_WORKERS
        and not command.startswith(_NO_FLAGS)
        and inherits(_script(command))
    ]

#: The partition count a run of `SQL_SHELL` states: its session's
#: `target_partitions`, from the environment in front of it.
#: A shell under the peak-resident wrapper states its count ahead of the
#: wrapper, which hands its environment to the shell (`LONG_RSS_FAMILY`).
_DFCLI_PARTITIONS = re.compile(
    rf"\b{DFCLI_PARTITIONS}=(\d+) (?:\S+=\S+ )*(?:{re.escape(parsing.PEAK_RSS)} )?{re.escape(SQL_SHELL)} "
)


def _dfcli_partitions(script: str) -> set[str] | None:
    """The partition counts every run of `SQL_SHELL` in `script` states, `set()`
    where one of them states none, or `None` for a script that runs none.

    **A run of the SQL shell is held to its own count**, because the
    shapes that run it run `pgdt parse` too: the untimed builder's `--jobs` would
    otherwise satisfy `_WORKER_COUNT` for a timed run that inherits
    DataFusion's `target_partitions`, which defaults to the core count."""
    runs = script.count(f"{SQL_SHELL} ")
    if not runs:
        return None
    stated = _DFCLI_PARTITIONS.findall(script)
    return set(stated) if len(stated) == runs else set()


def stated_threads(command: str) -> int | None:
    """The most workers any program in a command shape states, which is what
    `--pin-cpus` places it by, or `None` where one of them discovers its own.

    Read off the script rather than declared beside the shape, so the count a
    leg is placed by is the count it runs at. `dd` and the inflate instrument
    state none and are one thread each; the flagless family states none on purpose and stays unpinned,
    its reading being what discovery resolves."""
    if command in _NO_WORKERS:
        return 1
    if command.startswith(_NO_FLAGS):
        return None
    script = _script(command)
    counts = [int(n) for n in re.findall(r"--(?:jobs|workers) (\d+)", script)]
    partitions = _dfcli_partitions(script)
    if partitions == set():
        return None
    counts += [int(n) for n in partitions or ()]
    return max(counts) if counts else None

#: One `pgdt parse` invocation inside a command shape's script, up to the next
#: command separator: a `;`, or the `&&` every builder is joined by.
_PARSE_RUN = re.compile(r"/pgdt parse (?:(?!&&)[^;])*")

#: The command-shape prefixes `statistics_flag_problems` lets state
#: `GATHER_STATISTICS`.
GATHERING_FAMILIES: tuple[str, ...] = (
    STATISTICS_FAMILY,
    STRICT_PARSE,
    PRUNING_FAMILY,
    DYNFILTER_FAMILY,
    PARALLEL_SCAN,
    *DATA_LEVEL_QUERIES,
    LONG_RSS_FAMILY,
    LONG_FACTOR_FAMILY,
)


def statistics_flag_problems() -> list[str]:
    """Command shapes running a `pgdt parse` that does not state
    `NO_STATISTICS`.

    `parse` gathers statistics unless told not to, so such a shape times the
    gathering default rather than the scan its figure names. `--preamble-only`
    stops before any row and refuses the flag, so it is exempt.

    **The families whose subject is the gathering or what it buys may state
    `GATHER_STATISTICS` instead**, and only that — the two statistics
    figures', the dynamic-filter figures', whose scans prune by what the
    untimed builder gathered, and the query figures', which read the cache it
    wrote (`DATA_LEVEL_QUERIES`, `PARALLEL_SCAN`), and `long-line-resident`'s, a default
    parse being one of its passes (`LONG_RSS_FAMILY`) and its instrument legs reading
    through its builder's cache (`LONG_FACTOR_FAMILY`) — and a `parse` of theirs that inherits
    the request is reported exactly as anyone else's is."""
    return [
        command
        for command in command_shapes()
        if any(
            NO_STATISTICS not in run
            and "--preamble-only" not in run
            and not (command.startswith(GATHERING_FAMILIES) and GATHER_STATISTICS in run)
            for run in _PARSE_RUN.findall(_script(command))
        )
    ]

#: A byte budget stated on a command line. The flagless family must carry
#: neither this nor a worker count.
_BUDGET_STATED = re.compile(r"--memory \d+")


def flagless_flag_problems() -> list[str]:
    """Shapes in the flagless family that state a flag after all.

    The exemption `_NO_FLAGS` opens is from *stating a count*, and its whole
    premise is that the arrangement under test is the one a run with no flags
    resolves for itself. A shape that quietly acquired `--jobs` or
    `--memory` would still pass `worker_count_problems` — it is exempt
    — and would publish a stated arrangement under a heading that says
    discovered, which is the failure the count reconciliation exists against
    seen from the far side."""
    bad = []
    for command in command_shapes():
        if not command.startswith(_NO_FLAGS):
            continue
        script = _script(command)
        stated = [*_WORKER_COUNT.findall(script), *_BUDGET_STATED.findall(script)]
        if stated:
            bad.append(f"{command} states {', '.join(sorted(stated))}")
    return bad


def pinned_count_problems() -> list[str]:
    """Command shapes stating a worker count that is neither `SWEEP_JOBS` nor a
    declared axis.

    The other half of the exemption `JOBS_AXIS` opens. `worker_count_problems`
    catches a shape that pins *nothing*; this catches one that pins something
    else — a shape edited to `--jobs 4` because a sitting wanted it that day, or
    a shape moved into a parallel family without being declared one. Either
    produces a table whose apparatus line is wrong about it, which is the same
    defect from the other side and is the one no `--stale` can see.

    The decode instrument states `--workers`, not `--jobs`, and its counts are
    its own figure's axis; it is exempt for the same reason, by prefix, and so
    are `gzip-first-pass`'s, `gzip-resident`'s and `long-line-resident`'s families
    (`GZPASS_FAMILY`, `GZRSS_FAMILY`, `LONG_RSS_FAMILY`). So are the reserve figure's two stated families, whose
    count is the figure's own constant (`RESERVE_JOBS`), and its flagless one,
    which states nothing and is held to that by `flagless_flag_problems`.
    """
    bad = []
    for command in command_shapes():
        if command in _NO_WORKERS or command.startswith(
            ("decode-", GZPASS_FAMILY, GZRSS_FAMILY, LONG_RSS_FAMILY, RESERVE_FAMILY,
             RESERVE_STEP_FAMILY, *_NO_FLAGS, *JOBS_AXIS)
        ):
            continue
        stated = set(_WORKER_COUNT.findall(_script(command)))
        if stated != {f"--jobs {SWEEP_JOBS}"}:
            bad.append(f"{command} states {', '.join(sorted(stated)) or 'nothing'}")
        partitions = _dfcli_partitions(_script(command))
        if partitions is not None and partitions != {str(SWEEP_JOBS)}:
            bad.append(
                f"{command} states {DFCLI_PARTITIONS}="
                f"{', '.join(sorted(partitions)) or 'nothing'}"
            )
    return bad

#: The worker counts each leg is read at: the serial path, `POOL_DEPTH`, and
#: the machine's hardware threads. **Three, where the spec asks two**: the
#: block leg's pool is a constant below `POOL_DEPTH` and grows a unit a reader
#: above it, so a reading either side of the depth is what separates the two
#: regimes, and the top count is where a first pass runs the most tasks
#: ahead, which only a stated `--jobs` reaches, a first pass recommending its
#: physical cores. Each is a `PARALLEL_JOBS` row, so the shape is
#: `parallel-peak-rss`'s, over another file and under `GZPASS_BUDGET`
#: (`GZRSS_FAMILY`).
GZIP_RSS_JOBS: tuple[int, ...] = (1, 4, 24)
