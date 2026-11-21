"""The numbers a table prints: medians, spreads, and their formatting into the doc's units.
The part a silent error would hurt most."""

from __future__ import annotations

import statistics
from typing import Sequence

from measurement import config


def median(values: Sequence[float]) -> float:
    if not values:
        raise ValueError("median of no readings")
    return statistics.median(values)


def spread(values: Sequence[float]) -> tuple[float, float]:
    if not values:
        raise ValueError("spread of no readings")
    return (min(values), max(values))


def resolved_above(high: Sequence[float], low: Sequence[float]) -> bool:
    """Whether `high`'s readings lie wholly above `low`'s — its fewest above
    the other's most. The register's one test of a gap's being **resolved**
    rather than two spreads meeting (`downstream_verdict`, `--count-pair`)."""
    return spread(high)[0] > spread(low)[1]


def fmt_s(value: float) -> str:
    """Seconds, at the precision measurements.md quotes: three decimals below
    two seconds, two above -- the image's timer resolves a millisecond
    (`TIME_FORMAT`), which is the most any table quotes."""
    return f"{value:.3f}" if abs(value) < 2 else f"{value:.2f}"


def fmt_median_spread(values: Sequence[float]) -> str:
    lo, hi = spread(values)
    return f"**{fmt_s(median(values))} s** ({fmt_s(lo)}–{fmt_s(hi)})"


def fmt_rate(nbytes: int, seconds: float) -> str:
    """Decimal MB/s, which is what every rate in measurements.md is."""
    return f"~{nbytes / seconds / 1e6:.0f} MB/s"


def fmt_delta(off: float, on: float) -> str:
    """The Δ column of a two-binary comparison: absolute then percent."""
    d = on - off
    return f"**{d:+.3f} s, {d / off * 100:+.0f}%**"


def fmt_readings(values: Sequence[float]) -> str:
    return ", ".join(fmt_s(v) for v in values)


def fmt_us_per_row(seconds: float, rows: int) -> str:
    return f"{seconds / rows * 1e6:+.2f}"


def fmt_mib(kib: float) -> str:
    """A resident-set reading, in MiB.

    Mebibytes rather than decimal megabytes because the only thing this figure
    is read against is a bound stated in them -- `4 x max(8 MiB, announced)`
    from the buffer pool -- and a claim compared against a bound must not
    change units on the way."""
    return f"{kib / 1024:.2f} MiB"


def fmt_mib_median_spread(values: Sequence[float]) -> str:
    lo, hi = spread(values)
    return f"**{fmt_mib(median(values))}** ({lo / 1024:.2f}–{hi / 1024:.2f})"


def fmt_rss_delta(kib: float) -> str:
    """A *difference* between two resident-set readings.

    Two scales, because this figure's two axes are three orders of magnitude
    apart: the per-byte difference is tens of kibibytes, which MiB would round
    to `0.00`, and the per-block one is tens of mebibytes, which KiB would
    print as a five-digit number nobody can read against the table above it."""
    if abs(kib) >= 1024:
        return f"{kib / 1024:+.2f} MiB"
    return f"{kib:+.0f} KiB"


def md_table(headers: Sequence[str], rows: Sequence[Sequence[str]]) -> str:
    """A GitHub-flavoured table in measurements.md's own style: no padding,
    one `---` per column."""
    out = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for row in rows:
        if len(row) != len(headers):
            raise ValueError(f"row has {len(row)} cells, table has {len(headers)} columns")
        out.append("| " + " | ".join(row) + " |")
    return "\n".join(out)


def _fmt_bytes(n: int) -> str:
    # Three scales, because `peak-rss` puts a 2 MB input and a 3.00 GiB one in
    # adjacent rows and "3072.0 MB" is not the size anybody asked for.
    if n >= config.GIB:
        return f"{n / config.GIB:.2f} GiB"
    if n >= config.MIB:
        return f"{n / config.MIB:.1f} MB"
    return f"{n / 1024:.0f} KB"


def fmt_instructions(values: Sequence[int]) -> str:
    """A count's median and spread, in billions: a leg retires billions, and
    the spread across reps is in the third decimal."""
    lo, hi = spread(values)
    return f"**{median(values) / 1e9:.3f} G** ({lo / 1e9:.3f}–{hi / 1e9:.3f})"


def _fmt_budget_bytes(n: int) -> str:
    """A budget a *run* resolved for itself, in MiB to one place.

    `_fmt_budget` is for a number somebody typed and refuses anything but a
    whole mebibyte, which is the right refusal there. A discovered budget is
    `readers × what one reader holds` and is almost never whole — 195.1 MiB is
    three readers of a 24 MiB-block file — so it is formatted rather than
    checked."""
    return f"{n / config.MIB:.1f} MiB"


def _fmt_ns(ns: float) -> str:
    return f"{ns / 1000:.2f} µs" if ns >= 1000 else f"{ns:.0f} ns"
