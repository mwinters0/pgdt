"""`parallel-scan-throughput` and `parallel-peak-rss`: a parallel scan's rate and resident set by
worker count."""

from __future__ import annotations

from typing import Mapping
from typing import TYPE_CHECKING

import generate_perf_data as perf

from measurement import formatting
from measurement import gating
from measurement import memory_model
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement.figures import gate_table
from measurement.figures import prose

if TYPE_CHECKING:
    from measurement.timing import Session


#: The partition counts `parallel-scan-throughput`'s cold-NVMe leg is taken at:
#: the first three of `PARALLEL_JOBS`. **The regime test admits no more**
#: (`measurements.md`, "Scan throughput by input shape"): the warm typed scan's
#: demand approaches the NVMe's floor at two partitions and passes it at four,
#: so every row above four would read the device a second time.
PARALLEL_NVME_JOBS: tuple[int, ...] = (1, 2, 4)

#: The sub-stream count a provider leg of `parallel-scan-throughput` actually
#: gets from `PARALLEL_BUDGET`, keyed by input — `worker_count`'s
#: `WorkerMemory::affords`, reached through the scan budget's draw
#: (`crates/datafusion-pgdump/src/table.rs`, `scan`) — **for the legs
#: a budget clamp reaches at all**. A leg absent from this dict is one the
#: budget never clamps inside `PARALLEL_JOBS`, and it carries no per-cell
#: annotation, there being nothing to say.
#:
#: **The divisor is per source, not universal.** `plan_partitions` adds the
#: held batch's `max_source_span` only where the source retains by the read
#: chunk (`pgdt_io::RetainedUnit`); a block-decoding `CompressedFileSource` retains by the
#: partition, whose decoded blocks `partition_bytes` has already charged, so its
#: per-reader term is what one reader of it holds and nothing more.
#:
#: **A span is spent before a count is cut.** `plan_partitions` narrows the
#: held batch's span to what the budget leaves once the readers asked for are
#: paid for, stopping at one *announced* read chunk — `ReadOptions::chunk_size_bytes`
#: (`docs/design/decisions.md`, "D84"), which no leg here states, so the shipped
#: default — so a plain leg's count is solved against that narrowed span and
#: not against the shipped 64 MiB ceiling.
#:
#: **Hand-computed, not derived from a mirrored formula.** A Python
#: reimplementation of `worker_count`/`plan_partitions` would be a second
#: authority on the library's own arithmetic and go stale silently the moment
#: either constant moves; a hardcoded value, like `PARALLEL_JOBS`'s literal 4
#: for `POOL_DEPTH`, is checked by hand against the source once and is exactly
#: as good until the constants it was checked against move, at which point the
#: figure is stale on the paths already in its `depends`.
#:  `.xz`:   `24 MiB` block (`control_xz`'s block size, the one being decoded
#:           and then retained) + `1 MiB` chunk buffer + `9,471,776 B` of
#:           decoder (an 8 MiB dictionary, the 1 MiB input chunk and
#:           `liblzma`'s own 34,592 B of state) = `34.03 MiB` a reader, the
#:           span not charged, plus a `24 MiB` unit of the shared retention
#:           list for every reader past `POOL_DEPTH`. The count is solved
#:           against `margin_allowance`, which binds here — `1,689 MiB` against
#:           a `2,048 MiB` cap — so `floor((1689 + 24) / 58.03) = 29`, still
#:           past the top of `PARALLEL_JOBS`, so no entry.
#:  plain:   the source recommends nothing, so `fit` hands it
#:           `DEFAULT_MEMORY_BUDGET` whatever is stated
#:           (`docs/design/decisions.md`, "D83") — `64 MiB`, not the stated
#:           `PARALLEL_BUDGET`. A reader costs `8 MiB` (`POOL_MAX_BYTES`, which
#:           is also what `LocalFileSource::partitions` applies its multiple to
#:           and is capped straight back to), so `n` readers leave
#:           `(64 - 8n) / n MiB` of span apiece: `24 MiB` at two and `8 MiB` at
#:           four, both of which seat every worker asked for. At eight there is
#:           nothing left, the span stops at the `1 MiB` floor, and `9 MiB` a
#:           sub-stream affords **seven** — which is the cap, every higher row
#:           of `PARALLEL_JOBS` landing on the same floor and the same seven.
QUERY_SUBSTREAM_CAP: dict[str, int] = {"control": 7}

