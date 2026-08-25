#!/usr/bin/env python3
"""Generate a synthetic pg_dump --inserts-shaped data region for the
out-of-band item M3 measurement.

Backs docs/design/roadmap.md's out-of-band ledger item M3, which asks what
Phase 3.6 never measured: an `INSERT` run's actual per-byte scan cost. Slice
3.6 gave the large-object region a scanner-level fast path (lines skipped
unread) but left `INSERT` runs on a map-level one -- folded into a single
`Data(InsertRun)` span, but with every line still decoded into
`crate::scan::Event::Line` and pushed through the statement accumulator. This
generator produces the input that turns that argument into a number.

Like generate_large_object_bench.py and generate_perf_data.py, and unlike
generate_fixtures.py, this is *not* a correctness fixture: its output is never
checked against real pg_dump, only shaped closely enough to satisfy this
codebase's own grammar (one `INSERT INTO <table> VALUES (...);` per line,
under an ordinary TOC comment) to measure throughput. Correctness for real
`--inserts` output lives in fixtures/*/edge_cases/inserts.sql, which pg_dump
itself produces.

Generated, never committed: point --out somewhere outside the repo (the SSD
volume -- see CLAUDE.local.md) for a real measurement run, or under the
gitignored runs/ directory for routine use.
"""

from __future__ import annotations

import argparse
import random
import uuid as uuid_mod
from datetime import date, datetime, timedelta
from pathlib import Path

TABLE = "public.bench_inserts"

COLUMNS: list[tuple[str, str]] = [
    ("id", "integer"),
    ("v_bigint", "bigint"),
    ("v_numeric", "numeric(20,6)"),
    ("v_date", "date"),
    ("v_timestamp", "timestamp without time zone"),
    ("v_uuid", "uuid"),
    ("v_bool", "boolean"),
    ("v_text", "text"),
    ("v_long_text", "text"),
]

LOREM_WORDS = (
    "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod "
    "tempor incididunt ut labore et dolore magna aliqua enim ad minim veniam "
    "quis nostrud exercitation ullamco laboris nisi aliquip ex ea commodo "
    "consequat duis aute irure in reprehenderit voluptate velit esse cillum "
    "eu fugiat nulla pariatur excepteur sint occaecat cupidatat non proident"
).split()


def lorem(rng: random.Random, n_words: int) -> str:
    return " ".join(rng.choice(LOREM_WORDS) for _ in range(n_words))


def quote(value: str) -> str:
    """pg_dump --inserts emits standard-conforming strings: a single quote is
    doubled, everything else is literal. An apostrophe every so often is the
    point -- it is what the statement accumulator's quote tracker has to
    follow."""
    return "'" + value.replace("'", "''") + "'"


def random_row(rng: random.Random, row_id: int) -> str:
    ts = datetime(2000, 1, 1) + timedelta(seconds=rng.randint(0, 60 * 60 * 24 * 365 * 30))
    text = lorem(rng, rng.randint(3, 12))
    if rng.random() < 0.15:
        # An embedded apostrophe, doubled the way pg_dump would.
        text = text.replace(" ", "'s ", 1)
    values = [
        str(row_id),
        str(rng.randint(-(2**63), 2**63 - 1)),
        f"{rng.uniform(-1e12, 1e12):.6f}",
        quote((date(2000, 1, 1) + timedelta(days=rng.randint(0, 365 * 30))).isoformat()),
        quote(ts.strftime("%Y-%m-%d %H:%M:%S.%f")),
        quote(str(uuid_mod.UUID(int=rng.getrandbits(128)))),
        "true" if rng.random() < 0.5 else "false",
        quote(text),
        "NULL" if rng.random() < 0.02 else quote(lorem(rng, rng.randint(15, 40))),
    ]
    return f"INSERT INTO {TABLE} VALUES ({', '.join(values)});\n"


def generate(out: Path, size_bytes: int, seed: int | None) -> None:
    rng = random.Random(seed)
    col_decl = ",\n    ".join(f"{name} {typ}" for name, typ in COLUMNS)
    table_name = TABLE.split(".", 1)[1]

    with out.open("w") as f:
        f.write("--\n-- PostgreSQL database dump\n--\n\n")
        f.write("-- Dumped from database version 16.14\n")
        f.write("-- Dumped by pg_dump version 16.14\n\n")
        f.write("SET statement_timeout = 0;\n")
        f.write("SET client_encoding = 'UTF8';\n\n")
        f.write(
            f"--\n-- Name: {table_name}; Type: TABLE; Schema: public; Owner: postgres\n--\n\n"
        )
        f.write(f"CREATE TABLE {TABLE} (\n    {col_decl}\n);\n\n")
        f.write(
            f"--\n-- Data for Name: {table_name}; Type: TABLE DATA; "
            "Schema: public; Owner: postgres\n--\n\n"
        )

        written = 0
        row_id = 0
        while written < size_bytes:
            row_id += 1
            line = random_row(rng, row_id)
            f.write(line)
            written += len(line)

        # A trailing object past the run, so a scan that lost the run's
        # boundary would still have to find its way back out to classify this.
        f.write("\n--\n-- Name: post_insert_marker; Type: TABLE; Schema: public; Owner: postgres\n--\n\n")
        f.write("CREATE TABLE public.post_insert_marker (id integer);\n")
    print(f"rows: {row_id}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="Output path for the generated dump")
    parser.add_argument(
        "--size-gb",
        type=float,
        default=3.0,
        help="Approximate output size in GiB (default: 3.0, matching the large-object bench)",
    )
    parser.add_argument(
        "--seed", type=int, default=None, help="Optional RNG seed (reproducibility is not a goal)"
    )
    args = parser.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    generate(args.out, int(args.size_gb * 1024 * 1024 * 1024), args.seed)
    print(f"wrote {args.out} ({args.out.stat().st_size / (1024**3):.2f} GiB)")


if __name__ == "__main__":
    main()
