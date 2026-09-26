#!/usr/bin/env python3
"""Generate the input `dynamic-filter-join` and `dynamic-filter-topk` are
taken on: a probe table, the perf generator's control rows with two keys
appended, and three small build tables to join it against.

Backs docs/design/measurements.md's two figures pricing what DataFusion's
dynamic filters buy a query over `datafusion-cli-pgdump`
(`docs/design/roadmap-P27-dynamic-filters.md`, "Evidence"). Every byte of a
probe row but the two appended fields is the control's own
(`generate_perf_data.random_row`, same seed, same draws).

**The probe's three keys are the three shapes a join's filter meets.**

- `id`, from the control: ascending, so a join's bounds over it rule out
  every row group but the few holding the build side's run — the input
  where group pruning pays.
- `u_key`: `id` scrambled by a 32-bit multiplicative bijection, so every
  value is distinct and no row group's bounds narrow anything — the input
  where group pruning helps nothing and dropping a row before its other
  columns are decoded is all a filter can buy.
- `bucket`: `(id - 1) % BUCKETS`, every value in every group, and the
  `every` build table holding all of them — the input where the filter
  rejects nothing, so whatever it costs is pure overhead. `BUCKETS` is the
  largest `IN` list a hash join publishes, so the overhead priced is the
  most one can cost.

**The build tables are written after the probe**, because which rows they
select depends on how many rows the probe reached: `near` is `BUILD_ROWS`
consecutive ids at the middle of the table, and `scattered` the `u_key`s of
`BUILD_ROWS` ids spread evenly over it. Each join therefore matches exactly
`BUILD_ROWS` probe rows whatever `--size-mb` states. All DDL comes first, as
`pg_dump` writes it.

The TopK figure orders by `u_key`: unsorted, with no ties.

Like generate_perf_data.py, this is *not* a correctness fixture, and its output
is generated, never committed.
"""

from __future__ import annotations

import argparse
import random
from pathlib import Path

import generate_perf_data as perf

#: The appended columns, in order, after the control's.
COLUMNS = (("u_key", "bigint"), ("bucket", "integer"))
#: The multiplier of `u_key`'s bijection: odd, so invertible modulo 2^32.
SCRAMBLE = 0x9E3779B1
#: How many distinct `bucket`s there are: DataFusion 55's
#: `hash_join_inlist_pushdown_max_distinct_values` default, the most an `IN`
#: list a join publishes may hold.
BUCKETS = 150
#: The rows of `near` and of `scattered`, and so what each of their joins
#: matches: under `BUCKETS`, so each is published as an `IN` list too.
BUILD_ROWS = 100
#: The three build tables, each a single key column: `near` joins `id`,
#: `scattered` joins `u_key`, `every` joins `bucket`.
BUILD_TABLES = (
    ("public.near", "integer"),
    ("public.scattered", "bigint"),
    ("public.every", "integer"),
)


def u_key(row_id: int) -> int:
    return (row_id * SCRAMBLE) % (1 << 32)


def bucket(row_id: int) -> int:
    return (row_id - 1) % BUCKETS


def near_ids(rows: int) -> list[int]:
    """`BUILD_ROWS` consecutive ids at the middle of a probe `rows` long."""
    start = max(1, rows // 2 - BUILD_ROWS // 2)
    return list(range(start, min(rows, start + BUILD_ROWS - 1) + 1))


def scattered_ids(rows: int) -> list[int]:
    """`BUILD_ROWS` ids spread evenly over a probe `rows` long."""
    return sorted({1 + (rows * (2 * j + 1)) // (2 * BUILD_ROWS) for j in range(BUILD_ROWS)})


def generate(out: Path, size_bytes: int, seed: int | None) -> None:
    rng = random.Random(seed)
    columns = [*perf.COLUMNS, *COLUMNS]
    col_decl = ",\n    ".join(f"{name} {typ}" for name, typ in columns)
    col_names = ", ".join(name for name, _ in columns)
    with out.open("w") as f:
        f.write(f"CREATE TABLE {perf.TABLE} (\n    {col_decl}\n);\n\n")
        for table, typ in BUILD_TABLES:
            f.write(f"CREATE TABLE {table} (\n    k {typ}\n);\n\n")
        f.write(f"COPY {perf.TABLE} ({col_names}) FROM stdin;\n")
        written = 0
        row_id = 0
        while written < size_bytes:
            row_id += 1
            row = perf.random_row(rng, row_id, arrays=False, composites=False)
            line = "\t".join([*row, str(u_key(row_id)), str(bucket(row_id))]) + "\n"
            f.write(line)
            written += len(line)
        f.write("\\.\n\n")
        keys = (
            near_ids(row_id),
            [u_key(i) for i in scattered_ids(row_id)],
            list(range(BUCKETS)),
        )
        for (table, _), values in zip(BUILD_TABLES, keys):
            f.write(f"COPY {table} (k) FROM stdin;\n")
            f.writelines(f"{v}\n" for v in values)
            f.write("\\.\n\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="Output path for the generated dump")
    parser.add_argument(
        "--size-mb", type=float, default=256, help="Approximate probe size in MiB (default: 256)"
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
