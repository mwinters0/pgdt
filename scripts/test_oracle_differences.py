#!/usr/bin/env python3
"""Unit tests for oracle_differences.py, run with
`uv run python -m unittest test_oracle_differences`.

Stdlib `unittest`, no dependency added, same as test_measure.py.

Two halves, as in test_comparison_oracle.py. The unit tests below pin the
classification and the file's round trip against a synthetic tree, so a change
to the verdict rule fails here rather than in a diff of five hundred committed
rows. The `CommittedTree` cases at the bottom run over the **real** `fixtures/`
tree and are the suite assertion: the committed differences file is what a
fresh computation produces, no difference in it is non-additive but where
`EXEMPT` names its literal, and every transition is a row of the manual's
"Where PostgreSQL majors differ", every row asked.

The first two are deliberately separate assertions. A non-additive difference
is a real break in the union rule, so filing it must not silence the alarm;
only naming its literal, beside the invariant recording the narrowing, does.
"""

from __future__ import annotations

import io
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import comparison_oracle as co
import oracle_differences as od

REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURES = REPO_ROOT / "fixtures"


class Classification(unittest.TestCase):
    def test_unchanged_is_not_a_difference(self):
        self.assertIsNone(od.classify("t", "t"))
        self.assertIsNone(od.classify("E42704", "E42704"))
        self.assertIsNone(od.classify(None, None))

    def test_rejection_to_answer_is_additive(self):
        for answer in ("t", "f", "u"):
            self.assertEqual(od.classify("E42704", answer), od.ADDITIVE)

    def test_answer_to_answer_is_non_additive(self):
        self.assertEqual(od.classify("t", "f"), od.NON_ADDITIVE)
        self.assertEqual(od.classify("u", "t"), od.NON_ADDITIVE)

    def test_answer_to_rejection_is_non_additive(self):
        # The union rule inverted: a newer major refusing what an older one
        # accepted means a dump in hand can hold a value we would refuse.
        self.assertEqual(od.classify("t", "E22P02"), od.NON_ADDITIVE)

    def test_a_changed_sqlstate_is_non_additive(self):
        # Both majors reject, so nothing a dump can hold has moved -- but the
        # rule is strict in the conservative direction and this is the one
        # place that could raise an alarm meaning nothing. It has never fired.
        self.assertEqual(od.classify("E42704", "E42883"), od.NON_ADDITIVE)

    def test_a_sqlstate_is_e_plus_five(self):
        self.assertTrue(od.is_rejection("E42704"))
        self.assertFalse(od.is_rejection("t"))
        self.assertFalse(od.is_rejection("E4270"))
        self.assertFalse(od.is_rejection(None))


