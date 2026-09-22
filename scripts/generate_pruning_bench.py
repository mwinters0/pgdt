#!/usr/bin/env python3
"""Generate the input `statistics-pruning` is taken on: the perf generator's
control rows with one low-cardinality column appended.

Backs docs/design/measurements.md's `statistics-pruning` figure, which prices
what row-group statistics buy a `pgdt query` under a selective range on a
sorted column and an equality answered by a dictionary, and what they cost
under a filter they cannot narrow. The control already carries the first and
the third -- its `id` ascends from 1 in one block, and its `v_smallint` is
drawn uniformly per row -- so this generator adds only the second, and every
other byte of a row is the control's own (`generate_perf_data.random_row`,
same seed, same draws).

**The column is `text`, so a dictionary is the only statistic that can prune
it.** The generated file declares no collation, and text under anything but
`C`/`POSIX` is bounded bytewise, which `pgdt query`'s PostgreSQL semantics
never reads; an integer label would be pruned by its bounds as well, and the
leg would stop pricing a dictionary.

**Its values arrive in runs**, `RUN_ROWS` rows of one label, the labels cycling
through `LABELS` in order. A label drawn per row would put every label in every
row group, where a dictionary proves nothing absent and the leg would price
consulting statistics that skip nothing. Runs are the shape a low-cardinality
column takes in a heap-order dump of rows loaded in batches -- a tenant, an
import, a status set together -- and no claim is made that every such column
looks like this: the figure's table prints the groups the note says were
skipped, so what the shape bought is read beside the timing rather than
assumed.

Like generate_perf_data.py, this is *not* a correctness fixture, and its output
is generated, never committed.
"""

from __future__ import annotations

import argparse
import random
from pathlib import Path

import generate_perf_data as perf

#: The appended column. Last, so the control's columns keep their positions.
COLUMN = ("v_category", "text")
#: How many distinct labels the column holds. Well past a group's 64-entry
#: dictionary cap in the file as a whole, and one or two per row group.
LABELS = 256
#: Rows per run of one label: about four of the shipped 1 MiB row groups at the
#: control's row width.
RUN_ROWS = 1000


def label(index: int) -> str:
    return f"category-{index:03d}"


def category(row_id: int) -> str:
    """The label row `row_id` (from 1, as `id` counts) carries."""
    return label(((row_id - 1) // RUN_ROWS) % LABELS)


def generate(out: Path, size_bytes: int, seed: int | None) -> None:
    rng = random.Random(seed)
    columns = [*perf.COLUMNS, COLUMN]
    col_decl = ",\n    ".join(f"{name} {typ}" for name, typ in columns)
    col_names = ", ".join(name for name, _ in columns)
    with out.open("w") as f:
        f.write(f"CREATE TABLE {perf.TABLE} (\n    {col_decl}\n);\n\n")
        f.write(f"COPY {perf.TABLE} ({col_names}) FROM stdin;\n")
        written = 0
        row_id = 0
        while written < size_bytes:
            row_id += 1
            row = perf.random_row(rng, row_id, arrays=False, composites=False)
            line = "\t".join([*row, perf.encode_field(category(row_id))]) + "\n"
            f.write(line)
            written += len(line)
        f.write("\\.\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="Output path for the generated dump")
    parser.add_argument(
        "--size-mb", type=float, default=256, help="Approximate output size in MiB (default: 256)"
    )
    parser.add_argument(
        "--seed", type=int, default=None, help="RNG seed; a seeded run is byte-for-byte reproducible"
    )
    args = parser.parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    generate(args.out, int(args.size_mb * 1024 * 1024), args.seed)
    print(f"wrote {args.out} ({args.out.stat().st_size / (1024 * 1024):.1f} MiB)")


if __name__ == "__main__":
    main()
