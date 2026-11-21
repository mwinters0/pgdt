"""`per-block-quadratic`, `peak-rss` and `rss-attribution`: what a dump's block count costs in time
and in resident set, and whether a cost is paid per table or per block."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import borrowing
from measurement import formatting
from measurement import inputs
from measurement import runspec
from measurement import staging
from measurement.builds import leg_binaries
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


def run_per_block_quadratic(session: Session) -> str:
    """The series and its control, on the shipped binary alone.

    **There was a "before" column here, built at the commit preceding
    `SaveThrottle`, and it is retired rather than repaired.** A pinned
    historical build prices everything that differs between the two trees, and
    that set only grows: by the time anything ran the column the gap was 453
    commits, so it charged 453 commits of unrelated work to the throttle while
    the section conceded only that it was "a whole-commit comparison". What the
    throttle and its gate bought is a settled historical fact and is recorded as
    one beside the mechanism (`decisions.md`, "D63"), which costs a sentence rather than a build with no expiry
    condition on it."""
    figure = "per-block-quadratic"
    specs = [
        runspec.RunSpec("pgdt", name, "parse-cache-out", "warm", name) for name, _ in register._QUADRATIC_ROWS
    ]
    session.sweep(figure, specs, session.cfg.reps(2))

    counts = [input_block_count(name) for name, _ in register._QUADRATIC_ROWS]
    rows, per_rep, save_counts = [], [], []
    for name, label in register._QUADRATIC_ROWS:
        dump = session.input_path(name, "warm")
        readings = session.get(figure, runspec.RunSpec("pgdt", name, "parse-cache-out", "warm", ""))
        saves, cache_size = leg_binaries.count_saves(
            session.cfg, session.binary_path("pgdt"), dump, session.log
        )
        save_counts.append(saves)
        rows.append(
            [
                label,
                formatting._fmt_bytes(staging.file_size(session.cfg, dump, name)),
                formatting._fmt_bytes(cache_size),
                f"{formatting.fmt_s(formatting.median(readings))} s",
                str(saves),
            ]
        )
        per_rep.append(f"- {label}: {formatting.fmt_readings(readings)}")
    table = formatting.md_table(["blocks", "dump", "final cache", "parse", "saves"], rows)
    # Read off the column rather than asserted beside it: the sentence is the
    # throttle's whole visible signature here, and a claim the renderer states
    # without computing is one that goes false the sitting the shape changes.
    span = (
        f"stays at {save_counts[0]}"
        if len(set(save_counts)) == 1
        else f"runs {min(save_counts)}–{max(save_counts)}"
    )
    note = (
        "\n\nSave counts are `strace -f -e trace=open,openat` in a container, untimed. Across a "
        f"block count multiplying by {counts[-1] // counts[0]:,}× from the control, "
        f"the save count {span} — the throttle is a ratio against elapsed time, not a count of "
        "watermarks.\n"
    )
    return table + note + "\nPer-rep readings (s):\n" + "\n".join(per_rep) + "\n"


def input_block_count(name: str) -> int:
    """How many `COPY` blocks one registered input holds.

    Read off the generator and its arguments rather than written beside the
    row, so a block count that changes in the generator cannot leave a stale
    number in the table's own column."""
    spec = inputs.INPUTS[name]
    if spec.generator == "generate_perf_data.py":
        return 1
    if spec.generator == "generate_block_count_bench.py":
        return int(spec.args[spec.args.index("--blocks") + 1])
    raise ValueError(f"input {name!r} does not say how many blocks it holds")


