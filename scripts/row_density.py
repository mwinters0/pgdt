#!/usr/bin/env python3
"""A dump's row density, per `COPY` block, at every candidate group size.

Backs `docs/design/decisions.md`, "D82": the rule a block's group size
is chosen by reads a **distribution** of rows per group, and this is that
distribution measured on a real dump before the rule is written. What it
produces is a fact about the input, written to a `runs/` artifact -- not a
`docs/design/measurements.md` figure, so it has no median, no apparatus gate
and no staleness edge.

**The input is `pgdq info --json`**, whose blocks carry their statistics as the
cache holds them. Every group records its rows (`RowGroup::rows`) and groups
are byte ranges from the block's start, so the rows per group at `2N` are the
sums of adjacent pairs at `N`, a trailing odd group standing alone; the
distribution at every `N * 2^k` follows from the gathered one, down to the
single group holding the whole block. A group no row starts in is a group all
the same -- it carries an entry per tracked column -- so empty groups are in
the distribution. A `parse` that states no minimum coarsens a block of sparse
rows before it is cached, so the gathering run a reading reads states
`--statistics-min-rows 0`.

**A quantile is nearest-rank**: the `ceil(q * G)`-th smallest of `G` groups.
**The minimum's median is the exception**: it reads the *upper* middle group,
the `floor(G/2) + 1`-th smallest, which is what makes the predicate monotone in
size and so lets a block the length cap has already coarsened read its minimum
from the size the cap left ("Granularity follows row density"). The bound holds
under either: where that group holds at least `m` rows, `ceil(G/2)` groups do,
so a block of `R` rows holds at most `2R/m` groups at the size the minimum
chooses, whatever its distribution -- with equality where exactly half the
groups hold `m` and the rest none.

**The registered criterion** ("Granularity follows row density"): a quantile
lower than the median is chosen only if a koji block of reasonable width comes
close to that bound. Reasonable width is the spec's own judgement,
`REASONABLE_ROW_BYTES`, measured as **the median group's bytes over its rows at
the gathered size** -- the nearest-rank median by rows, ties broken by bytes --
and never as the block's mean row: a few wide rows lift a mean past the
judgement while the median group stays dense, so a mean would keep out exactly
the skewed block the criterion looks for. A median group no row starts in has no
width a row could have, being inside rows longer than the group, and is not of
reasonable width. "Close" is `CLOSE_RATIO` of the bound; both were set before
koji was read. A block of uniform density that coarsens at all lands
between a quarter and a half of it -- the size before the chosen one held under
`m` a group, the chosen one under `2m` -- and one already dense at the gathered
size lower still, so a ratio past `CLOSE_RATIO` is skew no uniform block
reaches. A block whose rows never reach `m` even as one group is reported and
kept out of the criterion: its bound is under two groups, and no size is
chosen by the minimum there at all.

**`select`** answers the other half of the reading's recipe: the
`--statistics` selection tracking one narrow column per table, so a gathering
`parse` counts every block's groups for a fraction of full gathering's memory.
It reads the tables' declared columns from `info --json` of any cache holding
the preamble -- a `parse --preamble-only` is enough -- and picks each table's
narrowest fixed-width column, its first column where it has none. Which column
is tracked moves no group's row count; it moves only what the run holds.

Usage:

    cd scripts
    uv run row_density.py select PREAMBLE_INFO.json         # prints the selection
    uv run row_density.py density INFO.json [...] --out runs/<dir>/density.json
    uv run python -m unittest test_row_density
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

#: The density minimum's default, in rows: `2^20` stays right for rows up to
#: `REASONABLE_ROW_BYTES`, so `2^20 / 2^10` rows a group.
#: `pgdump_query::statistics::DEFAULT_STATISTICS_MIN_ROWS`, mirrored, and held
#: to the library's by a test; `pgdq` chooses a block's size by the same rule
#: as `choose` at the median.
DEFAULT_MIN_ROWS = 1024

#: The spec's width judgement: a block whose median group at the gathered size
#: holds rows of at most this many bytes each is of reasonable width.
REASONABLE_ROW_BYTES = 1024

#: How near the `2R/m` bound a block's groups at the median's size must come to
#: count as "close" under the registered criterion. Past a half, which is the
#: most a block of uniform density reaches.
CLOSE_RATIO = 0.75

#: The quantile the minimum is registered at -- read as the upper middle group,
#: not nearest-rank -- and the lower ones reported beside it so a reader of a
#: failed criterion sees what each would retain.
MIN_QUANTILE = 0.5
REPORTED_QUANTILES = (0.5, 0.25, 0.1)

#: The distribution's summary at each size.
DISTRIBUTION_QUANTILES = (0.1, 0.25, 0.5, 0.75, 0.9)

#: Declared types of a fixed width, in bytes, as `pg_dump` spells them. Only
#: the ranking matters; `select` falls back to a table's first column.
FIXED_WIDTH = {
    "boolean": 1,
    '"char"': 1,
    "smallint": 2,
    "integer": 4,
    "real": 4,
    "date": 4,
    "oid": 4,
    "bigint": 8,
    "double precision": 8,
    "money": 8,
    "time without time zone": 8,
    "timestamp without time zone": 8,
    "timestamp with time zone": 8,
    "time with time zone": 12,
    "interval": 16,
    "uuid": 16,
}


def quantile(sorted_values: Sequence[int], q: float) -> int:
    """The nearest-rank `q`-quantile of a non-empty ascending sequence."""
    if not sorted_values:
        raise ValueError("no groups to take a quantile of")
    rank = max(1, math.ceil(q * len(sorted_values)))
    return sorted_values[rank - 1]


def min_group(sorted_values: Sequence[int], q: float) -> int:
    """The group the minimum's predicate reads at quantile `q`: the upper
    middle group -- the `floor(G/2) + 1`-th smallest, so at most half the
    groups fall short -- at the registered median, and the nearest-rank
    quantile at any lower one reported beside it."""
    if not sorted_values:
        raise ValueError("no groups to read a minimum against")
    if q == MIN_QUANTILE:
        return sorted_values[len(sorted_values) // 2]
    return quantile(sorted_values, q)


def coarsen(rows: Sequence[int]) -> list[int]:
    """Rows per group at twice the size: adjacent pairs summed from the block's
    start, a trailing odd group standing alone."""
    return [sum(rows[i : i + 2]) for i in range(0, len(rows), 2)]


def ladder(rows: Sequence[int]) -> list[list[int]]:
    """Rows per group at every size from the gathered one to a single group."""
    if not rows:
        return []
    sizes = [list(rows)]
    while len(sizes[-1]) > 1:
        sizes.append(coarsen(sizes[-1]))
    return sizes


@dataclass(frozen=True)
class Choice:
    """The size a minimum at one quantile chooses for a block."""

    quantile: float
    #: `k`, the chosen size being the gathered one times `2^k`.
    doublings: int
    groups: int
    #: Whether the quantile group reaches the minimum at that size; false only
    #: where it does not even as one group, and then the choice is that group.
    reaches: bool


def choose(sizes: Sequence[Sequence[int]], min_rows: int, q: float) -> Choice:
    """The smallest size at which the `q`-quantile group ([`min_group`]) holds
    at least `min_rows` rows, or the single group where none does. `pgdq`
    chooses a block's size by this rule, from whatever size the length cap left
    it; `pgdump_query::gather::density_merges`, mirrored."""
    for k, rows in enumerate(sizes):
        if min_group(sorted(rows), q) >= min_rows:
            return Choice(q, k, len(rows), True)
    return Choice(q, len(sizes) - 1, len(sizes[-1]), False)


def distribution(rows: Sequence[int]) -> dict:
    ordered = sorted(rows)
    out = {"groups": len(ordered), "min": ordered[0], "max": ordered[-1]}
    for q in DISTRIBUTION_QUANTILES:
        out[f"p{round(q * 100)}"] = quantile(ordered, q)
    out["mean"] = sum(ordered) / len(ordered)
    return out


def median_group_width(groups: Sequence[dict]) -> float | None:
    """Bytes per row in the nearest-rank median group by rows, ties broken by
    bytes; `None` where that group holds no row."""
    ordered = sorted(groups, key=lambda g: (g["rows"], g["bytes"]))
    group = ordered[max(1, math.ceil(MIN_QUANTILE * len(ordered))) - 1]
    return group["bytes"] / group["rows"] if group["rows"] else None


def copy_blocks(info: dict) -> Iterable[dict]:
    """Every `COPY` block in an `info --json` document, in file order."""
    for span in info.get("spans", []):
        body = span.get("body")
        if isinstance(body, dict):
            data = body.get("Data")
            if isinstance(data, dict) and "Copy" in data:
                yield data["Copy"]


def qualified(block: dict) -> str:
    header = block["header"]
    name = f"{header['schema']}.{header['table']}" if header.get("schema") else header["table"]
    return f"{block['database']}:{name}" if block.get("database") else name


def block_density(block: dict, min_rows: int) -> dict:
    """One block's row density: its distribution at every size and what the
    minimum chooses at each reported quantile."""
    stats = block["statistics"]
    rows = [group["rows"] for group in stats["groups"]]
    total = sum(rows)
    data_bytes = block["terminator_offset"] - block["data_offset"]
    sizes = ladder(rows)
    choices = [choose(sizes, min_rows, q) for q in REPORTED_QUANTILES]
    median = next(c for c in choices if c.quantile == MIN_QUANTILE)
    bound = 2 * total / min_rows if min_rows else math.inf
    ratio = median.groups / bound if median.reaches and bound else None
    mean_row = data_bytes / total if total else None
    width = median_group_width(stats["groups"])
    reasonable = width is not None and width <= REASONABLE_ROW_BYTES
    return {
        "table": qualified(block),
        "header_offset": block["header_offset"],
        "rows": total,
        "data_bytes": data_bytes,
        "mean_row_bytes": mean_row,
        "median_group_row_bytes": width,
        "reasonable_width": reasonable,
        "group_size": stats["group_size"],
        "choices": [
            {
                "quantile": c.quantile,
                "group_size": stats["group_size"] << c.doublings,
                "groups": c.groups,
                "reaches_minimum": c.reaches,
            }
            for c in choices
        ],
        "bound_groups": bound,
        "bound_ratio": ratio,
        "close": reasonable and ratio is not None and ratio >= CLOSE_RATIO,
        "sizes": [
            {"group_size": stats["group_size"] << k, **distribution(level)}
            for k, level in enumerate(sizes)
        ],
    }


def density(info: dict, min_rows: int) -> dict:
    """Every block's density in one `info --json` document, with the blocks
    that hold rows and no statistics listed rather than dropped."""
    blocks, untracked = [], []
    for block in copy_blocks(info):
        stats = block.get("statistics")
        if stats is None or not stats.get("groups"):
            if block.get("row_count"):
                untracked.append(qualified(block))
            continue
        blocks.append(block_density(block, min_rows))
    return {"min_rows": min_rows, "blocks": blocks, "untracked": untracked}


def verdict(blocks: Sequence[dict]) -> str:
    close = [b for b in blocks if b["close"]]
    judged = [b for b in blocks if b["reasonable_width"] and b["bound_ratio"] is not None]
    if not judged:
        return "criterion: no block of reasonable width reaches the minimum; nothing to judge"
    worst = max(judged, key=lambda b: b["bound_ratio"])
    if close:
        names = ", ".join(f"{b['table']} ({b['bound_ratio']:.3f})" for b in close)
        return f"criterion: MET, a lower quantile is in question -- close to the bound: {names}"
    return (
        f"criterion: not met, the median stands -- {len(judged)} block(s) of reasonable width, "
        f"the nearest the bound {worst['table']} at {worst['bound_ratio']:.3f} (close is {CLOSE_RATIO})"
    )


def render(source: str, result: dict) -> str:
    lines = [f"# {source}", f"minimum {result['min_rows']} rows at the median; close at {CLOSE_RATIO} of 2R/m"]
    header = (
        f"{'table':<44} {'rows':>14} {'MiB':>10} {'B/row':>8} {'p50 B/r':>8} {'N':>8} {'G@N':>8} "
        f"{'p50 size':>9} {'G':>7} {'2R/m':>12} {'ratio':>6} {'p25 G':>7} {'p10 G':>7}"
    )
    lines.append(header)
    for b in result["blocks"]:
        by_q = {c["quantile"]: c for c in b["choices"]}
        median = by_q[MIN_QUANTILE]
        size = f"2^{int(math.log2(median['group_size']))}" + ("" if median["reaches_minimum"] else "*")
        ratio = "-" if b["bound_ratio"] is None else f"{b['bound_ratio']:.3f}"
        mean = "-" if b["mean_row_bytes"] is None else f"{b['mean_row_bytes']:.0f}"
        width = "-" if b["median_group_row_bytes"] is None else f"{b['median_group_row_bytes']:.0f}"
        mark = "!" if b["close"] else ("" if b["reasonable_width"] else "~")
        lines.append(
            f"{(b['table'] + mark)[:44]:<44} {b['rows']:>14} {b['data_bytes'] / 2**20:>10.1f} {mean:>8} {width:>8} "
            f"{b['group_size']:>8} {b['sizes'][0]['groups']:>8} {size:>9} {median['groups']:>7} "
            f"{b['bound_groups']:>12.1f} {ratio:>6} {by_q[0.25]['groups']:>7} {by_q[0.1]['groups']:>7}"
        )
    lines.append("B/row the block's mean, p50 B/r its median group's at N (the width judged); "
                 "~ wider than reasonable, outside the criterion; ! close; * never reaches the minimum")
    if result["untracked"]:
        lines.append(f"untracked: {len(result['untracked'])} block(s) with rows and no statistics: "
                     + ", ".join(result["untracked"]))
    lines.append(verdict(result["blocks"]))
    return "\n".join(lines)


def narrowest(columns: Sequence[dict]) -> dict:
    """A table's narrowest fixed-width column, the first in DDL order among
    equals; its first column where none has a fixed width."""
    fixed = [(FIXED_WIDTH[c["declared_type"]], i) for i, c in enumerate(columns)
             if c.get("declared_type") in FIXED_WIDTH]
    return columns[min(fixed)[1]] if fixed else columns[0]


def selection(info: dict) -> str:
    """The `--statistics` value tracking one narrow column of every table the
    preamble declares."""
    targets = []
    for database in (info.get("metadata") or {}).get("databases", []):
        for table, columns in database.get("tables", {}).items():
            if not columns:
                continue
            column = narrowest(columns)["name"]
            parts = table.split(".")
            if len(parts) != 2 or "." in column or "," in column or "," in table:
                raise ValueError(f"{table}.{column} cannot be named in a --statistics selection")
            targets.append(f"{table}.{column}")
    if not targets:
        raise ValueError("the document declares no table; is it an index with its preamble read?")
    return ",".join(sorted(set(targets)))


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sel = sub.add_parser("select", help="print a --statistics selection of one narrow column a table")
    sel.add_argument("info", type=Path, help="`pgdq info --json` of a cache holding the preamble")
    den = sub.add_parser("density", help="each block's rows per group at every size")
    den.add_argument("info", type=Path, nargs="+", help="`pgdq info --json` output")
    den.add_argument("--min-rows", type=int, default=DEFAULT_MIN_ROWS)
    den.add_argument("--out", type=Path, help="write every distribution here as JSON")
    args = parser.parse_args(argv)

    if args.command == "select":
        print(selection(json.loads(args.info.read_text())))
        return 0
    results = {}
    for path in args.info:
        with path.open() as f:
            result = density(json.load(f), args.min_rows)
        results[str(path)] = result
        print(render(str(path), result))
        print()
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(results, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
