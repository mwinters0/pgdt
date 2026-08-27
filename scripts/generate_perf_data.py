#!/usr/bin/env python3
"""Generate a synthetic pg_dump-shaped plain-text dump for throughput benchmarks.

Backs docs/design/measurements.md ("Decoder and whole-file benchmarks") and
docs/design/roadmap-P7-scan-performance.md ("Measurement discipline"). Unlike generate_fixtures.py, this is *not* a correctness
fixture: its output is never checked against real pg_dump, only shaped
closely enough to satisfy this codebase's own COPY/DDL grammar so pgdq can
read it back. Two different runs producing different bytes is fine -- this
measures decode/scan throughput, not correctness, and correctness already
lives in fixtures/, which pg_dump itself produces.

"Closely enough" has a mechanical floor, and it is the one this script kept
failing silently: every column must declare a type pgdq maps and hold values
pgdq re-renders unchanged, or a benchmark for the typed path is quietly
measuring the untyped one. That floor is asserted from the Rust side --
pgdump_query-cli/tests/perf_generator_fidelity.rs generates a small file in
every flag combination a recorded figure is taken on and requires `pgdq query
--schema-mode typed` and `strings` to agree byte for byte on each.

Generated, never committed: point the output path somewhere outside the repo
(the SSD or root NVMe volume -- see CLAUDE.local.md) for a real measurement
run, or under the gitignored runs/ directory for routine benchmark use.

The default output is a *control*: no data row contains a `{` or a `[`, which
is what docs/design/measurements.md's array-shape-census figure and its
scan-throughput table were both taken on. The array and composite stress
columns live behind --arrays and --composite for exactly that reason -- see
that doc's array sections, and
docs/design/roadmap-P4-composite-decoding.md, "The performance deliverable
is a ratio, not a gate".

So the default output's *bytes* are frozen, and changing them is not a local
decision: five recorded figures name this script as the command that
reproduces them, and they have to be re-taken with the change (M10 did that in
the same commit -- docs/design/roadmap.md's out-of-band ledger; M12 changed
the date/time fractions and its re-take is M13, the queued item it was
deliberately ordered ahead of, so that the whole warm set moves onto tmpfs at
once rather than a figure at a time).
"""

from __future__ import annotations

import argparse
import random
import struct
import uuid as uuid_mod
from datetime import date, datetime, timedelta
from decimal import ROUND_HALF_UP, Context, Decimal
from pathlib import Path

TABLE = "public.perf"

# One column per mapped type family this codebase decodes, plus
# three text columns carrying the stress shapes
# measurements.md ("Decoder and whole-file benchmarks") calls
# for: high-escape-density fields, very long values, and -- the whole row,
# together -- a wide table.
#
# The three date/time spellings are the long ones because those are the only
# ones pg_dump writes and the only ones pgtype::resolve_declared_type maps:
# `timestamp` resolves Unknown and stays Utf8View, which is a column the typed
# path never touches.
COLUMNS: list[tuple[str, str]] = [
    ("id", "integer"),
    ("v_smallint", "smallint"),
    ("v_bigint", "bigint"),
    ("v_real", "real"),
    ("v_double", "double precision"),
    ("v_numeric", "numeric(20,6)"),
    ("v_date", "date"),
    ("v_time", "time without time zone"),
    ("v_timestamp", "timestamp without time zone"),
    ("v_timestamptz", "timestamp with time zone"),
    ("v_uuid", "uuid"),
    ("v_bytea", "bytea"),
    ("v_bool", "boolean"),
    ("v_text", "text"),
    ("v_long_text", "text"),
    ("v_escaped", "text"),
]

# The type `v_comp` is declared as. Two fields, so a record literal carries
# both a quoted and an unquoted member: record_out quotes a field holding
# whitespace and leaves a bare integer alone. This is the first type
# definition this generator writes -- the default output is a bare CREATE
# TABLE with no TOC comments.
COMPOSITE_TYPE = "public.perf_comp"
COMPOSITE_DDL = f"CREATE TYPE {COMPOSITE_TYPE} AS (\n\ta integer,\n\tb text\n);\n\n"

# --arrays only. Two integer[] columns of different lengths, because the
# question these back is what *per-element* decoding costs, and one length
# cannot separate the per-element slope from the per-value overhead; same
# element type in both, so element count is the only variable. Both are
# uniform 1-D and never NULL, so they resolve to List rather than degrading to
# Utf8View, which would measure nothing `v_text` does not.
ARRAY_COLUMNS: list[tuple[str, str]] = [
    ("v_int_array", "integer[]"),
    ("v_int_array_long", "integer[]"),
]

# --composite only, and separate from --arrays so a run can put the composite
# on one axis by itself: `pgdq query` has no column projection, so separating
# what one nested column costs end to end takes its own file
# (measurements.md, "A typed query over nested columns").
COMPOSITE_COLUMNS: list[tuple[str, str]] = [
    ("v_comp", COMPOSITE_TYPE),
]

