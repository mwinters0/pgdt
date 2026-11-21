"""Tests of `measurement.config`."""

from __future__ import annotations

import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import inputs


class Apparatus(unittest.TestCase):
    def test_the_recorded_apparatus_is_publishable(self):
        self.assertTrue(config.Config().publishable)
        self.assertIsNone(config.Config().unpublishable_reason)

    def test_a_smaller_input_is_not(self):
        self.assertFalse(config.Config(size_gib=0.05).publishable)

    def test_an_overridden_rep_count_is_not(self):
        self.assertFalse(config.Config(reps_override=1).publishable)

    def test_a_diagnostic_sitting_is_not(self):
        # `--alone` is not an apparatus change; it is a selection that leaves a
        # shared reading measured by the figure that borrows it, which is the
        # same table the publication refusal declines to let a sitting take.
        self.assertFalse(config.Config(alone=True).publishable)

    def test_each_reason_says_which_departure_it_is(self):
        # One sentence per departure, because the log line and the banner over
        # the tables both print it and a reader has to know which run this was.
        self.assertIn("--alone", config.Config(alone=True).unpublishable_reason)
        self.assertIn("--dry-run", config.Config(dry_run=True).unpublishable_reason)
        self.assertIn(
            "overrode the recorded apparatus",
            config.Config(reps_override=1).unpublishable_reason,
        )

    def test_size_scales_the_generator_argument(self):
        cfg = config.Config(size_gib=0.5)
        argv = inputs.INPUTS["control"].argv(cfg, Path("/tmp/out.sql"))
        self.assertIn("512", argv)
        self.assertIn("--size-mb", argv)

    def test_block_inputs_do_not_scale(self):
        cfg = config.Config(size_gib=0.5)
        argv = inputs.INPUTS["blocks500"].argv(cfg, Path("/tmp/out.sql"))
        self.assertEqual(argv[-4:], ["--blocks", "500", "--out", "/tmp/out.sql"])

    def test_the_output_path_is_the_last_argument(self):
        cfg = config.Config()
        for spec in inputs.INPUTS.values():
            with self.subTest(input=spec.name):
                self.assertEqual(spec.argv(cfg, Path("/tmp/out.sql"))[-1], "/tmp/out.sql")


if __name__ == "__main__":
    unittest.main()
