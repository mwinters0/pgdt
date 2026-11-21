"""`release-profile`: the release profile's legs, by wall clock and by instruction count."""

from __future__ import annotations

from typing import Sequence
from typing import TYPE_CHECKING

import generate_perf_data as perf

from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.builds import leg_binaries
from measurement.figures import parallel
from measurement.figures import prose
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


# -- the release profile ----------------------------------------------------


#: The `release-profile` figure's shapes, in its table's row order:
#: `_ALLOCATOR_SHAPES`, which hold the per-row split and the per-field decode
#: a crate boundary stops the compiler inlining across, then **the typed
#: DataFusion query**, the workload ranked first ("Where a setting trades one
#: workload for another, the typed DataFusion query wins", `roadmap.md`),
#: which the downstream rule checks first through this figure ("A downstream
#: build is within 3% of ours"). That is `PARALLEL_SCAN` unsuffixed, the
#: scaling figure's query at the apparatus's count, so its row is the same
#: engine and decode over the same file as the `pgdt query` rows above it.
RELEASE_PROFILE_SHAPES: tuple[tuple[str, str], ...] = (
    *((command, label) for command, label, _ in register._ALLOCATOR_SHAPES),
    (shapes.PARALLEL_SCAN, "`pgdt sql -c` — a typed DataFusion scan"),
)

#: How many times `release-profile` runs each leg of each shape, and so how
#: many counts a shape takes on each leg of `--count-pair`, whose spread is
#: the one build's run-to-run spread this figure first showed.
RELEASE_PROFILE_REPS = 5


def _release_profile_specs() -> list[runspec.RunSpec]:
    """Every leg of every `RELEASE_PROFILE_SHAPES` shape, plus the co-measured floor.

    No reading is borrowed (`Figure.shares` is empty), since no other figure
    times these builds."""
    specs = [
        runspec.RunSpec(f"profile:{leg}", "control", command, "warm", f"{label} ({leg})")
        for command, label in RELEASE_PROFILE_SHAPES
        for leg, _ in leg_binaries.PROFILE_LEGS
    ]
    specs.append(runspec.RunSpec("none", "control", "dd", "warm", "`dd` → `/dev/null` (warm)"))
    return specs


