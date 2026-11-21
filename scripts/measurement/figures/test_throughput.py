"""Tests of `measurement.figures.throughput`."""

from __future__ import annotations

import re
import unittest
import unittest.mock

from measurement import shapes
from measurement import test_support
from measurement.figures import register
from measurement.figures import throughput


class ChunkSize(unittest.TestCase):
    """The read chunk sweep — the one of the three I/O defaults that is a
    value, and the only one measurable without a second build.

    Every check here is aimed at the same failure: a table that formats
    perfectly while every row measured the default."""

    def test_the_flag_carries_the_size_into_the_command(self):
        for size in shapes.CHUNK_SIZES:
            script = shapes._script(f"parse-chunk-{size}")
            self.assertIn(f"--chunk-size {size}", script)
            self.assertIn("parse --source /dump.sql", script)

    def test_a_size_the_figure_does_not_carry_is_refused(self):
        # The failure this stops: a typo'd row that still builds a command
        # line, runs, and publishes a reading nobody asked for.
        with self.assertRaises(ValueError):
            shapes._script("parse-chunk-999")
        with self.assertRaises(ValueError):
            shapes._script("parse-chunk-big")

    def test_the_default_row_is_the_shipped_default(self):
        # The table's ratios are against this row, and the doc's conclusion is
        # about the constant the library ships. A drift between the two would
        # publish a comparison against a value nothing runs.
        self.assertIn(throughput.CHUNK_DEFAULT, shapes.CHUNK_SIZES)
        source = test_support.read_path_source()
        self.assertIn(
            f"pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = 1 << {throughput.CHUNK_DEFAULT.bit_length() - 1};",
            source,
        )

    def test_the_sweep_brackets_the_read_path_pool_ceiling(self):
        # Above `source::pool::POOL_MAX_BYTES` the buffer pool stops keeping the buffer,
        # so every chunk is allocated and zeroed afresh. The sweep has to hold
        # a row either side of it or the table cannot say so.
        source = test_support.read_path_source()
        match = re.search(r"POOL_MAX_BYTES: usize = (\d+) << 20", source)
        assert match is not None, "the read path no longer names a pool ceiling"
        ceiling = int(match.group(1)) << 20
        self.assertIn(ceiling, shapes.CHUNK_SIZES)
        self.assertTrue(any(s > ceiling for s in shapes.CHUNK_SIZES))

    def test_every_row_is_the_same_binary_over_the_same_file(self):
        specs = throughput._chunk_specs()
        self.assertEqual({s.binary for s in specs}, {"pgdt"})
        self.assertEqual({s.input for s in specs}, {"control"})
        self.assertEqual(
            len(specs), len(shapes.CHUNK_SIZES) * len(throughput.CHUNK_REGIMES)
        )

    def test_it_declares_an_input_on_each_of_the_three_devices(self):
        fig = register.FIGURES_BY_ID["chunk-size"]
        self.assertEqual(fig.warm_inputs, ("control",))
        self.assertEqual(fig.cold_inputs, ("control",))
        self.assertEqual(fig.nvme_inputs, ("control",))
        self.assertEqual(sorted(fig.stage.split("+")), ["cold", "cold-nvme", "warm"])

    def test_a_size_is_spelled_in_whole_units(self):
        for size in shapes.CHUNK_SIZES:
            self.assertRegex(throughput.fmt_chunk(size), r"^\d+ (KiB|MiB)$")
        with self.assertRaises(ValueError):
            throughput.fmt_chunk(1000)


if __name__ == "__main__":
    unittest.main()
