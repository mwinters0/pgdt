"""Tests of `measurement.inputs`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import config
from measurement import inputs


class Stamps(unittest.TestCase):
    def test_the_input_stamp_changes_with_the_arguments(self):
        cfg = config.Config()
        self.assertNotEqual(
            inputs.input_stamp(inputs.INPUTS["control"], cfg),
            inputs.input_stamp(inputs.INPUTS["arrays"], cfg),
        )

    def test_the_input_stamp_changes_with_the_size(self):
        self.assertNotEqual(
            inputs.input_stamp(inputs.INPUTS["control"], config.Config(size_gib=3.0)),
            inputs.input_stamp(inputs.INPUTS["control"], config.Config(size_gib=1.0)),
        )

    def test_the_input_stamp_is_stable(self):
        cfg = config.Config()
        self.assertEqual(
            inputs.input_stamp(inputs.INPUTS["control"], cfg),
            inputs.input_stamp(inputs.INPUTS["control"], cfg),
        )


if __name__ == "__main__":
    unittest.main()
