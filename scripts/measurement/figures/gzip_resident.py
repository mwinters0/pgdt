"""`gzip-resident`: a gzip parse's peak resident set by worker count, each against the charge the
budget rule billed it."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import formatting
from measurement import gating
from measurement import gzip_charge
from measurement import inputs
from measurement import memory_model
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement.figures import gate_table

if TYPE_CHECKING:
    from measurement.timing import Session


#: The limit leg, the first pass's falsifier, by name.
GZIP_LIMIT_LEG = shapes.GZIP_RSS_LEGS[1][0]

#: How far past the speculation limit's ratio the limit leg's member must
#: pack, plaintext over compressed bytes, before a sitting reads it
#: (`gzip_limit_ratio_problem`): a quarter, fixed before the sitting, so a
#: chunk packing looser than the file's mean still inflates to the limit.
GZIP_LIMIT_ROOM = 1.25


def gzip_limit_ratio_problem(plaintext: int, packed: int) -> str | None:
    """Why a limit leg of `packed` compressed bytes over `plaintext` bytes
    fills no task's charge, or `None`: its mean ratio must exceed what a chunk
    of `GZIP_CHUNK` needs to inflate to `GZIP_SPECULATION_LIMIT` by
    `GZIP_LIMIT_ROOM`.

    `plaintext` is the length of the dump the member was generated from,
    which the member's own plaintext keeps to within one run of rows
    (`generate_repeated_rows.py`)."""
    need = gzip_charge.GZIP_SPECULATION_LIMIT / gzip_charge.GZIP_CHUNK * GZIP_LIMIT_ROOM
    if packed <= 0 or plaintext / packed < need:
        ratio = plaintext / packed if packed > 0 else 0.0
        return (
            f"the limit leg packs {ratio:.2f} : 1, and a chunk reaches the speculation limit "
            f"with room only past {need:.2f} : 1, so its first pass fills no task's charge: "
            "move `generate_repeated_rows.REPEAT`"
        )
    return None


def _gzip_rss_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec("pgdt", leg, f"{shapes.GZRSS_FAMILY}{jobs}", "warm-parallel", f"{label}, {jobs}j")
        for leg, label, _ in shapes.GZIP_RSS_LEGS
        for jobs in shapes.GZIP_RSS_JOBS
    ]


def run_gzip_resident(session: Session) -> str:
    """Peak resident set of `pgdt parse` over gzip at three worker counts,
    each cell's worst rep read against the charge the budget rule billed it.

    **What is under test is `MEMORY_UNPOOLED_BOUND`, read off `.xz` alone**
    (`docs/design/decisions.md`, "D3"). The bound
    is what `margin_allowance` predicts a count's resident with above its
    charge, so a gzip decoder or first pass holding more outside its charge
    than liblzma's does gets a correct charge and an optimistic prediction —
    a kill at a large limit rather than an error. Each cell is asked
    `charge_fault`'s three lines with its own bill, so the verdict is
    `reserve`'s, band for band (`charge_verdict`).

    **What would falsify it**, written before any reading: a cell whose
    unnamed remainder exceeds `MEMORY_UNPOOLED_BOUND` (the bound is wrong for
    gzip, and the sitting re-derives it) or `MEMORY_RESERVE` (the rule does
    not hold for gzip); or an over-bill — a cell holding less than it was
    charged, which on a first-pass leg names the speculation limit, most of
    a task's charge (`gzip_task_bytes`). The secant
    between the two upper counts states what each added worker held against
    what it was charged, which is that pricing; it is a secant over two
    counts, not a fit.

    **What would falsify a task's charge as a bound** is the limit leg's
    secant: each added worker holding more than `Chunking::task_bytes` on a
    member whose chunks reach the limit, an under-bill that admits workers a
    container then kills (D124). The secant table says, leg by
    leg, whether a worker held past its charge. The limit leg is refused
    before any leg is read unless its member packs past the limit's ratio
    (`gzip_limit_ratio_problem`), a leg filling no term being no falsifier.

    **The bill is mirrored and then held to the run's own**: each leg's
    `scan started` line states the budget its stated count spends, which at a
    count the allowance affords is the charge, so a mirror that has drifted
    from the library refuses the figure rather than reading a remainder off a
    stale bill. A first-pass leg must say it opened a first pass, and a BGZF
    leg must not — a BGZF file read by a first pass is a walk that did not run.
    A `--dry-run` writes no such line, so its stand-in reports the mirror's own
    bill (`stand_ins.gzip_resident`): it rehearses every division, the check
    against the library's bill excepted.

    **Three reps**, as `parallel-peak-rss` takes: a peak is a maximum, steady
    across reps, and the reps are there to catch an outlier.
    """
    figure = "gzip-resident"
    limit_leg = GZIP_LIMIT_LEG
    packed = session.input_size(limit_leg, "warm-parallel")
    plaintext = session.input_size(inputs.INPUTS[limit_leg].derives_from, "warm-parallel")
    ratio_problem = gzip_limit_ratio_problem(plaintext, packed)
    if ratio_problem:
        raise RuntimeError(f"{figure} cannot read its limit leg: {ratio_problem}")
    specs = _gzip_rss_specs()
    # Every leg's gate twin beside it, as `parallel-peak-rss` pairs them: this
    # figure's resident against `GZPASS_MEMORY` is a gate reading too.
    gate = [gating.gate_twin(spec) for spec in specs]
    paired = [leg for pair in zip(specs, gate) for leg in pair]
    to_sweep = [s for s in paired if not session.has(figure, s)]
    session.build_before_sweep(to_sweep)
    session.sweep(figure, to_sweep, session.cfg.reps(3))

    kinds = {leg: kind for leg, _, kind in shapes.GZIP_RSS_LEGS}
    labels = {leg: label for leg, label, _ in shapes.GZIP_RSS_LEGS}
    stored = {
        leg: staging.file_size(session.cfg, session.input_path(leg, "warm-parallel"), leg)
        for leg, _, _ in shapes.GZIP_RSS_LEGS
    }
    by_leg: dict[str, dict[int, list[float]]] = {}
    held_worst: dict[tuple[str, int], float] = {}
    bills: dict[tuple[str, int], tuple[int, int]] = {}
    ran: dict[tuple[str, int], int] = {}
    problems = []
    for spec in specs:
        jobs = int(spec.command.rpartition("-jobs-")[2])
        readings = session.get_rss(figure, spec)
        by_leg.setdefault(spec.input, {})[jobs] = readings
        held_worst[(spec.input, jobs)] = max(readings) * 1024
        billed, pool = gzip_charge.gzip_leg_charge(kinds[spec.input], jobs, stored[spec.input])
        bills[(spec.input, jobs)] = (billed, pool)
        report = session.reported.get(spec.key(figure), {})
        if report.get("resolved_jobs") != str(jobs):
            problems.append(
                f"{spec.label}: the run resolved {report.get('resolved_jobs', 'no')} worker(s) "
                f"where the row states {jobs}"
            )
        if report.get("resolved_budget") != str(billed):
            problems.append(
                f"{spec.label}: the run billed {report.get('resolved_budget', 'nothing')} byte(s) "
                f"where the mirror charges {billed}, so the mirror is not the library's charge"
            )
        workers = report.get("first_pass_workers")
        if kinds[spec.input] == "pass":
            if workers is None:
                problems.append(f"{spec.label}: the run opened no first pass")
            else:
                ran[(spec.input, jobs)] = int(workers)
        elif workers is not None:
            problems.append(f"{spec.label}: a BGZF file was read by a first pass, not walked")
    if problems:
        raise RuntimeError(f"{figure} cannot read its legs: " + "; ".join(problems))

    rows = []
    for jobs in shapes.GZIP_RSS_JOBS:
        cells = [str(jobs) + (" *(serial)*" if jobs == shapes.PARALLEL_BASELINE else "")]
        for leg, _, _ in shapes.GZIP_RSS_LEGS:
            values = by_leg[leg][jobs]
            delta = formatting.median(values) - formatting.median(by_leg[leg][shapes.GZIP_RSS_JOBS[0]])
            cells.append(
                formatting.fmt_mib_median_spread(values)
                + ("" if jobs == shapes.GZIP_RSS_JOBS[0] else f" · {formatting.fmt_rss_delta(delta)}")
            )
        rows.append(cells)
    table = formatting.md_table(["`--jobs`", *(label for _, label, _ in shapes.GZIP_RSS_LEGS)], rows)

    model_rows, model_faults, model_unnamed = [], [], []
    for leg, label, kind in shapes.GZIP_RSS_LEGS:
        for jobs in shapes.GZIP_RSS_JOBS:
            billed, pool = bills[(leg, jobs)]
            held = held_worst[(leg, jobs)]
            fault = memory_model.charge_fault(billed, pool, jobs, held)
            model_unnamed.append(held - billed)
            if fault:
                model_faults.append((fault, f"- **{label}** at `--jobs {jobs}`: {fault.text}"))
            workers = f"{jobs}" + (f" · {ran[(leg, jobs)]} ran" if kind == "pass" else "")
            model_rows.append(
                [
                    f"{label}, `--jobs {jobs}`",
                    workers,
                    formatting._fmt_budget_bytes(billed),
                    formatting._fmt_budget_bytes(pool) if pool else "—",
                    formatting.fmt_mib(held / 1024),
                    formatting._fmt_budget_bytes(held - billed),
                    "met" if fault is None else f"**{fault.band}**",
                ]
            )
    model_table = formatting.md_table(
        [
            "Leg",
            "Workers",
            "Billed",
            "of which pool list",
            "Worst rep held",
            "Unnamed",
            "Criterion",
        ],
        model_rows,
    )
    verdict = memory_model.charge_verdict(
        model_faults,
        model_unnamed,
        len(model_rows),
        nowhere="**The model was evaluated at no cell.**",
    )

    low, high = shapes.GZIP_RSS_JOBS[-2], shapes.GZIP_RSS_JOBS[-1]
    secants = []
    for leg, label, kind in shapes.GZIP_RSS_LEGS:
        added = high - low
        held_each = (held_worst[(leg, high)] - held_worst[(leg, low)]) / added
        billed_each = (bills[(leg, high)][0] - bills[(leg, low)][0]) / added
        past = held_each - billed_each
        secants.append(
            [
                label,
                formatting._fmt_budget_bytes(held_each),
                formatting._fmt_budget_bytes(billed_each),
                f"**past it by {formatting._fmt_budget_bytes(past)}**" if past > 0 else "within it",
            ]
        )
    secant_table = formatting.md_table(
        [
            "Leg",
            f"Held a worker, `--jobs {low}` → `{high}`",
            "Charged a worker",
            "Against the charge",
        ],
        secants,
    )

    sizes = "\n".join(
        f"- {label}: `{leg}`, {formatting._fmt_bytes(stored[leg])} compressed"
        + (
            f", {plaintext / packed:.2f} : 1 against the {formatting._fmt_bytes(plaintext)} it was "
            "generated from"
            if leg == limit_leg
            else ""
        )
        for leg, label, _ in shapes.GZIP_RSS_LEGS
    )
    return (
        table
        + "\n\nEach cell is peak resident set, and the change from that leg's own one-job row. "
        f"Every row states `--memory {memory_model.stated_allowance(shapes.GZPASS_BUDGET)}`, as "
        f"`gzip-first-pass`'s do, in a {shapes.GZPASS_MEMORY} container, and every count is one "
        "the allowance affords, which each run's own `scan started` line confirms. **A "
        "single member's `parse` is its first pass**: no cache precedes it, so it inflates "
        "the file while it builds the index, its scan one reader and its inflate run ahead "
        "on the workers the count affords at a task's charge. **The BGZF file's is a block "
        "path**: walked at open, it is read in whole units by every reader, as an `.xz` file "
        "is. The first and the BGZF leg are `control` at `pg_dump`'s level 6, through one "
        "zlib; **the limit leg** is `control`'s rows repeated in runs, deflated the same way, "
        "so it packs past the ratio at which a chunk inflates to the speculation limit and "
        "fills the plaintext terms of a task's charge (`generate_repeated_rows.py`).\n\n"
        + sizes
        + "\n\n**The charge against what was held**, cell by cell, read against "
        "`reserve`'s three lines: the unnamed remainder must be **non-negative** — a "
        "negative one is an over-bill — and is read against "
        f"**`MEMORY_UNPOOLED_BOUND`** ({formatting._fmt_budget_bytes(memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND)}), "
        "above which the **bound** is wrong for gzip, and "
        f"**`MEMORY_RESERVE`** ({formatting._fmt_budget_bytes(memory_model.LIBRARY_MEMORY_RESERVE)}), above which the "
        "**rule** is. The `Criterion` column names the band. **A first-pass cell is billed "
        "per worker with no pool**: each worker the larger of the pass's two decodes and "
        f"skip scratch ({formatting._fmt_budget_bytes(gzip_charge.gzip_pass_bytes())}) and a task's charge "
        f"({formatting._fmt_budget_bytes(gzip_charge.gzip_task_bytes())}, six speculation limits of it), and its "
        "`Workers` column says how many the pass ran — the frontier and the tasks ahead. "
        "**A BGZF cell is billed as an `.xz` one is**, a reader its "
        f"{formatting._fmt_budget_bytes(gzip_charge.bgzf_unit())} unit, the scan's chunk and gzip's pulling "
        f"footprint ({formatting._fmt_budget_bytes(gzip_charge.gzip_block_reader_bytes(gzip_charge.bgzf_unit()))} in all), and "
        "the readers together the pool's retention list. Each bill is the one the run's own "
        "`scan started` line states:\n\n"
        + model_table
        + "\n\n"
        + verdict
        + "\n\n**What each added worker held, against what it was charged**, the secant "
        f"between `--jobs {low}` and `--jobs {high}` over the worst reps: two counts, so a "
        "difference the axis measured and not a fit. On a first-pass leg it is what a task "
        "ahead holds against `Chunking::task_bytes`, which prices the speculation limit no "
        "leg varies, and on the limit leg a worker held past it is an under-bill; on the "
        "BGZF leg, a reader's charge and the unit its pool grows by above "
        "`POOL_DEPTH`:\n\n"
        + secant_table
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
            [(spec.label, spec, shapes.GZPASS_BUDGET + shapes.PARALLEL_HEADROOM) for spec in specs],
        )
    )
