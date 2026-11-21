"""Tests of `measurement.staleness.stale`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement.staleness import stale


class Staleness(unittest.TestCase):
    def test_a_file_under_a_declared_directory_counts(self):
        touched = dict(
            (f.id, hits) for f, hits in stale.figures_touched(["crates/pgdt-dump/src/map.rs"])
        )
        self.assertIn("scan-throughput-warm", touched)
        self.assertIn("statistics-gathering", touched)

    def test_an_unrelated_path_touches_nothing(self):
        self.assertEqual(stale.figures_touched(["README.md"]), [])

    def test_a_generator_change_reaches_the_figures_taken_on_its_output(self):
        touched = [f.id for f, _ in stale.figures_touched(["scripts/generate_perf_data.py"])]
        self.assertIn("nested-end-to-end", touched)
        self.assertIn("scan-throughput-cold", touched)

    def test_the_bench_source_reaches_the_micro(self):
        touched = [
            f.id for f, _ in stale.figures_touched(["crates/pgdt-codec/benches/decoders.rs"])
        ]
        self.assertEqual(touched, ["nested-decode-micro"])


if __name__ == "__main__":
    unittest.main()