class SyntheticTree(unittest.TestCase):
    """A two-major tree built from the real case table, then perturbed.

    Building it from `comparison_cases()` rather than from a handful of
    invented rows is what lets the alignment guard be tested at all: the guard
    compares against that table, so a fake tree has to satisfy it before a
    perturbation means anything.
    """

    def setUp(self):
        self.tmp = TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)
        self.first_op = len(co.COMPARISON_COLUMNS) - len(co.OPERATORS)
        self.comparisons = [
            [t, l, r, c, *(["t"] * len(co.OPERATORS))]
            for t, l, r, c in co.comparison_cases()
        ]
        self.literals = [[t, lit, "ok", "out"] for t, lit in co.literal_cases()]
        for version in ("13", "14"):
            self.write(version)

    def write(self, version: str, comparisons=None, literals=None, meta=None):
        out = self.root / version / co.ORACLE_DIRNAME
        out.mkdir(parents=True, exist_ok=True)
        (out / "comparisons.tsv").write_text(
            co.format_tsv(comparisons or self.comparisons)
        )
        (out / "literals.tsv").write_text(co.format_tsv(literals or self.literals))
        (out / "meta.tsv").write_text(
            co.format_tsv(
                meta
                or [
                    ["server_version", f"{version}.0"],
                    *([k, "pinned"] for k in od.APPARATUS_KEYS),
                ]
            )
        )

    def test_an_identical_pair_has_no_differences(self):
        self.assertEqual(od.alignment_problems(self.root), [])
        self.assertEqual(od.apparatus_problems(self.root), [])
        self.assertEqual(od.differences(self.root), [])

    def test_one_moved_comparison_cell(self):
        rows = [list(row) for row in self.comparisons]
        rows[0][self.first_op] = "f"
        self.write("14", comparisons=rows)
        found = od.differences(self.root)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].file, "comparisons")
        self.assertEqual(found[0].field, co.OPERATORS[0])
        self.assertEqual((found[0].old, found[0].new), ("t", "f"))
        self.assertEqual(found[0].verdict, od.NON_ADDITIVE)

    def test_a_literal_that_starts_being_accepted_is_one_difference_not_two(self):
        # Its `output` moves from SQL NULL to text in the same breath, and
        # reporting that as a second row would double every additive
        # transition the tree actually holds.
        older = [list(row) for row in self.literals]
        older[0][2], older[0][3] = "E42704", None
        self.write("13", literals=older)
        found = od.differences(self.root)
        self.assertEqual(len(found), 1)
        self.assertEqual((found[0].field, found[0].verdict), ("status", od.ADDITIVE))

    def test_an_output_spelling_change_is_non_additive(self):
        # What `comparisons.tsv` cannot see: both majors accept, both agree on
        # every comparison, and the canonical form the file would hold moved.
        rows = [list(row) for row in self.literals]
        rows[0][3] = "elsewhere"
        self.write("14", literals=rows)
        found = od.differences(self.root)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].field, "output")
        self.assertEqual(found[0].verdict, od.NON_ADDITIVE)

    def test_a_collated_cell_carries_its_collation(self):
        # A text pair is asked under two collations, so `(type, left, right)`
        # no longer names one cell: the difference has to say which.
        rows = [list(row) for row in self.comparisons]
        n = next(
            i for i, row in enumerate(self.comparisons) if row[3] is not None
        )
        rows[n][self.first_op] = "f"
        self.write("14", comparisons=rows)
        found = od.differences(self.root)
        self.assertEqual(len(found), 1)
        self.assertIn(found[0].collation, co.COLLATIONS)
        self.assertIn(f"COLLATE {found[0].collation}", od.describe(found[0]))

    def test_a_mis_aligned_file_is_refused_rather_than_zipped(self):
        rows = [list(row) for row in self.comparisons]
        rows[0][1] = "something else"
        self.write("14", comparisons=rows)
        problems = od.alignment_problems(self.root)
        self.assertTrue(any("mis-aligned" in p for p in problems), problems)

    def test_a_short_file_is_refused(self):
        self.write("14", comparisons=self.comparisons[:-1])
        problems = od.alignment_problems(self.root)
        self.assertTrue(any("case table asks" in p for p in problems), problems)

    def test_a_missing_file_is_named(self):
        (self.root / "14" / co.ORACLE_DIRNAME / "literals.tsv").unlink()
        problems = od.alignment_problems(self.root)
        self.assertTrue(any("has no literals.tsv" in p for p in problems), problems)

    def test_one_major_is_not_a_diff(self):
        with TemporaryDirectory() as lone:
            root = Path(lone)
            (root / "13").mkdir()
            problems = od.alignment_problems(root)
            self.assertTrue(any("needs two" in p for p in problems), problems)

    def test_an_unpinned_session_is_a_problem_not_a_difference(self):
        self.write(
            "14",
            meta=[
                ["server_version", "14.0"],
                *([k, "elsewhere" if k == "DateStyle" else "pinned"] for k in od.APPARATUS_KEYS),
            ],
        )
        problems = od.apparatus_problems(self.root)
        self.assertTrue(any("not comparable" in p for p in problems), problems)
        self.assertEqual(od.differences(self.root), [])

    def test_the_committed_file_round_trips(self):
        rows = [list(row) for row in self.comparisons]
        # A bytea literal's backslash is the reason the differences file needs
        # a COPY TEXT *encoder* and not `"\t".join`.
        rows[0][self.first_op] = "f"
        self.write("14", comparisons=rows)
        found = od.differences(self.root)
        path = self.root / "oracle-differences.tsv"
        path.write_text(od.render(found))
        committed, problems = od.read_committed(path)
        self.assertEqual(problems, [])
        self.assertEqual(committed, found)

    def test_a_stale_committed_file_is_reported_both_ways(self):
        path = self.root / "oracle-differences.tsv"
        path.write_text(od.render([]))
        rows = [list(row) for row in self.comparisons]
        rows[0][self.first_op] = "f"
        self.write("14", comparisons=rows)
        out = io.StringIO()
        self.assertEqual(od.check(self.root, path, out=out, manual=None), 1)
        self.assertIn("not filed", out.getvalue())

    def test_a_headerless_committed_file_is_refused(self):
        path = self.root / "oracle-differences.tsv"
        path.write_text(co.format_tsv([]))
        _, problems = od.read_committed(path)
        self.assertTrue(any("column header" in p for p in problems), problems)

    def test_write_then_check_is_clean(self):
        path = self.root / "oracle-differences.tsv"
        out = io.StringIO()
        self.assertEqual(od.check(self.root, path, write=True, out=out, manual=None), 0)
        self.assertEqual(od.check(self.root, path, out=io.StringIO(), manual=None), 0)

    def test_a_non_additive_difference_fails_even_when_filed(self):
        rows = [list(row) for row in self.comparisons]
        rows[0][self.first_op] = "f"
        self.write("14", comparisons=rows)
        path = self.root / "oracle-differences.tsv"
        out = io.StringIO()
        self.assertEqual(od.check(self.root, path, write=True, out=out, manual=None), 1)
        self.assertIn("BREAK", out.getvalue())
        self.assertEqual(od.check(self.root, path, out=io.StringIO(), manual=None), 1)

    def test_a_non_additive_cell_naming_an_exempt_literal_is_not_a_break(self):
        rows = [list(row) for row in self.comparisons]
        rows[0][self.first_op] = "f"
        self.write("14", comparisons=rows)
        (d,) = od.differences(self.root)
        self.assertEqual(d.verdict, od.NON_ADDITIVE)
        self.assertFalse(od.exempt(d, {}))
        self.assertTrue(od.exempt(d, {(d.type, d.right): "I66"}))
        self.assertTrue(od.exempt(d, {(d.type, d.left): "I66"}))
        self.assertFalse(od.exempt(d, {("another type", d.left): "I66"}))


