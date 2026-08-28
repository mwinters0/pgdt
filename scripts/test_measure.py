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

import collections
import json
import tempfile
import unittest
import unittest.mock
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
        ids = [f.id for f in measure.ALL_FIGURES]
        self.assertEqual(len(ids), len(set(ids)))

    def test_every_figure_declares_what_invalidates_it(self):
        # A figure that cannot say what invalidates it is one nobody has
        # thought about.
        for fig in measure.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertTrue(fig.depends)

    def test_every_declared_path_exists(self):
        for fig in measure.ALL_FIGURES:
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
    def test_a_stamp_wrapped_across_lines_still_yields_a_commit(self):
        # The stamp is prose in a hard-wrapped doc, so it is never one line.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(
                "**Session stamp.** Every figure below was taken by `scripts/measure.py` on\n"
                "2026-08-28, against commit `3739c26`. One sweep, one apparatus.\n"
            )
            self.assertEqual(measure.stamped_commit(doc), "3739c26")

    def test_the_stamp_does_not_reach_across_the_document(self):
        # A bounded run, so "measure.py" in one paragraph cannot bind to a
        # commit hash mentioned much later.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text("measure.py\n" + ("filler line\n" * 40) + "commit `abc1234`\n")
            self.assertIsNone(measure.stamped_commit(doc))

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

    def _stager(self, budget: float | None):
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

    def test_the_budget_is_the_largest_figure_plus_headroom(self):
        stager = self._stager(None)
        stager.plan(measure.FIGURES)
        largest = max(stager.figure_need.values())
        self.assertEqual(stager.budget(), int(largest * measure.WARM_MARGIN))

    def test_every_figure_fits_the_computed_budget(self):
        stager = self._stager(None)
        stager.plan(measure.FIGURES)
        budget = stager.budget()
        for fig in measure.FIGURES:
            with self.subTest(figure=fig.id):
                self.assertLessEqual(stager.figure_need[fig.id], budget)

    def test_a_generator_overshooting_its_target_does_not_break_the_budget(self):
        # The real defect: three inputs whose nominal sum is exactly the budget
        # overshot it by 6,799 bytes, and the sweep died twenty minutes in.
        # The margin has to absorb that, and the check has to use real sizes.
        stager = self._stager(None)
        stager.plan(measure.FIGURES)
        largest = max(stager.figure_need.values())
        self.assertGreater(stager.budget() - largest, 64 * 1024)

    def test_an_explicit_budget_overrides_the_computed_one(self):
        stager = self._stager(4)
        stager.plan(measure.FIGURES)
        self.assertEqual(stager.budget(), 4 * measure.GIB)

    def test_a_figure_too_big_for_an_explicit_budget_is_refused_before_any_run(self):
        stager = self._stager(1)
        stager.plan(measure.FIGURES)
        problems = stager.preflight(measure.FIGURES)
        self.assertTrue(any("over the" in p for p in problems))

    def test_a_staging_area_too_small_is_refused_before_any_run(self):
        stager = self._stager(None)
        stager.plan(measure.FIGURES)
        tiny = collections.namedtuple("usage", "total used free")(0, 0, 1 * measure.GIB)
        with unittest.mock.patch.object(measure.shutil, "disk_usage", return_value=tiny):
            problems = stager.preflight(measure.FIGURES)
        self.assertTrue(any("usable" in p for p in problems), problems)


class Consumers(unittest.TestCase):
    """`depends` is the edge into a figure; `quoted_by` is the edge out. A
    figure whose numbers are repeated somewhere and does not say where is how
    `architecture.md` came to quote a save count `measurements.md` no longer
    holds."""

    def test_every_figure_names_its_consumers(self):
        for fig in measure.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertTrue(fig.quoted_by)

    def test_every_consumer_exists(self):
        for fig in measure.ALL_FIGURES:
            for path in fig.quoted_by:
                with self.subTest(figure=fig.id, path=path):
                    self.assertTrue((measure.REPO / path).exists(), path)

    def test_a_figure_does_not_list_the_doc_it_lives_in(self):
        # measurements.md is where the table goes, not somewhere that repeats
        # it; listing it would make every fold-in look like a cross-doc edit.
        for fig in measure.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertNotIn("docs/design/measurements.md", fig.quoted_by)


