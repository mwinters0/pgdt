"""Tests of `measurement.timing`."""

from __future__ import annotations

import dataclasses
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import gating
from measurement import shapes
from measurement import staging
from measurement import test_support
from measurement import timing
from measurement.builds import image
from measurement.figures import parallel
from measurement.figures import reserve


class KillLicence(unittest.TestCase):
    """Which families may lose a leg to the kernel and continue.

    The licence is narrow on purpose. `parallel-peak-rss` once measured
    3067 MiB inside a 3072 MiB container, and a kill there is the apparatus
    failure that being loud caught; the flagless family is the opposite case,
    since the rule under test aims resident at the allocation by construction.
    """

    def test_the_flagless_family_carries_it(self):
        for spec in reserve._reserve_flagless_specs():
            with self.subTest(leg=spec.label):
                self.assertTrue(timing.kill_tolerant(spec.command))

    def test_the_mechanism_legs_carry_it_because_they_are_that_same_shape(self):
        for _, spec in reserve._reserve_mechanism_specs():
            with self.subTest(leg=spec.label):
                self.assertTrue(timing.kill_tolerant(spec.command))

    def test_the_resident_figures_that_could_hide_a_near_miss_do_not(self):
        for spec in (*parallel._parallel_rss_specs(), *test_support._peak_rss_specs()):
            with self.subTest(leg=spec.label):
                self.assertFalse(timing.kill_tolerant(spec.command))

    def test_the_stated_families_of_the_same_figure_do_not(self):
        # They state a budget far below their container, so a kill there is the
        # apparatus and not the reading.
        for spec in (*reserve._reserve_specs(), *reserve._reserve_step_specs()):
            with self.subTest(leg=spec.label):
                self.assertFalse(timing.kill_tolerant(spec.command))

    def test_no_shape_outside_the_reserve_figure_is_tolerant(self):
        for command in shapes.command_shapes():
            if command.startswith(shapes.RESERVE_FLAGLESS):
                continue
            with self.subTest(command=command):
                self.assertFalse(timing.kill_tolerant(command))


