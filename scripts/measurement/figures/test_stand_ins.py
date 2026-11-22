"""Tests for the dry-run stand-ins and the session reading them off the figure in hand."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from measurement import config
from measurement import gzip_charge
from measurement import inputs
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.figures import long_line
from measurement.figures import register
from measurement.figures import runners
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
            "gzip_resident",
            "long_line",
            "gzip_spacing",
            "reserve",
        ):
            with self.subTest(name=name):
                self.assertEqual(getattr(stand_ins, name)(spec, bytes(32)), stand_ins.NOTHING)

    def test_a_gzip_resident_leg_reports_the_mirrors_bill_and_its_kinds_pass(self):
        for leg, _, kind in shapes.GZIP_RSS_LEGS:
            for jobs in shapes.GZIP_RSS_JOBS:
                with self.subTest(leg=leg, jobs=jobs):
                    spec = runspec.RunSpec("pgdt", leg, f"{shapes.GZRSS_FAMILY}{jobs}", "warm-parallel", "")
                    got = stand_ins.gzip_resident(spec, bytes(32)).reported
                    billed, _ = gzip_charge.gzip_leg_charge(kind, jobs, inputs.INPUTS[leg].nominal_bytes)
                    self.assertEqual(got["resolved_jobs"], str(jobs))
                    self.assertEqual(got["resolved_budget"], str(billed))
                    # A first pass ran on a single member; a BGZF file was walked.
                    self.assertEqual(got.get("first_pass_workers"), str(jobs) if kind == "pass" else None)


class EveryFigureDryRuns(unittest.TestCase):
    """A dry sitting of every selectable figure, in the register's run order,
    renders each one's table: a renderer reading a report its figure declares
    no stand-in for refuses its legs here, not on the first sitting that
    selects it. Every directory is a fresh one, so no input already on this
    machine decides what a renderer reads."""

    def test_every_selectable_figure_renders_from_a_dry_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = config.Config(
                dry_run=True,
                cache_dir=root / "ssd",
                warm_dir=root / "shm",
                nvme_dir=root / "nvme",
                out_dir=root / "runs",
                alloc_build_root=root / "builds",
            )
            stager = staging.Stager(cfg, lambda _m: None)
            stager.plan(register.SELECTABLE)
            session = timing.Session(cfg, stager, lambda _m: None, root / "runs")
            for fig in register.SELECTABLE:
                with self.subTest(figure=fig.id):
                    # What `emit` does for each figure before its run function.
                    session.figure_id = fig.id
                    session.memory = fig.memory
                    for name in fig.cold_inputs:
                        stager.ensure_generated(name)
                    for name in fig.nvme_inputs:
                        stager.nvme_path(name)
                    session.stage(fig.id, 0)
                    self.assertTrue(runners.RUNS[fig.id](session))


if __name__ == "__main__":
    unittest.main()
