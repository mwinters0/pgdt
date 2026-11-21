"""`gzip-first-pass`: one gzip member's first pass by writer and worker count, against the plain
scan's floor."""

from __future__ import annotations

from pathlib import Path
from typing import Mapping, Sequence
from typing import TYPE_CHECKING

import release

from measurement import formatting
from measurement import gzip_charge
from measurement import memory_model
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement.builds import leg_binaries
from measurement.figures import prose
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


#: The count the chunk-length leg is read at beside the shipped length: the
#: default, the physical count (`docs/design/decisions.md`, "D2"), since the
#: leg prices a constant every default pass runs (D121).
GZPASS_CHUNK_JOBS = shapes.GZPASS_PHYSICAL

#: The writers the chunk leg must beat the shipped length on to replace it,
#: and on how many of them (`gzpass_chunk_verdict`): the level-6 files, two of
#: the three, and beaten on none of `GZPASS_WRITERS` (D121's `Reopens`).
GZPASS_CHUNK_WRITERS: tuple[str, ...] = ("control_gz", "control_gz_ng", "control_gz_gnu")

GZPASS_CHUNK_WINS = 2

#: How many reps each leg takes, alternated within each and paired by rep:
#: the count of the hand probe whose pairs this figure's rule was fixed on
#: (`runs/p15.17.4/gate/`).
GZPASS_REPS = 7

#: How many of its pairs a leg may lose and still **beat** the other
#: (`gzpass_beats`), fixed before the sitting: at seven reps, six wins of seven.
#: Two legs neither of which beats the other are **level**. A gate on this line
#: reads alternated reps by their paired differences, never by whether two
#: ranges overlap, which a serial leg spread by placement decides.
GZPASS_SPARED = 1

#: How much faster a leg must be to **beat** another, besides its pairs: its
#: median paired difference over this fraction of its own median. Fixed before
#: the sitting at about half the gap between twelve and twenty-four jobs that
#: the pairs in `runs/p15.17.4/gate/` found, so a sub-percent gap neither moves
#: the default nor reads as a difference D2 could reject on.
GZPASS_MARGIN = 0.03


def gzpass_topology(root: Path = release.CPU_SYSFS) -> tuple[int, int]:
    """This machine's online hardware threads and physical cores, read off
    each CPU's `thread_siblings_list`."""
    cores = {
        release.parse_cpu_list(path.read_text())
        for path in root.glob("cpu[0-9]*/topology/thread_siblings_list")
    }
    return len(frozenset().union(*cores)), len(cores)


def gzpass_topology_problem(topology: tuple[int, int]) -> str | None:
    """Why `gzip-first-pass` cannot be taken on a machine of `topology`, or
    `None`: its rows are this machine's logical and physical counts, and a
    machine with others would read counts that mean something else there."""
    if topology != (shapes.GZPASS_LOGICAL, shapes.GZPASS_PHYSICAL):
        logical, physical = topology
        return (
            f"its rows are {shapes.GZPASS_LOGICAL} hardware threads on {shapes.GZPASS_PHYSICAL} cores, and "
            f"this machine has {logical} on {physical}: move `GZPASS_LOGICAL` and "
            "`GZPASS_PHYSICAL` to take it here"
        )
    return None


def gzpass_wins(a: Sequence[float], b: Sequence[float]) -> int:
    """In how many reps `a` was faster than `b`, rep by rep: the two legs'
    readings in one sweep's rep order."""
    if len(a) != len(b) or not a:
        raise ValueError(f"{len(a)} and {len(b)} readings do not pair")
    return sum(x < y for x, y in zip(a, b))