def literal(older, newer, type, lit, old, new, verdict=od.ADDITIVE):
    return od.Difference(older, newer, "literals", type, lit, None, None, "status", old, new, verdict)


def comparison(older, newer, type, left, right, old, new, verdict=od.ADDITIVE):
    return od.Difference(older, newer, "comparisons", type, left, right, None, "=", old, new, verdict)


class ManualRows(unittest.TestCase):
    """Transitions, exemptions and the manual's rows held to each other, over
    a small table rather than the committed one."""

    MANUAL = """\
## Where PostgreSQL majors differ

Prose.

| What changed | From | What pgdt does |
|---|---|---|
| `oid` reads octal | 16 | Reads decimal |
| `numeric` holds `Infinity` | 14 | Reads it |

## Next
"""
    INVARIANTS = "## I66 — `oidin`\n"
    ROWS = (
        od.ManualRow("16", "`oid` reads octal", asks=(("oid", "010"),)),
        od.ManualRow("14", "`numeric` holds", asks=(("numeric", "Infinity"),)),
    )
    TABLE = {("oid", "010"): "I66"}

    def found(self):
        return [
            literal("13", "14", "numeric", "Infinity", "E22P02", "ok"),
            comparison("13", "14", "numeric", "Infinity", "0", "E22P02", "f"),
            literal("15", "16", "oid", "010", "ok", "E22P02", od.NON_ADDITIVE),
        ]

    def problems(self, found=None, manual=None, rows=None, table=None, invariants=None):
        return od.manual_problems(
            self.found() if found is None else found,
            self.MANUAL if manual is None else manual,
            self.INVARIANTS if invariants is None else invariants,
            self.ROWS if rows is None else rows,
            self.TABLE if table is None else table,
        )

    def test_the_table_is_read_row_by_row(self):
        self.assertEqual(
            od.manual_table(self.MANUAL),
            [("`oid` reads octal", "16"), ("`numeric` holds `Infinity`", "14")],
        )

    def test_a_consistent_table_has_no_problem(self):
        self.assertEqual(self.problems(), [])

    def test_a_transition_with_no_manual_row_fails(self):
        found = self.found() + [literal("15", "16", "integer", "0x1F", "E22P02", "ok")]
        self.assertTrue(any("no manual row records" in p for p in self.problems(found)))

    def test_a_transition_at_another_major_than_its_row_fails(self):
        found = self.found() + [literal("14", "15", "oid", "010", "E22P02", "ok")]
        self.assertTrue(any("no manual row records" in p for p in self.problems(found)))

    def test_a_comparison_moving_with_neither_literal_fails(self):
        found = self.found() + [comparison("13", "14", "numeric", "1", "0", "t", "f")]
        self.assertTrue(any("neither literal moving" in p for p in self.problems(found)))

    def test_a_manual_row_nothing_asks_fails(self):
        problems = self.problems(found=self.found()[:2], table={})
        self.assertTrue(any("moves at no major" in p for p in problems), problems)

    def test_a_manual_row_with_no_entry_fails(self):
        problems = self.problems(rows=self.ROWS[1:], table={})
        self.assertTrue(any("named by 0 entries" in p for p in problems), problems)

    def test_an_entry_with_no_manual_row_fails(self):
        rows = self.ROWS + (od.ManualRow("16", "`xid` refuses", asks=()),)
        self.assertTrue(any("no row for" in p for p in self.problems(rows=rows)))

    def test_a_row_s_major_is_the_manual_s(self):
        rows = (od.ManualRow("15", "`oid` reads octal", asks=(("oid", "010"),)), self.ROWS[1])
        self.assertTrue(any("says 16" in p for p in self.problems(rows=rows)))

    def test_an_exemption_with_no_manual_row_fails(self):
        table = {**self.TABLE, ("xid", "abc"): "I66"}
        self.assertTrue(any("no manual row asks" in p for p in self.problems(table=table)))

    def test_an_exemption_no_non_additive_cell_asks_fails(self):
        table = {**self.TABLE, ("numeric", "Infinity"): "I66"}
        problems = self.problems(table=table)
        self.assertTrue(any("no non-additive cell asks" in p for p in problems), problems)

    def test_an_exemption_cites_an_invariant(self):
        problems = self.problems(invariants="")
        self.assertTrue(any("is no invariant" in p for p in problems), problems)