# -- what a second scan worker buys -----------------------------------------

#: The four legs of `parallel-scan-throughput`, in the table's column order:
#: an input, the command family whose `-jobs-<n>` shapes it is run under, and
#: the caption the column carries.
#:
#: **Two axes crossed, both of which the phase argues about separately.**
#: Plain against `.xz` is whether there is a decoder in front of the scan;
#: `parse` against a typed scan is discovery against extraction. The rule
#: the design argues from is one line over those two — *parallelize what is CPU-bound*
#: (`docs/design/decisions.md`, "D25").
#: A table missing a column cannot check that rule; it would confirm whichever
#: half it kept.
#:
#: **The extraction legs run the provider, not `pgdt query`**
#: (`PARALLEL_SCAN`): `pgdt query`'s in-order merge reads one sub-stream at a
#: time past its first round (`KD57`), so a column of it times the merge
#: rather than the library's sub-streams, which DataFusion polls together.
PARALLEL_LEGS: tuple[tuple[str, str, str], ...] = (
    ("control", "parse", "Plain, `parse`"),
    ("control", shapes.PARALLEL_SCAN, "Plain, typed provider scan"),
    ("control_xz", "parse", "`.xz`, `parse`"),
    ("control_xz", shapes.PARALLEL_SCAN, "`.xz`, typed provider scan"),
)

#: The input whose byte count a leg's rate is per.
#:
#: **A compressed leg's rate is per *plaintext* byte, and the plaintext is a
#: registered input.** `control_xz` is a compression of `control`, so the
#: plaintext volume is `control`'s own size exactly — no instrument has to
#: report it and no seek table has to be read, which is the one thing
#: `xz-decode-scaling` could not do (its koji leg is a prefix of a file nothing
#: here generates). Dividing by the *compressed* size instead would state a rate
#: five times too low under a heading that reads like the plain one's.
PARALLEL_PLAINTEXT: dict[str, str] = {"control": "control", "control_xz": "control"}

#: `parallel-scan-throughput`'s fifth column: the plain typed provider scan
#: again, every rep cold off the NVMe, at `PARALLEL_NVME_JOBS`. **The one leg
#: the regime test admits on a real device** (`measurements.md`, "Scan
#: throughput by input shape"): a compressed leg never reaches either SSD's
#: floor, and `parse` is past both at one worker. Its cache is built ahead of
#: the sweep (`PARALLEL_SCAN_PREBUILT`).
PARALLEL_NVME_LEG: tuple[str, str, str] = (
    "control",
    shapes.PARALLEL_SCAN_PREBUILT,
    "Plain, typed provider scan, cold on the NVMe",
)


def _parallel_specs() -> list[runspec.RunSpec]:
    inp, family, label = PARALLEL_NVME_LEG
    return [
        runspec.RunSpec(
            "dfcli" if family == shapes.PARALLEL_SCAN else "pgdt",
            inp,
            f"{family}-jobs-{jobs}",
            "warm-parallel",
            f"{label}, {jobs}{'p' if family == shapes.PARALLEL_SCAN else 'j'}",
        )
        for inp, family, label in PARALLEL_LEGS
        for jobs in shapes.PARALLEL_JOBS
    ] + [
        runspec.RunSpec("dfcli", inp, f"{family}-jobs-{jobs}", "cold-nvme-parallel", f"{label}, {jobs}p")
        for jobs in PARALLEL_NVME_JOBS
    ]


def parallel_answer_problems(reported: Mapping[str, Mapping[str, str]]) -> list[str]:
    """Why `parallel-scan-throughput`'s provider legs, or `release-profile`'s
    `PARALLEL_SCAN` row across its builds, do not price one answer, keyed by
    each leg's `RunSpec.label`.

    **Every provider cell must return one answer**, byte for byte, at every
    partition count over both files, and under every profile: the two files
    are one plaintext, and neither the count nor the build changes a row, so a
    difference is a scan that lost or repeated rows under the timer. An answer
    of no row, or a first count of none, is a query that read nothing."""
    digests = {answer.get("result_digest") for answer in reported.values()}
    if not reported or None in digests or "" in digests or len(digests) != 1:
        return ["the provider legs answered differently, or not at all"]
    bad = []
    for label, answer in reported.items():
        if answer.get("result_rows") != "1" or answer.get("result_first", "0") == "0":
            bad.append(f"{label}: the query counted no row")
    return bad


