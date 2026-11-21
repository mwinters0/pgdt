"""`gzseek-backend` and `gzseek-spacing`: the seekable-gzip engines, and the access-point spacing."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import config
from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.builds import image
from measurement.figures import prose

if TYPE_CHECKING:
    from measurement.timing import Session


#: The engine that figure reads every spacing on: the crate's default, the
#: one that ships (`docs/design/decisions.md`, "D112").
GZSPACING_ENGINE = shapes.GZSEEK_ENGINES[0]

# -- pgdt-gzip-seek's engine ---------------------------------------------------------

#: The `gzseek-backend` figure's reference rows: the serial inflate of the same
#: member by the `inflate-throughput` instrument, on the two zlib-API engines
#: it carries — zlib-ng's the floor `pgdt-gzip-seek`'s first pass is held to
#: (`docs/design/decisions.md`, "D127").
GZSEEK_REFERENCES: tuple[str, ...] = ("zlib-ng", "zlib-rs")


def _gzseek_specs() -> list[runspec.RunSpec]:
    references = [
        runspec.RunSpec("gzinflate", "control_gz", f"{shapes.INFLATE_FAMILY}{b}", "warm", f"Serial inflate, `{b}`")
        for b in GZSEEK_REFERENCES
    ]
    labels = {"forward": "First pass", "units": "First pass, then every unit"}
    legs = [
        runspec.RunSpec(
            f"gzseek:{engine}",
            "control_gz",
            f"{shapes.GZSEEK_FAMILY}{stage}-{engine}",
            "warm",
            f"{labels[stage]}, `{engine}`",
        )
        for engine in shapes.GZSEEK_ENGINES
        for stage in shapes.GZSEEK_STAGES
    ]
    return references + legs


def run_gzseek_backend(session: Session) -> str:
    """`pgdt-gzip-seek`'s first pass and its resumed unit decodes on each engine it
    builds over, against a serial inflate of the same member.

    What it decides (`docs/design/decisions.md`, "D112"): which engine
    `pgdt-gzip-seek` ships, the fastest winning and the lighter build within noise. **Two costs an engine sets**, read off one
    build of the `gz_read` example an engine (`ensure_gzseek_binary`): the
    first pass, which stops at every block's end and copies each
    checkpoint's window; and every unit decoded alone from its checkpoint,
    which is the `units` shape's time less the `forward` shape's, both of one
    binary in one sweep. A first pass is set against zlib-ng's serial inflate,
    the floor it is held to, taken beside it by the inflate instrument.

    **Warm alone**: serial inflate's rate is the CPU's in every regime
    (`inflate-throughput`), and an engine's is what is compared. The rate is
    per plaintext byte, `control`'s size, which each run's `delivered=` is held
    to, and each leg's `engine=` to the engine it names.

    **Five reps**, as the inflate figure takes."""
    figure = "gzseek-backend"
    specs = _gzseek_specs()
    session.sweep(figure, specs, session.cfg.reps(5))

    plaintext = session.input_size("control", "warm")
    units: set[str] = set()
    for spec in specs:
        reported = session.reported.get(spec.key(figure), {})
        delivered = reported.get("delivered")
        if delivered is not None and int(delivered) != plaintext:
            raise RuntimeError(
                f"{spec.label} delivered {delivered} plaintext bytes, and the file it is a "
                f"compression of holds {plaintext}: not a reading of that file"
            )
        if spec.binary.startswith("gzseek:"):
            engine = spec.binary.removeprefix("gzseek:")
            problem = image.gzseek_engine_problem(engine, reported.get("engine", ""))
            if problem is not None and not session.cfg.dry_run:
                raise RuntimeError(f"{spec.label}: {problem}")
            if "units" in reported:
                units.add(reported["units"])
    if len(units) > 1:
        raise RuntimeError(
            f"the engines built indexes of {sorted(units)} units over one file and spacing: "
            "a checkpoint's place is the file's alone, so one engine stops where another does not"
        )

    def got(command: str) -> float:
        return formatting.median(session.get(figure, next(s for s in specs if s.command == command)))

    floor = got(f"{shapes.INFLATE_FAMILY}zlib-ng")
    rows = []
    for spec in specs:
        values = session.get(figure, spec)
        cell = f"{formatting.fmt_median_spread(values)} · {formatting.fmt_rate(plaintext, formatting.median(values))}"
        if spec.command.startswith(f"{shapes.GZSEEK_FAMILY}forward-"):
            cell += f" · {formatting.median(values) / floor:.2f}× `zlib-ng`'s serial inflate"
        rows.append([spec.label, cell])
        if spec.command.startswith(f"{shapes.GZSEEK_FAMILY}units-"):
            engine = spec.command.removeprefix(f"{shapes.GZSEEK_FAMILY}units-")
            resumed = formatting.median(values) - got(f"{shapes.GZSEEK_FAMILY}forward-{engine}")
            rows.append([
                f"Every unit resumed, `{engine}` (the row above less the first pass)",
                f"**{resumed:.2f} s** · {formatting.fmt_rate(plaintext, resumed)}",
            ])
    table = formatting.md_table(["Row", "Warm, tmpfs"], rows)

    compressed = session.input_size("control_gz", "warm")
    count = next(iter(units), "?")
    notes = (
        "\n\nEach cell is wall clock and the rate it implies, per plaintext byte, and a first "
        "pass's time as a multiple of `zlib-ng`'s serial inflate of the same member, the floor "
        "it is held to. The instrument is `crates/pgdt-gzip-seek/examples/gz_read.rs`, one build an "
        "engine (`crates/pgdt-gzip-seek/Cargo.toml`'s features), one thread, every member's CRC-32 "
        "and `ISIZE` compared, and every unit's CRC-32 where a row decodes every unit; the "
        "serial inflate rows are "
        "`crates/gzip-inflate/src/main.rs`'s.\n\n"
        f"- `control_gz`: {formatting._fmt_bytes(compressed)} compressed, {formatting._fmt_bytes(plaintext)} of "
        f"plaintext, checkpoints every {shapes.GZSEEK_SPACING // config.MIB} MiB of plaintext, "
        f"{count} units\n"
    )
    return table + notes + "\n" + prose._per_rep(figure, session, specs)


# -- pgdt-gzip-seek's spacing --------------------------------------------------------


def _gzspacing_specs() -> list[runspec.RunSpec]:
    references = [
        runspec.RunSpec("gzinflate", "control_gz", f"{shapes.INFLATE_FAMILY}{b}", "warm", f"Serial inflate, `{b}`")
        for b in GZSEEK_REFERENCES
    ]
    labels = {
        "forward": "First pass",
        "starts": "First pass, then every unit begun",
        "units": "First pass, then every unit",
    }
    legs = [
        runspec.RunSpec(
            f"gzseek:{GZSPACING_ENGINE}",
            "control_gz",
            f"{shapes.GZSPACING_FAMILY}{stage}-{mib}",
            "warm",
            f"{labels[stage]}, {mib} MiB",
        )
        for mib in shapes.GZSPACING_MIB
        for stage in shapes.GZSPACING_STAGES
    ]
    return references + legs


def run_gzseek_spacing(session: Session) -> str:
    """gzip's own costs at each candidate checkpoint spacing, on the engine
    `pgdt-gzip-seek` ships, against a serial inflate of the same member.

    What it prices (`docs/design/decisions.md`, "D113"): the three costs a
    spacing sets that xz does not share — **the first pass**, which stops at every block's end whatever the spacing
    and copies one window a checkpoint, set against zlib-ng's serial inflate,
    the floor it is held to; **a resume's fixed cost**, the `starts` shape's
    time less the `forward` shape's over the unit count, both of one binary in
    one sweep, an upper bound because it holds the first block's header a
    whole unit decode reads anyway; and **the window section**, every
    checkpoint's window as the instrument counts it (`windows=`). Every unit
    decoded whole, the `units` shape less `forward`, is the whole of a read at
    each spacing. What a unit's size moves that xz shares — residency, the
    block pool, the partition grain — is read off xz's figures, not here.

    **What would falsify the premise that a spacing within xz's block sizes
    costs gzip only its window section**: a first pass moving with the spacing
    by more than its spread, or a resumed unit's fixed cost that is no one
    constant across the rows whose unit count resolves it, or that, times the
    unit count, is not small beside the unit decodes at the smallest spacing.

    **Warm alone and five reps**, as `gzseek-backend`: inflate is the CPU's in
    every regime (`inflate-throughput`). Each run's `delivered=` is held to
    `control`'s size, each leg's `engine=` to the shipped engine, the three
    stages at one spacing to one index, the unit count to fall as the spacing
    grows, and a `starts` run to have begun every unit."""
    figure = "gzseek-spacing"
    specs = _gzspacing_specs()
    session.sweep(figure, specs, session.cfg.reps(5))

    plaintext = session.input_size("control", "warm")
    index: dict[int, tuple[int, int]] = {}
    for spec in specs:
        reported = session.reported.get(spec.key(figure), {})
        delivered = reported.get("delivered")
        if delivered is not None and int(delivered) != plaintext:
            raise RuntimeError(
                f"{spec.label} delivered {delivered} plaintext bytes, and the file it is a "
                f"compression of holds {plaintext}: not a reading of that file"
            )
        if not spec.command.startswith(shapes.GZSPACING_FAMILY):
            continue
        problem = image.gzseek_engine_problem(GZSPACING_ENGINE, reported.get("engine", ""))
        if problem is not None and not session.cfg.dry_run:
            raise RuntimeError(f"{spec.label}: {problem}")
        stage, _, mib = spec.command.removeprefix(shapes.GZSPACING_FAMILY).partition("-")
        units, windows = int(reported["units"]), int(reported["windows"])
        if index.setdefault(int(mib), (units, windows)) != (units, windows):
            raise RuntimeError(
                f"{spec.label} built an index of {units} units and {windows} window bytes, "
                f"another stage at {mib} MiB one of {index[int(mib)]}: a checkpoint's place "
                "is the file's alone"
            )
        if stage == "starts" and int(reported.get("started", -1)) != units:
            raise RuntimeError(
                f"{spec.label} began {reported.get('started')} of its {units} units"
            )
    counts = [index[mib][0] for mib in shapes.GZSPACING_MIB]
    if any(a < b for a, b in zip(counts, counts[1:])):
        raise RuntimeError(f"unit counts {counts} rise with the spacing {shapes.GZSPACING_MIB} MiB")

    def got(command: str) -> float:
        return formatting.median(session.get(figure, next(s for s in specs if s.command == command)))

    floor = got(f"{shapes.INFLATE_FAMILY}zlib-ng")
    compressed = session.input_size("control_gz", "warm")
    rows = []
    for mib in shapes.GZSPACING_MIB:
        units, windows = index[mib]
        forward_spec = next(s for s in specs if s.command == f"{shapes.GZSPACING_FAMILY}forward-{mib}")
        values = session.get(figure, forward_spec)
        forward = formatting.median(values)
        begun = got(f"{shapes.GZSPACING_FAMILY}starts-{mib}") - forward
        resumed = got(f"{shapes.GZSPACING_FAMILY}units-{mib}") - forward
        rows.append([
            f"{mib} MiB",
            str(units),
            f"{formatting._fmt_bytes(windows)} · {windows / compressed:.2%} of the `.gz`",
            f"{formatting.fmt_median_spread(values)} · {forward / floor:.2f}× `zlib-ng`'s serial inflate",
            f"{begun / units * 1e6:+.1f} µs",
            f"**{formatting.fmt_s(resumed)} s** · {formatting.fmt_rate(plaintext, resumed)}",
        ])
    table = formatting.md_table(
        [
            "Spacing",
            "Units",
            "Window section",
            "First pass",
            "A unit begun (less the first pass, a unit)",
            "Every unit resumed (less the first pass)",
        ],
        rows,
    )

    references = "".join(
        f"- {spec.label}: {formatting.fmt_median_spread(session.get(figure, spec))} · "
        f"{formatting.fmt_rate(plaintext, formatting.median(session.get(figure, spec)))}\n"
        for spec in specs
        if spec.binary == "gzinflate"
    )
    notes = (
        "\n\nThe first pass is wall clock, and its time as a multiple of `zlib-ng`'s serial "
        "inflate of the same member, the floor it is held to. A unit begun is the median of "
        "the first pass with every unit begun and brought to its first plaintext byte, less "
        "the first pass's, over the unit count: a fresh inflate state, the window set, the "
        "bits primed, a positioned read and the first block's header — which a whole unit "
        "decode reads anyway, so it bounds a resume's fixed cost from above. Every unit "
        "resumed is the median of the first pass with every unit decoded whole, less the "
        "first pass's, and the rate it implies per plaintext byte. The window section is "
        "every checkpoint's window as the instrument counts it, and its share of the "
        "compressed file. The instrument is `crates/pgdt-gzip-seek/examples/gz_read.rs`, the "
        f"`{GZSPACING_ENGINE}` build (`crates/pgdt-gzip-seek/Cargo.toml`'s default), one thread, "
        "every member's CRC-32 and `ISIZE` compared, and every unit's CRC-32 where a row "
        "decodes every unit; the serial inflate rows are `crates/gzip-inflate/src/main.rs`'s.\n\n"
        f"- `control_gz`: {formatting._fmt_bytes(compressed)} compressed, {formatting._fmt_bytes(plaintext)} of "
        "plaintext\n"
        + references
    )
    return table + notes + "\n" + prose._per_rep(figure, session, specs)
