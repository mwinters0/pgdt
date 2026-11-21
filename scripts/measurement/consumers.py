"""Consumers: who repeats a figure, computed from who names it."""

from __future__ import annotations

import functools
import re
from typing import TYPE_CHECKING

from measurement import config
from measurement import machine_state
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.figures.declaration import Figure


#: Where a consumer can live. Documents, the crates and the scripts that
#: are not this harness -- a figure's numbers are quoted in prose and its
#: claims are asserted in doc comments, and both of those are in here.
CONSUMER_ROOTS: tuple[tuple[str, str], ...] = (
    ("docs", "**/*.md"),
    ("crates", "**/*.rs"),
    ("scripts", "*.py"),
)

#: Top-level documents, named rather than globbed: the working tree also holds
#: `CLAUDE.local.md`, which is nobody's checkout but this one's.
CONSUMER_FILES: tuple[str, ...] = ("README.md", "CONTRIBUTING.md", "CLAUDE.md")

#: Not consumers, however often they name a figure.
#:
#: `measurements.md` is where the table *is*, so listing it would make every
#: fold-in look like a cross-document edit. The register and its tests are the
#: declaration itself. And `docs/status/history/` is dated: an entry states
#: what was true on its day and is never revised, which is the same exemption
#: `scripts/citations.py` grants a closed day — a fold-in that re-read one
#: could only make it untrue.
CONSUMER_SKIP: tuple[str, ...] = (
    "docs/design/measurements.md",
    "docs/status/history/",
    "scripts/measure.py",
    "scripts/measurement/",
    "scripts/acknowledged.py",
)

#: How a document addresses a figure: the id in backticks, a trailing `*`
#: standing for a family of them (`scan-throughput-*`), or the marker spelling
#: the doc itself uses.
_FIGURE_MENTION = re.compile(r"`([a-z0-9][a-z0-9-]*\*?)`|figure: ([a-z0-9-]+)")


def consumer_files() -> list[str]:
    """Every repo-relative path the consumer scan reads, sorted."""
    found: set[str] = set()
    for root, pattern in CONSUMER_ROOTS:
        for path in (config.REPO / root).glob(pattern):
            found.add(str(path.relative_to(config.REPO)))
    for name in CONSUMER_FILES:
        if (config.REPO / name).exists():
            found.add(name)
    return sorted(p for p in found if not p.startswith(CONSUMER_SKIP))


@functools.lru_cache(maxsize=1)
def _mentions_by_file() -> tuple[tuple[str, frozenset[str]], ...]:
    """Each scanned file, and the figure names it spells."""
    out = []
    for path in consumer_files():
        try:
            text = (config.REPO / path).read_text(errors="replace")
        except OSError:
            continue
        names = {a or b for a, b in _FIGURE_MENTION.findall(text)}
        if names:
            out.append((path, frozenset(names)))
    return tuple(out)


def consumers(fig: Figure) -> tuple[str, ...]:
    """What a moved figure obliges a fold-in to re-read.

    **Computed from who names the figure, not declared beside it.** `depends`
    is the edge into a figure; this is the edge out, and it used to be a tuple
    per `Figure` that only a session noticing a stale citation ever corrected
    — so retargeting a document left every tuple naming the old one. A
    consumer that cites `` `peak-rss` `` or `<!-- figure: peak-rss -->` says so
    in its own text, and a glob (`` `scan-throughput-*` ``) reaches the family
    it spells.

    `also_quoted_by` is the residue: a consumer that repeats the *numbers*
    without naming the figure, which is what the manual and the README do —
    they state the claim to a reader who will never see a figure id, and no
    scan can find them."""
    named = sorted(
        path
        for path, names in _mentions_by_file()
        if any(n == fig.id or (n.endswith("*") and fig.id.startswith(n[:-1])) for n in names)
    )
    return tuple(dict.fromkeys((*named, *fig.also_quoted_by)))


def registered_regimes() -> tuple[str, ...]:
    """Every regime the registered figures are taken in, read off their own
    `stage` declarations.

    One list, not two. The regimes a sweep actually runs in are already stated
    per figure -- `--stage` selects on them -- so anything that has to iterate
    them derives the set from there rather than repeating it. The tuple that
    used to be written out by hand omitted `warm-parallel` from the day it was
    added, which is the failure a second list has every time: the copy nobody
    has to touch to add a regime is the copy that goes stale.
    """
    return tuple(
        sorted(
            {
                token
                for fig in register.EVERY_FIGURE
                for token in fig.stage.split("+")
                if token not in machine_state.NON_REGIME_STAGES
            }
        )
    )
