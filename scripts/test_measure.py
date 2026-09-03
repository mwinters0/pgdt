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
import re
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
    #: The timer, where a timed command can start: the whole script, or after a
    #: `;`. Not a bare `time ` anywhere in the string — the perf table has a
    #: column named `v_time`, so a projection's flags carry that substring
    #: without timing anything.
    TIMER = re.compile(r"(?:\A|; )time ")

    def test_every_command_times_exactly_one_thing(self):
        for command in (
            "parse",
            "parse-preamble",
            "parse-cache-out",
            "query-typed",
            "query-strings",
            "query-nomatch",
            *(f"query-project-{w}" for w in measure.PROJECTION_WIDTHS),
            "dd",
        ):
            with self.subTest(command=command):
                self.assertEqual(len(self.TIMER.findall(measure._script(command))), 1)

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

    def test_a_width_the_register_does_not_carry_is_an_error(self):
        # The command shape is parsed rather than matched, so an unregistered
        # width has to be refused explicitly or it would run a query with no
        # projection flags at all and be read as a width.
        for command in ("query-project-7", "query-project-", "query-project-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    measure._script(command)


class Allocator(unittest.TestCase):
    """The allocator figure: three binaries, three shapes, one table.

    Every assertion here is a way to get a plausible table of the wrong
    comparison, which is the same family of failure as profiling the `release`
    binary and labelling it `profiling`. Two are about the *reference* column
    and are the ones the design turns on: it must be the binary every other
    figure was taken with, and its readings must be shared rather than retaken,
    or the doc carries two numbers for one measurement.
    """

    def test_the_reference_leg_is_the_shipped_binary(self):
        # Not a fourth build of the same source: two builds of one source
        # differ by ~10% from code layout alone, which is larger than the
        # effect being measured.
        specs = measure._allocator_specs("system")
        reference = [s for s in specs if s.command == "parse" and s.binary == "pgdq"]
        self.assertEqual(len(reference), 1)

    def test_every_other_leg_is_its_own_build(self):
        specs = measure._allocator_specs("system")
        binaries = {s.binary for s in specs if s.command == "parse"}
        self.assertEqual(binaries, {"pgdq", "alloc:jemalloc", "alloc:mimalloc"})

    def test_the_reference_is_read_off_the_binary_not_assumed(self):
        # Adopt a leg and it becomes the reference with no code change; assume
        # `system` instead and the figure would compare a leg against itself.
        for reference in measure.ALLOCATOR_LEGS:
            with self.subTest(reference=reference):
                columns = measure.allocator_columns(reference)
                self.assertEqual(columns[0], reference)
                self.assertEqual(sorted(columns), sorted(measure.ALLOCATOR_LEGS))
                specs = measure._allocator_specs(reference)
                shipped = {s.binary for s in specs if s.command == "parse"}
                self.assertIn("pgdq", shipped)
                self.assertNotIn(f"alloc:{reference}", shipped)

    def test_an_unknown_reference_is_an_error(self):
        with self.assertRaises(ValueError):
            measure._allocator_specs("tcmalloc")

    def test_every_shape_is_run_on_every_leg(self):
        specs = measure._allocator_specs("system")
        timed = collections.Counter(
            (s.command, s.binary) for s in specs if s.command != "dd"
        )
        self.assertEqual(len(timed), len(measure._ALLOCATOR_SHAPES) * 3)
        self.assertEqual(set(timed.values()), {1})

    def test_the_floor_is_one_row_not_one_per_leg(self):
        # `dd` links no allocator, so three floor readings would be three
        # readings of one thing.
        specs = measure._allocator_specs("system")
        floors = [s for s in specs if s.command == "dd"]
        self.assertEqual(len(floors), 1)
        self.assertEqual(floors[0].binary, "none")
        self.assertIs(floors[0], specs[-1])

    def test_every_shape_is_warm_on_the_control(self):
        for spec in measure._allocator_specs("system"):
            with self.subTest(spec=spec.label):
                self.assertEqual(spec.regime, "warm")
                self.assertEqual(spec.input, "control")

    def test_the_shapes_are_the_three_the_baseline_quotes(self):
        commands = [c for c, _, _ in measure._ALLOCATOR_SHAPES]
        self.assertEqual(commands, ["parse", "query-strings", "query-typed"])
        for command in commands:
            with self.subTest(command=command):
                # A shape the sweep does not time is a shape no figure can be
                # read against.
                self.assertIn("time /pgdq", measure._script(command))

    def test_each_shape_names_a_figure_that_takes_its_reference_reading(self):
        # The borrow is what keeps one number in the doc per measurement, and
        # it silently does nothing if the source figure never takes that spec.
        order = [f.id for f in measure.FIGURES]
        for command, _, source in measure._ALLOCATOR_SHAPES:
            with self.subTest(command=command):
                self.assertIn(source, measure.FIGURES_BY_ID)
                self.assertLess(order.index(source), order.index("allocator"))
        census = measure._census_specs("control", "warm")
        self.assertIn(
            measure.RunSpec("pgdq", "control", "parse", "warm", "").key("census-brace-free"),
            {s.key("census-brace-free") for s in census},
        )
        nested = measure._nested_specs()
        for command in ("query-strings", "query-typed"):
            with self.subTest(command=command):
                self.assertIn(
                    measure.RunSpec("pgdq", "control", command, "warm", "").key(
                        "nested-end-to-end"
                    ),
                    {s.key("nested-end-to-end") for s in nested},
                )

    def test_a_version_string_yields_its_allocator(self):
        with unittest.mock.patch.object(
            measure, "run", return_value="pgdq 0.1.0 (allocator: mimalloc)\n"
        ):
            self.assertEqual(measure.binary_allocator(Path("/pgdq")), "mimalloc")

    def test_a_binary_that_names_no_allocator_is_an_error(self):
        # Not a default: a binary too old to report it would otherwise be
        # published as the reference leg under a name nothing checked.
        with unittest.mock.patch.object(measure, "run", return_value="pgdq 0.1.0\n"):
            with self.assertRaises(RuntimeError):
                measure.binary_allocator(Path("/pgdq"))

    def test_an_unknown_leg_is_never_built(self):
        with self.assertRaises(ValueError):
            measure.ensure_allocator_binary(measure.Config(), "tcmalloc", lambda _: None)

    def test_a_leg_builds_with_no_default_features_and_its_own_target_dir(self):
        # Both flags are load-bearing. Without `--no-default-features` the
        # reference leg stops being the platform allocator the day one is
        # adopted, so the figure stops being re-takeable at the moment it
        # matters; without its own `--target-dir` a `--features` build
        # overwrites `target/release/pgdq` and every other figure in the same
        # sweep is timed under the wrong allocator.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )
            calls = []

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                calls.append(list(argv))
                built = cfg.alloc_build_root / "jemalloc" / "release"
                built.mkdir(parents=True, exist_ok=True)
                (built / "pgdq").write_text("#!/bin/true\n")
                return ""

            with unittest.mock.patch.object(measure, "run", fake_run), \
                 unittest.mock.patch.object(
                     measure, "binary_allocator", return_value="jemalloc"
                 ):
                out = measure.ensure_allocator_binary(cfg, "jemalloc", lambda _: None)
            self.assertEqual(len(calls), 1)
            argv = calls[0]
            self.assertIn("--no-default-features", argv)
            self.assertEqual(argv[argv.index("--features") + 1], "jemalloc")
            self.assertEqual(
                argv[argv.index("--target-dir") + 1],
                str(cfg.alloc_build_root / "jemalloc"),
            )
            self.assertEqual(out, cfg.out_dir / "pgdq-alloc-jemalloc")

    def test_a_leg_whose_build_dropped_its_feature_is_refused(self):
        # The build succeeds and produces a working binary, so nothing else
        # would notice: the table would compare two identical binaries.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                built = cfg.alloc_build_root / "mimalloc" / "release"
                built.mkdir(parents=True, exist_ok=True)
                (built / "pgdq").write_text("#!/bin/true\n")
                return ""

            with unittest.mock.patch.object(measure, "run", fake_run), \
                 unittest.mock.patch.object(
                     measure, "binary_allocator", return_value="system"
                 ):
                with self.assertRaises(RuntimeError):
                    measure.ensure_allocator_binary(cfg, "mimalloc", lambda _: None)
            self.assertFalse((cfg.out_dir / "pgdq-alloc-mimalloc").exists())

    def test_an_allocator_leg_resolves_to_a_binary(self):
        session = measure.Session(measure.Config(dry_run=True), None, lambda _: None)
        self.assertEqual(
            session.binary_path("alloc:jemalloc"),
            measure.Config().out_dir / "pgdq-alloc-jemalloc",
        )

    def test_an_unknown_binary_is_still_an_error(self):
        session = measure.Session(measure.Config(dry_run=True), None, lambda _: None)
        with self.assertRaises(ValueError):
            session.binary_path("alloc")

    def test_the_stamp_names_the_allocator_it_was_given(self):
        stamp = measure.session_stamp("deadbee", dirty=False, allocator="mimalloc")
        self.assertIn("mimalloc", stamp)
        # And still reads back as a stamp: the commit is what `--stale`
        # resolves, and the allocator sits after it.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(stamp + "\n")
            self.assertEqual(measure.stamped_commit(doc), "deadbee")

    def test_a_stamp_with_no_binary_to_ask_names_no_allocator(self):
        self.assertNotIn("allocator", measure.session_stamp("deadbee", dirty=False))


class ProjectionWidths(unittest.TestCase):
    """One file read at five widths, which is the whole instrument.

    Two things make the adjacent-row subtraction mean what the table says it
    means: each width is a superset of the one above it, so their difference is
    exactly the columns they differ by; and every name is one the generator
    actually writes, so the query does not fail three minutes into a sweep."""

    def test_each_width_names_that_many_columns(self):
        for width, names in measure.PROJECTION_WIDTHS.items():
            with self.subTest(width=width):
                self.assertEqual(len(names), width)

    def test_no_width_repeats_a_column(self):
        for width, names in measure.PROJECTION_WIDTHS.items():
            with self.subTest(width=width):
                self.assertEqual(len(set(names)), len(names))

    def test_every_name_is_a_column_the_generator_writes(self):
        emitted = {
            name
            for name, _ in (
                *measure.perf.COLUMNS,
                *measure.perf.ARRAY_COLUMNS,
                *measure.perf.COMPOSITE_COLUMNS,
            )
        }
        for width, names in measure.PROJECTION_WIDTHS.items():
            for name in names:
                with self.subTest(width=width, column=name):
                    self.assertIn(name, emitted)

    def test_each_width_contains_the_one_below_it(self):
        widths = sorted(measure.PROJECTION_WIDTHS)
        for narrow, wide in zip(widths, widths[1:]):
            with self.subTest(narrow=narrow, wide=wide):
                self.assertLessEqual(
                    set(measure.PROJECTION_WIDTHS[narrow]),
                    set(measure.PROJECTION_WIDTHS[wide]),
                )

    def test_zero_columns_asks_for_no_columns_rather_than_nothing(self):
        # An empty repetition of `--column` is an unprojected query, which
        # would silently make the floor row the widest row.
        self.assertEqual(measure.projection_flags(0), "--no-columns")
        script = measure._script("query-project-0")
        self.assertIn("--no-columns", script)
        self.assertNotIn("--column", script)

    def test_a_width_repeats_the_flag_once_per_column(self):
        self.assertEqual(measure.projection_flags(16).count("--column "), 16)
        self.assertNotIn(",", measure.projection_flags(16))

    def test_every_run_is_typed(self):
        # `strings` builds every column the same cheap way, so a table taken
        # that way would measure nothing this figure is about.
        for width in measure.PROJECTION_WIDTHS:
            with self.subTest(width=width):
                self.assertIn("--schema-mode typed", measure._script(f"query-project-{width}"))

    def test_the_table_rows_are_the_registered_widths_ascending(self):
        widths = [w for w, _, _ in measure._PROJECTION_ROWS]
        self.assertEqual(widths, sorted(measure.PROJECTION_WIDTHS))

    def test_the_figure_is_taken_on_the_nineteen_column_file(self):
        fig = measure.SELECTABLE_BY_ID["projection-widths"]
        self.assertEqual(fig.warm_inputs, ("arrays",))
        self.assertEqual(measure.INPUTS["arrays"].args[:2], ("--arrays", "--composite"))


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

    def test_every_defined_input_is_consumed(self):
        """An input no figure takes is a control that goes stale in silence.

        `one_block` was exactly that: the doc quoted a one-block parse at
        "under 10 ms" and gave the command that regenerates the *input*, while
        no figure took the reading — so nothing re-took it when the code it
        controls for moved."""
        used = {n for f in measure.SELECTABLE for n in (*f.cold_inputs, *f.warm_inputs)}
        self.assertEqual(sorted(set(measure.INPUTS) - used), [])

    def test_the_quadratic_carries_its_one_block_control(self):
        # The series says a cost rises with block count; only the control says
        # how much of the cost *is* block count.
        fig = measure.FIGURES_BY_ID["per-block-quadratic"]
        self.assertIn("one_block", fig.warm_inputs)
        self.assertEqual(measure._QUADRATIC_ROWS[0][0], "one_block")

    def test_every_generator_named_by_an_input_exists(self):
        for spec in measure.INPUTS.values():
            with self.subTest(input=spec.name):
                self.assertTrue((measure.SCRIPTS / spec.generator).exists())


class Untaken(unittest.TestCase):
    """An instrument that is built and whose figure has not been taken.

    It has to be selectable and runnable, and it must not be mistaken for a
    figure the doc is missing — those pull in opposite directions, which is why
    the two registers are separate.

    **`measure.UNTAKEN` is empty as this stands, so every case below is
    vacuous.** They are kept rather than deleted with the last entry: an
    instrument is registered here the moment one is built, and a register whose
    checks were deleted along with its contents acquires an entry with nothing
    holding it."""

    def test_an_untaken_instrument_is_not_a_figure_the_doc_must_carry(self):
        for fig in measure.UNTAKEN:
            with self.subTest(figure=fig.id):
                self.assertNotIn(fig, measure.ALL_FIGURES)

    def test_a_sweep_does_not_take_it(self):
        # `--all` is `FIGURES`; an untaken instrument needs asking for by name.
        for fig in measure.UNTAKEN:
            with self.subTest(figure=fig.id):
                self.assertNotIn(fig.id, [f.id for f in measure.FIGURES])

    def test_it_is_still_selectable_by_name(self):
        for fig in measure.UNTAKEN:
            with self.subTest(figure=fig.id):
                got = [f.id for f in measure.resolve_selection([fig.id])]
                self.assertEqual(got, [fig.id])

    def test_its_inputs_and_declared_paths_are_real(self):
        # The structural checks `Register` applies to a figure apply here too:
        # an instrument nobody can stage is not built.
        for fig in measure.UNTAKEN:
            for name in (*fig.cold_inputs, *fig.warm_inputs):
                with self.subTest(figure=fig.id, input=name):
                    self.assertIn(name, measure.INPUTS)
            for path in fig.depends:
                with self.subTest(figure=fig.id, path=path):
                    self.assertTrue((measure.REPO / path).exists(), path)

    def test_no_id_collides_with_a_figure(self):
        ids = [f.id for f in measure.ALL_FIGURES] + [f.id for f in measure.UNTAKEN]
        self.assertEqual(len(ids), len(set(ids)))


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


class Acknowledgements(unittest.TestCase):
    """A commit that touched a declared path and moved no reading.

    The mechanism's whole risk is that it becomes a way to wave staleness away,
    so what these tests hold are the refusals: a dirty path is never excused, a
    path is excused only when *every* commit that touched it is, and an excuse
    is scoped to the figures it names.
    """

    ACKS = (
        measure.Acknowledged(commit="aaa", figures=("census-arrays",), why="additive"),
        measure.Acknowledged(commit="bbb", figures=(), why="touches no figure's subject"),
    )

    def test_a_path_whose_only_commit_is_excused_is_excused(self):
        got = measure.excused_paths(
            "census-arrays",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, ["scripts/generate_perf_data.py"])

    def test_an_excuse_does_not_reach_a_figure_it_does_not_name(self):
        got = measure.excused_paths(
            "census-brace-free",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_an_empty_figure_list_excuses_every_figure(self):
        got = measure.excused_paths(
            "census-brace-free",
            ["scripts/measure.py"],
            {"scripts/measure.py": ["bbb"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, ["scripts/measure.py"])

    def test_one_unexamined_commit_keeps_the_path_stale(self):
        # The failure this exists against: a path changed by an excused commit
        # and an unexamined one is stale on the strength of the second.
        got = measure.excused_paths(
            "census-arrays",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "ccc"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_an_uncommitted_path_is_never_excused(self):
        # There is no commit to point at, so nobody has read the diff.
        got = measure.excused_paths(
            "census-arrays",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            {"scripts/generate_perf_data.py"},
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_a_path_with_no_commits_in_range_is_not_excused(self):
        got = measure.excused_paths(
            "census-arrays", ["scripts/generate_perf_data.py"], {}, set(), self.ACKS
        )
        self.assertEqual(got, [])

    def test_an_inert_entry_is_named_with_what_holds_the_path_red(self):
        # The failure this exists against: an entry that excuses one commit on
        # a path another commit also touched vanishes from every output, which
        # reads as a missing entry and has been mistaken for one.
        got = measure.inert_excuses(
            "census-arrays",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "ccc"]},
            self.ACKS,
        )
        self.assertEqual(got, [("scripts/generate_perf_data.py", ["aaa"], ["ccc"])])

    def test_a_path_nothing_excuses_gets_no_commentary(self):
        got = measure.inert_excuses(
            "census-arrays",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["ccc"]},
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_the_live_register_names_only_real_figures(self):
        known = set(measure.ALL_BY_ID)
        for ack in measure.ACKNOWLEDGED:
            for fid in ack.figures:
                with self.subTest(commit=ack.commit, figure=fid):
                    self.assertIn(fid, known)

    def test_every_live_entry_carries_its_evidence(self):
        # An entry may legitimately have no mechanical check, but it may not
        # have no *reason*: `why` is what a reader weighs when the excuse is
        # the only thing between a figure and a re-take.
        for ack in measure.ACKNOWLEDGED:
            with self.subTest(commit=ack.commit):
                self.assertTrue(ack.why.strip())

    def test_a_spent_entry_is_reported_rather_than_kept(self):
        acks = (measure.Acknowledged(commit="aaa", figures=(), why="x"),)
        unknown, spent = measure.acknowledgement_problems(acks, measure.ALL_BY_ID, ["aaa"])
        self.assertEqual(unknown, [])
        self.assertEqual(spent, ["aaa"])

    def test_an_entry_naming_no_figure_is_reported(self):
        acks = (measure.Acknowledged(commit="aaa", figures=("gone",), why="x"),)
        unknown, spent = measure.acknowledgement_problems(acks, measure.ALL_BY_ID, [])
        self.assertEqual(unknown, ["aaa names gone"])
        self.assertEqual(spent, [])


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
            for fig in measure.ALL_FIGURES
            for name in (*fig.cold_inputs, *fig.warm_inputs)
        }
        untaken_only = {
            name
            for fig in measure.UNTAKEN
            for name in (*fig.cold_inputs, *fig.warm_inputs)
        } - published
        self.assertEqual(published & untaken_only, set())
        self.assertIn("composite", published)

    def test_the_verification_size_is_small_enough_to_be_run(self):
        # A verification nobody runs is worth nothing; the sweep's own 3 GiB
        # would make this a minutes-long command per input.
        self.assertLess(measure.VERIFY_SIZE_GIB, 0.1)


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


class ProfileRecipe(unittest.TestCase):
    """The sampling profile is printed here for the same reason koji's scan is,
    but its failure mode is the opposite one. koji's mistakes lose a run
    loudly; every mistake below returns a profile that looks fine and describes
    something else."""

    def _recipe(self) -> str:
        return measure.profile_recipe(measure.Config())

    def test_the_profiling_binary_is_the_one_profiled(self):
        # `release` carries no line tables and no frame pointers, so a profile
        # of it is a flat list of unnameable addresses. The published figures
        # stay on `release`, which is why this is a second binary.
        recipe = self._recipe()
        self.assertIn("target/profiling/pgdq", recipe)
        self.assertNotIn("target/release/pgdq", recipe)

    def test_frame_pointers_come_from_the_build_line(self):
        # Cargo has no profile key for them, so `[profile.profiling]` alone
        # gives line tables and a call graph that stops at the leaf.
        recipe = self._recipe()
        self.assertIn("-C force-frame-pointers=yes", recipe)
        self.assertIn("--profile profiling", recipe)

    def test_the_unwinder_matches_the_build(self):
        # `dwarf` needs debug info this profile does not carry, and its 8 KiB
        # stack copy per sample would change the thing being measured.
        self.assertIn("--call-graph fp", self._recipe())
        self.assertNotIn("--call-graph dwarf", self._recipe())

    def test_the_input_is_warm(self):
        # Off the SSD this profiles `pread` waiting for a device; the
        # proportions the phase reads are CPU proportions. The cold cache dir
        # may therefore appear only as the source of the staging copy.
        cfg = measure.Config()
        recipe = measure.profile_recipe(cfg)
        for name in measure.PROFILE_INPUTS:
            with self.subTest(input=name):
                self.assertIn(f"--source {cfg.warm_dir / f'{name}.sql'}", recipe)
        for line in recipe.splitlines():
            if str(cfg.cache_dir) in line:
                with self.subTest(line=line):
                    self.assertTrue(line.startswith("cp "), line)

    def test_the_libc_frames_are_named_before_anything_is_recorded(self):
        # Without symbols, ~48% of a warm `parse` profile is bare addresses in
        # libc.so.6 — and they are memmove and memset, which is the half of a
        # zero-copy phase's answer. The step must precede the first `perf
        # record`, or the first profile is the unreadable one.
        recipe = self._recipe()
        self.assertIn(measure.DEBUGINFOD, recipe)
        self.assertLess(recipe.index("buildid-cache"), recipe.index("perf record"))

    def test_an_installed_package_is_preferred_over_the_fetch(self):
        # The package is kept in lockstep with libc by the package manager and
        # needs no network; the fetch is neither. So the recipe looks under
        # /usr/lib/debug first and only a miss there reaches debuginfod — and
        # it looks by *build ID*, which is the only thing that separates
        # symbols that match from symbols that merely have the right filename.
        recipe = self._recipe()
        self.assertIn("/usr/lib/debug/.build-id/", recipe)
        self.assertIn('BID=$(readelf -n "$LIBC"', recipe)
        self.assertLess(
            recipe.index("/usr/lib/debug/.build-id/"), recipe.index("--debuginfod")
        )

    def test_the_fetch_is_perfs_own_and_not_a_hand_rolled_one(self):
        # A `curl` into ~/.debug re-derives perf's cache layout from outside,
        # and a path construction that drifts fails silently: the `debug` file
        # lands where perf does not read it and the profile is bare addresses
        # with no error. `perf buildid-cache --debuginfod` is perf writing its
        # own cache. The flag lives on that subcommand alone.
        recipe = self._recipe()
        self.assertIn(f"{measure.PERF} buildid-cache --debuginfod=", recipe)
        self.assertNotIn("curl", recipe)

    def test_the_step_says_which_source_the_symbols_came_from(self):
        # Both ways of getting this wrong — a skewed package, a fetch that
        # failed — return a profile that looks entirely plausible, so the one
        # cheap defence is that the recipe says out loud which source the
        # `perf record`s below are reading.
        echoes = [
            ln for ln in self._recipe().splitlines() if ln.strip().startswith("echo ")
        ]
        self.assertTrue(echoes)
        for line in echoes:
            with self.subTest(line=line):
                self.assertIn("libc symbols:", line)

    def test_the_step_numbering_has_no_hole_without_debuginfod(self):
        # The step is optional — right on a machine whose libc carries symbols.
        with unittest.mock.patch.object(measure, "DEBUGINFOD", ""):
            recipe = measure.profile_recipe(measure.Config())
        self.assertNotIn("debuginfod", recipe)
        numbered = [
            int(ln.split(".")[0][2:]) for ln in recipe.splitlines()
            if re.match(r"^# \d+\. ", ln)
        ]
        self.assertEqual(numbered, list(range(len(numbered))))

    def test_no_container_is_involved(self):
        # A profile is about proportions, and the 512 MB cgroup adds capability
        # plumbing without changing them.
        self.assertNotIn("nerdctl", self._recipe())
        self.assertNotIn("--memory-swap", self._recipe())

    def test_every_shape_is_profiled_over_every_input(self):
        recipe = self._recipe()
        for name in measure.PROFILE_INPUTS:
            for shape in measure.PROFILE_SHAPES:
                with self.subTest(input=name, shape=shape):
                    self.assertIn(f"profile-{shape}-{name}.data", recipe)
                    self.assertIn(f"profile-{shape}-{name}.txt", recipe)

    def test_a_profiled_shape_is_the_shape_the_sweep_times(self):
        """The reconciliation that keeps a profile readable against a figure.

        `_script` builds a container command line and `profile_argv` a host
        argv, so the two cannot be one function — but a flag that moves in one
        and not the other gives a profile of something no figure measures, and
        nothing else would notice."""
        for shape in measure.PROFILE_SHAPES:
            with self.subTest(shape=shape):
                timed = measure._script(shape).split()
                # Drop `time /pgdq`, the trailing redirect, and the container's
                # own paths; what is left is the flags both must agree on.
                self.assertEqual(timed[:2], ["time", "/pgdq"])
                timed = [w for w in timed[2:] if w != ">/dev/null"]
                profiled = measure.profile_argv(shape, "/dump.sql", "/tmp/x.dqcache")
                self.assertEqual(profiled, timed)

    def test_the_recipe_never_runs_anything(self):
        # The same rule koji's recipe obeys: this prints, and a session runs it
        # by hand. A harness that ran it would be taking a figure.
        with unittest.mock.patch.object(measure, "run") as ran:
            with unittest.mock.patch("sys.stdout"):
                measure.cmd_profile()
        ran.assert_not_called()


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

    def test_the_derived_figure_is_still_checked_for_staleness(self):
        # It is out of the sweep but not out of the register: it declares
        # `scripts/measure.py`, because the harness's own timing path is the
        # apparatus it measures. Iterating the sweep list here would leave
        # that declaration inert.
        touched = [f.id for f, _ in measure.figures_touched(["scripts/measure.py"])]
        self.assertIn("session-drift", touched)


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
            got = measure.read_psi_totals(self._psi_dir(tmp, cpu_some=1234, cpu_full=56))
            self.assertEqual(got, {"cpu.some": 1234, "cpu.full": 56})

    def test_absent_psi_is_missing_keys_not_an_error(self):
        # PSI is a kernel build option; a harness that died without it would be
        # dead on arrival at the next machine.
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(measure.read_psi_totals(Path(tmp) / "nope"), {})

    def test_the_avg_fields_are_not_mistaken_for_the_total(self):
        # `avg10` describes a 10-second window and a warm reading is 0.5 s, so
        # reading it instead of `total` would describe the wrong thing entirely.
        with tempfile.TemporaryDirectory() as tmp:
            got = measure.read_psi_totals(self._psi_dir(tmp, cpu_some=7))
            self.assertEqual(set(got.values()), {7, 0})

    def test_cpu_jiffies_name_every_field_including_steal(self):
        with tempfile.TemporaryDirectory() as tmp:
            stat = Path(tmp) / "stat"
            stat.write_text("cpu  1 2 3 4 5 6 7 8 9 10\ncpu0 1 2 3 4 5 6 7 8 9 10\n")
            got = measure.read_cpu_jiffies(stat)
            self.assertEqual(got["user"], 1)
            self.assertEqual(got["steal"], 8)
            self.assertEqual(got["idle"], 4)

    def test_a_stat_file_without_the_cpu_line_is_empty_not_wrong(self):
        with tempfile.TemporaryDirectory() as tmp:
            stat = Path(tmp) / "stat"
            stat.write_text("intr 1 2 3\n")
            self.assertEqual(measure.read_cpu_jiffies(stat), {})

    def _counters(self, mono, psi, cpu):
        return measure.Counters(monotonic=mono, psi=psi, cpu=cpu)

    def test_stall_is_reported_absolutely_and_as_a_share_of_the_window(self):
        before = self._counters(10.0, {"cpu.some": 1_000_000}, {})
        after = self._counters(12.0, {"cpu.some": 1_200_000}, {})
        got = measure.counter_delta(before, after)
        self.assertEqual(got["psi_cpu_some_us"], 200_000)
        # 0.2 s of stall in a 2 s window.
        self.assertAlmostEqual(got["psi_cpu_some_pct"], 10.0)

    def test_the_percentage_makes_a_short_and_a_long_reading_comparable(self):
        short = measure.counter_delta(
            self._counters(0.0, {"cpu.some": 0}, {}), self._counters(0.5, {"cpu.some": 50_000}, {})
        )
        long = measure.counter_delta(
            self._counters(0.0, {"cpu.some": 0}, {}), self._counters(8.0, {"cpu.some": 800_000}, {})
        )
        self.assertAlmostEqual(short["psi_cpu_some_pct"], long["psi_cpu_some_pct"])

    def test_busy_and_steal_come_out_of_the_jiffy_difference(self):
        before = self._counters(0.0, {}, {"user": 0, "idle": 0, "iowait": 0, "steal": 0})
        after = self._counters(1.0, {}, {"user": 10, "idle": 80, "iowait": 10, "steal": 5})
        got = measure.counter_delta(before, after)
        # 105 jiffies total, 90 of them idle+iowait.
        self.assertAlmostEqual(got["cpu_busy_pct"], round(100 * 15 / 105, 2))
        self.assertAlmostEqual(got["cpu_steal_pct"], round(100 * 5 / 105, 3))

    def test_a_zero_length_window_does_not_divide_by_zero(self):
        same = self._counters(5.0, {"cpu.some": 1}, {"user": 1})
        self.assertIn("window_s", measure.counter_delta(same, same))

    def test_only_counters_present_on_both_sides_are_differenced(self):
        before = self._counters(0.0, {"cpu.some": 1}, {})
        after = self._counters(1.0, {"cpu.some": 2, "io.some": 9}, {})
        got = measure.counter_delta(before, after)
        self.assertIn("psi_cpu_some_us", got)
        self.assertNotIn("psi_io_some_us", got)


class SamplerWindow(unittest.TestCase):
    def _sampler(self, samples):
        s = measure.Sampler(hz=0)  # no thread; the samples are supplied
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


class Gate(unittest.TestCase):
    LIMITS = {"warm": {"cpu_busy_pct": 25.0}, "cold": {"cpu_busy_pct": 25.0}}

    def test_a_reading_over_a_limit_is_named_with_the_limit_it_broke(self):
        verdict = measure.contention_verdict({"cpu_busy_pct": 60.0}, "warm", self.LIMITS)
        self.assertIn("cpu_busy_pct", verdict)
        self.assertIn("25.0", verdict)

    def test_a_reading_under_every_limit_is_clean(self):
        self.assertIsNone(measure.contention_verdict({"cpu_busy_pct": 3.0}, "warm", self.LIMITS))

    def test_a_limit_with_no_reading_behind_it_does_not_fire(self):
        # A machine without PSI must not fail every reading for lack of it.
        self.assertIsNone(
            measure.contention_verdict({}, "warm", {"warm": {"psi_cpu_some_pct": 1.0}})
        )

    def test_a_regime_with_no_limits_gates_nothing(self):
        self.assertIsNone(measure.contention_verdict({"cpu_busy_pct": 99.0}, "warm", {}))

    def test_io_stall_is_gated_in_neither_regime(self):
        # A cold run drops the page cache and reads 3 GiB off the SSD, so it
        # stalls on I/O for a fifth of its window *by construction*. Gating
        # that would fail every cold reading there is.
        for regime in ("cold", "warm"):
            self.assertNotIn("psi_io_some_pct", measure.CONTENTION_LIMITS[regime])
        self.assertIsNone(measure.contention_verdict({"psi_io_some_pct": 23.0}, "cold"))

    def test_the_armed_limits_clear_the_calibration_sweep_with_headroom(self):
        # Every p95 from the 182-reading fa186ab sweep, which its apparatus
        # lines witness as quiet. A limit that fires here is mis-set.
        for regime, p95 in (
            ("cold", {"cpu_busy_pct": 4.29, "psi_cpu_some_pct": 1.16, "cpu_steal_pct": 0.0}),
            ("warm", {"cpu_busy_pct": 4.52, "psi_cpu_some_pct": 0.20, "cpu_steal_pct": 0.0}),
        ):
            self.assertIsNone(measure.contention_verdict(p95, regime), regime)

    def test_an_obviously_busy_machine_is_caught_in_either_regime(self):
        for regime in ("cold", "warm"):
            self.assertIsNotNone(measure.contention_verdict({"cpu_busy_pct": 40.0}, regime))
            self.assertIsNotNone(measure.contention_verdict({"cpu_steal_pct": 12.0}, regime))


class ApparatusNote(unittest.TestCase):
    def test_the_note_quotes_the_worst_run_not_the_average(self):
        records = [
            {"telemetry": {"cpu_busy_pct": 3.0, "psi_cpu_some_pct": 0.1}},
            {"telemetry": {"cpu_busy_pct": 40.0, "psi_cpu_some_pct": 9.0}},
        ]
        note = measure.apparatus_note(records)
        self.assertIn("≤40% busy", note)
        self.assertIn("9.00%", note)

    def test_no_telemetry_emits_no_note(self):
        # A machine exposing none of this emits the table it always did.
        self.assertEqual(measure.apparatus_note([{"seconds": 1.0}]), "")

    def test_the_slowest_frequency_is_the_one_reported(self):
        records = [
            {"telemetry": {"freq_busiest_min_mhz": 4500.0}},
            {"telemetry": {"freq_busiest_min_mhz": 2100.0}},
        ]
        self.assertIn("≥2.10 GHz", measure.apparatus_note(records))


class Governor(unittest.TestCase):
    def test_pinning_is_off_by_default(self):
        # It measures as a no-op on amd-pstate-epp, and turning it on is an
        # apparatus change that obliges a full re-sweep.
        self.assertFalse(measure.Config().pin_governor)

    def test_a_machine_without_cpufreq_is_reported_not_crashed(self):
        with tempfile.TemporaryDirectory() as tmp:
            pin = measure.GovernorPin(log=lambda _m: None, root=Path(tmp))
            pin.__enter__()
            self.assertFalse(pin.ok)
            pin.__exit__()

    def test_the_previous_governor_is_read_before_it_is_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for cpu in ("cpu0", "cpu1"):
                d = root / cpu / "cpufreq"
                d.mkdir(parents=True)
                (d / "scaling_governor").write_text("powersave\n")
            written = []
            pin = measure.GovernorPin(log=lambda _m: None, root=root)
            pin._write_all = lambda values: written.append(dict(values))
            pin.__enter__()
            self.assertTrue(pin.ok)
            self.assertEqual(set(written[0].values()), {"performance"})
            pin.restore()
            self.assertEqual(set(written[1].values()), {"powersave"})

    def test_restoring_twice_is_a_no_op(self):
        # `__exit__` and the atexit hook both call it, and the second must not
        # re-write a governor the first already put back.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            d = root / "cpu0" / "cpufreq"
            d.mkdir(parents=True)
            (d / "scaling_governor").write_text("powersave\n")
            written = []
            pin = measure.GovernorPin(log=lambda _m: None, root=root)
            pin._write_all = lambda values: written.append(dict(values))
            pin.__enter__()
            pin.restore()
            pin.restore()
            self.assertEqual(len(written), 2)
