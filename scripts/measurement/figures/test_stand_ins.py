"""Tests for the dry-run stand-ins and the session reading them off the figure in hand."""

from __future__ import annotations

import unittest
from pathlib import Path

from measurement import config
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.figures import long_line
from measurement.figures import register
from measurement.figures import stand_ins


class StandIns(unittest.TestCase):
    def _session(self, figure):
        cfg = config.Config(dry_run=True)
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        session.figure_id = figure
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path(f"/bin/{which}")
        return session

    def test_the_session_reports_the_stand_in_its_figure_declares(self):
        spec = runspec.RunSpec("dfcli", "dynfilter", shapes.DYNFILTER_STARTUP, "warm", "")
        session = self._session("dynamic-filter-topk")
        session.time_run(spec)
        self.assertEqual(session._last_stdout, {"startup_answer": "1"})
        # The same leg under a figure declaring no stand-in reports nothing:
        # the session knows no family of its own.
        self.assertIsNone(register.EVERY_BY_ID["map-only"].stand_in)
        session = self._session("map-only")
        session.time_run(spec)
        self.assertEqual(session._last_stdout, {})

    def test_a_factor_leg_names_its_factor_in_the_instrument_report_alone(self):
        spec = long_line._long_factor_specs()[0]
        self.assertTrue(spec.instrument)
        session = self._session("long-line-resident")
        session.time_run(spec)
        self.assertNotIn("planned_factor", session._last_stdout)
        self.assertIn("planned_factor", session._last_instrument)
        self.assertIn("live_peak_bytes", session._last_instrument)

    def test_a_leg_outside_the_family_reports_nothing(self):
        spec = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        for name in (
            "decode",
            "dynamic_filter",
            "parallel_scan",
            "pruning",
            "gzip_first_pass",
            "long_line",
            "gzip_spacing",
            "reserve",
        ):
            with self.subTest(name=name):
                self.assertEqual(getattr(stand_ins, name)(spec, bytes(32)), stand_ins.NOTHING)


if __name__ == "__main__":
    unittest.main()
