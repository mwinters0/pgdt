"""`purge-delay` and `purge-delay-resident`: what each purge delay costs in wall clock and in
resident set."""

from __future__ import annotations

import dataclasses
from typing import Mapping, Sequence
from typing import TYPE_CHECKING

from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.builds import leg_binaries
from measurement.figures import long_line
from measurement.figures import parallel
from measurement.figures import prose
from measurement.figures import release_profile

if TYPE_CHECKING:
    from measurement.timing import Session


#: `purge-delay`'s rows: `release-profile`'s four shapes, at the apparatus's
#: count in the register's container, then `parse` and the provider's typed
#: scan at four workers, the baseline a setting is tuned against, and at
#: twenty-four, the guard (`roadmap.md`, "Four workers is the optimization
#: baseline; twenty-four is the guard"), in `parallel-scan-throughput`'s
#: container: a delay prices each free, and the parallel rows are where frees
#: cross threads. Each is (command, label, regime, container).
PURGE_DELAY_SHAPES: tuple[tuple[str, str, str, str | None], ...] = (
    *((command, label, "warm", None) for command, label in release_profile.RELEASE_PROFILE_SHAPES),
    *(
        row
        for jobs in (4, 24)
        for row in (
            (f"parse-jobs-{jobs}", f"`pgdt parse`, {jobs}j", "warm-parallel", shapes.PARALLEL_MEMORY),
            (
                f"{shapes.PARALLEL_SCAN}-jobs-{jobs}",
                f"`pgdt sql -c` — a typed DataFusion scan, {jobs}p",
                "warm-parallel",
                shapes.PARALLEL_MEMORY,
            ),
        )
    ),
)


def _purge_delay_spec(command: str, label: str, regime: str, memory: str | None, delay: int | None) -> runspec.RunSpec:
    """One `purge-delay` leg of one shape: the shipped binary, under the
    leg's environment."""
    return runspec.RunSpec(
        "dfcli" if command.startswith(shapes.PARALLEL_SCAN) else "pgdt",
        "control",
        command,
        regime,
        f"{label} ({leg_binaries.purge_delay_caption(delay)})",
        memory=memory,
        env=leg_binaries.purge_delay_env(delay),
    )


def _purge_delay_specs() -> list[runspec.RunSpec]:
    """Every leg of every `PURGE_DELAY_SHAPES` row, then the co-measured
    floor, which links no allocator and so is one row."""
    specs = [
        _purge_delay_spec(command, label, regime, memory, delay)
        for command, label, regime, memory in PURGE_DELAY_SHAPES
        for delay in leg_binaries.purge_delay_legs()
    ]
    specs.append(runspec.RunSpec("none", "control", "dd", "warm", "`dd` → `/dev/null` (warm)"))
    return specs


def purge_delays_asked(delays: Mapping[int | None, int | None]) -> str:
    """What each leg answered `binary_purge_delay`, as a figure's notes say
    it: the shipped leg's delay, every stating leg having answered the delay
    it states or been refused; a sitting that asked nothing says so."""
    shipped = delays.get(None)
    if shipped is None:
        return "this sitting asked none, so no column names the delay it ran under"
    return f"the shipped leg {shipped} ms, and each other leg the delay it states"


def purge_delay_cost(leg: Sequence[float], shipped: Sequence[float]) -> str:
    """What a leg's wall clock is against the shipped leg's on one row: the
    gap against the shipped median, and whether it is **resolved**
    (`resolved_above`) — slower, faster, or the two legs' readings meeting,
    which is within the figure's noise."""
    gap = formatting.median(leg) / formatting.median(shipped) - 1
    if formatting.resolved_above(leg, shipped):
        verdict = "**slower, resolved**"
    elif formatting.resolved_above(shipped, leg):
        verdict = "faster, resolved"
    else:
        verdict = "within the noise, the two legs' readings meeting"
    return f"{gap * 100:+.2f}%, {verdict}"


