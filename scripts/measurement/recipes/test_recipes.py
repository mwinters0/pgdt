"""Tests of `measurement.recipes.recipes`."""

from __future__ import annotations

import inspect
import unittest
import unittest.mock

from measurement import config
from measurement import staging
from measurement import test_support
from measurement.builds import leg_binaries
from measurement.recipes import callgrind
from measurement.recipes import heaptrack
from measurement.recipes import koji
from measurement.recipes import profile


class ReadingsContained(unittest.TestCase):
    """Every run of a built binary the harness makes or prints is in a
    memory-limited container: its figures' own containers, `contained.argv`
    for what it starts besides, and every printed recipe line, held to the
    classifier the hook refuses by."""

    def test_every_recipe_runs_its_binaries_contained(self):
        cfg = config.Config()
        recipes = {
            "profile": profile.profile_recipe(cfg),
            "heaptrack": heaptrack.heaptrack_recipe(cfg),
            "callgrind": callgrind.callgrind_recipe(cfg),
            **{f"koji wrap={w}": koji.koji_recipe(cfg, "pgdt-koji", w, 8) for w in (False, True)},
        }
        for name, recipe in recipes.items():
            with self.subTest(recipe=name):
                self.assertEqual(test_support.recipe_uncontained(recipe), [])

    def test_the_untimed_runs_go_through_contained(self):
        for function in (staging.Stager.profile, leg_binaries.count_saves):
            with self.subTest(function=function.__name__):
                self.assertIn("contained.argv(", inspect.getsource(function))


if __name__ == "__main__":
    unittest.main()