def run_parallel_scan_throughput(session: Session) -> str:
    """Wall clock against the worker count, over four legs of one 3.00 GiB
    plaintext: `pgdt parse` at each `--jobs`, and the provider's typed scan at
    each `target_partitions`.

    **Every cell is read against the one-job cell of its own leg**, which is
    both the figure's content — what the second worker through the twenty-fourth
    buy — and its witness: `warm-parallel` gates on almost nothing, a reading
    that occupies every hardware thread being busy by construction, so what
    stands in for the gate is that a machine busy with someone else's work moves
    a leg's whole column and leaves the ratio (`CONTENTION_LIMITS`). It is also
    why two programs may share a table: no cell is read against another
    leg's.

    **The baseline row is the serial path, not a pool of one.** `--jobs 1` is
    `Parallelism::Serial` carrying the same stated allowance as every other row,
    so a compressed leg's first row is one block-decoding reader rather than the
    streaming fallback. That serial path is the arrangement this project ships,
    which is what makes it the right denominator.

    **Five reps**, for `xz-decode-scaling`'s reason: the increments that matter
    are between adjacent counts near the top of the curve, where three reps
    leave the ordering ambiguous.
    """
    figure = "parallel-scan-throughput"
    specs = _parallel_specs()
    session.prebuild(specs)
    session.sweep(figure, specs, session.cfg.reps(5))
    problems = parallel_answer_problems(
        {
            spec.label: session.reported.get(spec.key(figure), {})
            for spec in specs
            if spec.binary == "dfcli"
        }
    )
    if problems:
        raise RuntimeError(f"{figure} does not price one answer: " + "; ".join(problems))

    by_leg: dict[tuple[str, str], dict[int, list[float]]] = {}
    for spec in specs:
        family, _, jobs = spec.command.rpartition("-jobs-")
        by_leg.setdefault((spec.input, family), {})[int(jobs)] = session.get(figure, spec)

    rows = []
    for jobs in shapes.PARALLEL_JOBS:
        cells = [str(jobs) + (" *(serial)*" if jobs == shapes.PARALLEL_BASELINE else "")]
        for inp, family, _ in (*PARALLEL_LEGS, PARALLEL_NVME_LEG):
            values = by_leg[(inp, family)].get(jobs)
            if values is None:
                # The cold leg's rows stop where the regime test does
                # (`PARALLEL_NVME_JOBS`).
                cells.append("—")
                continue
            got, base = formatting.median(values), formatting.median(by_leg[(inp, family)][shapes.PARALLEL_BASELINE])
            nbytes = staging.file_size(
                session.cfg,
                session.input_path(PARALLEL_PLAINTEXT[inp], "warm-parallel"),
                PARALLEL_PLAINTEXT[inp],
            )
            cell = f"{formatting.fmt_median_spread(values)} · {formatting.fmt_rate(nbytes, got)} · {base / got:.2f}×"
            # A budget clamp is not `POOL_DEPTH`'s to footnote once — see
            # `QUERY_SUBSTREAM_CAP`. A provider leg the budget clamps inside
            # the axis states, on every row above four, the count it actually
            # planned, so a reader never has to ask whether a given cell is
            # the label or the ceiling; a leg absent from that dict is never
            # clamped and carries nothing.
            if family == shapes.PARALLEL_SCAN and jobs > 4 and inp in QUERY_SUBSTREAM_CAP:
                achieved = min(jobs, QUERY_SUBSTREAM_CAP[inp])
                cell += f" · {achieved} sub-stream{'s' if achieved != 1 else ''}"
            cells.append(cell)
        rows.append(cells)
    table = formatting.md_table(
        [
            "`--jobs` · `target_partitions`",
            *(label for _, _, label in (*PARALLEL_LEGS, PARALLEL_NVME_LEG)),
        ],
        rows,
    )

    plain = staging.file_size(session.cfg, session.input_path("control", "warm-parallel"), "control")
    compressed = staging.file_size(
        session.cfg, session.input_path("control_xz", "warm-parallel"), "control_xz"
    )
    allowance = memory_model.stated_allowance(shapes.PARALLEL_BUDGET)
    notes = (
        "\n\nEach cell is wall clock, the plaintext rate it implies, and the speedup over that "
        "leg's own one-worker row. Both `.xz` legs decode the same "
        f"{formatting._fmt_bytes(plain)} of plaintext the plain legs read directly "
        f"({formatting._fmt_bytes(compressed)} on disk, {plain / compressed:.2f}×), so every rate is per "
        "the same bytes.\n\n"
        "**The `parse` legs run `pgdt parse` at `--jobs` N; the provider legs run "
        f"`pgdt sql -c` at `{shapes.DFCLI_PARTITIONS}=N`**, the session's "
        "`target_partitions`, which the provider plans a scan's sub-streams against and "
        "DataFusion polls together. `pgdt query` is not timed: its in-order merge reads one "
        "sub-stream at a time past its first round "
        "([`../status/deficiencies.md`](../status/deficiencies.md), `KD57`). The query is "
        f"`SELECT count({perf.COLUMNS[0][0]}), …, count({perf.COLUMNS[-1][0]}) FROM "
        f"{perf.TABLE} WHERE id IS NOT NULL`, a `count` of each of the table's "
        f"{len(perf.COLUMNS)} columns: every column decoded typed "
        "and one row out, the filter keeping every row and leaving the scan's exact NULL "
        'counts estimates, so no count is answered without the rows (`docs/design/decisions.md`, '
        '"D89"). Every provider cell answered alike, byte for byte, at every count over both '
        "files. A provider cell is read against its own leg and never against a `parse` "
        "cell: each carries the program's startup and the dump's registration, which "
        "`dynamic-filter-join`'s startup leg reads.\n\n"
        f"Every row states the allowance `{allowance}` — `--memory {allowance}` on a `parse` "
        f"leg, `SET pgdump.memory = {allowance}` run ahead of the query in the same process "
        f"on a provider leg — which leaves read buffers {formatting._fmt_bytes(shapes.PARALLEL_BUDGET)} under "
        "the reserve, and the margin's ceiling below that from two workers up, in "
        f"a {shapes.PARALLEL_MEMORY} container — **not** the "
        "register's 512 MB, which cannot hold twenty-four decoded 24 MiB blocks. The "
        "one-worker row states the same allowance: `--jobs 1` is `Parallelism::Serial` "
        "carrying it, as is a provider scan planned at one partition, so an `.xz` leg's "
        "one-worker row is one block-decoding reader rather than the streaming fallback, and "
        "that serial path is what a speedup is a speedup over.\n\n"
        "**A plain leg's count is what is asked for, not what is delivered.** "
        "`POOL_DEPTH` clamps the chunk pool to four slots and the interior split lets a "
        "worker wait for one, so a fifth fused worker on a plain source waits. What that "
        "wait costs the rows above four is not separated from anything else they pay "
        '(`docs/design/decisions.md`, "D25").\n\n'
        + _substream_note()
        + "Each provider leg, at every count, reads a cache one untimed `pgdt parse` stating "
        f"`{shapes.GATHER_STATISTICS}` wrote beside the dump in the same container, where `--source` "
        "looks for it — but the cold leg below — so its reading carries decoding that cache "
        "whole, statistics included, and no mapping pass, and its filter rules out no row "
        "group.\n\n"
        "**The last column is the plain typed provider scan again, every rep cold off the "
        f"NVMe** (`cold-nvme-parallel`), at {', '.join(map(str, PARALLEL_NVME_JOBS))} "
        "partitions: the one leg the regime test admits on a real device (\"Scan throughput "
        "by input shape\"), each row read against its own one-partition row, and the device's "
        "floor being `scan-throughput-nvme`'s `dd`. The page cache is dropped before every "
        "rep, and the cache it reads is written by the same untimed `parse` once a sitting, "
        "in a container of its own, and mounted read-only beside the dump, so the dump and "
        "the cache both come off the device inside the timer. Every other column reads "
        "tmpfs.\n\n"
        f"**`PARALLEL_BUDGET` is {formatting._fmt_bytes(shapes.PARALLEL_BUDGET)} so that no `.xz` row is "
        "budget-clamped;** no stated allowance raises a plain source past the library's default "
        'budget (`docs/design/decisions.md`, "D83"), which is the clamp the counts above '
        "state. A compressed reader is charged its block, the chunk buffer and the "
        "decoder's own retention, and the readers together the block pool's retention list "
        '(`docs/design/decisions.md`, "I/O, memory and parallelism"), so a smaller '
        "budget would hold the widest `.xz` rows below the twenty-four they are labelled.\n"
    )
    return table + notes + "\n" + prose._per_rep(figure, session, specs)


