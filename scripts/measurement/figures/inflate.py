"""`inflate-throughput`: serial inflate of one gzip member, against the plain parse of its
plaintext, in every regime."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import formatting
from measurement import runspec
from measurement import shapes
from measurement.figures import prose

if TYPE_CHECKING:
    from measurement.timing import Session


# -- serial inflate against the plain parse ---------------------------------

#: The regimes `inflate-throughput` is taken in, in its table's column order:
#: every device the register measures. **A first pass reads the compressed
#: file and the plain parse the plaintext**, several times as many bytes, so
#: which of the two a device leaves slower is the device's to say, and the
#: answer on tmpfs is the CPU's alone.
INFLATE_REGIMES: tuple[tuple[str, str], ...] = (
    ("warm", "Warm, tmpfs"),
    ("cold", "Cold, SATA SSD"),
    ("cold-nvme", "Cold, NVMe"),
)

#: The figure's rows: a binary, an input, a command shape and its caption.
#: Each file's `dd` floor, the plain `parse` every inflate row is read
#: against, then a row a backend.
INFLATE_ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("none", "control", "dd", "`dd` → `/dev/null`, plain"),
    ("none", "control_gz", "dd", "`dd` → `/dev/null`, `.gz`"),
    ("pgdt", "control", "parse", "`pgdt parse`, plain"),
    *(
        ("gzinflate", "control_gz", f"{shapes.INFLATE_FAMILY}{b}", f"Inflate, `{b}`")
        for b in shapes.INFLATE_BACKENDS
    ),
)


def _inflate_specs() -> list[runspec.RunSpec]:
    return [
        runspec.RunSpec(binary, inp, command, regime, f"{label} ({regime})")
        for regime, _ in INFLATE_REGIMES
        for binary, inp, command, label in INFLATE_ROWS
    ]


def run_inflate_throughput(session: Session) -> str:
    """Serial inflate of one gzip member, against the plain `parse` of the
    same plaintext, in every regime.

    What it decides (`docs/design/decisions.md`, "D117", "D127"): a single
    member's first pass inflates serially, so an inflate row slower than that
    pass's own floor — the `.gz` file's `dd` row or the warm `parse`, the
    plaintext reaching its scan from memory, whichever is slower — is a first
    pass bound by the inflate, which only a parallel first pass would lift;
    and the backends' rates are what `pgdt-gzip-seek`'s throughput floor is read
    off. **The ratio each inflate cell carries answers a different
    question**, whether a `.gz` first pass is slower than the plain file's
    `parse` on the same device: below 1 on a device holding the plain
    `parse`, the pass can still be its inflate's.

    **The plain `parse` is this figure's own reading**, not borrowed from
    `scan-throughput-*`: each inflate row is read as a ratio against it, and
    a ratio between two inputs holds only within one sweep
    (`Figure.subtracts`), so it is taken beside them. It is the shape and
    count every throughput figure states, `--jobs 1`; what a wider parse
    reaches is `parallel-scan-throughput`'s, set beside this table as a rate
    and never divided into it.

    **The rate is per plaintext byte**, which is `control`'s size exactly —
    the `.gz` leg is a compression of it — and the instrument's own count of
    what it delivered is held to that, so a short inflate cannot read as a
    fast one. A floor's rate is per byte of its own file.

    **Five reps**, as the decode figure takes: the reading that decides is a
    ratio near wherever the two rates cross, and the whole sitting is the
    cold `parse` rows' minutes rather than a sweep's hours.
    """
    figure = "inflate-throughput"
    specs = _inflate_specs()
    session.sweep(figure, specs, session.cfg.reps(5))

    plaintext = {regime: session.input_size("control", regime) for regime, _ in INFLATE_REGIMES}
    parse = {
        spec.regime: formatting.median(session.get(figure, spec))
        for spec in specs
        if spec.command == "parse"
    }
    for spec in specs:
        delivered = session.reported.get(spec.key(figure), {}).get("delivered")
        if delivered is not None and int(delivered) != plaintext[spec.regime]:
            raise RuntimeError(
                f"{spec.label} delivered {delivered} plaintext bytes, and the file it is a "
                f"compression of holds {plaintext[spec.regime]}: not a reading of that file"
            )

    rows = []
    for _, inp, command, label in INFLATE_ROWS:
        cells = [label]
        for regime, _ in INFLATE_REGIMES:
            spec = next(
                s for s in specs if (s.regime, s.command, s.input) == (regime, command, inp)
            )
            values = session.get(figure, spec)
            got = formatting.median(values)
            nbytes = (
                session.input_size(inp, regime) if command == "dd" else plaintext[regime]
            )
            cell = f"{formatting.fmt_median_spread(values)} · {formatting.fmt_rate(nbytes, got)}"
            if command.startswith(shapes.INFLATE_FAMILY):
                cell += f" · {got / parse[regime]:.2f}× the `parse`"
            cells.append(cell)
        rows.append(cells)
    table = formatting.md_table(["Row", *(label for _, label in INFLATE_REGIMES)], rows)

    compressed = session.input_size("control_gz", "warm")
    plain = session.input_size("control", "warm")
    notes = (
        "\n\nEach cell is wall clock and the rate it implies — per plaintext byte for the "
        "`parse` and inflate rows, per byte of its own file for a floor — and an inflate "
        "row's time as a multiple of the same regime's `parse`: above 1, a `.gz` first pass "
        "is slower than the plain file's. What holds a first pass to its inflate is an inflate "
        "row slower than the pass's own floor, the `.gz` file's `dd` row or the warm `parse`, "
        "whichever is slower. The instrument is `crates/gzip-inflate/src/main.rs`, "
        "one thread, checking the member's CRC-32 and `ISIZE` as a first pass must.\n\n"
        f"- `control_gz`: {formatting._fmt_bytes(compressed)} compressed, {formatting._fmt_bytes(plain)} of "
        f"plaintext, {plain / compressed:.2f}× (`scripts/generate_gzip_input.py`: one member, "
        "`pg_dump`'s header, level 6)\n"
    )
    return table + notes + "\n" + prose._per_rep(figure, session, specs)
