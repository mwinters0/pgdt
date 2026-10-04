#!/usr/bin/env python3
"""The cross-major differ: where two adjacent PostgreSQL majors disagree.

`docs/design/decisions.md`, "D70", is the description.
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
between majors is a rendering no single implementation can get right. The
strict reading is the conservative direction -- a false alarm, never a false
silence -- and a two-rejection SQLSTATE change is the one place it raises an
alarm that means nothing: a pair whose other literal a major started reading
fails on its first refused operand instead.

**A non-additive cell fails unless [`EXEMPT`] names it**, by a literal it
asks and the invariant recording why majors read that literal apart: the
narrowings the manual's "Where PostgreSQL majors differ" lists, where no
major's reading holds every other's and pgdt chooses one, row by row.

**Every transition answers to a row of that table, and every row is asked.**
[`MANUAL_ROWS`] says which literals ask each row; a literal whose acceptance
or spelling moves between two majors must be one of them, at the row's own
major, a comparison cell may move only with one of its two literals, and a
row no transition meets, or an exemption with no row, is stale. So a
regeneration moving an answer no row records fails, and so does a row added
to the manual with no case asking it.

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

#: The manual's table of what differs between majors, which every transition
#: answers to (`docs/design/roadmap.md`, "A literal is guaranteed in `*_out`'s
#: form and never read past `*_in`'s").
MANUAL = REPO / "docs" / "manual" / "type-handling.md"
MANUAL_HEADING = "## Where PostgreSQL majors differ"

#: Where an exemption's invariant is headed.
INVARIANTS = REPO / "docs" / "design" / "postgres-invariants.md"


@dataclass(frozen=True)
class ManualRow:
    """A row of the manual's table, and the oracle literals that ask it."""

    #: The row's "From" column: the newer major of the pair each literal moves
    #: between.
    since: str
    #: The leading words of its "What changed" cell, which name it.
    what: str
    #: `(type, literal)`, each a literal row that moves at `since`.
    asks: tuple[tuple[str, str | None], ...] = ()
    #: Types every literal of which moves at `since`: the type is new there.
    types: tuple[str, ...] = ()

    def claims(self, type: str, literal: str | None) -> bool:
        return type in self.types or (type, literal) in self.asks


#: Every row of the manual's "Where PostgreSQL majors differ", in its order.
MANUAL_ROWS: tuple[ManualRow, ...] = (
    ManualRow("14", "bare `numeric` holds `Infinity`",
              asks=(("numeric", "-Infinity"), ("numeric", "Infinity"))),
    ManualRow("14", "the multirange types exist",
              types=("int4multirange", "public.myrange_multi")),
    ManualRow("15", "a `numeric` scale may exceed its precision", types=("numeric(2,5)",)),
    ManualRow("15", "an `interval`'s time part reaches",
              asks=(("interval", "2147483648:00:00"),)),
    ManualRow("16", "an `oid` is read in hex",
              asks=(("oid", "010"), ("oid", "08"), ("oid", "0x1F"))),
    ManualRow("14", "a `line` given by two points",
              asks=(("line", "[(2,0),(3,1.7976931348623157e308)]"),
                    ("line", "[(Infinity,1),(0,2)]"))),
    ManualRow("16", "an `xid`, `xid8` or `cid` is refused",
              asks=(("xid", "abc"), ("xid8", "abc"), ("cid", "abc"))),
    ManualRow("16", "an integer or `numeric` may be written",
              asks=(("integer", "0x1F"), ("integer", "1_000"),
                    ("numeric", "0x1F"), ("numeric", "1_000"))),
    ManualRow("16", "a `numeric` exponent is read with no blank",
              asks=(("numeric", "1e 5"), ("numeric", "0e1073741823"))),
    ManualRow("15", "an `interval` part past `int32` is refused",
              asks=(("interval", "4294968 millennium"),)),
    ManualRow("16", "a date, time or timestamp no longer labels",
              asks=(("date", "y2001m02d04"),)),
    ManualRow("17", "`interval` holds `infinity`",
              asks=(("interval", "-infinity"), ("interval", "infinity"))),
    ManualRow("17", "an `interval` unit word with no number",
              asks=(("interval", "1 day h"),)),
    ManualRow("18", "a run-together time after `t`",
              asks=(("timestamp without time zone", "2020-01-01 t abcd-05"),
                    ("timestamp without time zone", "1999-12-30 995959"))),
)

