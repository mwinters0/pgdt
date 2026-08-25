#!/usr/bin/env python3
"""Generate a synthetic pg_dump-shaped plain-text dump for throughput benchmarks.

Backs docs/design/measurements.md ("Decoder and whole-file benchmarks") and
docs/design/roadmap-phase7-scan-performance.md ("Measurement discipline"). Unlike generate_fixtures.py, this is *not* a correctness
fixture: its output is never checked against real pg_dump, only shaped
closely enough to satisfy this codebase's own COPY/DDL grammar so pgdq can
read it back. Two different runs producing different bytes is fine -- this
measures decode/scan throughput, not correctness, and correctness already
lives in fixtures/, which pg_dump itself produces.

Generated, never committed: point --out somewhere outside the repo (the SSD
or root NVMe volume -- see CLAUDE.local.md) for a real measurement run, or
under the gitignored runs/ directory for routine benchmark use.
"""

from __future__ import annotations

import argparse
import random
import uuid as uuid_mod
from datetime import date, datetime, timedelta
from pathlib import Path

TABLE = "public.perf"

# One column per mapped type family this codebase decodes, plus
# three text columns carrying the stress shapes
# measurements.md ("Decoder and whole-file benchmarks") calls
# for: high-escape-density fields, very long values, and -- the whole row,
# together -- a wide table.
COLUMNS: list[tuple[str, str]] = [
    ("id", "integer"),
    ("v_smallint", "smallint"),
    ("v_bigint", "bigint"),
    ("v_real", "real"),
    ("v_double", "double precision"),
    ("v_numeric", "numeric(20,6)"),
    ("v_date", "date"),
    ("v_time", "time"),
    ("v_timestamp", "timestamp"),
    ("v_timestamptz", "timestamptz"),
    ("v_uuid", "uuid"),
    ("v_bytea", "bytea"),
    ("v_bool", "boolean"),
    ("v_text", "text"),
    ("v_long_text", "text"),
    ("v_escaped", "text"),
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


def random_row(rng: random.Random, row_id: int) -> list[str]:
    ts = datetime(2000, 1, 1) + timedelta(seconds=rng.randint(0, 60 * 60 * 24 * 365 * 30))
    values = [
        str(row_id),
        str(rng.randint(-32768, 32767)),
        str(rng.randint(-(2**63), 2**63 - 1)),
        repr(rng.uniform(-1e6, 1e6)),
        repr(rng.uniform(-1e12, 1e12)),
        f"{rng.uniform(-1e12, 1e12):.6f}",
        (date(2000, 1, 1) + timedelta(days=rng.randint(0, 365 * 30))).isoformat(),
        ts.strftime("%H:%M:%S.%f"),
        ts.strftime("%Y-%m-%d %H:%M:%S.%f"),
        ts.strftime("%Y-%m-%d %H:%M:%S.%f") + "+00",
        str(uuid_mod.UUID(int=rng.getrandbits(128))),
        "\\x" + rng.randbytes(64).hex(),
        "t" if rng.random() < 0.5 else "f",
        lorem(rng, rng.randint(3, 12)),
        lorem(rng, rng.randint(200, 800)),
        high_escape_text(rng, rng.randint(20, 60)),
    ]
    # Every column but `id` is nullable -- occasional NULLs exercise the
    # `\N`-vs-empty-string path the same way real data would.
    return [values[0]] + [
        encode_field(None) if rng.random() < 0.02 else encode_field(v) for v in values[1:]
    ]


def generate(out: Path, size_bytes: int, seed: int | None) -> None:
    rng = random.Random(seed)
    col_decl = ",\n    ".join(f"{name} {typ}" for name, typ in COLUMNS)
    col_names = ", ".join(name for name, _ in COLUMNS)

    with out.open("w") as f:
        f.write(f"CREATE TABLE {TABLE} (\n    {col_decl}\n);\n\n")
        f.write(f"COPY {TABLE} ({col_names}) FROM stdin;\n")
        written = 0
        row_id = 0
        while written < size_bytes:
            row_id += 1
            line = "\t".join(random_row(rng, row_id)) + "\n"
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
    args = parser.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    generate(args.out, int(args.size_mb * 1024 * 1024), args.seed)
    print(f"wrote {args.out} ({args.out.stat().st_size / (1024 * 1024):.1f} MiB)")


if __name__ == "__main__":
    main()