class TimeRunHandlesAKill(unittest.TestCase):
    """`Session.time_run`'s kill branch, which no other test reaches.

    It is the one path that only runs when something dies, so it is the one
    most likely to be wrong when it finally does — the previous sitting spent
    an hour finding that out.
    """

    EVENTS = "low 0\nhigh 0\nmax 22\noom 1\noom_kill 1\noom_group_kill 0\n"
    TIMED = "maxrss_kib=68228\n\nreal\t0m0.012s\nuser\t0m0.008s\nsys\t0m0.004s\n"

    def _session(self):
        cfg = config.Config()
        with tempfile.TemporaryDirectory() as tmp:
            session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        # A unit test builds no `peak-rss`: a fresh checkout has none to stage.
        self.enterContext(
            unittest.mock.patch.object(image, "ensure_peak_rss_binary", return_value=Path("/dev/null"))
        )
        session.figure_id = "reserve"
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        return session

    def _run(self, spec, stderr, returncode):
        session = self._session()
        proc = subprocess.CompletedProcess([], returncode, stdout="", stderr=stderr)
        with unittest.mock.patch.object(subprocess, "run", lambda *a, **k: proc):
            return session, session.time_run(spec)

    def _flagless(self):
        return reserve._reserve_flagless_specs()[0]

    def test_a_killed_tolerant_leg_is_recorded_and_does_not_raise(self):
        session, _ = self._run(self._flagless(), self.TIMED + self.EVENTS, 1)
        self.assertTrue(session._last_killed)
        self.assertEqual(len(session.records), 1)
        record = session.records[0]
        self.assertTrue(record["killed"])
        self.assertEqual(record["oom_kill"], 1)
        self.assertIsNone(record["seconds"])
        self.assertIsNone(record["maxrss_kib"])
        # The bound the run still reported, under a name no fit reads.
        self.assertEqual(record["maxrss_bound_kib"], 68228)
        self.assertAlmostEqual(record["seconds_to_kill"], 0.012)

    def test_a_non_oom_failure_of_the_same_leg_still_raises_and_says_so(self):
        with self.assertRaises(RuntimeError) as caught:
            self._run(self._flagless(), self.TIMED + self.EVENTS.replace("oom_kill 1", "oom_kill 0"), 1)
        self.assertIn("not an OOM kill", str(caught.exception))

    def test_an_unreadable_counter_raises_rather_than_reading_as_no_kill(self):
        with self.assertRaises(RuntimeError) as caught:
            self._run(self._flagless(), self.TIMED, 1)
        self.assertIn("cannot be told", str(caught.exception))

    def test_a_kill_outside_the_licence_raises_and_names_the_kill(self):
        # `parallel-peak-rss`'s shape: the near-miss that being loud caught.
        with self.assertRaises(RuntimeError) as caught:
            self._run(parallel._parallel_rss_specs()[0], self.TIMED + self.EVENTS, 1)
        self.assertIn("OOM-killed", str(caught.exception))

    def test_a_surviving_run_records_the_counter_beside_its_reading(self):
        session, seconds = self._run(
            self._flagless(), self.TIMED + self.EVENTS.replace("oom_kill 1", "oom_kill 0"), 0
        )
        self.assertFalse(session._last_killed)
        self.assertAlmostEqual(seconds, 0.012)
        self.assertEqual(session.records[0]["oom_kill"], 0)

    def _kill_then(self, spec, twin_stderr, twin_returncode):
        """The shipped leg killed, then its gate twin's run as given; the
        error raised, and the binaries each run asked for."""
        session = self._session()
        asked = []
        session.binary_path = lambda which: asked.append(which) or Path("/dev/null")
        procs = iter(
            [
                subprocess.CompletedProcess([], 137, stdout="", stderr=self.TIMED + self.EVENTS),
                subprocess.CompletedProcess(
                    [], twin_returncode, stdout="", stderr=twin_stderr
                ),
            ]
        )
        with unittest.mock.patch.object(subprocess, "run", lambda *a, **k: next(procs)):
            with self.assertRaises(RuntimeError) as caught:
                session.time_run(spec)
        return str(caught.exception), asked

    def test_a_kill_outside_the_licence_is_read_on_the_gate_build_before_it_raises(self):
        # A kill is a gate reading wherever it lands: the error has to say
        # whether the platform allocator's build of the same leg survived.
        text, asked = self._kill_then(
            parallel._parallel_rss_specs()[0],
            self.TIMED + self.EVENTS.replace("oom_kill 1", "oom_kill 0"),
            0,
        )
        self.assertEqual(asked, ["pgdt", gating.GATE_BINARY])
        self.assertIn("survived at 66.63 MiB", text)
        self.assertIn("the shipped build alone", text)
        self.assertIn("D13", text)

    def test_a_kill_on_both_legs_says_it_is_not_the_allocators(self):
        text, _ = self._kill_then(parallel._parallel_rss_specs()[0], self.TIMED + self.EVENTS, 137)
        self.assertIn("was OOM-killed too", text)
        self.assertIn("KD34", text)

    def test_a_twin_failing_another_way_classifies_nothing(self):
        text, _ = self._kill_then(
            parallel._parallel_rss_specs()[0],
            self.TIMED + self.EVENTS.replace("oom_kill 1", "oom_kill 0"),
            1,
        )
        self.assertIn("failed another way", text)
        self.assertNotIn("survived", text)

    def test_a_kill_of_a_build_that_does_not_ship_has_no_twin(self):
        # The gate is the shipped build's: an instrument or another allocator
        # leg killed raises as before, one run and no second.
        spec = dataclasses.replace(parallel._parallel_rss_specs()[0], binary="alloc:jemalloc")
        session = self._session()
        asked = []
        session.binary_path = lambda which: asked.append(which) or Path("/dev/null")
        proc = subprocess.CompletedProcess([], 137, stdout="", stderr=self.TIMED + self.EVENTS)
        with unittest.mock.patch.object(subprocess, "run", lambda *a, **k: proc):
            with self.assertRaises(RuntimeError) as caught:
                session.time_run(spec)
        self.assertEqual(asked, ["alloc:jemalloc"])
        self.assertNotIn("gate twin", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