def _substream_note() -> str:
    """What the provider columns say about the count they actually planned.

    Read off `QUERY_SUBSTREAM_CAP` rather than stated, because the dict is what
    decides whether a cell carries the annotation: a paragraph asserting a
    clamp the renderer did not print, or silence over one it did, is the
    divergence the harness owns its own prose to avoid.
    """
    head = (
        "**A provider leg's count can be clamped a second way, and that one the "
        "table states per cell rather than footnotes once.** `plan_partitions` solves a "
        "scan's sub-stream count against the read-buffer budget, each sub-stream costing "
        "its read plus a batch span narrowed toward the chunk size before the count is cut "
        '(`docs/design/decisions.md`, "D4", "D84") — a budget '
        "the *harness* chose, not a ceiling the library ships. "
    )
    if not QUERY_SUBSTREAM_CAP:
        return (
            head + "**At this budget neither provider leg reaches it**, so no cell "
            "carries the annotation: what each leg is charged and what it affords is "
            "`QUERY_SUBSTREAM_CAP`'s own argument, computed once there.\n\n"
        )
    clamped = ", ".join(
        f"`{count}` on {'`.xz`' if inp.endswith('_xz') else 'plain'}"
        for inp, count in sorted(QUERY_SUBSTREAM_CAP.items())
    )
    return (
        head + f"The rows at or above that count on such a leg state the count they actually "
        f"planned: {clamped}. Below that count a cell's sub-stream figure equals its "
        "row label; at or above it, every further worker asked for buys nothing more "
        "to plan.\n\n"
    )