class CopyTextEncoding(unittest.TestCase):
    def test_escape_is_unescape_inverted(self):
        for value in (None, "", "plain", r"\xdeadbeef", "a\tb", "a\nb", "a\\\\b"):
            self.assertEqual(co.unescape_copy_text(co.escape_copy_text(value)), value)

    def test_null_is_backslash_n(self):
        self.assertEqual(co.escape_copy_text(None), r"\N")

    def test_a_row_of_values_becomesdescribe(self):
        self.assertEqual(co.format_tsv([["a", None, "b\tc"]]), "a\t\\N\tb\\tc\n")


class CommittedTree(unittest.TestCase):
    """The slice's suite assertion, over the real tree."""

    def test_the_tree_is_aligned_and_comparable(self):
        self.assertEqual(od.alignment_problems(FIXTURES), [])
        self.assertEqual(od.apparatus_problems(FIXTURES), [])

    def test_the_committed_file_is_what_the_oracles_now_say(self):
        committed, problems = od.read_committed(od.DIFFERENCES)
        self.assertEqual(problems, [])
        found = od.differences(FIXTURES)
        self.assertEqual(
            od.compare_to_committed(found, committed, od.DIFFERENCES), []
        )

    def test_no_two_majors_disagree_but_where_an_exemption_names_it(self):
        # The union rule, checked rather than asserted: every difference across
        # the supported majors is a value that could not previously exist, but
        # the narrowings `EXEMPT` names beside their invariants.
        self.assertEqual([od.describe(d) for d in od.broken(od.differences(FIXTURES))], [])

    def test_every_transition_is_a_manual_row_s_and_every_row_is_asked(self):
        self.assertEqual(
            od.manual_problems(
                od.differences(FIXTURES), od.MANUAL.read_text(), od.INVARIANTS.read_text()
            ),
            [],
        )

    def test_the_check_passes(self):
        self.assertEqual(od.check(FIXTURES, od.DIFFERENCES, out=io.StringIO()), 0)


if __name__ == "__main__":
    unittest.main()