def gzpass_beats(a: Sequence[float], b: Sequence[float]) -> bool:
    """Whether `a` beats `b`: faster in all its pairs but `GZPASS_SPARED`, and
    in more than half of them, by a median paired difference over
    `GZPASS_MARGIN` of `a`'s median."""
    n = len(a)
    if gzpass_wins(a, b) < max(n - GZPASS_SPARED, n // 2 + 1):
        return False
    return formatting.median([y - x for x, y in zip(a, b)]) > GZPASS_MARGIN * formatting.median(a)


def gzpass_level(a: Sequence[float], b: Sequence[float]) -> bool:
    """Whether neither of `a` and `b` beats the other."""
    return not gzpass_beats(a, b) and not gzpass_beats(b, a)

#: What each outcome of one file's counts says, read against the shipped
#: default, the physical count (`docs/design/decisions.md`, "D2"): only
#: `reopens` moves it, an upper count beating the physical count on a file
#: either lifts (D2's `Reopens`); the cap holds on every other, and the three
#: below it still name the cost an upper count adds, by the falsifiers the
#: figure's first sitting fixed (D2's `Rejected`). **Every order of the
#: four counts falls in one.**
GZPASS_OUTCOMES: dict[str, str] = {
    "reopens": "an upper count beats the physical count on a file it or the physical count "
    "lifts: D2's gzip clause reopens",
    "shared": "the physical count beats the logical count and the logical count less one: the "
    "cost an upper count adds is a core or a cache shared, which the cap removes",
    "oversubscription": "the physical count beats the logical count and is level with the "
    "logical count less one: the cost is oversubscription, which the cap removes too",
    "level": "the physical count is level with the logical count and no upper count beats it: "
    "the cap holds, and the file names no cost",
    "no lift": "the physical count does not beat the serial pass, nor the serial pass any "
    "count, and no upper count beats the physical count: the default lifts nothing here, so "
    "the file says nothing of it, whatever an upper count lifts",
    "regresses": "the physical count does not beat the serial pass and the serial pass beats "
    "a count: a defect in speculation that finds nothing, owed a `KD<k>` by the round "
    "that reads it, which does not block the default",
}

#: The outcomes that say nothing of the default (`gzpass_verdict`).
GZPASS_UNLIFTED = frozenset({"no lift", "regresses"})


def gzpass_outcome(
    serial: Sequence[float],
    physical: Sequence[float],
    less_one: Sequence[float],
    logical: Sequence[float],
) -> str:
    """Which of `GZPASS_OUTCOMES` one file's readings at the serial, physical,
    logical-less-one and logical counts fall in, by `gzpass_beats`. An upper
    count beating the physical count reopens wherever either lifts the file —
    the physical count beating serial, or that count beating serial itself —
    and is read before anything else, the challenger's burden being the only
    one the figure carries. Reading the lift at the physical count alone would
    let the cap's own failure to lift a file excuse it; reading none would
    reopen on a file no count lifts, a regression of speculation rather than a
    cost of the cap. Every other outcome reads the lift at the physical count,
    each naming what an upper count costs a default that lifts."""
    lifts = gzpass_beats(physical, serial)
    if any(
        gzpass_beats(upper, physical) and (lifts or gzpass_beats(upper, serial))
        for upper in (less_one, logical)
    ):
        return "reopens"
    if not lifts:
        if any(gzpass_beats(serial, upper) for upper in (physical, less_one, logical)):
            return "regresses"
        return "no lift"
    if not gzpass_beats(physical, logical):
        return "level"
    if gzpass_beats(physical, less_one):
        return "shared"
    return "oversubscription"


def gzpass_verdict(outcomes: Mapping[str, str]) -> str:
    """What the files' outcomes settle together, keyed by each file's label,
    read against the shipped physical cap: it reopens where any file reopens
    it, holds where the physical count lifts some file and none reopens, each
    such file's outcome listed, and settles nothing where it lifts none; each
    file the pass regresses on named whatever the rest settle."""
    lifted = {label: o for label, o in outcomes.items() if o not in GZPASS_UNLIFTED}
    reopened = [label for label, o in lifted.items() if o == "reopens"]
    regressed = [label for label, o in outcomes.items() if o == "regresses"]
    named = (
        f" **The pass regresses on {', '.join(regressed)}**: {GZPASS_OUTCOMES['regresses']}."
        if regressed
        else ""
    )
    if not lifted:
        return "**The figure settles nothing**: the physical count lifts no file." + named
    if reopened:
        return (
            "**The figure reopens D2's gzip clause**: an upper count beats the physical count "
            f"on {', '.join(reopened)}, each a file it or the physical count lifts." + named
        )
    each = "; ".join(f"{label}: {o}" for label, o in lifted.items())
    return (
        "**The physical cap holds**: no upper count beats the physical count on a file either "
        f"lifts, and the files the physical count lifts read {each}." + named
    )


def gzpass_chunk_verdict(marks: Mapping[str, str], labels: Mapping[str, str]) -> str:
    """What the chunk leg settles, from each writer's mark against the shipped
    length at `GZPASS_CHUNK_JOBS` — `beats`, `beaten` or `level`, keyed by
    the writer's input — with `labels` naming each: the leg's chunk replaces
    the shipped one where it beats it on at least `GZPASS_CHUNK_WINS` of
    `GZPASS_CHUNK_WRITERS` and is beaten on none, and the shipped one stands
    otherwise."""
    leg, shipped = f"{leg_binaries.GZPASS_CHUNK_BYTES >> 20} MiB", f"{gzip_charge.GZIP_CHUNK >> 20} MiB"
    wins = [w for w in GZPASS_CHUNK_WRITERS if marks[w] == "beats"]
    beaten = [w for w in marks if marks[w] == "beaten"]
    tally = (
        f"the {leg} chunk beats the shipped {shipped} on {len(wins)} of the "
        f"{len(GZPASS_CHUNK_WRITERS)} level-6 writers' files"
        + (f" ({', '.join(labels[w] for w in wins)})" if wins else "")
        + (
            f" and is beaten on {', '.join(labels[w] for w in beaten)}"
            if beaten
            else " and is beaten on none"
        )
    )
    if len(wins) >= GZPASS_CHUNK_WINS and not beaten:
        return f"**The chunk leg replaces the shipped chunk**: {tally}."
    return (
        f"**The shipped chunk stands**: {tally}, where replacing it asks "
        f"{GZPASS_CHUNK_WINS} and none."
    )


def _gzpass_specs() -> list[runspec.RunSpec]:
    """Every leg, a sweep a writer: the plain `parse` the pass falls toward,
    read in each sweep, then the writer's file at every count and at the
    chunk-length leg's."""
    figure = "gzip-first-pass"
    specs = []
    for gi, (leg, label, _) in enumerate(register.GZPASS_WRITERS):
        floor = runspec.RunSpec(
            "pgdt", "control", f"{shapes.GZPASS_FAMILY}1", "warm-parallel",
            f"plain `parse`, the scan's floor, beside {label}",
        )
        specs.append(staging.in_sweep(figure, floor, gi))
        specs += [
            runspec.RunSpec("pgdt", leg, f"{shapes.GZPASS_FAMILY}{jobs}", "warm-parallel", f"{label}, {jobs}j")
            for jobs in shapes.GZPASS_JOBS
        ]
        specs.append(
            runspec.RunSpec(
                leg_binaries.GZPASS_CHUNK_BINARY, leg, f"{shapes.GZPASS_FAMILY}{GZPASS_CHUNK_JOBS}", "warm-parallel",
                f"{label}, {leg_binaries.GZPASS_CHUNK_BYTES >> 20} MiB chunks, {GZPASS_CHUNK_JOBS}j",
            )
        )
    return specs


def run_gzip_first_pass(session: Session) -> str:
    """Wall clock of `pgdt parse` over one gzip member — its first pass —
    by four writers, at the serial count, four, the physical count, the
    logical count less one and the logical count, and at 1 MiB chunks at the
    physical count; beside each writer's sweep, the plain `parse` of
    `control`, the scan's floor a first pass falls toward.

    **What it reads is whether the gzip first pass's default worker count
    holds** (`docs/design/decisions.md`, "D2"): the physical count, the
    incumbent, which only an upper count beating it on a file either lifts
    reopens. Each file's four counts fall in one of `GZPASS_OUTCOMES`, by
    `gzpass_beats` over paired reps, and one file reopening reopens the
    default (`gzpass_verdict`). Where none does, a file's outcome still names
    the cost an upper count adds, by falsifiers written before any reading:
    once the physical count beats the logical one, oversubscription is
    falsified by the physical count beating the logical count less one too,
    and a shared core or cache by its not beating it. A file the serial pass
    beats an upper count on is named as a regression and settles nothing
    (`GZPASS_UNLIFTED`).

    **It prices the chunk length too**: 1 MiB, the input chunk, against the
    shipped 4 MiB, rapidgzip's, at the physical count, the default, the
    speculation limit held, by the same pairs — a separate build, whose layout is a second
    difference no pair removes — and says which chunk ships
    (`gzpass_chunk_verdict`), by a rule written before any reading.

    **Every leg is checked against its own run**: the count each `scan
    started` line resolved must be the row's, a gzip leg must say its first
    pass ran that many workers, and the floor must open none; the shipped
    binary must cut the shipped chunk and the leg its own
    (`ensure_chunk_binary`); and the machine must be the one the rows are
    named for (`gzpass_topology_problem`).
    """
    figure = "gzip-first-pass"
    specs = _gzpass_specs()
    to_sweep = [s for s in specs if not session.has(figure, s)]
    if to_sweep and not session.cfg.dry_run:
        if problem := gzpass_topology_problem(gzpass_topology()):
            raise RuntimeError(f"{figure} cannot be taken here: {problem}")
        if (shipped := leg_binaries.binary_chunk(session.cfg.bin_pgdt)) != gzip_charge.GZIP_CHUNK:
            raise RuntimeError(
                f"{figure}: the shipped binary cuts {shipped}-byte chunks, not the shipped "
                f"{gzip_charge.GZIP_CHUNK}: it is a leg's build, and every other row would read it"
            )
    session.build_before_sweep(to_sweep)
    session.sweep(figure, to_sweep, session.cfg.reps(GZPASS_REPS))

    problems = []
    for spec in specs:
        report = session.reported.get(spec.key(figure), {})
        jobs = spec.command.removeprefix(shapes.GZPASS_FAMILY)
        if report.get("resolved_jobs") != jobs:
            problems.append(
                f"{spec.label}: the run resolved {report.get('resolved_jobs', 'no')} worker(s) "
                f"where the row states {jobs}"
            )
        workers = report.get("first_pass_workers")
        if spec.input == "control":
            if workers is not None:
                problems.append(f"{spec.label}: a plain dump opened a first pass")
        elif workers != jobs:
            problems.append(
                f"{spec.label}: the first pass ran {workers or 'no'} worker(s) where the row "
                f"states {jobs}"
            )
    if len({len(session.get(figure, s)) for s in specs}) != 1:
        problems.append("the legs hold different numbers of reps, so they do not pair")
    if problems:
        raise RuntimeError(f"{figure} cannot read its legs: " + "; ".join(problems))

    def at(leg: str, jobs: int, binary: str = "pgdt") -> list[float]:
        return session.get(figure, runspec.RunSpec(binary, leg, f"{shapes.GZPASS_FAMILY}{jobs}", "warm-parallel", ""))

    plain = session.input_size("control", "warm-parallel")
    floor = runspec.RunSpec("pgdt", "control", f"{shapes.GZPASS_FAMILY}1", "warm-parallel", "")
    floors = {
        leg: session.get(figure, staging.in_sweep(figure, floor, gi))
        for gi, (leg, _, _) in enumerate(register.GZPASS_WRITERS)
    }
    labels = [label for _, label, _ in register.GZPASS_WRITERS]
    rows = [
        ["the scan's floor: plain `parse`, `--jobs 1`"]
        + [
            f"{formatting.fmt_median_spread(floors[leg])} · {formatting.fmt_rate(plain, formatting.median(floors[leg]))}"
            for leg, _, _ in register.GZPASS_WRITERS
        ]
    ]
    for jobs in shapes.GZPASS_JOBS:
        name = {
            1: " *(serial)*",
            shapes.GZPASS_PHYSICAL: " *(physical)*",
            shapes.GZPASS_LOGICAL - 1: " *(logical less one)*",
            shapes.GZPASS_LOGICAL: " *(logical)*",
        }.get(jobs, "")
        cells = [f"`{jobs}`{name}"]
        for leg, _, _ in register.GZPASS_WRITERS:
            values, serial = at(leg, jobs), at(leg, 1)
            cell = f"{formatting.fmt_median_spread(values)} · {formatting.fmt_rate(plain, formatting.median(values))}"
            if jobs != 1:
                cell += f" · {formatting.median(serial) / formatting.median(values):.2f}× serial"
            cells.append(cell)
        rows.append(cells)
    rows.append(
        [f"`{GZPASS_CHUNK_JOBS}`, {leg_binaries.GZPASS_CHUNK_BYTES >> 20} MiB chunks"]
        + [
            f"{formatting.fmt_median_spread(v)} · {formatting.fmt_rate(plain, formatting.median(v))} · "
            f"{formatting.median(at(leg, 1)) / formatting.median(v):.2f}× serial"
            for leg, _, _ in register.GZPASS_WRITERS
            for v in (at(leg, GZPASS_CHUNK_JOBS, leg_binaries.GZPASS_CHUNK_BINARY),)
        ]
    )
    table = formatting.md_table(["`--jobs`", *labels], rows)

    def mark(a: Sequence[float], b: Sequence[float]) -> str:
        return "beats" if gzpass_beats(a, b) else "beaten" if gzpass_beats(b, a) else "level"

    def pair(a: Sequence[float], b: Sequence[float]) -> str:
        diff = formatting.median([x - y for x, y in zip(a, b)])
        shown = "**beats**" if (m := mark(a, b)) == "beats" else m
        share = diff / min(formatting.median(a), formatting.median(b))
        return f"{shown} · faster in {gzpass_wins(a, b)}/{len(a)} · {diff:+.3f} s, {share:+.1%}"

    p, l1, lg = shapes.GZPASS_PHYSICAL, shapes.GZPASS_LOGICAL - 1, shapes.GZPASS_LOGICAL
    outcomes: dict[str, str] = {}
    chunk_marks: dict[str, str] = {}
    pair_rows = []
    for leg, label, _ in register.GZPASS_WRITERS:
        outcome = gzpass_outcome(at(leg, 1), at(leg, p), at(leg, l1), at(leg, lg))
        outcomes[label] = outcome
        chunk_marks[leg] = mark(
            at(leg, GZPASS_CHUNK_JOBS, leg_binaries.GZPASS_CHUNK_BINARY), at(leg, GZPASS_CHUNK_JOBS)
        )
        pair_rows.append(
            [
                label,
                f"{formatting.median(at(leg, p)) / formatting.median(floors[leg]):.2f}×",
                pair(at(leg, p), at(leg, 1)),
                pair(at(leg, lg), at(leg, p)),
                pair(at(leg, l1), at(leg, p)),
                pair(at(leg, l1), at(leg, lg)),
                pair(at(leg, GZPASS_CHUNK_JOBS, leg_binaries.GZPASS_CHUNK_BINARY), at(leg, GZPASS_CHUNK_JOBS)),
                f"**{outcome}**",
            ]
        )
    pair_table = formatting.md_table(
        [
            "Writer",
            f"`{p}` over the floor",
            f"`{p}` against serial",
            f"`{lg}` against `{p}`",
            f"`{l1}` against `{p}`",
            f"`{l1}` against `{lg}`",
            f"{leg_binaries.GZPASS_CHUNK_BYTES >> 20} MiB against {gzip_charge.GZIP_CHUNK >> 20} MiB chunks, "
            f"`{GZPASS_CHUNK_JOBS}`",
            "Outcome",
        ],
        pair_rows,
    )

    files = "\n".join(
        f"- {label}: `{leg}`, {formatting._fmt_bytes(size)} compressed, "
        f"{plain / size:.2f} : 1 — {what}"
        for leg, label, what in register.GZPASS_WRITERS
        for size in (session.input_size(leg, "warm-parallel"),)
    )
    n = len(at(register.GZPASS_WRITERS[0][0], 1))
    spared = max(n - GZPASS_SPARED, n // 2 + 1)
    return (
        table
        + "\n\nEach cell is wall clock, the rate it implies per byte of `control`'s plaintext, "
        "and a count's rate as a multiple of the same file's serial pass. Every leg is "
        f"`pgdt parse` stating `--jobs` and `--memory {memory_model.stated_allowance(shapes.GZPASS_BUDGET)}` in a "
        f"{shapes.GZPASS_MEMORY} container, the allowance affording every row its count at a task's "
        "charge, which each run's `scan started` line confirms; a gzip leg's is its first "
        "pass, no cache preceding it, and its started line names the workers it ran. `--jobs` "
        "counts the frontier and not the scan's thread. Each writer's file is its own sweep, "
        "with the plain `parse` of `control` read in it, the floor a first pass falls toward. "
        f"The {leg_binaries.GZPASS_CHUNK_BYTES >> 20} MiB row is a build of its own, the shipped build but "
        "for the chunk (`pgdt --version` names it), so its layout differs too.\n\n"
        + files
        + "\n\n**Paired by rep.** Every leg of a sweep runs in each rep, the order reversed "
        "in the second half, so a count is read against another rep by rep. The first column "
        f"is the `{p}`-job pass's median over the floor's in its sweep, how far the pass is "
        "from the scan it falls toward. Each other cell says "
        f"whether its first leg beats the second — faster in at least {spared} of {n} pairs, "
        f"by a median paired difference over {GZPASS_MARGIN:.0%} of its own median — is "
        "beaten, or is level, in how many pairs it was faster, and the median paired "
        "difference in seconds and as a share of the faster leg's median, negative where the "
        "first is faster. A file's outcome is read off its serial, physical, logical-less-one "
        "and logical counts (`measure.gzpass_outcome`):\n\n"
        + pair_table
        + "\n\n"
        + gzpass_verdict(outcomes)
        + " "
        + gzpass_chunk_verdict(chunk_marks, {leg: label for leg, label, _ in register.GZPASS_WRITERS})
        + "\n\n"
        + prose._per_rep(figure, session, specs)
    )
