#!/usr/bin/env python3
"""Generate a synthetic pg_dump-shaped dump with a chosen number of COPY blocks.

Backs docs/design/measurements.md ("Per-block cache saving is quadratic in
block count"): `pgdt parse` serializes the *whole* structure cache at a
completed block, and the cache grows with the block count, so the total cost of
saving is O(blocks^2) in a regime no real sample here can reach -- koji is 784
GB across 74 blocks. This generator supplies the other regime: block-rich and
byte-poor, the shape a schema with thousands of tables (or one partitioned
table with a daily leaf over a decade) actually has.

Parameterized by *block count*, which is why it is its own script rather than a
section in generate_perf_data.py: that one is parameterized by size, and the
figure this backs is blind to size.

Unlike generate_fixtures.py, this is *not* a correctness fixture -- its output
is never checked against real pg_dump, only shaped closely enough to satisfy
this codebase's own grammar (a schema section of `CREATE TABLE`s under TOC
comments, then a data section of `COPY ... FROM stdin;` blocks in the same
order, which is how pg_dump orders a plain dump).

Generated, never committed: point --out somewhere outside the repo (the SSD
volume -- see CLAUDE.local.md) for a real measurement run, or under the
gitignored runs/ directory for routine use.
"""

from __future__ import annotations

import argparse
from pathlib import Path

HEADER = """\
--
-- PostgreSQL database dump
--

-- Dumped from database version 16.15
-- Dumped by pg_dump version 16.15

SET statement_timeout = 0;
SET client_encoding = 'UTF8';
SELECT pg_catalog.set_config('search_path', '', false);
SET row_security = off;

"""

DDL = """\
--
-- Name: t{i}; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t{i} (
    id integer,
    name text,
    created_at timestamp without time zone,
    amount numeric(12,2)
);


ALTER TABLE public.t{i} OWNER TO postgres;

"""

DATA = """\
--
-- Data for Name: t{i}; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t{i} (id, name, created_at, amount) FROM stdin;
1\trow one\t2026-01-01 00:00:00\t1.00
2\trow two\t2026-01-02 00:00:00\t2.00
3\trow three\t2026-01-03 00:00:00\t3.00
\\.


"""

FOOTER = """\
--
-- PostgreSQL database dump complete
--

"""


def generate(out: Path, blocks: int) -> None:
    with out.open("w") as f:
        f.write(HEADER)
        # Schema section first, then the data section -- pg_dump's own order,
        # and the one that makes every block's DDL already mapped by the time
        # its COPY block is reached.
        for i in range(blocks):
            f.write(DDL.format(i=i))
        for i in range(blocks):
            f.write(DATA.format(i=i))
        f.write(FOOTER)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--blocks", type=int, required=True, help="Number of COPY blocks (one table each)"
    )
    parser.add_argument(
        "--out", type=Path, required=True, help="Output path for the generated dump"
    )
    args = parser.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    generate(args.out, args.blocks)
    print(f"wrote {args.out} ({args.blocks} blocks, {args.out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