def run_purge_delay(session: Session) -> str:
    """What each purge delay costs in wall clock: one row per shape, one
    column per leg, each against the shipped leg stating none.

    **The figure's question is D166's** (`docs/design/decisions.md`):
    which delays cost nothing the throughput figures resolve. Its verdict
    lines say, per leg and row, whether the gap is resolved, which the choice
    reads; `purge-delay-resident` and the reference dump's hand-run query say
    what each delay keeps resident.

    **What would falsify "a lower delay is free"**, written before any
    reading: a leg's readings lying wholly above the shipped leg's on any
    row (`purge_delay_cost`). A purge is an `madvise` and a later fault for
    each page it returns, so the cost, where there is one, is system time at
    the rate pages empty, and the parallel rows, where many threads free at
    once, are where it shows first.

    **Every leg is the shipped binary**, differing by its environment alone,
    and each is asked what delay it runs under (`binary_purge_delay`) before
    the first reading, the answers recorded with the sitting
    (`Session.purge_delay`). **A sitting is read against itself**: every
    leg is one build, so only the gaps between them are this figure's."""
    figure = "purge-delay"
    legs = leg_binaries.purge_delay_legs()
    delays = {delay: session.purge_delay(figure, delay) for delay in legs}
    specs = _purge_delay_specs()
    session.sweep(figure, specs, session.cfg.reps(5))
    problems = parallel.parallel_answer_problems(
        {
            spec.label: session.reported.get(spec.key(figure), {})
            for spec in specs
            if spec.binary == "dfcli"
        }
    )
    if problems:
        raise RuntimeError(f"{figure} does not price one answer: " + "; ".join(problems))

    def walls(command: str, regime: str, memory: str | None, delay: int | None) -> list[float]:
        return session.get(figure, _purge_delay_spec(command, "", regime, memory, delay))

    rows = []
    verdicts: dict[int, list[str]] = {delay: [] for delay in leg_binaries.PURGE_DELAY_LEGS}
    for command, label, regime, memory in PURGE_DELAY_SHAPES:
        shipped = walls(command, regime, memory, None)
        cells = [label, formatting.fmt_median_spread(shipped)]
        for delay in leg_binaries.PURGE_DELAY_LEGS:
            values = walls(command, regime, memory, delay)
            cells.append(f"{formatting.fmt_median_spread(values)} — {formatting.median(values) / formatting.median(shipped):.2f}×")
            verdicts[delay].append(f"{label}: {purge_delay_cost(values, shipped)}")
        rows.append(cells)
    rows.append(
        [
            "`dd` → `/dev/null` — the co-measured floor",
            formatting.fmt_median_spread(session.get(figure, specs[-1])),
            *(["—"] * len(leg_binaries.PURGE_DELAY_LEGS)),
        ]
    )
    header = [
        f"`{leg_binaries.purge_delay_caption(delay)}`"
        + (" — the environment states none" if delay is None else f" — `{leg_binaries.purge_delay_env(delay)}`")
        for delay in legs
    ]
    table = formatting.md_table(["Warm, on tmpfs", *header], rows)
    asked = purge_delays_asked(delays)
    notes = (
        "Every leg is the shipped binary, differing by its environment alone, and each was "
        f"asked which purge delay it runs under before it was timed: "
        f"{asked}.\n\n"
        + prose._cached_query_note("Each `query` row")
        + "\nThe `pgdt sql` rows run `parallel-scan-throughput`'s query, the serial one at "
        f"`{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS}` stating no allowance in the register's container, "
        f"as `release-profile`'s does, and the four- and twenty-four-partition ones stating "
        f"that figure's allowance in its {shapes.PARALLEL_MEMORY} container, as the `parse` rows at "
        "four and twenty-four workers do. Every leg answered alike, byte for byte.\n\n"
        "Each leg against the shipped one, row by row:\n"
        + "".join(
            f"- `{leg_binaries.purge_delay_caption(delay)}`:\n" + "".join(f"  - {v}.\n" for v in lines)
            for delay, lines in verdicts.items()
        )
        + "\n"
        + prose._per_rep(figure, session, specs)
    )
    return table + "\n\n" + notes


def _purge_delay_resident_specs(allowance: int) -> list[runspec.RunSpec]:
    """`long-line-resident`'s timed legs but its anchors, each at every
    purge-delay leg, the legs of one shape beside each other so a sweep
    interleaves them."""
    return [
        dataclasses.replace(
            spec,
            label=f"{spec.label} ({leg_binaries.purge_delay_caption(delay)})",
            env=leg_binaries.purge_delay_env(delay),
        )
        for spec in long_line._long_line_specs(allowance)
        if spec.sweep is None
        for delay in leg_binaries.purge_delay_legs()
    ]


def purge_delay_allowance_line(delay: int | None, kib: Sequence[float], allowance: int) -> str:
    """Whether every rep of one leg held its peak within the `allowance`
    every leg states, `kib` its every cell's readings: the resident half of
    the purge delay's criterion (`docs/design/decisions.md`, "D166")."""
    caption = leg_binaries.purge_delay_caption(delay)
    if not kib:
        return f"- `{caption}`: no rep was read"
    past = [v for v in kib if v * 1024 > allowance]
    if past:
        return f"- `{caption}`: **{len(past)} reps past it**, the largest {formatting.fmt_mib(max(past))}"
    return f"- `{caption}`: every rep within it, the largest {formatting.fmt_mib(max(kib))}"


