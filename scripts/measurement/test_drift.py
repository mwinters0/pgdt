"""Tests of `measurement.drift`."""

from __future__ import annotations

import json
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import drift
from measurement.figures import register
from measurement.staleness import stale


class Drift(unittest.TestCase):
    """The instrument measured against itself: two sweeps of the same figures,
    on identical inputs and binaries. The standing rules assert "~10%"
    session-to-session drift from history rather than from a reading."""

    def _sweep(self, tmp: str, name: str, readings: dict, commit="abc1234", day="2026-08-28"):
        d = Path(tmp) / name
        d.mkdir()
        (d / "raw.json").write_text(
            json.dumps({"commit": commit, "date": day, "readings": readings})
        )
        return d

    def test_the_delta_is_the_second_sweep_against_the_first(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"scan-throughput-warm/pgdt/control/parse/warm": [1.0, 1.0]})
            b = self._sweep(tmp, "b", {"scan-throughput-warm/pgdt/control/parse/warm": [1.1, 1.1]})
            table = drift.drift_table(a / "raw.json", b / "raw.json")
            self.assertIn("+10.0%", table)

    def test_a_faster_second_sweep_reads_negative(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/pgdt/control/parse/warm": [2.0]})
            b = self._sweep(tmp, "b", {"f/pgdt/control/parse/warm": [1.0]})
            self.assertIn("-50.0%", drift.drift_table(a / "raw.json", b / "raw.json"))

    def test_only_shared_readings_are_compared(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/a": [1.0], "f/only-in-a": [1.0]})
            b = self._sweep(tmp, "b", {"f/a": [1.0], "f/only-in-b": [1.0]})
            table = drift.drift_table(a / "raw.json", b / "raw.json")
            self.assertNotIn("only-in-a", table)
            self.assertNotIn("only-in-b", table)

    def test_two_sweeps_sharing_nothing_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/a": [1.0]})
            b = self._sweep(tmp, "b", {"f/b": [1.0]})
            with self.assertRaises(ValueError):
                drift.drift_table(a / "raw.json", b / "raw.json")

    def test_the_summary_quotes_the_median_and_the_worst(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/x": [1.0], "f/y": [1.0], "f/z": [1.0]})
            b = self._sweep(tmp, "b", {"f/x": [1.01], "f/y": [1.05], "f/z": [1.20]})
            table = drift.drift_table(a / "raw.json", b / "raw.json")
            self.assertIn("median absolute drift is **5.0%**", table)
            self.assertIn("largest is **20.0%**", table)

    def test_the_derived_figure_is_not_in_the_sweep(self):
        # It is computed across two sweeps, so `--all` must not try to run it.
        self.assertNotIn("session-drift", [f.id for f in register.FIGURES])
        self.assertIn("session-drift", register.ALL_BY_ID)

    def test_the_derived_figure_is_still_checked_for_staleness(self):
        # It is out of the sweep but not out of the register: it declares
        # the harness's source, because the harness's own timing path is the
        # apparatus it measures. Iterating the sweep list here would leave
        # that declaration inert.
        for path in ("scripts/measure.py", "scripts/measurement/timing.py"):
            touched = [f.id for f, _ in stale.figures_touched([path])]
            self.assertIn("session-drift", touched)


if __name__ == "__main__":
    unittest.main()
