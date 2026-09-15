"""Tests for generate_pruning_bench.py: the control's rows, one clustered
low-cardinality column appended."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import generate_perf_data as perf
import generate_pruning_bench as gen


def _rows(path: Path) -> tuple[list[str], list[list[str]]]:
    text = path.read_text()
    head, _, body = text.partition(" FROM stdin;\n")
    data = body.split("\\.\n", 1)[0]
    return head.splitlines(), [line.split("\t") for line in data.splitlines()]


class GeneratePruningBench(unittest.TestCase):
    def _generate(self, size_bytes: int, perf_too: bool = False):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "pruning.sql"
            gen.generate(out, size_bytes, seed=42)
            got = _rows(out)
            if perf_too:
                control = Path(tmp) / "control.sql"
                perf.generate(control, size_bytes, 42, arrays=False, composites=False)
                return got, _rows(control)
            return got

    def test_every_row_is_the_controls_with_the_label_appended(self):
        # Same seed, same draws: the column is added without moving a byte of
        # what the control's rows hold, so the two inputs differ by it alone.
        (head, rows), (control_head, control_rows) = self._generate(1 << 20, perf_too=True)
        n = min(len(rows), len(control_rows))
        self.assertGreater(n, 100)
        for row, control in zip(rows[:n], control_rows[:n]):
            self.assertEqual(row[:-1], control)
            self.assertEqual(row[-1], gen.category(int(row[0])))
        self.assertIn(f"    {gen.COLUMN[0]} {gen.COLUMN[1]}", head)
        self.assertTrue(head[-1].endswith(f", {gen.COLUMN[0]})"))

    def test_labels_arrive_in_runs_that_cycle(self):
        self.assertEqual(gen.category(1), gen.label(0))
        self.assertEqual(gen.category(gen.RUN_ROWS), gen.label(0))
        self.assertEqual(gen.category(gen.RUN_ROWS + 1), gen.label(1))
        self.assertEqual(gen.category(gen.RUN_ROWS * gen.LABELS + 1), gen.label(0))

    def test_a_label_is_never_null_and_needs_no_escape(self):
        for i in range(gen.LABELS):
            with self.subTest(label=i):
                self.assertEqual(perf.encode_field(gen.label(i)), gen.label(i))

    def test_the_labels_outnumber_a_groups_dictionary_cap(self):
        # A column the whole file keeps under the cap would hold every label in
        # one dictionary; the runs are what keep a group's to one or two.
        self.assertGreater(gen.LABELS, 64)


if __name__ == "__main__":
    unittest.main()
