#!/usr/bin/env python3
"""Unit tests for comparison_oracle.py, run with
`uv run python -m unittest test_comparison_oracle`.

Stdlib `unittest`, no dependency added, same as test_measure.py.

Two halves, and the second is the one that decays. The unit tests below pin
the parts a silent mistake would corrupt without failing anything -- a
duplicated case, a mis-escaped literal, a `parse_tsv` that loses a tab. The
structural tests at the bottom run over the **real** `fixtures/` tree and
assert that every committed file still lines up with the case table, row for
row and in order: that is the only thing standing between an edited case table
and answer files that silently describe different questions.

The cross-major differ is `oracle_differences.py`, with its own tests: it reads
these same files but answers a different question, and its committed artifact
is one file for the whole tree rather than one per major. What is *not* in
either is the register-to-oracle reconciliation, which needs the L2 comparison
register to exist first.
"""

from __future__ import annotations

import unittest
from pathlib import Path

import comparison_oracle as co

REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURES = REPO_ROOT / "fixtures"


class CaseTable(unittest.TestCase):
    def test_each_type_and_collation_appears_once(self):
        # A type may appear more than once only by naming a different
        # collation: two entries with the same pair would ask the server the
        # same question twice and align the differ on rows that mean the same
        # thing while a real case went missing.
        keys = [(case.type, case.collation or "") for case in co.TYPE_CASES]
        self.assertEqual(sorted(keys), sorted(set(keys)))

    def test_a_literal_asked_under_two_collations_is_asked_once(self):
        # Parsing and output are collation-blind, so the second collation's
        # literals would be the same rows again -- and `literals.tsv` carries
        # no collation column to tell them apart. Deduplicating rather than
        # skipping a collated case is what keeps a value that exists only
        # there, such as `character(10)`'s tab, in the file.
        types = [
            co.TypeCases("text", ("A", "a"), collation="C"),
            co.TypeCases("text", ("A", "z"), collation="default"),
        ]
        original = co.TYPE_CASES
        try:
            co.TYPE_CASES = types
            self.assertEqual(
                co.literal_cases(), [("text", "A"), ("text", "a"), ("text", "z")]
            )
            self.assertEqual(len(co.comparison_cases()), 8)
        finally:
            co.TYPE_CASES = original

    def test_both_collations_are_asked_over_the_same_alphabet(self):
        # The register's text row has two halves -- bytewise agrees under `C`
        # and does not under a libc locale -- and they are only comparable if
        # the same pairs are asked under both.
        by_collation = {
            case.collation: case.values
            for case in co.TYPE_CASES
            if case.collation is not None
        }
        self.assertEqual(sorted(by_collation), sorted(co.COLLATIONS))
        self.assertEqual(len(set(by_collation.values())), 1)

    def test_every_type_has_at_least_one_value(self):
        for case in co.TYPE_CASES:
            self.assertTrue(case.values, f"{case.type} has no values")

    def test_no_duplicate_comparison_case(self):
        # A duplicate is not an error the server would report: it answers
        # twice, identically, and the differ then aligns two files on rows
        # that mean the same thing while a real case is missing.
        cases = co.comparison_cases()
        self.assertEqual(len(cases), len(set(cases)))

    def test_no_duplicate_literal_case(self):
        cases = co.literal_cases()
        self.assertEqual(len(cases), len(set(cases)))

    def test_a_literal_in_both_values_and_inputs_is_asked_once(self):
        types = [co.TypeCases("integer", ("1", "2"), ("1", "3"))]
        original = co.TYPE_CASES
        try:
            co.TYPE_CASES = types
            self.assertEqual(
                co.literal_cases(),
                [("integer", "1"), ("integer", "2"), ("integer", "3")],
            )
        finally:
            co.TYPE_CASES = original

    def test_null_is_last_in_every_values_list(self):
        # Not cosmetic: `inputs` are compared against `values[0]`, so a NULL
        # first would make every malformed literal's comparison `u` and record
        # nothing about whether the server accepted it.
        for case in co.TYPE_CASES:
            if None in case.values:
                self.assertIsNone(case.values[-1], case.type)
                self.assertEqual(case.values.count(None), 1, case.type)


class SqlBuilding(unittest.TestCase):
    def test_none_is_sql_null(self):
        self.assertEqual(co.sql_literal(None), "NULL")

    def test_quote_is_doubled(self):
        self.assertEqual(co.sql_literal("has'quote"), "'has''quote'")

    def test_backslash_is_left_alone(self):
        # Only sound because the session block pins
        # `standard_conforming_strings`; a bytea or array case would otherwise
        # be silently re-escaped.
        self.assertEqual(co.sql_literal(r"\xdeadbeef"), r"'\xdeadbeef'")
        self.assertIn("standard_conforming_strings = on", co.SESSION_SQL)

    def test_comparison_script_asks_every_operator_once_per_row(self):
        script = co.comparisons_script()
        for op in co.OPERATORS:
            self.assertIn(f"pg_temp.pgdq_cmp(c.ty, c.l, c.r, '{op}', c.coll)", script)

    def test_a_collation_reaches_the_comparison_as_a_quoted_identifier(self):
        # `quote_ident` rather than interpolation: `default` is a reserved
        # word and `C` is upper-case, so both need quoting and neither may be
        # spelled by hand in two places.
        self.assertIn("quote_ident(coll)", co.FUNCTIONS_SQL)
        self.assertIn("' COLLATE '", co.FUNCTIONS_SQL)

    def test_scripts_are_ordered_and_complete(self):
        for build in co.SCRIPTS.values():
            script = build()
            self.assertIn("TO STDOUT;", script)
        for build in (co.literals_script, co.comparisons_script):
            self.assertIn("ORDER BY c.o", build())


