#!/usr/bin/env python3
"""Generate a synthetic pg_dump-shaped large-object data region for the
Phase 3.6 gating measurement.

Backs docs/design/roadmap-phase3-object-inventory.md ("Verification"): "A
synthetic large-object dump, a few GB, on the SSD... enough to distinguish
'skipped' from 'walked' without an hour of HDD time." Unlike
generate_fixtures.py, this is *not* a correctness fixture -- its output is
never run against real pg_dump, only shaped closely enough to satisfy this
codebase's own scanner grammar (three statement forms: `lo_open`, `lowrite`,
`lo_close`, `BEGIN;`/`COMMIT;`-wrapped) to measure whether the Phase 3.6 fast
path actually skips the region rather than walking it statement by statement.
Real pg_dump chunks large-object writes at LOBBUFSIZE (4096 raw bytes, ~8200
hex chars per `lowrite` line) -- reproduced here rather than shelling out to
a real dump, since generating a multi-GB large object through an actual
Postgres instance is exactly the cost this synthetic generator exists to
avoid.

Generated, never committed: point --out somewhere outside the repo (the SSD
volume -- see CLAUDE.local.md) for a real measurement run, or under the
gitignored runs/ directory for routine use.
"""

from __future__ import annotations

import argparse
import random
from pathlib import Path

LOBBUFSIZE = 4096


def generate(out: Path, size_bytes: int, seed: int | None) -> None:
    rng = random.Random(seed)

    with out.open("w") as f:
        f.write("--\n-- PostgreSQL database dump\n--\n\n")
        f.write("-- Dumped from database version 18.0\n")
        f.write("-- Dumped by pg_dump version 18.0\n\n")
        f.write("SET statement_timeout = 0;\n")
        f.write("SET client_encoding = 'UTF8';\n\n")
        f.write("--\n-- Data for Name: BLOBS; Type: BLOBS; Schema: -; Owner: -\n--\n\n")
        f.write("BEGIN;\n\n")

        written = 0
        oid = 16000
        while written < size_bytes:
            oid += 1
            f.write(f"SELECT pg_catalog.lo_open('{oid}', 131072);\n")
            written += len(f"SELECT pg_catalog.lo_open('{oid}', 131072);\n")
            # A handful of chunks per object, matching a real large object's
            # shape more closely than one giant object would.
            for _ in range(rng.randint(50, 200)):
                if written >= size_bytes:
                    break
                chunk = rng.randbytes(LOBBUFSIZE).hex()
                line = f"SELECT pg_catalog.lowrite(0, '\\x{chunk}');\n"
                f.write(line)
                written += len(line)
            f.write("SELECT pg_catalog.lo_close(0);\n\n")
            written += len("SELECT pg_catalog.lo_close(0);\n\n")

        f.write("COMMIT;\n\n")
        # A trailing statement past the region, so a scan that (incorrectly)
        # walked into the region rather than skipping it would still have to
        # find its way back out to classify this correctly.
        f.write("--\n-- Name: post_lo_marker; Type: TABLE; Schema: public; Owner: postgres\n--\n\n")
        f.write("CREATE TABLE public.post_lo_marker (id integer);\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="Output path for the generated dump")
    parser.add_argument(
        "--size-gb",
        type=float,
        default=3.0,
        help="Approximate output size in GiB (default: 3.0)",
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
