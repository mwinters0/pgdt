#!/usr/bin/env python3
"""The cross-major differ: where two adjacent PostgreSQL majors disagree.

`docs/design/architecture.md`, "The comparison oracle", is the description.
This module is the classification and the committed file.

**What it exists to check.** Version-varying semantics are implemented as the
newest semantics unconditionally, with no branch on the version the dump
header records, because an older server cannot have produced a value it does
not accept. That union rule is safe exactly where the difference between two
majors is *which values can exist*; it stops being safe the moment two majors
disagree about what the same text **means**. So the condition is checked rather
than asserted:

| Older major | Newer major | Verdict |
|---|---|---|
| rejects the input | accepts, answers | **additive** -- the value could not previously exist |
| accepts | accepts, **different** answer | **non-additive** -- a real break |
| accepts | accepts, same answer | unchanged, and not recorded |

Every other transition is **non-additive** too, deliberately, and there are
three of them: a newer major *rejecting* what an older accepted (the union rule
inverted, and worse), two rejections whose SQLSTATE changed, and an accepted
literal whose canonical output spelling moved. The last is the one
`comparisons.tsv` cannot see on its own: the canonicalize-the-literal-once path
renders a literal into the form the *file* holds, so a spelling that differs
between majors is a rendering no single implementation can get right. None of
the three exists across 13-18 today. The strict reading is the conservative
direction -- a false alarm, never a false silence -- and a two-rejection
SQLSTATE change is the one place it could raise an alarm that means nothing.

**Adjacent pairs, not all pairs.** If any two majors disagree then some
adjacent pair does, so a chain is complete, and it says *where* the transition
happened rather than only that one exists.

**The differences that exist are committed**, in `fixtures/oracle-differences.tsv`,
and `test_oracle_differences.py` asserts the file against a fresh computation.
A regeneration that moves an answer therefore cannot land without the file
being re-filed, which is the discipline `docs/design/measurements.md` uses for
its tables. This is not the same check as "no non-additive difference exists":
that one is asserted separately, so a real break is loud in the suite even
after someone files it.

Usage:

    cd scripts
    uv run oracle_differences.py            # check the committed file
    uv run oracle_differences.py --write    # re-file it after a regeneration
    uv run python -m unittest test_oracle_differences
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

import comparison_oracle as co

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "fixtures"

#: The committed record. It sits at the top of `fixtures/` rather than under a
#: version, because it is the one oracle artifact that belongs to no single
#: major. A file there is invisible to both fixture walks -- Rust's
#: `all_fixtures()` recurses into directories only, and
#: `test_comparison_oracle.py`'s version list does the same.
DIFFERENCES = FIXTURES / "oracle-differences.tsv"

ADDITIVE = "additive"
NON_ADDITIVE = "non-additive"

#: The differences file's columns. Unlike the oracle files, this one carries a
#: header row: the server writes those and we write this, it is the artifact a
#: reviewer reads in a diff, and ten positional columns are too many to hold.
COLUMNS = [
    "older",
    "newer",
    "file",
    "type",
    "left",
    "right",
    "field",
    "old",
    "new",
    "verdict",
]

#: Everything in `meta.tsv` that must be *identical* across majors for their
#: answers to be comparable at all -- the session GUCs `comparison_oracle`
#: pins, plus the database's collation. `server_version`, `server_version_num`
#: and `version` are the ones expected to differ.
APPARATUS_KEYS = (
    "datcollate",
    "datctype",
    "DateStyle",
    "IntervalStyle",
    "TimeZone",
    "extra_float_digits",
    "bytea_output",
    "array_nulls",
    "client_encoding",
)


@dataclass(frozen=True)
class Difference:
    """One cell that moved between two adjacent majors."""

    older: str
    newer: str
    #: `comparisons` or `literals` -- the oracle file it was found in.
    file: str
    type: str
    #: The left literal of a comparison, or the literal of a literal row.
    left: str | None
    #: The right literal of a comparison; SQL NULL for a literal row.
    right: str | None
    #: The operator for a comparison; `status` or `output` for a literal row.
    field: str
    old: str | None
    new: str | None
    verdict: str

    def row(self) -> list[str | None]:
        return [
            self.older,
            self.newer,
            self.file,
            self.type,
            self.left,
            self.right,
            self.field,
            self.old,
            self.new,
            self.verdict,
        ]

    @staticmethod
    def of_row(row: Sequence[str | None]) -> "Difference":
        return Difference(*row)  # type: ignore[arg-type]


def is_rejection(cell: str | None) -> bool:
    """Whether a cell records that the server refused the input.

    `E` plus a five-character SQLSTATE, which is the only shape either helper
    in `comparison_oracle` returns for a refusal. An answer is `t`, `f` or `u`.
    """
    return bool(cell) and cell[0] == "E" and len(cell) == 6


def classify(old: str | None, new: str | None) -> str | None:
    """The verdict for one cell, or `None` where nothing moved."""
    if old == new:
        return None
    if is_rejection(old) and not is_rejection(new):
        return ADDITIVE
    return NON_ADDITIVE


# --------------------------------------------------------------------------
# Reading the committed tree
# --------------------------------------------------------------------------


def majors(fixtures: Path = FIXTURES) -> list[str]:
    """Every major present in `fixtures/`, oldest first.

    Sorted numerically, because the chain of adjacent pairs is what the differ
    walks and `"9"` must not sort after `"13"` if this tree ever holds one.
    """
    return sorted(
        (p.name for p in fixtures.iterdir() if p.is_dir()),
        key=lambda name: int(name) if name.isdigit() else 0,
    )


def oracle_path(version: str, filename: str, fixtures: Path = FIXTURES) -> Path:
    return fixtures / version / co.ORACLE_DIRNAME / filename


def load(version: str, filename: str, fixtures: Path = FIXTURES):
    return co.parse_tsv(oracle_path(version, filename, fixtures).read_text())


def alignment_problems(fixtures: Path = FIXTURES) -> list[str]:
    """Everything that must hold before two majors' files may be zipped.

    The answer files carry **no case identifiers**: a row means what it means
    only by sitting at the case table's index. Zipping mis-aligned files is a
    silent wrong answer rather than an error, so the differ re-checks the
    alignment itself rather than inheriting `test_comparison_oracle.py`'s --
    the two run at different moments and only one of them is a prerequisite
    for this.
    """
    problems: list[str] = []
    versions = majors(fixtures)
    if len(versions) < 2:
        problems.append(
            f"{len(versions)} major(s) in {fixtures} — a cross-major diff needs two"
        )
    for version in versions:
        for filename in co.SCRIPTS:
            path = oracle_path(version, filename, fixtures)
            if not path.is_file():
                problems.append(f"{version} has no {filename} — regenerate its oracle")
    if problems:
        return problems

    expected = {
        "comparisons.tsv": (co.comparison_cases(), 3, len(co.COMPARISON_COLUMNS)),
        "literals.tsv": (co.literal_cases(), 2, len(co.LITERAL_COLUMNS)),
    }
    for version in versions:
        for filename, (cases, keys, width) in expected.items():
            rows = load(version, filename, fixtures)
            if len(rows) != len(cases):
                problems.append(
                    f"{version}/{filename} has {len(rows)} rows and the case table "
                    f"asks {len(cases)} — regenerate it"
                )
                continue
            for n, (row, case) in enumerate(zip(rows, cases), 1):
                if len(row) != width:
                    problems.append(f"{version}/{filename}:{n} has {len(row)} columns")
                    break
                if tuple(row[:keys]) != case:
                    problems.append(
                        f"{version}/{filename}:{n} asks {tuple(row[:keys])!r} where "
                        f"the case table asks {case!r} — the files are mis-aligned"
                    )
                    break
    return problems


def apparatus_problems(fixtures: Path = FIXTURES) -> list[str]:
    """Whether every major's oracle was taken under the same apparatus.

    A `DateStyle` or a collation that differed between two runs would move
    every spelling in the file, and the differ would report a hundred breaks
    for one unpinned session. That is a fault in the generation, not a
    difference between majors, so it is reported as a problem and not as a row.
    """
    problems: list[str] = []
    versions = majors(fixtures)
    metas = {
        version: dict(
            (row[0], row[1]) for row in load(version, "meta.tsv", fixtures)
        )
        for version in versions
    }
    baseline = versions[0]
    for key in APPARATUS_KEYS:
        want = metas[baseline].get(key)
        for version in versions[1:]:
            got = metas[version].get(key)
            if got != want:
                problems.append(
                    f"{key} is {want!r} on {baseline} and {got!r} on {version} — "
                    "the two oracles are not comparable"
                )
    return problems


# --------------------------------------------------------------------------
# The diff
# --------------------------------------------------------------------------


def diff_pair(older: str, newer: str, fixtures: Path = FIXTURES) -> list[Difference]:
    """Every cell that moved between two adjacent majors, in file order."""
    out: list[Difference] = []

    old_rows = load(older, "comparisons.tsv", fixtures)
    new_rows = load(newer, "comparisons.tsv", fixtures)
    for a, b in zip(old_rows, new_rows):
        for n, op in enumerate(co.OPERATORS):
            verdict = classify(a[3 + n], b[3 + n])
            if verdict:
                out.append(
                    Difference(
                        older, newer, "comparisons", a[0], a[1], a[2],
                        op, a[3 + n], b[3 + n], verdict,
                    )
                )

    old_rows = load(older, "literals.tsv", fixtures)
    new_rows = load(newer, "literals.tsv", fixtures)
    for a, b in zip(old_rows, new_rows):
        verdict = classify(a[2], b[2])
        if verdict:
            # The output column moves with the status -- a rejected literal has
            # no output at all -- so an accepted/rejected transition is one
            # difference, not two.
            out.append(
                Difference(
                    older, newer, "literals", a[0], a[1], None,
                    "status", a[2], b[2], verdict,
                )
            )
        elif a[2] == "ok" and a[3] != b[3]:
            out.append(
                Difference(
                    older, newer, "literals", a[0], a[1], None,
                    "output", a[3], b[3], NON_ADDITIVE,
                )
            )
    return out


def differences(fixtures: Path = FIXTURES) -> list[Difference]:
    """The whole chain, oldest pair first."""
    versions = majors(fixtures)
    out: list[Difference] = []
    for older, newer in zip(versions, versions[1:]):
        out.extend(diff_pair(older, newer, fixtures))
    return out


# --------------------------------------------------------------------------
# The committed file
# --------------------------------------------------------------------------


def render(found: Sequence[Difference]) -> str:
    return co.format_tsv([list(COLUMNS)] + [d.row() for d in found])


def read_committed(path: Path = DIFFERENCES) -> tuple[list[Difference], list[str]]:
    """The committed file, and what is wrong with it structurally."""
    if not path.is_file():
        return [], [f"{path} does not exist — run with --write"]
    rows = co.parse_tsv(path.read_text())
    if not rows or rows[0] != COLUMNS:
        return [], [f"{path} does not open with the column header"]
    out: list[Difference] = []
    problems: list[str] = []
    for n, row in enumerate(rows[1:], 2):
        if len(row) != len(COLUMNS):
            problems.append(f"{path}:{n} has {len(row)} columns, expected {len(COLUMNS)}")
            continue
        if row[9] not in (ADDITIVE, NON_ADDITIVE):
            problems.append(f"{path}:{n} has an unknown verdict {row[9]!r}")
            continue
        out.append(Difference.of_row(row))
    return out, problems


def compare_to_committed(
    found: Sequence[Difference], committed: Sequence[Difference], path: Path
) -> list[str]:
    """Where the committed file and a fresh computation disagree."""
    if list(found) == list(committed):
        return []
    problems = [
        f"{path} is stale: it holds {len(committed)} differences and the oracle "
        f"now shows {len(found)} — re-file it with `uv run oracle_differences.py --write`"
    ]
    found_set, committed_set = set(found), set(committed)
    for d in found:
        if d not in committed_set:
            problems.append(f"  not filed: {describe(d)}")
    for d in committed:
        if d not in found_set:
            problems.append(f"  filed and gone: {describe(d)}")
    if not problems[1:]:
        problems.append("  the same differences, in a different order")
    return problems


def describe(d: Difference) -> str:
    where = f"{d.type} {d.left!r}" + (f" {d.field} {d.right!r}" if d.right is not None else f" {d.field}")
    return f"{d.older}→{d.newer} {d.file} {where}: {d.old!r} → {d.new!r} ({d.verdict})"


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def _counts(found: Iterable[Difference]) -> dict[tuple[str, str, str], list[int]]:
    out: dict[tuple[str, str, str], list[int]] = {}
    for d in found:
        cell = out.setdefault((d.older, d.newer, d.file), [0, 0])
        cell[0 if d.verdict == ADDITIVE else 1] += 1
    return out


def report(
    found: Sequence[Difference],
    problems: Sequence[str],
    fixtures: Path = FIXTURES,
    out=sys.stdout,
) -> None:
    versions = majors(fixtures)
    counts = _counts(found)
    print(
        f"{len(versions)} majors ({', '.join(versions)}), "
        f"{max(len(versions) - 1, 0)} adjacent pairs, {len(found)} differences.\n",
        file=out,
    )
    for older, newer in zip(versions, versions[1:]):
        for filename in ("comparisons", "literals"):
            additive, non_additive = counts.get((older, newer, filename), [0, 0])
            flag = "  <-- BREAK" if non_additive else ""
            print(
                f"  {older} → {newer}  {filename:<12} "
                f"{additive:>4} additive, {non_additive:>3} non-additive{flag}",
                file=out,
            )
    print(file=out)

    broken = [d for d in found if d.verdict == NON_ADDITIVE]
    if broken:
        print(
            "Non-additive differences — two majors disagree about what the same "
            "input means, and the union rule does not hold for them:",
            file=out,
        )
        for d in broken:
            print(f"  {describe(d)}", file=out)
        print(file=out)

    if problems:
        print("Problems:", file=out)
        for p in problems:
            print(f"  {p}", file=out)
    elif not broken:
        print(
            "The committed file is current, and no two majors disagree about an "
            "input both accept.",
            file=out,
        )


def check(
    fixtures: Path = FIXTURES,
    path: Path = DIFFERENCES,
    write: bool = False,
    out=sys.stdout,
) -> int:
    problems = alignment_problems(fixtures)
    if problems:
        report([], problems, fixtures, out=out)
        return 1
    problems = apparatus_problems(fixtures)

    found = differences(fixtures)
    if write:
        path.write_text(render(found))
        print(f"wrote {len(found)} differences to {path}\n", file=out)
    else:
        committed, structural = read_committed(path)
        problems += structural
        if not structural:
            problems += compare_to_committed(found, committed, path)

    report(found, problems, fixtures, out=out)
    broken = any(d.verdict == NON_ADDITIVE for d in found)
    return 1 if problems or broken else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="re-file fixtures/oracle-differences.tsv from the committed oracles",
    )
    parser.add_argument(
        "--fixtures", type=Path, default=FIXTURES, help="fixture root (tests use this)"
    )
    parser.add_argument(
        "--differences", type=Path, default=None, help="the committed file to check"
    )
    args = parser.parse_args(argv)
    path = args.differences or (args.fixtures / DIFFERENCES.name)
    return check(args.fixtures, path, write=args.write)


if __name__ == "__main__":
    raise SystemExit(main())