# -- what a parallel scan holds resident ------------------------------------

#: The two legs of `parallel-peak-rss`: the same plaintext at two block sizes.
#: See `INPUTS["control_xz128"]` for why one of them would be a figure about the
#: file rather than about the library.
PARALLEL_RSS_LEGS: tuple[tuple[str, str], ...] = (
    ("control_xz", "24 MiB blocks"),
    ("control_xz128", "128 MiB blocks"),
)


def _parallel_rss_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec("pgdt", leg, f"parse-rss-jobs-{jobs}", "warm-parallel", f"{label}, {jobs}j")
        for leg, label in PARALLEL_RSS_LEGS
        for jobs in shapes.PARALLEL_JOBS
    ]


def run_parallel_peak_rss(session: Session) -> str:
    """Peak resident set against `--jobs`, at two `.xz` block sizes.

    **The claim under test is that one stated number bounds the read path**, so
    the table's own witness is the column that stops rising: a leg whose peak
    keeps climbing with `--jobs` is a budget that is not a bound, and a leg that
    stops *above* the read-buffer budget its allowance leaves is the block
    pool's slot ceiling following the announced count rather than the delivered
    one (`KD21`). Two block
    sizes because a compressed reader's per-worker footprint is one decoded
    block, so the count the budget admits is a property of the *file* — at one
    size the table would publish that file's shape as the library's ceiling.

    **Every row block-decodes, the one-job row included**: `--jobs 1` is
    `Parallelism::Serial` carrying the stated budget, which affords one reader
    of either block size (`BlockCache::affordable`). So the baseline each row is
    read against is a one-reader block path, and on the 128 MiB leg it already
    holds the pool's four slots.

    **Three reps**, as `peak-rss` takes: a peak is a maximum rather than a mean,
    so it is far steadier across reps than a wall clock, and the reps are here
    to catch an outlier rather than to resolve a small difference.
    """
    figure = "parallel-peak-rss"
    specs = _parallel_rss_specs()
    # Every leg's gate twin in the same interleave, beside its leg so the pair
    # is read under one machine state: this figure's resident against
    # `PARALLEL_MEMORY` is a gate reading (`GATE_LEG`).
    gate = [gating.gate_twin(spec) for spec in specs]
    paired = [leg for pair in zip(specs, gate) for leg in pair]
    to_sweep = [s for s in paired if not session.has(figure, s)]
    session.build_before_sweep(to_sweep)
    session.sweep(figure, to_sweep, session.cfg.reps(3))

    by_leg: dict[str, dict[int, list[float]]] = {}
    for spec in specs:
        jobs = int(spec.command.rpartition("-jobs-")[2])
        by_leg.setdefault(spec.input, {})[jobs] = session.get_rss(figure, spec)

    rows = []
    for jobs in shapes.PARALLEL_JOBS:
        cells = [str(jobs) + (" *(serial)*" if jobs == shapes.PARALLEL_BASELINE else "")]
        for leg, _ in PARALLEL_RSS_LEGS:
            values = by_leg[leg][jobs]
            delta = formatting.median(values) - formatting.median(by_leg[leg][shapes.PARALLEL_BASELINE])
            cells.append(
                formatting.fmt_mib_median_spread(values)
                + ("" if jobs == shapes.PARALLEL_BASELINE else f" · {formatting.fmt_rss_delta(delta)}")
            )
        rows.append(cells)
    table = formatting.md_table(["`--jobs`", *(label for _, label in PARALLEL_RSS_LEGS)], rows)

    sizes = []
    for leg, label in PARALLEL_RSS_LEGS:
        sizes.append(
            f"- {label}: `{leg}`, "
            f"{formatting._fmt_bytes(staging.file_size(session.cfg, session.input_path(leg, 'warm-parallel'), leg))}"
            " compressed"
        )
    notes = (
        "\n\nEach cell is peak resident set, and the change from that leg's own one-job row. "
        f"Every row states `--memory {memory_model.stated_allowance(shapes.PARALLEL_BUDGET)}`, the allowance "
        f"that leaves read buffers {formatting._fmt_bytes(shapes.PARALLEL_BUDGET)} under the reserve, and the "
        "margin's ceiling below that from two workers up, in a "
        f"{shapes.PARALLEL_MEMORY} container — an apparatus "
        "departure from the register's 512 MB, which is smaller than the budget under "
        "test. The one-job row states the same allowance — `--jobs 1` is "
        "`Parallelism::Serial` carrying it — so every row on both legs block-decodes, "
        "the one-job row with one reader.\n\n"
        + "\n".join(sizes)
        + "\n\nPer-rep readings (peak RSS):\n"
        + "\n".join(
            f"- {spec.label}: "
            + ", ".join(formatting.fmt_mib(v) for v in session.get_rss(figure, spec))
            for spec in (*specs, *(g for g in gate if session.has(figure, g)))
        )
        + "\n\n"
        + gate_table.gate_section(
            session,
            figure,
            [(spec.label, spec, shapes.PARALLEL_BUDGET + shapes.PARALLEL_HEADROOM) for spec in specs],
        )
        + "\n**Where a leg goes flat, it is the block pool's slot ceiling that stopped "
        "growing.** The block pool's slots are `clamp((budget - held) / unit, 1, "
        "max(POOL_DEPTH, jobs))`, where `held` is the chunk pool's own retention — "
        "`clamp(budget / chunk, 1, POOL_DEPTH) * chunk`, which is `POOL_DEPTH * chunk` "
        "only once the budget affords four chunk slots. It holds one unit below that beside the block "
        "each reader has in flight. Above four workers the depth term is the stated "
        "`--jobs`, so which term binds is set by the *file's* block size: on the fine "
        "leg the budget term is far above the axis, the depth term binds, and the curve "
        "is still climbing at the right-hand end; on the coarse leg the budget term "
        "binds and the leg levels off. That ceiling follows the `--jobs` announced "
        "rather than the readers the budget affords, so the coarse leg's flat value "
        "sits above the read-buffer budget the allowance leaves, though within the stated "
        "`--memory` — `KD21`, not a bound the library keeps.\n"
    )
    return table + notes