def run_purge_delay_resident(session: Session) -> str:
    """Peak resident set of each of `long-line-resident`'s legs at each
    purge delay: one row per leg, one column per delay, each against the
    shipped leg stating none.

    **What it reads is the allocator's retention over a held line's
    passes**: a delay below the shipped one decommits a freed page sooner,
    so the resident it leaves should only fall, and the fall is what the
    shipped delay keeps of what a pass frees (`docs/design/decisions.md`,
    "D165"). Every leg states `long-line-resident`'s allowance, sized
    from the factors its instrument legs read in this sitting
    (`long_line_allowance`), and whether each delay holds every rep within it
    is the resident half of D166's criterion; the reference dump's hand-run
    query is the other reading it names. **That half cannot tell the legs
    apart**: the allowance is the widest leg's, which the shipped delay
    already holds on every leg, so the choice turns on the query run and
    `purge-delay` (`measurements.md`, "What mimalloc's purge delay costs, and
    what it leaves resident"), and this figure reads what each delay leaves
    resident.

    **What would falsify the account**, written before any reading: a
    stating leg whose readings lie wholly above the shipped one's on any
    row (`resolved_above`), a delay then holding something the shipped one
    does not. Its first reading fires on fewer pairs than three-rep spreads
    part by chance, and the one pattern chance leaves, `jsonb`'s ordering
    filter at a zero delay, is unattributed; it moves no allowance, so the
    rule stays as written (`measurements.md`, "What mimalloc's purge delay
    costs, and what it leaves resident").

    **Three reps**, as `long-line-resident` takes, and its legs' checks:
    each answers its input's rows, each parse at its row's count."""
    figure = "purge-delay-resident"
    legs = leg_binaries.purge_delay_legs()
    delays = {delay: session.purge_delay(figure, delay) for delay in legs}
    factor_specs = long_line._long_factor_specs()
    to_read = [s for s in factor_specs if not session.has(figure, s)]
    session.build_before_sweep(to_read)
    session.sweep(figure, to_read, session.cfg.reps(1))
    allowance = shapes.long_line_allowance(long_line.long_line_factors(session, factor_specs, figure))

    specs = _purge_delay_resident_specs(allowance)
    to_sweep = [s for s in specs if not session.has(figure, s)]
    session.build_before_sweep(to_sweep)
    session.sweep(figure, to_sweep, session.cfg.reps(3))
    problems = long_line.long_line_leg_problems(session, figure, specs)
    if problems:
        raise RuntimeError(f"{figure} cannot read its legs: " + "; ".join(problems))

    by_leg: dict[int | None, list[float]] = {delay: [] for delay in legs}
    rows = []
    for base in (s for s in long_line._long_line_specs(allowance) if s.sweep is None):
        values = {
            delay: session.get_rss(figure, dataclasses.replace(base, env=leg_binaries.purge_delay_env(delay)))
            for delay in legs
        }
        shipped = values[None]
        cells = [base.label]
        for delay in legs:
            got = values[delay]
            by_leg[delay] += got
            if not got:
                cells.append("killed")
                continue
            cell = formatting.fmt_mib_median_spread(got)
            if delay is not None and shipped:
                cell += f" — {formatting.fmt_rss_delta(formatting.median(got) - formatting.median(shipped))}"
            cells.append(cell)
        rows.append(cells)
    header = [
        f"`{leg_binaries.purge_delay_caption(delay)}`"
        + (" — the environment states none" if delay is None else f" — `{leg_binaries.purge_delay_env(delay)}`")
        for delay in legs
    ]
    asked = purge_delays_asked(delays)
    notes = (
        "\n\nEach cell is a peak resident set, and each stating leg's its median less the "
        "shipped leg's. The legs are `long-line-resident`'s but its anchors beside the bound, "
        f"over its inputs, in a {shapes.LONG_LINE_MEMORY} container — its apparatus departure from the "
        "register's 512 MB, which one held line outgrows — each the shipped binary differing by "
        "its environment alone, and each was asked which purge delay it runs under before it "
        f"was read: {asked}. Every leg states "
        f"`--memory {allowance}`, sized as `long-line-resident` sizes it from the factors its "
        "instrument legs read in this sitting.\n\n"
        "Each leg against that allowance, over every rep of every row:\n"
        + "\n".join(purge_delay_allowance_line(d, by_leg[d], allowance) for d in legs)
        + "\n\nPer-rep readings (peak RSS):\n"
        + "\n".join(
            f"- {spec.label}: " + ", ".join(formatting.fmt_mib(v) for v in session.get_rss(figure, spec))
            for spec in specs
        )
        + "\n"
    )
    return formatting.md_table(["Leg", *header], rows) + notes
