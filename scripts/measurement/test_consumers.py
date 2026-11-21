"""Tests of `measurement.consumers`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import config
from measurement import consumers
from measurement.figures import register


class Consumers(unittest.TestCase):
    """`depends` is the edge into a figure; `consumers()` is the edge out, and
    it is computed from who names the figure rather than declared beside it. A
    declared list is corrected only by a session that happens to notice, which
    is how a retarget left every tuple pointing at a document that had been
    replaced."""

    def test_every_figure_has_a_consumer(self):
        for fig in register.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertTrue(consumers.consumers(fig))

    def test_every_consumer_exists(self):
        for fig in register.ALL_FIGURES:
            for path in consumers.consumers(fig):
                with self.subTest(figure=fig.id, path=path):
                    self.assertTrue((config.REPO / path).exists(), path)

    def test_a_figure_does_not_list_the_doc_it_lives_in(self):
        # measurements.md is where the table goes, not somewhere that repeats
        # it; listing it would make every fold-in look like a cross-doc edit.
        for fig in register.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertNotIn("docs/design/measurements.md", consumers.consumers(fig))

    def test_a_document_naming_the_id_is_a_consumer(self):
        # The register's topic files cite every figure they argue from by id,
        # which is what makes the computed edge possible at all.
        self.assertIn(
            "docs/design/decisions/io.md", consumers.consumers(register.FIGURES_BY_ID["allocator"])
        )

    def test_a_glob_reaches_the_family_it_spells(self):
        # `scan-throughput-*` is how one sentence cites three tables, and a
        # scan that only matched the exact id would read it as citing none.
        for fid in ("scan-throughput-cold", "scan-throughput-warm", "scan-throughput-nvme"):
            with self.subTest(figure=fid):
                self.assertIn(
                    "docs/design/decisions/io.md", consumers.consumers(register.FIGURES_BY_ID[fid])
                )

    def test_a_dated_history_entry_is_not_a_consumer(self):
        # An entry states what was true on its day and is never revised, so a
        # fold-in that re-read one could only make it untrue. Every figure is
        # named in some entry, so this would otherwise be the whole output.
        for fig in register.ALL_FIGURES:
            for path in consumers.consumers(fig):
                with self.subTest(figure=fig.id, path=path):
                    self.assertFalse(path.startswith("docs/status/history/"), path)

    def test_the_register_and_its_tests_are_not_consumers(self):
        # They name every figure because they *are* the declaration.
        for fig in register.ALL_FIGURES:
            got = consumers.consumers(fig)
            for path in ("scripts/measure.py", "scripts/measurement/figures/register.py"):
                with self.subTest(figure=fig.id, path=path):
                    self.assertNotIn(path, got)

    def test_the_declared_residue_is_carried_through(self):
        # The manual states `peak-rss`'s claim to a reader who will never see a
        # figure id, so no scan can find it and the tuple still holds it.
        got = consumers.consumers(register.FIGURES_BY_ID["peak-rss"])
        self.assertIn("docs/manual/dump-inspection.md", got)
        self.assertIn("README.md", got)

    def test_a_declared_consumer_is_not_repeated_by_the_scan(self):
        for fig in register.ALL_FIGURES:
            got = consumers.consumers(fig)
            with self.subTest(figure=fig.id):
                self.assertEqual(len(got), len(set(got)))


if __name__ == "__main__":
    unittest.main()
