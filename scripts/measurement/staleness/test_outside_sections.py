"""Tests of `measurement.staleness.outside_sections`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement.staleness import outside_sections


class Selection(unittest.TestCase):
    def test_a_shared_reading_pulls_its_source_in(self):
        got = [f.id for f in outside_sections.resolve_selection(["allocator"])]
        self.assertIn("scan-throughput-warm", got)

    def test_the_source_runs_first(self):
        got = [f.id for f in outside_sections.resolve_selection(["allocator"])]
        self.assertLess(got.index("scan-throughput-warm"), got.index("allocator"))

    def test_unknown_figure_is_refused(self):
        with self.assertRaises(SystemExit):
            outside_sections.resolve_selection(["no-such-figure"])

    def test_selection_is_deduplicated(self):
        got = [f.id for f in outside_sections.resolve_selection(["map-only", "map-only"])]
        self.assertEqual(got, ["map-only"])


if __name__ == "__main__":
    unittest.main()
