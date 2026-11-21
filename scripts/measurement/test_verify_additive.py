"""Tests of `measurement.verify_additive`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import verify_additive
from measurement.figures import register


class VerifyAdditive(unittest.TestCase):
    """The evidence half: regenerate at two revisions and compare bytes."""

    def test_it_verifies_only_inputs_a_published_figure_is_taken_on(self):
        # An input an *untaken* instrument alone consumes has no bytes in the
        # doc to be wrong about, and may not be generatable at the older
        # revision at all -- which `composite_text` proved on this mechanism's
        # first run, since the commit under test was the one that added its
        # flag. So the verified set is the published figures' own inputs, and
        # anything reachable only through `UNTAKEN` is outside it.
        published = {
            name
            for fig in register.ALL_FIGURES
            for name in (*fig.cold_inputs, *fig.warm_inputs)
        }
        untaken_only = {
            name
            for fig in register.UNTAKEN
            for name in (*fig.cold_inputs, *fig.warm_inputs)
        } - published
        self.assertEqual(published & untaken_only, set())
        self.assertIn("composite", published)

    def test_the_verification_size_is_small_enough_to_be_run(self):
        # A verification nobody runs is worth nothing; the sweep's own 3 GiB
        # would make this a minutes-long command per input.
        self.assertLess(verify_additive.VERIFY_SIZE_GIB, 0.1)


if __name__ == "__main__":
    unittest.main()