#: The literals whose non-additive cells are exempt, each with the invariant
#: recording the narrowing. Three are read by every major from some major on
#: and exempt only for a pair whose other literal a major started reading,
#: which then fails on this one's SQLSTATE instead: `2147483648:00:00`,
#: `0e1073741823` and `[(Infinity,1),(0,2)]`.
EXEMPT: dict[tuple[str, str], str] = {
    ("oid", "010"): "I66",
    ("oid", "08"): "I66",
    ("line", "[(2,0),(3,1.7976931348623157e308)]"): "I74",
    ("line", "[(Infinity,1),(0,2)]"): "I74",
    ("xid", "abc"): "I79",
    ("xid8", "abc"): "I79",
    ("cid", "abc"): "I79",
    ("numeric", "1e 5"): "I82",
    ("numeric", "0e1073741823"): "I82",
    ("interval", "2147483648:00:00"): "I62",
    ("interval", "4294968 millennium"): "I84",
    ("interval", "1 day h"): "I84",
    ("date", "y2001m02d04"): "I83",
    ("timestamp without time zone", "2020-01-01 t abcd-05"): "I83",
}

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
    "collation",
    "field",
    "old",
    "new",
    "verdict",
]

#: Everything in `meta.tsv` that must be *identical* across majors for their
#: answers to be comparable at all -- the session GUCs `comparison_oracle`
#: pins, the database's collation, and the two keys that say what that
#: collation *is*: the platform triple and the collation's own version.
#: `server_version`, `server_version_num` and `version` are the ones expected
#: to differ, and `version` is why the triple is a key of its own -- a major
#: left on, or reverted to, a different base image would otherwise pass this
#: guard in silence while `datcollate` still read `en_US.utf8` and meant a
#: different order.
APPARATUS_KEYS = (
    "datcollate",
    "datctype",
    "default_collversion",
    "platform",
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
    #: The collation the comparison was qualified with, where the case named
    #: one; SQL NULL otherwise and for every literal row. It is part of the
    #: cell's identity: a text pair is asked under two collations, so
    #: `(type, left, right)` alone no longer names one cell.
    collation: str | None
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
            self.collation,
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


def exempt(d: "Difference", table: dict[tuple[str, str], str] = EXEMPT) -> bool:
    """Whether a non-additive cell asks a literal [`EXEMPT`] names."""
    return any((d.type, literal) in table for literal in (d.left, d.right))


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
        "comparisons.tsv": (co.comparison_cases(), 4, len(co.COMPARISON_COLUMNS)),
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
    first_op = len(co.COMPARISON_COLUMNS) - len(co.OPERATORS)
    for a, b in zip(old_rows, new_rows):
        for n, op in enumerate(co.OPERATORS):
            verdict = classify(a[first_op + n], b[first_op + n])
            if verdict:
                out.append(
                    Difference(
                        older, newer, "comparisons", a[0], a[1], a[2], a[3],
                        op, a[first_op + n], b[first_op + n], verdict,
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
                    older, newer, "literals", a[0], a[1], None, None,
                    "status", a[2], b[2], verdict,
                )
            )
        elif a[2] == "ok" and a[3] != b[3]:
            out.append(
                Difference(
                    older, newer, "literals", a[0], a[1], None, None,
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
# The manual's table
# --------------------------------------------------------------------------


def manual_table(text: str) -> list[tuple[str, str]]:
    """`(What changed, From)` for each row of the manual's table."""
    out: list[tuple[str, str]] = []
    lines = text.splitlines()
    if MANUAL_HEADING not in lines:
        return out
    for line in lines[lines.index(MANUAL_HEADING) + 1:]:
        if line.startswith("#"):
            break
        if not line.startswith("|"):
            if out:
                break
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 2 or cells[0] == "What changed" or set(cells[0]) <= {"-"}:
            continue
        out.append((cells[0], cells[1]))
    return out


def manual_problems(
    found: Sequence[Difference],
    manual_text: str,
    invariants_text: str,
    rows: Sequence[ManualRow] = MANUAL_ROWS,
    table: dict[tuple[str, str], str] = EXEMPT,
) -> list[str]:
    """Where the transitions, the exemptions and the manual's rows fail to
    answer to each other."""
    problems: list[str] = []
    manual = manual_table(manual_text)
    if not manual:
        problems.append(f"{MANUAL.name} has no table under {MANUAL_HEADING!r}")
    for what, since in manual:
        named = [row for row in rows if what.startswith(row.what)]
        if len(named) != 1:
            problems.append(
                f"the manual's row {what[:60]!r} is named by {len(named)} entries "
                "of MANUAL_ROWS — the oracle asks every row, by one entry"
            )
        elif named[0].since != since:
            problems.append(
                f"the manual's row {what[:60]!r} says {since} and MANUAL_ROWS {named[0].since}"
            )
    for row in rows:
        if not any(what.startswith(row.what) for what, _ in manual):
            problems.append(f"MANUAL_ROWS names {row.what!r}, which the manual has no row for")

    moved = {(d.older, d.newer, d.type, d.left) for d in found if d.file == "literals"}
    met: set[tuple[str, str | None]] = set()
    for d in found:
        if d.file == "literals":
            claims = [row for row in rows if row.claims(d.type, d.left)]
            if len(claims) != 1 or claims[0].since != d.newer:
                problems.append(f"no manual row records {describe(d)}")
                continue
            met.add((d.type, d.left))
        elif not any(
            (d.older, d.newer, d.type, literal) in moved for literal in (d.left, d.right)
        ):
            problems.append(
                f"{describe(d)}, with neither literal moving: a change in meaning no "
                "manual row records"
            )
    for row in rows:
        for case in row.asks:
            if case not in met:
                problems.append(f"{row.what!r} asks {case!r}, which moves at no major")
        for type in row.types:
            if not any(t == type for t, _ in met):
                problems.append(f"{row.what!r} names {type!r}, which moves at no major")

    for (type, literal), invariant in table.items():
        if not any(row.claims(type, literal) for row in rows):
            problems.append(f"EXEMPT names {type} {literal!r}, which no manual row asks")
        if not any(
            d.verdict == NON_ADDITIVE and d.type == type and literal in (d.left, d.right)
            for d in found
        ):
            problems.append(f"EXEMPT names {type} {literal!r}, which no non-additive cell asks")
        if f"## {invariant} — " not in invariants_text:
            problems.append(f"EXEMPT cites {invariant} for {type} {literal!r}, which is no invariant")
    return problems


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
        if row[-1] not in (ADDITIVE, NON_ADDITIVE):
            problems.append(f"{path}:{n} has an unknown verdict {row[-1]!r}")
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
    if d.collation is not None:
        where += f" COLLATE {d.collation}"
    return f"{d.older}→{d.newer} {d.file} {where}: {d.old!r} → {d.new!r} ({d.verdict})"


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def _counts(found: Iterable[Difference]) -> dict[tuple[str, str, str], list[int]]:
    """Additive, exempt and broken cells per pair and file."""
    out: dict[tuple[str, str, str], list[int]] = {}
    for d in found:
        cell = out.setdefault((d.older, d.newer, d.file), [0, 0, 0])
        cell[0 if d.verdict == ADDITIVE else 1 if exempt(d) else 2] += 1
    return out


def broken(found: Iterable[Difference]) -> list[Difference]:
    """The non-additive cells [`EXEMPT`] does not name."""
    return [d for d in found if d.verdict == NON_ADDITIVE and not exempt(d)]


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
            additive, exempted, unexempt = counts.get((older, newer, filename), [0, 0, 0])
            flag = "  <-- BREAK" if unexempt else ""
            print(
                f"  {older} → {newer}  {filename:<12} {additive:>4} additive, "
                f"{exempted:>3} exempt, {unexempt:>3} non-additive{flag}",
                file=out,
            )
    print(file=out)

    breaks = broken(found)
    if breaks:
        print(
            "Non-additive differences — two majors disagree about what the same "
            "input means, the union rule does not hold for them, and EXEMPT "
            "names no literal they ask:",
            file=out,
        )
        for d in breaks:
            print(f"  {describe(d)}", file=out)
        print(file=out)

    if problems:
        print("Problems:", file=out)
        for p in problems:
            print(f"  {p}", file=out)
    elif not breaks:
        print(
            "The committed file is current, every transition is a manual row's, "
            "and two majors disagree only where EXEMPT names the literal.",
            file=out,
        )


def check(
    fixtures: Path = FIXTURES,
    path: Path = DIFFERENCES,
    write: bool = False,
    out=sys.stdout,
    manual: Path | None = MANUAL,
) -> int:
    """`manual` is the table every transition answers to; `None` checks a
    tree no manual describes, as the unit tests' synthetic ones."""
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
    if manual is not None:
        problems += manual_problems(found, manual.read_text(), INVARIANTS.read_text())

    report(found, problems, fixtures, out=out)
    return 1 if problems or broken(found) else 0


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