def run_release_profile(session: Session) -> str:
    """What cross-crate inlining is worth to the shipped build: one row per
    shape, one column per profile, each leg's median against the first's.

    **A sitting is read against itself, never against another commit's.**
    Every leg is built from the tree being measured, so what one sitting
    answers is the gap between its legs; set beside a sitting on another
    tree, the gaps compare where the absolutes would price every commit
    between the two (`.claude/skills/evidence/SKILL.md`, rule 7)."""
    figure = "release-profile"
    specs = _release_profile_specs()
    # Every leg built before the first reading, for `run_allocator`'s reason.
    for leg, _ in leg_binaries.PROFILE_LEGS:
        leg_binaries.ensure_profile_binary(session.cfg, leg, session.log)
    session.sweep(figure, specs, session.cfg.reps(RELEASE_PROFILE_REPS))
    # The three builds are one source, so the query's answer is one answer.
    problems = parallel.parallel_answer_problems(
        {
            spec.label: session.reported.get(spec.key(figure), {})
            for spec in specs
            if spec.command == shapes.PARALLEL_SCAN
        }
    )
    if problems:
        raise RuntimeError(f"{figure} does not price one answer: " + "; ".join(problems))

    base = leg_binaries.PROFILE_LEGS[0][0]
    rows = []
    for command, label in RELEASE_PROFILE_SHAPES:
        reference = formatting.median(
            session.get(figure, runspec.RunSpec(f"profile:{base}", "control", command, "warm", ""))
        )
        cells = [label]
        for leg, _ in leg_binaries.PROFILE_LEGS:
            values = session.get(figure, runspec.RunSpec(f"profile:{leg}", "control", command, "warm", ""))
            cell = formatting.fmt_median_spread(values)
            if leg != base:
                cell += f" — {formatting.median(values) / reference:.2f}×"
            cells.append(cell)
        rows.append(cells)
    rows.append(
        [
            "`dd` → `/dev/null` — the co-measured floor",
            formatting.fmt_median_spread(session.get(figure, specs[-1])),
            *(["—"] * (len(leg_binaries.PROFILE_LEGS) - 1)),
        ]
    )
    header = [f"`{leg}` — {caption}" for leg, caption in leg_binaries.PROFILE_LEGS]
    table = formatting.md_table(["Warm, on tmpfs", *header], rows)
    counts = _release_profile_counts(session, figure)

    if session.cfg.dry_run:
        provenance = "Legs are not built or interrogated under `--dry-run`.\n"
    else:
        named = ", ".join(
            f"`{leg_binaries.binary_profile(leg_binaries.ensure_profile_binary(session.cfg, leg, session.log))}`"
            for leg, _ in leg_binaries.PROFILE_LEGS
        )
        provenance = (
            f"Every leg was asked which profile built it before it was timed — {named} — "
            f"and each links `{leg_binaries.ALLOCATOR_LEGS[0]}`, the shipped allocator, so the columns "
            "differ by the profile alone.\n"
        )
    sql = (
        f"The `pgdt sql` row runs `SELECT count({perf.COLUMNS[0][0]}), …, "
        f"count({perf.COLUMNS[-1][0]}) FROM {perf.TABLE} WHERE id IS NOT NULL` at "
        f"`{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS}`, stating no allowance — "
        "`parallel-scan-throughput`'s query at the apparatus's count — over a cache an "
        f"untimed `pgdt parse` stating `{shapes.GATHER_STATISTICS}` wrote beside the dump: every "
        "column decoded typed and one row out, the filter keeping every row, so its "
        "statistics skip no group. Its reading carries the program's startup and the dump's "
        "registration, which the `pgdt query` rows' do not, so it is read across its own "
        "row and never against theirs. Every leg answered alike, byte for byte.\n"
    )
    return (
        table
        + "\n\n"
        + _release_profile_count_table(counts)
        + provenance
        + "\n"
        + prose._cached_query_note("Each `query` row")
        + "\n"
        + sql
        + "\n"
        + _release_profile_verdicts(session, figure, counts)
        + "\n"
        + prose._per_rep(figure, session, specs)
        + _per_rep_counts(figure, specs, counts)
    )


def _release_profile_counts(
    session: Session, figure: str
) -> dict[tuple[str, str], list[int]] | None:
    """Every leg's `instructions:u` by (shape, leg), or `None` for a sitting
    taken before the figure counted. **All or none**: a sitting counting some
    legs and not others was taken under two apparatus, and is refused."""
    got = {
        (command, leg): session.get_instructions(
            figure, runspec.RunSpec(f"profile:{leg}", "control", command, "warm", "")
        )
        for command, _ in RELEASE_PROFILE_SHAPES
        for leg, _ in leg_binaries.PROFILE_LEGS
    }
    missing = [key for key, values in got.items() if not values]
    if len(missing) == len(got):
        return None
    if missing:
        raise RuntimeError(
            f"{figure} counted some legs and not {missing}: one sitting, two apparatus"
        )
    return {key: list(values or ()) for key, values in got.items()}


def _release_profile_count_table(counts: dict[tuple[str, str], list[int]] | None) -> str:
    """The legs' `instructions:u`, one row per shape, each against the default
    leg's: what a codegen change moves, which layout does not
    (`roadmap.md`, "A downstream build is within 3% of ours")."""
    if counts is None:
        return (
            "This sitting took no `instructions:u`: it was taken before the figure counted, "
            "so no gap in it is attributed.\n\n"
        )
    base = leg_binaries.PROFILE_LEGS[0][0]
    rows = []
    for command, label in RELEASE_PROFILE_SHAPES:
        reference = formatting.median(counts[(command, base)])
        cells = [label]
        for leg, _ in leg_binaries.PROFILE_LEGS:
            cell = formatting.fmt_instructions(counts[(command, leg)])
            if leg != base:
                cell += f" — {formatting.median(counts[(command, leg)]) / reference:.3f}×"
            cells.append(cell)
        rows.append(cells)
    header = [f"`{leg}`" for leg, _ in leg_binaries.PROFILE_LEGS]
    return (
        formatting.md_table(["`instructions:u`, the same runs", *header], rows)
        + "\n\nEach count is the timed command's own, every thread's, from its `exec` to its "
        "exit, taken by `count-instructions` in the run its wall clock is: user space only, "
        "never scaled.\n\n"
    )