class CopyTextParsing(unittest.TestCase):
    def test_null_field(self):
        self.assertIsNone(co.unescape_copy_text(r"\N"))

    def test_escapes(self):
        self.assertEqual(co.unescape_copy_text(r"a\tb\nc\\d"), "a\tb\nc\\d")

    def test_rows_split_on_tabs(self):
        rows = co.parse_tsv("a\tb\t\\N\nc\td\te\n")
        self.assertEqual(rows, [["a", "b", None], ["c", "d", "e"]])


def oracle_versions() -> list[str]:
    return sorted(
        p.name
        for p in FIXTURES.iterdir()
        if p.is_dir() and (p / co.ORACLE_DIRNAME).is_dir()
    )


class CommittedTree(unittest.TestCase):
    """The committed answer files against the case table that asked for them.

    This is the check that keeps an edited case table from landing beside
    stale answers: the file carries no case identifiers of its own, so its
    rows mean what they mean *only* by being in the case table's order.
    """

    def test_every_version_directory_has_an_oracle(self):
        versions = sorted(p.name for p in FIXTURES.iterdir() if p.is_dir())
        self.assertEqual(versions, oracle_versions())
        self.assertTrue(versions, "no fixture versions found -- did the tree move?")

    def test_every_oracle_has_all_three_files(self):
        for version in oracle_versions():
            for filename in co.SCRIPTS:
                path = FIXTURES / version / co.ORACLE_DIRNAME / filename
                self.assertTrue(path.is_file(), path)

    def test_comparisons_match_the_case_table_row_for_row(self):
        cases = co.comparison_cases()
        width = len(co.COMPARISON_COLUMNS)
        for version in oracle_versions():
            path = FIXTURES / version / co.ORACLE_DIRNAME / "comparisons.tsv"
            rows = co.parse_tsv(path.read_text())
            self.assertEqual(len(rows), len(cases), path)
            for row, case in zip(rows, cases):
                self.assertEqual(len(row), width, (path, row))
                self.assertEqual(tuple(row[:4]), case, path)

    def test_literals_match_the_case_table_row_for_row(self):
        cases = co.literal_cases()
        width = len(co.LITERAL_COLUMNS)
        for version in oracle_versions():
            path = FIXTURES / version / co.ORACLE_DIRNAME / "literals.tsv"
            rows = co.parse_tsv(path.read_text())
            self.assertEqual(len(rows), len(cases), path)
            for row, case in zip(rows, cases):
                self.assertEqual(len(row), width, (path, row))
                self.assertEqual(tuple(row[:2]), case, path)

    def test_every_comparison_cell_is_a_legal_outcome(self):
        for version in oracle_versions():
            path = FIXTURES / version / co.ORACLE_DIRNAME / "comparisons.tsv"
            first_op = len(co.COMPARISON_COLUMNS) - len(co.OPERATORS)
            for row in co.parse_tsv(path.read_text()):
                for cell in row[first_op:]:
                    self.assertTrue(
                        cell in ("t", "f", "u") or self.is_sqlstate(cell),
                        (path, row[:3], cell),
                    )

    def test_every_literal_status_is_ok_or_a_sqlstate(self):
        for version in oracle_versions():
            path = FIXTURES / version / co.ORACLE_DIRNAME / "literals.tsv"
            for row in co.parse_tsv(path.read_text()):
                status, output = row[2], row[3]
                self.assertTrue(
                    status == "ok" or self.is_sqlstate(status), (path, row)
                )
                if status != "ok":
                    self.assertIsNone(output, (path, row))

    def test_meta_records_the_apparatus(self):
        for version in oracle_versions():
            path = FIXTURES / version / co.ORACLE_DIRNAME / "meta.tsv"
            meta = dict(tuple(row) for row in co.parse_tsv(path.read_text()))
            self.assertEqual(meta["server_version"].split(".")[0], version)
            # The collation and the libc behind it are what say how far the
            # text answers can be trusted -- see comparison_oracle's docstring.
            for key in (
                "datcollate",
                "default_collversion",
                "platform",
                "version",
                "DateStyle",
                "TimeZone",
            ):
                self.assertIn(key, meta)

    def test_every_oracle_was_taken_on_the_same_libc(self):
        # The text answers are glibc's by choice, and that is only true while
        # every major's container is the Debian one -- `datcollate` reads
        # `en_US.utf8` on musl too and means a different order there.
        for version in oracle_versions():
            path = FIXTURES / version / co.ORACLE_DIRNAME / "meta.tsv"
            meta = dict(tuple(row) for row in co.parse_tsv(path.read_text()))
            self.assertEqual(meta["platform"], "x86_64-pc-linux-gnu", path)

    @staticmethod
    def is_sqlstate(cell: str | None) -> bool:
        return bool(cell) and cell[0] == "E" and len(cell) == 6 and cell[1:].isalnum()


if __name__ == "__main__":
    unittest.main()
