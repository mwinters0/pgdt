"""Tests of `measurement.contention`."""

from __future__ import annotations

import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import contention


class Telemetry(unittest.TestCase):
    """The contention witnesses. What a silent error costs here is a sweep
    that reports itself quiet when it was not, so the parsing and the window
    arithmetic are what get pinned."""

    def _psi_dir(self, tmp, cpu_some=1000, cpu_full=0):
        root = Path(tmp) / "pressure"
        root.mkdir()
        (root / "cpu").write_text(
            f"some avg10=0.00 avg60=0.00 avg300=0.00 total={cpu_some}\n"
            f"full avg10=0.00 avg60=0.00 avg300=0.00 total={cpu_full}\n"
        )
        return root

    def test_psi_totals_are_read_per_resource_and_kind(self):
        with tempfile.TemporaryDirectory() as tmp:
            got = contention.read_psi_totals(self._psi_dir(tmp, cpu_some=1234, cpu_full=56))
            self.assertEqual(got, {"cpu.some": 1234, "cpu.full": 56})

    def test_absent_psi_is_missing_keys_not_an_error(self):
        # PSI is a kernel build option; a harness that died without it would be
        # dead on arrival at the next machine.
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(contention.read_psi_totals(Path(tmp) / "nope"), {})

    def test_the_avg_fields_are_not_mistaken_for_the_total(self):
        # `avg10` describes a 10-second window and a warm reading is 0.5 s, so
        # reading it instead of `total` would describe the wrong thing entirely.
        with tempfile.TemporaryDirectory() as tmp:
            got = contention.read_psi_totals(self._psi_dir(tmp, cpu_some=7))
            self.assertEqual(set(got.values()), {7, 0})

    def test_cpu_jiffies_name_every_field_including_steal(self):
        with tempfile.TemporaryDirectory() as tmp:
            stat = Path(tmp) / "stat"
            stat.write_text("cpu  1 2 3 4 5 6 7 8 9 10\ncpu0 1 2 3 4 5 6 7 8 9 10\n")
            got = contention.read_cpu_jiffies(stat)
            self.assertEqual(got["user"], 1)
            self.assertEqual(got["steal"], 8)
            self.assertEqual(got["idle"], 4)

    def test_a_stat_file_without_the_cpu_line_is_empty_not_wrong(self):
        with tempfile.TemporaryDirectory() as tmp:
            stat = Path(tmp) / "stat"
            stat.write_text("intr 1 2 3\n")
            self.assertEqual(contention.read_cpu_jiffies(stat), {})

    def _counters(self, mono, psi, cpu):
        return contention.Counters(monotonic=mono, psi=psi, cpu=cpu)

    def test_stall_is_reported_absolutely_and_as_a_share_of_the_window(self):
        before = self._counters(10.0, {"cpu.some": 1_000_000}, {})
        after = self._counters(12.0, {"cpu.some": 1_200_000}, {})
        got = contention.counter_delta(before, after)
        self.assertEqual(got["psi_cpu_some_us"], 200_000)
        # 0.2 s of stall in a 2 s window.
        self.assertAlmostEqual(got["psi_cpu_some_pct"], 10.0)

    def test_the_percentage_makes_a_short_and_a_long_reading_comparable(self):
        short = contention.counter_delta(
            self._counters(0.0, {"cpu.some": 0}, {}), self._counters(0.5, {"cpu.some": 50_000}, {})
        )
        long = contention.counter_delta(
            self._counters(0.0, {"cpu.some": 0}, {}), self._counters(8.0, {"cpu.some": 800_000}, {})
        )
        self.assertAlmostEqual(short["psi_cpu_some_pct"], long["psi_cpu_some_pct"])

    def test_busy_and_steal_come_out_of_the_jiffy_difference(self):
        before = self._counters(0.0, {}, {"user": 0, "idle": 0, "iowait": 0, "steal": 0})
        after = self._counters(1.0, {}, {"user": 10, "idle": 80, "iowait": 10, "steal": 5})
        got = contention.counter_delta(before, after)
        # 105 jiffies total, 90 of them idle+iowait.
        self.assertAlmostEqual(got["cpu_busy_pct"], round(100 * 15 / 105, 2))
        self.assertAlmostEqual(got["cpu_steal_pct"], round(100 * 5 / 105, 3))

    def test_a_zero_length_window_does_not_divide_by_zero(self):
        same = self._counters(5.0, {"cpu.some": 1}, {"user": 1})
        self.assertIn("window_s", contention.counter_delta(same, same))

    def test_only_counters_present_on_both_sides_are_differenced(self):
        before = self._counters(0.0, {"cpu.some": 1}, {})
        after = self._counters(1.0, {"cpu.some": 2, "io.some": 9}, {})
        got = contention.counter_delta(before, after)
        self.assertIn("psi_cpu_some_us", got)
        self.assertNotIn("psi_io_some_us", got)


class SamplerWindow(unittest.TestCase):
    def _sampler(self, samples):
        s = contention.Sampler(hz=0)  # no thread; the samples are supplied
        s.samples = samples
        return s

    def test_only_samples_inside_the_window_are_used(self):
        s = self._sampler([(0.0, 1e6, 2e6, 40_000), (5.0, 2e6, 4e6, 50_000), (9.0, 1e6, 1e6, 90_000)])
        got = s.window(4.0, 6.0)
        self.assertEqual(got["freq_samples"], 1)
        self.assertAlmostEqual(got["freq_busiest_mhz"], 4000.0)
        self.assertAlmostEqual(got["temp_max_c"], 50.0)

    def test_a_window_with_no_samples_says_so_rather_than_guessing(self):
        # A warm reading can be shorter than the sample interval; reporting a
        # count of zero is what stops the mean being read as a measurement.
        s = self._sampler([(0.0, 1e6, 2e6, 40_000)])
        self.assertEqual(s.window(10.0, 11.0), {"freq_samples": 0})

    def test_the_busiest_core_is_reported_apart_from_the_all_core_mean(self):
        # A scan is near enough single-threaded that the mean across 24 cores
        # is dominated by the idle ones and reads far below the frequency the
        # work ran at.
        s = self._sampler([(1.0, 1_000_000, 4_500_000, 50_000)])
        got = s.window(0.0, 2.0)
        self.assertAlmostEqual(got["freq_busiest_mhz"], 4500.0)
        self.assertAlmostEqual(got["freq_allcore_mean_mhz"], 1000.0)

    def test_unreadable_sensors_drop_out_rather_than_poisoning_the_mean(self):
        nan = float("nan")
        s = self._sampler([(1.0, nan, nan, 50_000)])
        got = s.window(0.0, 2.0)
        self.assertNotIn("freq_busiest_mhz", got)
        self.assertAlmostEqual(got["temp_max_c"], 50.0)


if __name__ == "__main__":
    unittest.main()