# A fixed word list rather than a `lorem`-style dependency -- this script has
# no dependencies today (scripts/pyproject.toml) and generating throwaway
# throughput data doesn't need real Latin, just realistic word/line shape.
LOREM_WORDS = (
    "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod "
    "tempor incididunt ut labore et dolore magna aliqua enim ad minim veniam "
    "quis nostrud exercitation ullamco laboris nisi aliquip ex ea commodo "
    "consequat duis aute irure in reprehenderit voluptate velit esse cillum "
    "eu fugiat nulla pariatur excepteur sint occaecat cupidatat non proident"
).split()

# The exact escapes pg_dump's own COPY TO ever emits -- postgres-invariants.md
# I15. Reproduced here (rather than shelling out to `pgdq`) because this
# script has no Rust runtime to call into; pgdump_query::copy::encode_field
# implements the identical mapping and its round trip against real pg_dump
# output is what backs I15 in the first place.
ESCAPES = {
    "\\": "\\\\",
    "\b": "\\b",
    "\f": "\\f",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
    "\v": "\\v",
}

# The microsecond component of the three date/time columns is a uniform draw
# over the whole range, because this is a benchmark input and a benchmark
# input's job is to look like real data: 300,000 rows of koji's
# task.create_time hold no value at all with an empty fraction, and 9.9% end
# in a zero digit, which is about what a uniform draw gives.
#
# Rendered *shape* coverage -- an absent fraction, one digit, six digits, and
# both ends of PostgreSQL's trailing-zero trim -- is a correctness goal, and
# it belongs to fixtures/, which already carries all five and is produced by
# pg_dump itself. A weighted list here bought that coverage a second time at
# the cost of an input 37.5% of whose timestamps render with no fraction.

# FLT_DIG / DBL_DIG: the significant-digit counts float4out/float8out switch
# to scientific notation at. `pgdump_query::decode::format_shortest` holds the
# other half of this pair.
FLT_DIG = 6
DBL_DIG = 15


def encode_field(value: str | None) -> str:
    if value is None:
        return "\\N"
    return "".join(ESCAPES.get(ch, ch) for ch in value)


def lorem(rng: random.Random, n_words: int) -> str:
    return " ".join(rng.choice(LOREM_WORDS) for _ in range(n_words))


def high_escape_text(rng: random.Random, n_words: int) -> str:
    """Lorem text interspersed with the bytes COPY TEXT must escape, at a much
    higher density than ordinary prose -- the per-byte escaping cost is what
    this column exists to stress, not realism."""
    parts = []
    for _ in range(n_words):
        parts.append(rng.choice(LOREM_WORDS))
        parts.append(rng.choice(["\\", "\t", "\n", "\r"]))
    return "".join(parts)


def float_text(value: float, *, single: bool) -> str:
    """`float4out`/`float8out` output for `value`: the shortest decimal that
    round-trips, formatted fixed or scientific by the classic %g rule at
    FLT_DIG/DBL_DIG.

    The point is that pgdq re-renders this string unchanged. A float64
    `repr()` in a `real` column does not survive that -- 17 significant digits
    round-trip through an f64 and not through the f32 the column decodes to,
    so `typed` prints something else and the benchmark's two modes disagree on
    bytes that no real dump can hold. `pgdump_query::decode::render_f32` and
    `render_f64` are the other side.
    """
    if single:
        value = struct.unpack("<f", struct.pack("<f", value))[0]
    sig_digits = FLT_DIG if single else DBL_DIG

    def round_trips(candidate: Decimal) -> bool:
        if single:
            return struct.pack("<f", float(candidate)) == struct.pack("<f", value)
        return float(candidate) == value

    # Shortest first: the smallest precision whose correctly-rounded decimal
    # still names this float is what a shortest-round-trip formatter emits.
    # Rounding runs over Decimal(value), which is the float's *exact* binary
    # value, and breaks ties away from zero -- `%.Ne` breaks them to even, and
    # the two disagree on values like -390238.125, where Rust's own shortest
    # formatter (which is what pgdq re-renders through) answers -390238.13.
    exact = Decimal(value)
    for precision in range(1, 18):
        rounded = Context(prec=precision, rounding=ROUND_HALF_UP).create_decimal(exact)
        if round_trips(rounded):
            break
    sign, digit_tuple, last_exp = rounded.as_tuple()
    digits = "".join(str(d) for d in digit_tuple).rstrip("0") or "0"
    # `last_exp` is the power of ten on the *last* digit; `exp` is the one on
    # the leading digit, which is what the %g rule below tests.
    exp = len(digit_tuple) + int(last_exp) - 1

    out = "-" if sign == 1 else ""
    if exp < -4 or exp >= sig_digits:
        out += digits[0]
        if len(digits) > 1:
            out += "." + digits[1:]
        return out + "e" + ("-" if exp < 0 else "+") + f"{abs(exp):02}"
    dp = exp + 1  # digits before the decimal point
    if dp <= 0:
        return out + "0." + "0" * (-dp) + digits
    if dp >= len(digits):
        return out + digits + "0" * (dp - len(digits))
    return out + digits[:dp] + "." + digits[dp:]