class Markers(unittest.TestCase):
    """The doc addresses a figure by id, never by heading — so a heading is
    free to quote a number and free to be rewritten when the number moves."""

    def _doc(self, tmp: str, text: str) -> Path:
        doc = Path(tmp) / "measurements.md"
        doc.write_text(text)
        return doc

    def test_a_marker_is_read_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(tmp, "## Anything at all\n\n<!-- figure: map-only -->\n")
            self.assertEqual(measure.markers_in(doc), ["map-only"])

    def test_the_emitted_marker_is_the_one_that_is_read_back(self):
        # What emit() writes above each table must be what --check finds after
        # the paste, trailing prose in the comment included.
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(
                tmp,
                "<!-- figure: census-arrays — reproduce with `cd scripts && "
                "uv run measure.py --figure census-arrays` -->\n",
            )
            self.assertEqual(measure.markers_in(doc), ["census-arrays"])

    def test_a_heading_that_quotes_a_number_is_not_an_address(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(
                tmp,
                "## The census on brace-free rows costs 6% of a warm scan\n\n"
                "<!-- figure: census-brace-free -->\n",
            )
            self.assertEqual(measure.markers_in(doc), ["census-brace-free"])

    def test_no_marker_is_no_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(measure.markers_in(self._doc(tmp, "# Measurements\n")), [])

    def test_the_doc_and_the_register_agree_exactly(self):
        """Bidirectional, now that every figure has been folded in: a rename on
        either side breaks this rather than going unnoticed."""
        markers = measure.markers_in(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(sorted(markers), sorted(f.id for f in measure.ALL_FIGURES))

    def test_no_figure_is_marked_twice(self):
        # One figure is one table.
        markers = measure.markers_in(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(len(markers), len(set(markers)))

    def test_the_doc_s_session_stamp_names_a_commit(self):
        # `--stale` defaults to it, so an unparseable stamp silently disarms
        # the only mechanism that says a figure has gone stale.
        stamp = measure.stamped_commit(measure.REPO / "docs/design/measurements.md")
        self.assertIsNotNone(stamp)


class KojiRecipe(unittest.TestCase):
    """koji is never run from here, but the invocation is owned here — three
    hand-maintained copies is how a documented command was found that could
    not execute. Each assertion below is a mistake that has cost a run."""

    def _recipe(self, wrap=False) -> str:
        return measure.koji_recipe(measure.Config(), "pgdq-koji", wrap)

    def test_pgdq_is_pid_one(self):
        # A compound command cannot be exec'd, so nothing may be appended to
        # report the exit status: `sh` would take the signal and not forward
        # it, the runtime's SIGKILL would follow, and the interrupt guard would
        # never run.
        self.assertIn("sh -c 'exec /pgdq parse", self._recipe())

    def test_nothing_follows_the_parse_inside_the_shell(self):
        for line in self._recipe().splitlines():
            if "exec /pgdq parse" in line:
                with self.subTest(line=line):
                    self.assertNotIn("; echo", line)

    def test_the_cgroup_limit_is_part_of_the_apparatus(self):
        self.assertIn("-m 512m --memory-swap 512m", self._recipe())

    def test_the_cache_lands_in_the_mounted_volume(self):
        # The dump is mounted read-only, so the colocated default would land in
        # the container's ephemeral layer and die with it — an hour of scanning
        # lost with no error, because the write itself succeeds.
        self.assertIn("--dqcache /out/", self._recipe())
        self.assertNotIn("--dqcache /dump.sql", self._recipe())

    def test_the_dump_is_read_only(self):
        self.assertIn(":/dump.sql:ro", self._recipe())

    def test_the_exit_status_is_read_from_inspect(self):
        self.assertIn("{{.State.ExitCode}}", self._recipe())

    def test_the_wrap_recipe_stops_reports_resumes_and_compares(self):
        wrap = self._recipe(wrap=True)
        for fragment in ("nerdctl stop", "info --dqcache", "--verbose", "cmp "):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, wrap)

    def test_the_wrap_resumes_the_identical_command(self):
        wrap = self._recipe(wrap=True)
        legs = [ln for ln in wrap.splitlines() if "exec /pgdq parse" in ln]
        self.assertEqual(len(legs), 2)
        self.assertEqual(legs[0], legs[1])

    def test_every_continued_line_carries_its_continuation(self):
        # A dropped trailing backslash silently splits one command into two.
        for recipe in (self._recipe(), self._recipe(wrap=True)):
            lines = recipe.splitlines()
            for i, line in enumerate(lines[:-1]):
                if line.strip().startswith("-v ") and not lines[i + 1].strip().startswith("-v "):
                    with self.subTest(line=line):
                        self.assertTrue(line.rstrip().endswith("\\"), line)


class SharedSections(unittest.TestCase):
    def test_the_two_throughput_tables_share_one_section(self):
        # The INSERT path's per-byte CPU is then a division within one place,
        # which is the defect that allocated the warm table.
        cold = measure.FIGURES_BY_ID["scan-throughput-cold"]
        warm = measure.FIGURES_BY_ID["scan-throughput-warm"]
        self.assertEqual(cold.section, warm.section)

    def test_a_shared_section_labels_each_table(self):
        by_section: dict[str, list[measure.Figure]] = {}
        for fig in measure.FIGURES:
            by_section.setdefault(fig.section, []).append(fig)
        for section, figs in by_section.items():
            if len(figs) > 1:
                for fig in figs:
                    with self.subTest(section=section, figure=fig.id):
                        self.assertTrue(fig.table_label)


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
            a = self._sweep(tmp, "a", {"census-brace-free/pgdq/control/parse/warm": [1.0, 1.0]})
            b = self._sweep(tmp, "b", {"census-brace-free/pgdq/control/parse/warm": [1.1, 1.1]})
            table = measure.drift_table(a / "raw.json", b / "raw.json")
            self.assertIn("+10.0%", table)

    def test_a_faster_second_sweep_reads_negative(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/pgdq/control/parse/warm": [2.0]})
            b = self._sweep(tmp, "b", {"f/pgdq/control/parse/warm": [1.0]})
            self.assertIn("-50.0%", measure.drift_table(a / "raw.json", b / "raw.json"))

    def test_only_shared_readings_are_compared(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/a": [1.0], "f/only-in-a": [1.0]})
            b = self._sweep(tmp, "b", {"f/a": [1.0], "f/only-in-b": [1.0]})
            table = measure.drift_table(a / "raw.json", b / "raw.json")
            self.assertNotIn("only-in-a", table)
            self.assertNotIn("only-in-b", table)

    def test_two_sweeps_sharing_nothing_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/a": [1.0]})
            b = self._sweep(tmp, "b", {"f/b": [1.0]})
            with self.assertRaises(ValueError):
                measure.drift_table(a / "raw.json", b / "raw.json")

    def test_the_summary_quotes_the_median_and_the_worst(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/x": [1.0], "f/y": [1.0], "f/z": [1.0]})
            b = self._sweep(tmp, "b", {"f/x": [1.01], "f/y": [1.05], "f/z": [1.20]})
            table = measure.drift_table(a / "raw.json", b / "raw.json")
            self.assertIn("median absolute drift is **5.0%**", table)
            self.assertIn("largest is **20.0%**", table)

    def test_the_derived_figure_is_not_in_the_sweep(self):
        # It is computed across two sweeps, so `--all` must not try to run it.
        self.assertNotIn("session-drift", [f.id for f in measure.FIGURES])
        self.assertIn("session-drift", measure.ALL_BY_ID)