def run_peak_rss(session: Session) -> str:
    figure = "peak-rss"
    specs = [
        runspec.RunSpec("pgdt", name, "parse-rss", "warm", f"peak RSS {name}") for name in register._RSS_ROWS
    ]
    session.sweep(figure, specs, session.cfg.reps(3))

    by_name = dict(zip(register._RSS_ROWS, specs))
    rows, per_rep = [], []
    for name, spec in zip(register._RSS_ROWS, specs):
        readings = session.get_rss(figure, spec)
        size = staging.file_size(session.cfg, session.input_path(name, "warm"), name)
        rows.append(
            [
                f"`{name}`",
                formatting._fmt_bytes(size),
                f"{input_block_count(name):,}",
                formatting.fmt_mib_median_spread(readings),
            ]
        )
        per_rep.append(f"- `{name}`: " + ", ".join(formatting.fmt_mib(v) for v in readings))
    table = formatting.md_table(["Input", "Bytes", "`COPY` blocks", "Peak RSS"], rows)

    pivot = formatting.median(session.get_rss(figure, by_name[register._RSS_PIVOT]))
    pivot_size = staging.file_size(
        session.cfg, session.input_path(register._RSS_PIVOT, "warm"), register._RSS_PIVOT
    )
    byte_size = staging.file_size(session.cfg, session.input_path("control", "warm"), "control")
    per_byte = formatting.median(session.get_rss(figure, by_name["control"])) - pivot
    steps = []
    for name in register._RSS_ROWS:
        count = input_block_count(name)
        if count == input_block_count(register._RSS_PIVOT):
            continue
        delta = formatting.median(session.get_rss(figure, by_name[name])) - pivot
        extra = count - input_block_count(register._RSS_PIVOT)
        steps.append(
            f"{count:,} blocks cost {formatting.fmt_rss_delta(delta)} "
            f"({delta * 1024 / extra:+,.0f} bytes a block)"
        )
    note = (
        f"\n\nEvery subtraction is against `{register._RSS_PIVOT}`, the 1-block "
        f"{formatting._fmt_bytes(pivot_size)} pivot. **Per byte:** "
        f"{byte_size / pivot_size:.0f}× the bytes costs **{formatting.fmt_rss_delta(per_byte)}**. "
        f"**Per block**, at byte counts within an order of magnitude of the pivot's: "
        + ", and ".join(steps)
        + ".\n"
    )
    return table + note + "\nPer-rep readings:\n" + "\n".join(per_rep) + "\n"


def run_rss_attribution(session: Session) -> str:
    """What the per-block resident growth `peak-rss` measures is made of.

    `peak-rss` measures the whole; this attributes it, by holding the block
    count as the only axis and varying one mechanism at a time. Every reading
    is a resident set, and the published quantity is the **slope** -- bytes a
    block -- so an allocator's baseline cannot masquerade as growth.
    """
    figure = "rss-attribution"
    legs = register._attribution_specs()
    # Before the first reading, as the `allocator` figure builds its own: a leg
    # discovered missing at rep two has already spent the session's first rep
    # under a different machine state.
    for _, binary, _ in register._ATTRIBUTION_LEGS:
        if binary.startswith("alloc:"):
            leg_binaries.ensure_allocator_binary(session.cfg, binary.removeprefix("alloc:"), session.log)
    # The `parse` reference row, from `peak-rss` where this sitting took it.
    # Borrowed rather than re-measured so the doc carries one number per
    # measurement, and *dropped from the interleave* rather than left in it: a
    # spec the sitting already holds would spend a reading to overwrite one.
    note = borrowing.share_readings(session, figure)
    specs = [spec for _, small, big in legs for spec in (small, big)]
    session.sweep(
        figure, [s for s in specs if not session.has(figure, s)], session.cfg.reps(3)
    )

    small_n, big_n = (input_block_count(name) for name in register._ATTRIBUTION_INPUTS)
    rows, per_rep = [], []
    for label, small_spec, big_spec in legs:
        small_reps = session.get_rss(figure, small_spec)
        big_reps = session.get_rss(figure, big_spec)
        small, big = formatting.median(small_reps), formatting.median(big_reps)
        rows.append(
            [
                label,
                formatting.fmt_mib(small),
                formatting.fmt_mib(big),
                f"{(big - small) * 1024 / (big_n - small_n):+,.0f} B",
            ]
        )
        per_rep.append(
            f"- {label}: "
            + " · ".join(
                ", ".join(f"{v / 1024:.2f}" for v in reps) for reps in (small_reps, big_reps)
            )
        )
    table = formatting.md_table(
        ["Leg", f"{small_n:,} blocks", f"{big_n:,} blocks", "Per block"], rows
    )
    return (
        table
        + (f"\n{note}" if note else "")
        + f"\n\nPer-rep readings (MiB, {small_n:,} then {big_n:,}):\n"
        + "\n".join(per_rep)
        + "\n"
    )
