#!/usr/bin/env python3
"""Unit tests for measure.py, run with `uv run python -m unittest test_measure`.

Stdlib `unittest`, no dependency added to a repo that has none. What is tested
is where a silent error would be worst: the median, the spread, the table
formatting and the parsing of another tool's output. A wrong median produces a
*confidently wrong* table, which is the failure the harness exists to prevent —
whereas a broken container invocation fails loudly, on the first run, and needs
root and a container runtime to exercise at all.

The structural tests at the bottom are the other half: they hold the register
itself to the rules that make selection safe — every figure declares what
invalidates it, and a figure that borrows a reading runs after the figure that
takes it.
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import measure


class Numbers(unittest.TestCase):
    def test_median_odd_and_even(self):
        self.assertEqual(measure.median([3.0, 1.0, 2.0]), 2.0)
        self.assertEqual(measure.median([1.0, 2.0, 3.0, 4.0]), 2.5)

    def test_median_of_one(self):
        self.assertEqual(measure.median([0.57]), 0.57)

    def test_median_refuses_nothing(self):
        with self.assertRaises(ValueError):
            measure.median([])

    def test_spread_is_min_and_max_not_first_and_last(self):
        self.assertEqual(measure.spread([0.562, 0.515, 0.531]), (0.515, 0.562))

    def test_spread_refuses_nothing(self):
        with self.assertRaises(ValueError):
            measure.spread([])


class Formatting(unittest.TestCase):
    def test_seconds_precision_switches_at_two(self):
        # measurements.md quotes 0.531 s and 6.68 s; the switch is the doc's,
        # not an accident of %g.
        self.assertEqual(measure.fmt_s(0.5312), "0.531")
        self.assertEqual(measure.fmt_s(1.9999), "2.000")
        self.assertEqual(measure.fmt_s(2.0), "2.00")
        self.assertEqual(measure.fmt_s(45.837), "45.84")

    def test_negative_seconds_use_the_magnitude(self):
        self.assertEqual(measure.fmt_s(-0.075), "-0.075")

    def test_median_and_spread_render_together(self):
        self.assertEqual(
            measure.fmt_median_spread([0.531, 0.515, 0.562]),
            "**0.531 s** (0.515–0.562)",
        )

    def test_rate_is_decimal_megabytes(self):
        # 3.00 GiB in 6.68 s is the doc's ~481 MB/s row.
        self.assertEqual(measure.fmt_rate(3 * measure.GIB, 6.68), "~482 MB/s")

    def test_delta_carries_absolute_and_percent(self):
        self.assertEqual(measure.fmt_delta(0.531, 0.568), "**+0.037 s, +7%**")

    def test_delta_of_a_faster_binary_is_negative(self):
        self.assertTrue(measure.fmt_delta(1.0, 0.9).startswith("**-0.100 s, -10%"))

    def test_readings_are_listed_in_order_taken(self):
        self.assertEqual(measure.fmt_readings([0.51, 0.53]), "0.510, 0.530")

    def test_bytes_render_at_the_doc_s_two_scales(self):
        self.assertEqual(measure._fmt_bytes(248 * 1024), "248 KB")
        self.assertEqual(measure._fmt_bytes(2 * measure.MIB), "2.0 MB")

    def test_nanoseconds_become_microseconds_above_a_thousand(self):
        self.assertEqual(measure._fmt_ns(265.4), "265 ns")
        self.assertEqual(measure._fmt_ns(3850.0), "3.85 µs")


class Tables(unittest.TestCase):
    def test_table_shape(self):
        got = measure.md_table(["a", "b"], [["1", "2"], ["3", "4"]])
        self.assertEqual(
            got,
            "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |",
        )

    def test_a_short_row_is_an_error_not_a_ragged_table(self):
        with self.assertRaises(ValueError):
            measure.md_table(["a", "b"], [["1"]])

    def test_empty_first_header_is_allowed(self):
        # The census table's first column has no header.
        self.assertTrue(measure.md_table(["", "on"], [["warm", "0.5"]]).startswith("|  | on |"))


class BashTime(unittest.TestCase):
    def test_reads_the_real_line(self):
        self.assertAlmostEqual(
            measure.parse_bash_time("real\t0m0.570s\nuser\t0m0.4s\nsys\t0m0.1s"), 0.570
        )

    def test_minutes_are_added(self):
        self.assertAlmostEqual(measure.parse_bash_time("real\t1m2.003s\n"), 62.003)

    def test_dd_s_own_chatter_is_not_mistaken_for_a_timing(self):
        text = (
            "3221225472 bytes (3.2 GB, 3.0 GiB) copied, 5.73374 s, 562 MB/s\n"
            "real\t0m5.734s\nuser\t0m0.0s\n"
        )
        self.assertAlmostEqual(measure.parse_bash_time(text), 5.734)

    def test_no_timing_is_an_error_not_a_zero(self):
        # A labelled run with no number under it is exactly what redirecting
        # stderr inside a timed command produces.
        with self.assertRaises(ValueError):
            measure.parse_bash_time("some output but no timing\n")

    def test_two_timings_are_an_error(self):
        with self.assertRaises(ValueError):
            measure.parse_bash_time("real\t0m1.0s\nreal\t0m2.0s\n")


class Criterion(unittest.TestCase):
    def _bench(self, root: Path, directory: str, full_id: str, median_ns: float, which="new"):
        d = root / directory / which
        d.mkdir(parents=True)
        (d / "benchmark.json").write_text(json.dumps({"full_id": full_id}))
        (d / "estimates.json").write_text(
            json.dumps({"median": {"point_estimate": median_ns}, "mean": {"point_estimate": 0}})
        )

    def test_median_comes_from_the_new_estimates(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._bench(root, "array_4_decode", "nested/array_4/decode", 265.1)
            self.assertAlmostEqual(
                measure.criterion_median_ns(root, "nested/array_4/decode"), 265.1
            )

    def test_the_previous_run_is_not_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._bench(root, "array_4_decode", "nested/array_4/decode", 265.1)
            self._bench(root, "array_4_decode", "nested/array_4/decode", 999.9, which="base")
            self.assertAlmostEqual(
                measure.criterion_median_ns(root, "nested/array_4/decode"), 265.1
            )

    def test_a_missing_bench_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(FileNotFoundError):
                measure.criterion_median_ns(Path(tmp), "nested/array_4/decode")


class BenchConstants(unittest.TestCase):
    """The micro table's byte counts are computed from the bench's own literal
    shape, so they cannot drift out of agreement with it silently."""

    def test_array_literal_lengths(self):
        self.assertEqual(measure.array_literal_bytes(4), 49)
        self.assertEqual(measure.array_literal_bytes(50), 601)

    def test_record_literal_length(self):
        self.assertEqual(len(measure._RECORD_LITERAL), 42)

    def test_the_table_rows_carry_those_lengths(self):
        self.assertEqual([row[1] for row in measure._MICRO_ROWS], [49, 601, 42])


class Scripts(unittest.TestCase):
    def test_every_command_times_exactly_one_thing(self):
        for command in (
            "parse",
            "parse-preamble",
            "parse-cache-out",
            "query-typed",
            "query-strings",
            "query-nomatch",
            "dd",
        ):
            with self.subTest(command=command):
                self.assertEqual(measure._script(command).count("time "), 1)

    def test_no_command_redirects_stderr_inside_the_timer(self):
        # Some shells route `time`'s own report through the timed command's
        # redirection, which deletes the figure.
        for command in ("parse", "query-typed", "parse-cache-out", "dd"):
            with self.subTest(command=command):
                self.assertNotIn("2>", measure._script(command))

    def test_the_cache_removal_sits_outside_the_timer(self):
        script = measure._script("parse-cache-out")
        self.assertLess(script.index("rm -f"), script.index("time "))

    def test_an_unknown_command_is_an_error(self):
        with self.assertRaises(ValueError):
            measure._script("no-such-command")


class Selection(unittest.TestCase):
    def test_a_shared_reading_pulls_its_source_in(self):
        got = [f.id for f in measure.resolve_selection(["scan-throughput-warm"])]
        self.assertIn("census-brace-free", got)

    def test_the_source_runs_first(self):
        got = [f.id for f in measure.resolve_selection(["scan-throughput-warm"])]
        self.assertLess(got.index("census-brace-free"), got.index("scan-throughput-warm"))

    def test_unknown_figure_is_refused(self):
        with self.assertRaises(SystemExit):
            measure.resolve_selection(["no-such-figure"])

    def test_selection_is_deduplicated(self):
        got = [f.id for f in measure.resolve_selection(["map-only", "map-only"])]
        self.assertEqual(got, ["map-only"])


class Register(unittest.TestCase):
    def test_ids_are_unique(self):
        ids = [f.id for f in measure.FIGURES]
        self.assertEqual(len(ids), len(set(ids)))

    def test_every_figure_declares_what_invalidates_it(self):
        # A figure that cannot say what invalidates it is one nobody has
        # thought about.
        for fig in measure.FIGURES:
            with self.subTest(figure=fig.id):
                self.assertTrue(fig.depends)

    def test_every_declared_path_exists(self):
        for fig in measure.FIGURES:
            for dep in fig.depends:
                with self.subTest(figure=fig.id, dep=dep):
                    self.assertTrue((measure.REPO / dep).exists(), dep)

    def test_every_named_input_is_defined(self):
        for fig in measure.FIGURES:
            for name in (*fig.cold_inputs, *fig.warm_inputs):
                with self.subTest(figure=fig.id, input=name):
                    self.assertIn(name, measure.INPUTS)

    def test_a_borrower_is_declared_after_what_it_borrows_from(self):
        order = [f.id for f in measure.FIGURES]
        for fig in measure.FIGURES:
            for dep in fig.requires:
                with self.subTest(figure=fig.id, requires=dep):
                    self.assertLess(order.index(dep), order.index(fig.id))

    def test_every_generator_named_by_an_input_exists(self):
        for spec in measure.INPUTS.values():
            with self.subTest(input=spec.name):
                self.assertTrue((measure.SCRIPTS / spec.generator).exists())


class Staleness(unittest.TestCase):
    def test_a_file_under_a_declared_directory_counts(self):
        touched = dict(
            (f.id, hits) for f, hits in measure.figures_touched(["pgdump_query/src/map.rs"])
        )
        self.assertIn("census-brace-free", touched)
        self.assertIn("census-arrays", touched)

    def test_an_unrelated_path_touches_nothing(self):
        self.assertEqual(measure.figures_touched(["README.md"]), [])

    def test_a_generator_change_reaches_the_figures_taken_on_its_output(self):
        touched = [f.id for f, _ in measure.figures_touched(["scripts/generate_perf_data.py"])]
        self.assertIn("nested-end-to-end", touched)
        self.assertIn("scan-throughput-cold", touched)

    def test_the_bench_source_reaches_the_micro(self):
        touched = [
            f.id for f, _ in measure.figures_touched(["pgdump_query/benches/decoders.rs"])
        ]
        self.assertEqual(touched, ["nested-decode-micro"])


class Stamp(unittest.TestCase):
    def test_the_stamp_line_yields_a_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(
                "**Session stamp.** Every figure below was taken by `scripts/measure.py` on "
                "2026-08-28, against commit `a376d47`.\n"
            )
            self.assertEqual(measure.stamped_commit(doc), "a376d47")

    def test_an_unstamped_doc_yields_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text("# Measurements\n")
            self.assertIsNone(measure.stamped_commit(doc))

    def test_the_stamp_the_harness_emits_is_the_one_it_can_read_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(measure.session_stamp("deadbee", dirty=False) + "\n")
            self.assertEqual(measure.stamped_commit(doc), "deadbee")

    def test_a_tree_dirty_under_a_measured_path_says_so(self):
        self.assertIn("uncommitted", measure.session_stamp("deadbee", dirty=True))

    def test_a_clean_stamp_claims_nothing_extra(self):
        self.assertNotIn("uncommitted", measure.session_stamp("deadbee", dirty=False))


class Apparatus(unittest.TestCase):
    def test_the_recorded_apparatus_is_publishable(self):
        self.assertTrue(measure.Config().publishable)

    def test_a_smaller_input_is_not(self):
        self.assertFalse(measure.Config(size_gib=0.05).publishable)

    def test_an_overridden_rep_count_is_not(self):
        self.assertFalse(measure.Config(reps_override=1).publishable)

    def test_size_scales_the_generator_argument(self):
        cfg = measure.Config(size_gib=0.5)
        argv = measure.INPUTS["control"].argv(cfg, Path("/tmp/out.sql"))
        self.assertIn("512", argv)
        self.assertIn("--size-mb", argv)

    def test_block_inputs_do_not_scale(self):
        cfg = measure.Config(size_gib=0.5)
        argv = measure.INPUTS["blocks500"].argv(cfg, Path("/tmp/out.sql"))
        self.assertEqual(argv[-4:], ["--blocks", "500", "--out", "/tmp/out.sql"])

    def test_the_output_path_is_the_last_argument(self):
        cfg = measure.Config()
        for spec in measure.INPUTS.values():
            with self.subTest(input=spec.name):
                self.assertEqual(spec.argv(cfg, Path("/tmp/out.sql"))[-1], "/tmp/out.sql")


class Stamps(unittest.TestCase):
    def test_the_input_stamp_changes_with_the_arguments(self):
        cfg = measure.Config()
        self.assertNotEqual(
            measure.input_stamp(measure.INPUTS["control"], cfg),
            measure.input_stamp(measure.INPUTS["arrays"], cfg),
        )

    def test_the_input_stamp_changes_with_the_size(self):
        self.assertNotEqual(
            measure.input_stamp(measure.INPUTS["control"], measure.Config(size_gib=3.0)),
            measure.input_stamp(measure.INPUTS["control"], measure.Config(size_gib=1.0)),
        )

    def test_the_input_stamp_is_stable(self):
        cfg = measure.Config()
        self.assertEqual(
            measure.input_stamp(measure.INPUTS["control"], cfg),
            measure.input_stamp(measure.INPUTS["control"], cfg),
        )


if __name__ == "__main__":
    unittest.main()


class Eviction(unittest.TestCase):
    """The tmpfs budget is smaller than the union of the inputs, so the sweep
    is staged. What must never happen is evicting something the figure in hand
    still needs, or thrashing an input the next figure wants."""

    def _stager(self, budget: float):
        cfg = measure.Config(dry_run=True, warm_budget=budget)
        return measure.Stager(cfg, lambda _msg: None)

    def test_an_input_nothing_wants_again_goes_first(self):
        stager = self._stager(8)
        stager.needs = {"control": [0, 3], "arrays": [1], "composite": [3]}
        stager._staged = {"control": 3 * measure.GIB, "arrays": 3 * measure.GIB}
        stager._make_room(3 * measure.GIB, figure_index=3)
        self.assertNotIn("arrays", stager._staged)
        self.assertIn("control", stager._staged)

    def test_what_the_current_figure_needs_is_never_the_victim(self):
        stager = self._stager(3)
        stager.needs = {"control": [0]}
        stager._staged = {"control": 3 * measure.GIB}
        with self.assertRaises(RuntimeError):
            stager._make_room(3 * measure.GIB, figure_index=0)

    def test_a_budget_too_small_for_one_input_is_an_error(self):
        stager = self._stager(1)
        stager.needs = {}
        with self.assertRaises(RuntimeError):
            stager._make_room(3 * measure.GIB, figure_index=0)

    def test_room_already_free_evicts_nothing(self):
        stager = self._stager(9)
        stager.needs = {"control": [0, 5]}
        stager._staged = {"control": 3 * measure.GIB}
        stager._make_room(3 * measure.GIB, figure_index=1)
        self.assertEqual(list(stager._staged), ["control"])

    def test_the_full_sweep_fits_the_default_budget(self):
        # Every figure's own inputs must fit at once, or the sweep cannot run
        # at all -- which is a planning error, not a runtime one.
        cfg = measure.Config()
        for fig in measure.FIGURES:
            need = sum(measure.nominal_size(cfg, n) for n in fig.warm_inputs)
            with self.subTest(figure=fig.id):
                self.assertLessEqual(need, cfg.warm_budget * measure.GIB)