def hms_frac(micros_of_day: int) -> str:
    """`HH:MM:SS[.frac]`, trailing zeros trimmed and an all-zero fraction
    dropped -- `pgdump_query::decode::format_hms_frac`, in Python."""
    seconds, micros = divmod(micros_of_day, 1_000_000)
    h, rest = divmod(seconds, 3600)
    mi, se = divmod(rest, 60)
    if micros == 0:
        return f"{h:02}:{mi:02}:{se:02}"
    return f"{h:02}:{mi:02}:{se:02}.{micros:06}".rstrip("0")


def int_array(rng: random.Random, n_elements: int) -> str:
    return "{" + ",".join(str(rng.randint(-(2**31), 2**31 - 1)) for _ in range(n_elements)) + "}"


def composite(rng: random.Random) -> str:
    """A two-field `record_out` literal, quoted the way PostgreSQL quotes it."""
    return f'({rng.randint(-(2**31), 2**31 - 1)},"{lorem(rng, rng.randint(3, 8))}")'


def random_row(rng: random.Random, row_id: int, arrays: bool, composites: bool) -> list[str]:
    ts = datetime(2000, 1, 1) + timedelta(seconds=rng.randint(0, 60 * 60 * 24 * 365 * 30))
    micros = rng.randrange(1_000_000)
    time_of_day = ts.hour * 3600 + ts.minute * 60 + ts.second
    hms = hms_frac(time_of_day * 1_000_000 + micros)
    values = [
        str(row_id),
        str(rng.randint(-32768, 32767)),
        str(rng.randint(-(2**63), 2**63 - 1)),
        float_text(rng.uniform(-1e6, 1e6), single=True),
        float_text(rng.uniform(-1e12, 1e12), single=False),
        f"{rng.uniform(-1e12, 1e12):.6f}",
        (date(2000, 1, 1) + timedelta(days=rng.randint(0, 365 * 30))).isoformat(),
        hms,
        f"{ts.date().isoformat()} {hms}",
        f"{ts.date().isoformat()} {hms}+00",
        str(uuid_mod.UUID(int=rng.getrandbits(128))),
        "\\x" + rng.randbytes(64).hex(),
        "t" if rng.random() < 0.5 else "f",
        lorem(rng, rng.randint(3, 12)),
        lorem(rng, rng.randint(200, 800)),
        high_escape_text(rng, rng.randint(20, 60)),
    ]
    # Every column but `id` is nullable -- occasional NULLs exercise the
    # `\N`-vs-empty-string path the same way real data would.
    row = [values[0]] + [
        encode_field(None) if rng.random() < 0.02 else encode_field(v) for v in values[1:]
    ]
    # Never NULL, and never non-uniform: the stress columns exist to be
    # decoded, and a shape the census refuses degrades the column to Utf8View.
    if arrays:
        row += [
            encode_field(int_array(rng, rng.randint(3, 5))),
            encode_field(int_array(rng, 50)),
        ]
    if composites:
        row += [encode_field(composite(rng))]
    return row


def generate(out: Path, size_bytes: int, seed: int | None, arrays: bool, composites: bool) -> None:
    rng = random.Random(seed)
    columns = list(COLUMNS)
    if arrays:
        columns += ARRAY_COLUMNS
    if composites:
        columns += COMPOSITE_COLUMNS
    col_decl = ",\n    ".join(f"{name} {typ}" for name, typ in columns)
    col_names = ", ".join(name for name, _ in columns)

    with out.open("w") as f:
        if composites:
            f.write(COMPOSITE_DDL)
        f.write(f"CREATE TABLE {TABLE} (\n    {col_decl}\n);\n\n")
        f.write(f"COPY {TABLE} ({col_names}) FROM stdin;\n")
        written = 0
        row_id = 0
        while written < size_bytes:
            row_id += 1
            line = "\t".join(random_row(rng, row_id, arrays, composites)) + "\n"
            f.write(line)
            written += len(line)
        f.write("\\.\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="Output path for the generated dump")
    parser.add_argument(
        "--size-mb",
        type=float,
        default=256,
        help="Approximate output size in MiB (default: 256, chosen to fit page cache)",
    )
    parser.add_argument(
        "--seed", type=int, default=None, help="Optional RNG seed (reproducibility is not a goal)"
    )
    parser.add_argument(
        "--arrays",
        action="store_true",
        help="Append the array stress columns (v_int_array, v_int_array_long). Off by default: "
        "the default output is the brace-free control measurements.md's census and "
        "scan-throughput figures were taken on.",
    )
    parser.add_argument(
        "--composite",
        action="store_true",
        help="Append the composite stress column (v_comp) and the CREATE TYPE it needs. Off by "
        "default, and independent of --arrays so either nested shape can be measured alone.",
    )
    args = parser.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    generate(args.out, int(args.size_mb * 1024 * 1024), args.seed, args.arrays, args.composite)
    print(f"wrote {args.out} ({args.out.stat().st_size / (1024 * 1024):.1f} MiB)")


if __name__ == "__main__":
    main()
