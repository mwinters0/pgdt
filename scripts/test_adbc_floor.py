#!/usr/bin/env python3
"""The committed ADBC floor oracles, checked for the properties a reader relies
on. No container and no driver: this reads `fixtures/<major>/adbc/floor.tsv`.

What it does *not* check is the pin -- that the driver version recorded here
equals `scripts/pyproject.toml`'s -- which belongs to the reconciliation that
joins this file against our own mapping, per the phase spec's D8.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

import adbc_floor
from generate_fixtures import ROUTINE_VERSIONS

FIXTURES_DIR = Path(__file__).resolve().parent.parent / "fixtures"

STATUS_RE = re.compile(r"\A(ok|E[0-9A-Z]{5})\Z")


def majors() -> list[str]:
    return sorted(ROUTINE_VERSIONS, key=int)


class FloorFilesTest(unittest.TestCase):
    def setUp(self) -> None:
        self.floors = {v: adbc_floor.read_floor(FIXTURES_DIR, v) for v in majors()}

    def test_every_routine_major_has_a_floor(self) -> None:
        for version, rows in self.floors.items():
            self.assertTrue(rows, f"fixtures/{version}/adbc/floor.tsv is empty")

    def test_one_driver_version_across_every_row(self) -> None:
        """A floor is a claim about one driver release (the spec's D1), so a
        file holding two versions is a half-regenerated sweep."""
        seen = {row["driver"] for rows in self.floors.values() for row in rows}
        self.assertEqual(len(seen), 1, f"floor.tsv rows disagree on the driver: {sorted(seen)}")

    def test_each_row_names_its_own_major(self) -> None:
        for version, rows in self.floors.items():
            for row in rows:
                self.assertEqual(row["server"], version, f"{row['declared']} in {version}")

    def test_rows_are_sorted_and_unique_by_declared_type(self) -> None:
        """Code-point order, written by the sweep rather than by the server, so
        a regeneration diff is the type set moving and never a collation."""
        for version, rows in self.floors.items():
            declared = [row["declared"] for row in rows]
            self.assertEqual(declared, sorted(declared), f"{version}: not in code-point order")
            self.assertEqual(len(declared), len(set(declared)), f"{version}: duplicate type")

    def test_a_cell_is_answered_or_refused_and_never_both(self) -> None:
        for version, rows in self.floors.items():
            for row in rows:
                where = f"{version}/{row['declared']}"
                self.assertRegex(row["status"], STATUS_RE, where)
                if row["status"] == "ok":
                    self.assertIsNotNone(row["arrow"], f"{where}: ok with no Arrow type")
                else:
                    self.assertIsNone(row["arrow"], f"{where}: refused, yet an Arrow type")
                    self.assertIsNone(row["extension"], f"{where}: refused, yet an extension")

    def test_the_type_set_is_additive_across_majors(self) -> None:
        """A declarable type present at one major is present at the next.

        True of 13-18 -- the six multirange types and the two BRIN summary
        types arrive at 14 and nothing goes -- and the same property
        `oracle_differences.py` checks for the comparison oracle. A failure is
        a major that *removed* a declarable type, which is a thing to read
        rather than a thing to re-baseline: every floor row for it, and every
        stance resting on it, has just stopped being about anything.
        """
        ordered = majors()
        for older, newer in zip(ordered, ordered[1:], strict=False):
            gone = {r["declared"] for r in self.floors[older]} - {
                r["declared"] for r in self.floors[newer]
            }
            self.assertEqual(gone, set(), f"types present at {older} and absent at {newer}")


if __name__ == "__main__":
    unittest.main()