def downstream_verdict(
    label: str,
    walls: tuple[Sequence[float], Sequence[float]],
    counts: tuple[Sequence[int], Sequence[int]] | None,
) -> str:
    """The downstream-build rule read on one workload: the default leg's
    `walls[0]` against the shipped leg's `walls[1]`, and where it fails, the
    same legs' `counts` (`roadmap.md`, "A downstream build is within 3% of
    ours").

    A gap is how far the default trails, against the default's median, and
    **resolved** where the two legs' readings do not meet — the default's
    fewest above the shipped leg's most. The gate fails on a resolved gap past
    `DOWNSTREAM_GAP`; a failure owes the remedy only where the count's gap is
    resolved too, no second threshold, and is layout where it is not."""
    default, shipped = walls
    gap = 1 - formatting.median(shipped) / formatting.median(default)
    resolved = formatting.resolved_above(default, shipped)
    fails = gap > leg_binaries.DOWNSTREAM_GAP and resolved
    met = "resolved" if resolved else "unresolved, the two legs' readings meeting"
    head = f"{'**fails**' if fails else 'passes'} — {gap * 100:+.2f}% in wall clock, {met}"
    if counts is None:
        tail = "; no `instructions:u` was taken" + (
            ", so the failure is unattributed" if fails else ""
        )
    else:
        cdefault, cshipped = counts
        cgap = 1 - formatting.median(cshipped) / formatting.median(cdefault)
        cresolved = formatting.resolved_above(cdefault, cshipped)
        tail = f"; `instructions:u` {cgap * 100:+.2f}%, {'resolved' if cresolved else 'unresolved'}"
        if fails:
            tail += (
                ": lost inlining, which owes the remedy"
                if cresolved
                else ": layout, which owes nothing"
            )
    return f"- {label}: {head}{tail}."


def _release_profile_verdicts(
    session: Session, figure: str, counts: dict[tuple[str, str], list[int]] | None
) -> str:
    """`downstream_verdict` for every shape, in the table's row order."""
    base = leg_binaries.PROFILE_LEGS[0][0]
    lines = []
    for command, label in RELEASE_PROFILE_SHAPES:
        walls = tuple(
            session.get(figure, runspec.RunSpec(f"profile:{leg}", "control", command, "warm", ""))
            for leg in (base, leg_binaries.SHIPPED_PROFILE_LEG)
        )
        pair = (
            None
            if counts is None
            else (counts[(command, base)], counts[(command, leg_binaries.SHIPPED_PROFILE_LEG)])
        )
        lines.append(downstream_verdict(label, walls, pair))
    return (
        f"The downstream-build rule, `{base}` against `{leg_binaries.SHIPPED_PROFILE_LEG}` "
        "(`roadmap.md`, \"A downstream build is within 3% of ours\"):\n"
        + "\n".join(lines)
        + "\n"
    )


def _per_rep_counts(
    figure: str, specs: Sequence[runspec.RunSpec], counts: dict[tuple[str, str], list[int]] | None
) -> str:
    """The per-rep counts, beside `_per_rep`'s seconds, in its rep order."""
    if counts is None:
        return ""
    lines = [
        f"- {spec.label}: "
        + ", ".join(f"{v / 1e9:.3f}" for v in counts[(spec.command, spec.binary.removeprefix("profile:"))])
        for spec in specs
        if spec.binary != "none"
    ]
    return "\nPer-rep `instructions:u` (G):\n" + "\n".join(lines) + "\n"
