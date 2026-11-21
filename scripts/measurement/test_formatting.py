"""Tests of `measurement.formatting`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import config
from measurement import formatting


class Numbers(unittest.TestCase):
    def test_median_odd_and_even(self):
        self.assertEqual(formatting.median([3.0, 1.0, 2.0]), 2.0)
        self.assertEqual(formatting.median([1.0, 2.0, 3.0, 4.0]), 2.5)

    def test_median_of_one(self):
        self.assertEqual(formatting.median([0.57]), 0.57)

    def test_median_refuses_nothing(self):
        with self.assertRaises(ValueError):
            formatting.median([])

    def test_spread_is_min_and_max_not_first_and_last(self):
        self.assertEqual(formatting.spread([0.562, 0.515, 0.531]), (0.515, 0.562))

    def test_spread_refuses_nothing(self):
        with self.assertRaises(ValueError):
            formatting.spread([])


class Formatting(unittest.TestCase):
    def test_seconds_precision_switches_at_two(self):
        # measurements.md quotes 0.531 s and 6.68 s; the switch is the doc's,
        # not an accident of %g.
        self.assertEqual(formatting.fmt_s(0.5312), "0.531")
        self.assertEqual(formatting.fmt_s(1.9999), "2.000")
        self.assertEqual(formatting.fmt_s(2.0), "2.00")
        self.assertEqual(formatting.fmt_s(45.837), "45.84")

    def test_negative_seconds_use_the_magnitude(self):
        self.assertEqual(formatting.fmt_s(-0.075), "-0.075")

    def test_median_and_spread_render_together(self):
        self.assertEqual(
            formatting.fmt_median_spread([0.531, 0.515, 0.562]),
            "**0.531 s** (0.515–0.562)",
        )

    def test_rate_is_decimal_megabytes(self):
        # 3.00 GiB in 6.68 s is the doc's ~481 MB/s row.
        self.assertEqual(formatting.fmt_rate(3 * config.GIB, 6.68), "~482 MB/s")

    def test_delta_carries_absolute_and_percent(self):
        self.assertEqual(formatting.fmt_delta(0.531, 0.568), "**+0.037 s, +7%**")

    def test_delta_of_a_faster_binary_is_negative(self):
        self.assertTrue(formatting.fmt_delta(1.0, 0.9).startswith("**-0.100 s, -10%"))

    def test_readings_are_listed_in_order_taken(self):
        self.assertEqual(formatting.fmt_readings([0.51, 0.53]), "0.510, 0.530")

    def test_bytes_render_at_the_doc_s_three_scales(self):
        self.assertEqual(formatting._fmt_bytes(248 * 1024), "248 KB")
        self.assertEqual(formatting._fmt_bytes(2 * config.MIB), "2.0 MB")
        # `peak-rss` puts a 2 MB input beside a 3.00 GiB one, and "3072.0 MB"
        # is not the size anybody asked for.
        self.assertEqual(formatting._fmt_bytes(3 * config.GIB), "3.00 GiB")

    def test_resident_sets_render_in_mebibytes(self):
        # The only bound this figure is read against — four buffers of at most
        # 8 MiB — is stated in MiB, and a claim compared against a bound must
        # not change units on the way.
        self.assertEqual(formatting.fmt_mib(6144), "6.00 MiB")
        self.assertEqual(
            formatting.fmt_mib_median_spread([6100, 6000, 6300]), "**5.96 MiB** (5.86–6.15)"
        )

    def test_a_resident_set_difference_renders_at_the_scale_it_lands_on(self):
        # The figure's two axes are three orders of magnitude apart: tens of
        # KiB per byte, tens of MiB per block.
        self.assertEqual(formatting.fmt_rss_delta(157.0), "+157 KiB")
        self.assertEqual(formatting.fmt_rss_delta(-12.4), "-12 KiB")
        self.assertEqual(formatting.fmt_rss_delta(40132.0), "+39.19 MiB")

    def test_nanoseconds_become_microseconds_above_a_thousand(self):
        self.assertEqual(formatting._fmt_ns(265.4), "265 ns")
        self.assertEqual(formatting._fmt_ns(3850.0), "3.85 µs")


class Tables(unittest.TestCase):
    def test_table_shape(self):
        got = formatting.md_table(["a", "b"], [["1", "2"], ["3", "4"]])
        self.assertEqual(
            got,
            "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |",
        )

    def test_a_short_row_is_an_error_not_a_ragged_table(self):
        with self.assertRaises(ValueError):
            formatting.md_table(["a", "b"], [["1"]])

    def test_empty_first_header_is_allowed(self):
        # The preamble table's first column has no header.
        self.assertTrue(formatting.md_table(["", "on"], [["warm", "0.5"]]).startswith("|  | on |"))


if __name__ == "__main__":
    unittest.main()
