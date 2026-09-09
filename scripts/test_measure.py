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
import contextlib
import hashlib
import inspect
import io
import json
import re
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import generate_xz_input
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

    def test_bytes_render_at_the_doc_s_three_scales(self):
        self.assertEqual(measure._fmt_bytes(248 * 1024), "248 KB")
        self.assertEqual(measure._fmt_bytes(2 * measure.MIB), "2.0 MB")
        # `peak-rss` puts a 2 MB input beside a 3.00 GiB one, and "3072.0 MB"
        # is not the size anybody asked for.
        self.assertEqual(measure._fmt_bytes(3 * measure.GIB), "3.00 GiB")

    def test_resident_sets_render_in_mebibytes(self):
        # The only bound this figure is read against — four buffers of at most
        # 8 MiB — is stated in MiB, and a claim compared against a bound must
        # not change units on the way.
        self.assertEqual(measure.fmt_mib(6144), "6.00 MiB")
        self.assertEqual(
            measure.fmt_mib_median_spread([6100, 6000, 6300]), "**5.96 MiB** (5.86–6.15)"
        )

    def test_a_resident_set_difference_renders_at_the_scale_it_lands_on(self):
        # The figure's two axes are three orders of magnitude apart: tens of
        # KiB per byte, tens of MiB per block.
        self.assertEqual(measure.fmt_rss_delta(157.0), "+157 KiB")
        self.assertEqual(measure.fmt_rss_delta(-12.4), "-12 KiB")
        self.assertEqual(measure.fmt_rss_delta(40132.0), "+39.19 MiB")

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
            "parse-rss",
            *(f"query-project-{w}" for w in measure.PROJECTION_WIDTHS),
            *(f"query-where-{s}" for s in measure.PREDICATE_SHAPES),
            "dd",
        ):
            with self.subTest(command=command):
                self.assertEqual(len(self.TIMER.findall(measure._script(command))), 1)

    def test_no_command_redirects_stderr_inside_the_timer(self):
        # Some shells route `time`'s own report through the timed command's
        # redirection, which deletes the figure.
        for command in ("parse", "query-typed", "parse-cache-out", "parse-rss", "dd"):
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


class WorkerCount(unittest.TestCase):
    """A worker count is apparatus, so nothing here inherits the CLI's.

    `pgdq --jobs`' default moved underneath every figure in the document twice
    — to the machine's available parallelism and back to 1 — without one
    command shape changing, which is the failure this reconciles against. That
    it now agrees with `SWEEP_JOBS` is a coincidence of the day and not a
    reason to inherit it. `--stale` cannot see
    it either: staleness says *re-take*, never *the apparatus moved underneath
    you*."""

    def test_every_shape_states_a_worker_count(self):
        self.assertEqual(measure.worker_count_problems(), [])

    def test_a_shape_that_inherits_one_is_reported(self):
        # The check must fail loudly, since the shape it would pass still runs
        # and still produces a table.
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdq parse --source /dump.sql"
        ):
            reported = measure.worker_count_problems()
        self.assertEqual(
            sorted(reported),
            sorted(c for c in measure.command_shapes() if c != "dd"),
        )

    def test_check_fails_on_a_shape_that_inherits_one(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdq parse --source /dump.sql"
        ):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = measure.cmd_check(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("inheriting a worker count", out.getvalue())

    def test_the_enumeration_covers_every_branch_of_the_builder(self):
        """`command_shapes` is a second list of what `_script` accepts, and a
        second list drifts. This is what stops a shape added there from being
        exempted from the reconciliation by not being enumerated here."""
        src = inspect.getsource(measure._script)
        exact = set(re.findall(r'command == "([^"]+)"', src))
        for group in re.findall(r"command in \(([^)]*)\)", src):
            exact |= set(re.findall(r'"([^"]+)"', group))
        prefixes = set(re.findall(r'command\.startswith\("([^"]+)"\)', src))
        # A branch may dispatch on a *named* tuple of prefixes rather than on a
        # literal — `JOBS_AXIS` does, because three other places read the same
        # list. Resolving the name here is what keeps such a branch inside this
        # reconciliation: written to match literals only, the test would pass
        # while a whole family of shapes went unenumerated, which is precisely
        # the drift it exists to catch.
        for name in re.findall(r"command\.startswith\((_?[A-Z][A-Z_0-9]*)\)", src):
            prefixes |= set(getattr(measure, name))
        shapes = set(measure.command_shapes())
        self.assertTrue(exact)
        self.assertTrue(prefixes)
        self.assertEqual(exact - shapes, set())
        for prefix in prefixes:
            with self.subTest(prefix=prefix):
                self.assertTrue([s for s in shapes if s.startswith(prefix)])

    def test_the_only_exempt_shape_runs_no_binary_of_ours(self):
        # `dd` is the device floor. Anything else claiming the exemption would
        # be a pgdq run measuring whatever the machine had.
        for command in measure._NO_WORKERS:
            with self.subTest(command=command):
                self.assertNotIn("/pgdq", measure._script(command))

    def test_the_decode_instrument_pins_its_own_spelling(self):
        # It is not `pgdq`, so it has no `--jobs`; `--workers` is the same
        # statement in the instrument's own vocabulary, and the figure's whole
        # axis is that count.
        for workers in measure.DECODE_WORKERS:
            with self.subTest(workers=workers):
                self.assertIn(f"--workers {workers}", measure._script(f"decode-{workers}"))

    def test_the_untimed_profiling_parse_states_one_too(self):
        # Untimed, but its row counts are the divisor under every per-row
        # number in the document.
        src = inspect.getsource(measure.Stager.profile)
        self.assertIn('"--jobs", str(SWEEP_JOBS)', src)

    def test_the_traced_save_count_states_one_too(self):
        # Untimed as well, and its number is published — and `strace -f`
        # follows every thread a parallel mapping pass would spawn.
        src = inspect.getsource(measure.count_saves)
        self.assertIn('"--jobs", str(SWEEP_JOBS)', src)

    def test_the_rss_attribution_states_one_on_every_leg(self):
        # A resident set is exactly the quantity a worker count moves, each
        # worker holding read buffers of its own — so a leg inheriting the
        # CLI's default would attribute a growth this apparatus never measured.
        # `info` takes no such flag because it starts no workers; its shape
        # states the count on the builder that precedes it.
        for _, _, command in measure._ATTRIBUTION_LEGS:
            with self.subTest(command=command):
                self.assertIn(f"--jobs {measure.SWEEP_JOBS}", measure._script(command))

    def test_the_apparatus_line_states_the_count_the_harness_pins(self):
        """The document's one apparatus line names the worker count, and the
        number in it is the harness's — a sentence describing the previous
        arrangement is exactly what this whole reconciliation is against."""
        doc = (measure.REPO / "docs/design/measurements.md").read_text()
        section = doc.split("\n## The apparatus\n", 1)[1].split("\n## ", 1)[0]
        stated = set(re.findall(r"--jobs (\d+)", section))
        self.assertEqual(stated, {str(measure.SWEEP_JOBS)})


class Allocator(unittest.TestCase):
    """The allocator figure: three binaries, three shapes, one table.

    Every assertion here is a way to get a plausible table of the wrong
    comparison, which is the same family of failure as profiling the `release`
    binary and labelling it `profiling`. Two are about the *reference* column
    and are the ones the design turns on: it must be the binary every other
    figure was taken with, and its readings must be shared rather than retaken,
    or the doc carries two numbers for one measurement.
    """

    def setUp(self):
        # The build memo is module state, so one test's build would otherwise
        # satisfy the next test's.
        measure._ALLOC_BUILT.clear()
        measure._ALLOC_ANNOUNCED.clear()

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

    def test_a_leg_left_over_from_an_earlier_session_is_rebuilt(self):
        # The failure this stops is silent and total: `runs/pgdq-alloc-<leg>`
        # survives between sessions, so short-circuiting on its existence times
        # a leg built from last week's source against a reference built from
        # today's, and the leg still answers `--version` with its own allocator
        # name, so nothing downstream notices.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )
            stale = cfg.out_dir / "pgdq-alloc-jemalloc"
            stale.parent.mkdir(parents=True, exist_ok=True)
            stale.write_text("last session's binary\n")
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
                measure.ensure_allocator_binary(cfg, "jemalloc", lambda _: None)
                self.assertEqual(len(calls), 1)
                self.assertEqual(stale.read_text(), "#!/bin/true\n")
                # Twice in one process is one build: `cargo` is incremental,
                # but a build between two timed reps moves the second one.
                measure.ensure_allocator_binary(cfg, "jemalloc", lambda _: None)
                self.assertEqual(len(calls), 1)

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


class CensusBinary(unittest.TestCase):
    """The census-off binary's stamp: the harness will not build that binary,
    and will not trust one whose tree could have moved a reading.

    A census figure is a subtraction between it and `target/release/pgdq`, so
    every difference between the two trees is attributed to the census — which
    is why the age of the hand-built half has to be checkable at all. The
    threshold is that hazard rather than commit equality: an **ancestor** of
    HEAD with no path the figures being taken declare changed in between.
    """

    HEAD = "a" * 40
    ANCESTOR = "b" * 40
    DIVERGENT = "c" * 40

    #: Two figures declaring different paths, which is what makes the check
    #: per sitting: `census-attribution` declares no scanner path where the
    #: other two do.
    MAPPER = measure.Figure(
        id="census-fake-mapper",
        section="",
        stage="warm",
        depends=("pgdump_query/src/map.rs", "pgdump_query/src/scan.rs"),
    )
    READER = measure.Figure(
        id="census-fake-reader",
        section="",
        stage="warm",
        depends=("pgdump_query/src/io.rs",),
    )

    def _resolve(self, rev):
        if rev == "HEAD":
            return self.HEAD
        return {
            "aaaaaaa": self.HEAD,
            "bbbbbbb": self.ANCESTOR,
            "ccccccc": self.DIVERGENT,
        }.get(rev[:7])

    def _ancestor(self, earlier, later):
        return (earlier, later) == (self.ANCESTOR, self.HEAD)

    def _problem(self, cfg, *, figures=None, changed=()):
        def between(earlier, later):
            self.assertEqual((earlier, later), (self.ANCESTOR, self.HEAD))
            return list(changed)

        return measure.census_binary_problem(
            cfg,
            [self.MAPPER] if figures is None else figures,
            self._resolve,
            self._ancestor,
            between,
        )

    def _cfg(self, tmp, *, binary=True, stamp=None):
        cfg = measure.Config(bin_nocensus=Path(tmp) / "runs" / "pgdq-nocensus")
        cfg.bin_nocensus.parent.mkdir(parents=True, exist_ok=True)
        if binary:
            cfg.bin_nocensus.write_text("#!/bin/true\n")
        if stamp is not None:
            cfg.bin_nocensus_stamp.write_text(stamp)
        return cfg

    def test_the_stamp_sits_beside_the_binary_it_describes(self):
        # Derived from the binary's path, so PGDQ_MEASURE_CENSUS_OFF_BIN moves
        # both and cannot leave them describing different files.
        cfg = measure.Config(bin_nocensus=Path("/elsewhere/pgdq-nocensus"))
        self.assertEqual(
            cfg.bin_nocensus_stamp, Path("/elsewhere/pgdq-nocensus.stamp")
        )

    def test_a_missing_binary_is_still_refused_by_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, binary=False)
            problem = self._problem(cfg)
            self.assertIn("pgdq-nocensus", problem)
            self.assertIn("is missing", problem)

    def test_a_binary_with_no_stamp_is_refused(self):
        # The state every checkout was in before this rule: a binary from some
        # tree, and nothing saying which.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp)
            problem = self._problem(cfg)
            self.assertIn("pgdq-nocensus.stamp", problem)

    def test_a_stamp_that_names_no_commit_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp="not-a-sha\n")
            self.assertIn("not a commit", self._problem(cfg))

    def test_an_empty_stamp_is_refused_rather_than_read_as_head(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp="\n")
            self.assertIsNotNone(self._problem(cfg))

    def test_a_stamp_naming_the_commit_being_measured_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.HEAD + "\n")
            # And without consulting a diff: HEAD against itself has nothing in
            # between, so the equality case short-circuits.
            def never(earlier, later):
                raise AssertionError("the diff was consulted for HEAD against itself")

            self.assertIsNone(
                measure.census_binary_problem(
                    cfg, [self.MAPPER], self._resolve, self._ancestor, never
                )
            )

    def test_a_short_stamp_still_resolves(self):
        # `git rev-parse HEAD` writes a full sha, but a stamp written by hand
        # may be short and still name the same commit.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp="aaaaaaa\n")
            self.assertIsNone(self._problem(cfg))

    def test_an_ancestor_that_moved_nothing_measured_is_tolerated(self):
        # The loosening: a doc-only commit cannot move a reading a census
        # figure takes, so it does not cost a hand rebuild.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.ANCESTOR + "\n")
            self.assertIsNone(
                self._problem(cfg, changed=["docs/design/measurements.md"])
            )

    def test_an_ancestor_that_moved_a_declared_path_is_refused_naming_it(self):
        # The 2026-09-05 failure: a binary 40 commits behind, differenced
        # against a fresh one, with read-path work charged to the census.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.ANCESTOR + "\n")
            problem = self._problem(cfg, changed=["pgdump_query/src/scan.rs"])
            self.assertIn(self.ANCESTOR[:7], problem)
            self.assertIn(self.HEAD[:7], problem)
            self.assertIn("pgdump_query/src/scan.rs", problem)
            self.assertIn("census-fake-mapper", problem)

    def test_a_declared_directory_still_matches_everything_under_it(self):
        # The same prefix predicate `--stale` argues staleness from, which is
        # what keeps this from being a second authority over what moves a
        # reading.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.ANCESTOR + "\n")
            figure = measure.Figure(
                id="census-fake-dir", section="", stage="warm", depends=("pgdump_query/",)
            )
            self.assertIsNotNone(
                self._problem(
                    cfg, figures=[figure], changed=["pgdump_query/src/copy.rs"]
                )
            )

    def test_the_check_reads_the_figures_being_taken(self):
        # Per sitting, not against a fixed list: the same commit refuses one
        # selection and passes another.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.ANCESTOR + "\n")
            moved = ["pgdump_query/src/io.rs"]
            self.assertIsNone(self._problem(cfg, figures=[self.MAPPER], changed=moved))
            self.assertIsNotNone(
                self._problem(cfg, figures=[self.READER], changed=moved)
            )

    def test_naming_no_figure_refuses_rather_than_checking_ancestry_alone(self):
        # The declared-path question is asked of the selection, so an empty one
        # would quietly weaken the check to the half that is not the point.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.ANCESTOR + "\n")
            self.assertIsNotNone(self._problem(cfg, figures=[]))

    def test_a_stamp_that_is_not_an_ancestor_is_refused(self):
        # A divergent or ahead commit has no "in between" to inspect, so the
        # diff would not mean what it says — and the binary comes from a tree
        # outside this one's history.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp, stamp=self.DIVERGENT + "\n")

            def never(earlier, later):
                raise AssertionError("a non-ancestor stamp was diffed against HEAD")

            problem = measure.census_binary_problem(
                cfg, [self.MAPPER], self._resolve, self._ancestor, never
            )
            self.assertIn("not an ancestor", problem)
            self.assertIn(self.DIVERGENT[:7], problem)
            self.assertIn(self.HEAD[:7], problem)

    def test_the_doc_s_recipe_writes_the_stamp_the_harness_reads(self):
        # The two halves have to meet: a recipe that does not write the stamp
        # leaves the check unsatisfiable, and the recipe is where a hand build
        # is described.
        doc = (measure.REPO / "docs/design/measurements.md").read_text()
        name = measure.Config().bin_nocensus_stamp.name
        self.assertIn(f"git rev-parse HEAD > runs/{name}", doc)


class PredicateShapes(unittest.TestCase):
    """Six predicates over one file, which is the whole instrument.

    Three things make the adjacent-row subtraction mean what the table says it
    means: the terms are OR'd and every one of them is false, so all of them
    are evaluated and no row survives to be decoded; the two depths differ in
    the column and not in the term count, so their difference is the walk; and
    every column named is one the generator writes, so the query does not fail
    three minutes into a sweep."""

    def test_every_shape_names_a_column_the_generator_writes(self):
        emitted = {name for name, _ in measure.perf.COLUMNS}
        for shape, (column, _) in measure.PREDICATE_SHAPES.items():
            with self.subTest(shape=shape):
                self.assertIn(column, emitted)

    def test_an_expression_carries_exactly_that_many_terms(self):
        for shape, (column, terms) in measure.PREDICATE_SHAPES.items():
            with self.subTest(shape=shape):
                expr = measure.predicate_expr(shape)
                self.assertEqual(expr.count(f"{column}="), terms)
                self.assertEqual(expr.count(" OR "), terms - 1)

    def test_the_terms_are_distinct(self):
        for shape in measure.PREDICATE_SHAPES:
            with self.subTest(shape=shape):
                parts = measure.predicate_expr(shape).split(" OR ")
                self.assertEqual(len(set(parts)), len(parts))

    def test_the_terms_are_disjoined_never_conjoined(self):
        # `And` stops at the first non-`True` conjunct, so a conjunction of N
        # false terms evaluates one of them and the table would read flat.
        for shape in measure.PREDICATE_SHAPES:
            with self.subTest(shape=shape):
                self.assertNotIn(" AND ", measure.predicate_expr(shape))

    def test_a_shape_the_register_does_not_carry_is_an_error(self):
        # The command shape is parsed rather than matched, so an unregistered
        # one has to be refused explicitly or it would run a query with no
        # filter at all and be read as a predicate.
        for command in ("query-where-deep-4", "query-where-", "query-where-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    measure._script(command)

    def test_every_run_is_strings_and_carries_its_filter(self):
        # Typed `=` decodes the literal against the column's own type, so
        # `zzz1` on an `integer` column is refused before the first row.
        for shape in measure.PREDICATE_SHAPES:
            with self.subTest(shape=shape):
                script = measure._script(f"query-where-{shape}")
                self.assertIn("--schema-mode strings", script)
                self.assertIn(f"--where '{measure.predicate_expr(shape)}'", script)

    def test_the_two_depths_differ_only_in_the_column(self):
        deep, shallow = measure.PREDICATE_SHAPES["deep-5"], measure.PREDICATE_SHAPES["shallow-5"]
        self.assertEqual(deep[1], shallow[1])
        self.assertNotEqual(deep[0], shallow[0])

    def test_the_deep_column_is_deeper_than_the_shallow_one(self):
        order = [name for name, _ in measure.perf.COLUMNS]
        deep = order.index(measure.PREDICATE_SHAPES["deep-5"][0])
        shallow = order.index(measure.PREDICATE_SHAPES["shallow-5"][0])
        self.assertGreater(deep, shallow)

    def test_the_table_rows_are_the_registered_shapes(self):
        shapes = [s for s, _, _ in measure._PREDICATE_ROWS]
        self.assertEqual(sorted(shapes), sorted(measure.PREDICATE_SHAPES))

    def test_the_figure_is_taken_on_the_brace_free_control(self):
        fig = measure.SELECTABLE_BY_ID["predicate-terms"]
        self.assertEqual(fig.warm_inputs, ("control",))
        self.assertIn("pgdump_query/src/predicate.rs", fig.depends)


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


class PeakRss(unittest.TestCase):
    """The one figure whose reading is not a time.

    Two things decide whether the number means what the table says. The
    *instrument* must report pgdq's peak and not the wrapper's or the client's
    — every wrong answer here is a plausible-looking one, which is why the
    wrapper is asserted rather than remembered. And the *rows* must vary one
    thing each: the byte pair differs in bytes alone, the block rows in blocks
    alone, and both are read against the same pivot."""

    def _rows(self) -> tuple[str, ...]:
        return measure._RSS_ROWS

    def test_the_command_is_the_same_parse_the_throughput_tables_time(self):
        # A wrapped `parse`, not a different command: what this figure reports
        # has to be the resident set of the scan the rest of the doc measures.
        script = measure._script("parse-rss")
        self.assertIn("/pgdq parse --source /dump.sql --dqcache /tmp/x.dqcache", script)

    def test_the_redirection_takes_pgdq_s_stdout_and_not_the_reading(self):
        # The reading goes to stderr, where bash's own `time` report goes, so
        # `>/dev/null` on the whole command cannot swallow it.
        script = measure._script("parse-rss")
        self.assertTrue(script.rstrip().endswith(">/dev/null"))
        self.assertIn("printf STDERR", script)

    def test_the_wrapper_execs_so_its_own_footprint_is_not_the_reading(self):
        # `exec` installs a fresh `mm`, so the forked interpreter's ~5.4 MiB is
        # not in the child's high-water mark. Without it the table would read
        # the wrapper.
        self.assertIn("exec @ARGV", measure.rss_wrapper("x86_64"))

    def test_the_wrapper_reads_the_children_high_water_mark(self):
        # RUSAGE_CHILDREN (-1), read after `waitpid`. Polling `/proc` instead
        # would miss a peak in the cache write a `parse` ends with, because the
        # Vm* lines are gone the moment the process becomes a zombie.
        wrapper = measure.rss_wrapper("x86_64")
        self.assertIn("waitpid($pid, 0)", wrapper)
        self.assertIn(f"syscall({measure.GETRUSAGE_SYSCALL['x86_64']}, -1,", wrapper)

    def test_the_wrapper_propagates_a_failed_run(self):
        # Otherwise a pgdq that died would be reported as a resident set.
        self.assertIn("exit($st == 0 ? 0 :", measure.rss_wrapper("x86_64"))

    def test_an_unregistered_machine_is_an_error_not_a_guess(self):
        # A wrong syscall number returns EINVAL on one architecture and a
        # plausible reading of the wrong field on another.
        with self.assertRaises(ValueError):
            measure.rss_wrapper("s390x")

    def test_the_reading_is_read_back_off_the_wrapper_s_own_line(self):
        self.assertEqual(measure.parse_maxrss_kib("real\t0m0.5s\nmaxrss_kib=6144\n"), 6144)

    def test_no_reading_is_an_error_not_a_zero(self):
        with self.assertRaises(ValueError):
            measure.parse_maxrss_kib("real\t0m0.5s\n")

    def test_two_readings_are_an_error(self):
        with self.assertRaises(ValueError):
            measure.parse_maxrss_kib("maxrss_kib=6144\nmaxrss_kib=6200\n")

    def test_block_counts_are_read_off_the_generator(self):
        # Written beside the row instead, a block count that moved in the
        # generator would leave a stale number in the table's own column.
        self.assertEqual(measure.input_block_count("one_block"), 1)
        self.assertEqual(measure.input_block_count("control"), 1)
        self.assertEqual(measure.input_block_count("blocks4000"), 4000)

    def test_the_pivot_is_a_row_and_holds_one_block(self):
        self.assertIn(measure._RSS_PIVOT, self._rows())
        self.assertEqual(measure.input_block_count(measure._RSS_PIVOT), 1)

    def test_the_byte_pair_differs_in_bytes_alone(self):
        # `one_block` and `control` are the same generator, same seed, same
        # shape; only `--size-mb` differs. So their difference is bytes.
        pivot, big = measure.INPUTS[measure._RSS_PIVOT], measure.INPUTS["control"]
        self.assertEqual(pivot.generator, big.generator)
        self.assertEqual(measure.input_block_count("control"), 1)
        self.assertIn("--seed", pivot.args)
        self.assertEqual(
            pivot.args[pivot.args.index("--seed") + 1],
            big.args[big.args.index("--seed") + 1],
        )

    def test_the_block_rows_multiply_blocks_against_the_pivot(self):
        # koji cannot test this half at all — 74 blocks over 784 GB — which is
        # why the claim was re-homed here.
        counts = [measure.input_block_count(n) for n in self._rows()]
        self.assertGreater(max(counts), 1000 * measure.input_block_count(measure._RSS_PIVOT))

    def test_every_row_is_an_input_the_figure_stages(self):
        fig = measure.SELECTABLE_BY_ID["peak-rss"]
        self.assertEqual(tuple(fig.warm_inputs), self._rows())

    def test_the_figure_declares_the_read_path_first(self):
        # The mechanism the claim is about, and the edge whose absence let the
        # koji row go a megabyte wrong for a whole slice.
        fig = measure.SELECTABLE_BY_ID["peak-rss"]
        for path in (*measure.READ, *measure.MAP_BUILD, *measure.CACHE):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)


class RssAttribution(unittest.TestCase):
    """The nine legs that say what `peak-rss`'s per-block growth is made of.

    Every failure this class covers returns a plausible-looking table of
    something else, which is the family the allocator legs and the census
    binary already have tests for. Two matter most. A leg that is not
    *distinguishable* as a reading silently becomes another leg's number, since
    `RunSpec.key` carries the binary, the input, the shape and the regime and
    not the words the table prints. And a leg whose shape carries no RSS
    wrapper has no reading at all, which surfaces as a `KeyError` a sitting
    into rather than as a refusal before it.
    """

    def _fig(self) -> measure.Figure:
        return measure.EVERY_BY_ID["rss-attribution"]

    def test_every_leg_is_its_own_reading(self):
        # Two legs differing only in their label would share one key and
        # publish one measurement as two rows.
        keys = [
            spec.key("rss-attribution")
            for _, small, big in measure._attribution_specs()
            for spec in (small, big)
        ]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertEqual(len(keys), 2 * len(measure._ATTRIBUTION_LEGS))

    def test_every_leg_reports_a_resident_set(self):
        # `Session.time_run` keys the RSS capture off `"rss" in spec.command`,
        # so a shape not spelled that way is timed and never measured.
        for _, _, command in measure._ATTRIBUTION_LEGS:
            with self.subTest(command=command):
                self.assertIn("rss", command)
                self.assertIn("printf STDERR", measure._script(command))

    def test_each_shape_wraps_and_times_exactly_one_command(self):
        # `parse_bash_time` and `parse_maxrss_kib` both refuse two reports, and
        # the `info` leg builds its cache in the same shell — untimed and
        # unwrapped, or the reading would be the builder's.
        for _, _, command in measure._ATTRIBUTION_LEGS:
            with self.subTest(command=command):
                script = measure._script(command)
                self.assertEqual(script.count("time "), 1)
                self.assertEqual(script.count("printf STDERR"), 1)

    def test_the_two_block_counts_differ_in_blocks_alone(self):
        # Both are `generate_block_count_bench.py` outputs at one seed, so a
        # slope over the pair is a slope in blocks and not in anything else.
        small, big = (measure.INPUTS[n] for n in measure._ATTRIBUTION_INPUTS)
        self.assertEqual(small.generator, big.generator)
        self.assertLess(
            measure.input_block_count(measure._ATTRIBUTION_INPUTS[0]),
            measure.input_block_count(measure._ATTRIBUTION_INPUTS[1]),
        )

    def test_the_reference_leg_is_the_shape_peak_rss_times(self):
        # The first row is read against `peak-rss`'s block-count rows, which
        # only holds while the two run the same command.
        self.assertEqual(measure._ATTRIBUTION_LEGS[0][1:], ("pgdq", "parse-rss"))
        self.assertEqual(
            [s.command for s in _peak_rss_specs()][0], measure._ATTRIBUTION_LEGS[0][2]
        )

    def test_the_allocator_legs_are_the_allocator_figure_s_own(self):
        # Borrowed by name, never built by a second recipe: `binary_path`
        # routes an `alloc:` binary through `ensure_allocator_binary`, which is
        # where the leg is built and then interrogated.
        legs = {b.removeprefix("alloc:") for _, b, _ in measure._ATTRIBUTION_LEGS if ":" in b}
        self.assertEqual(legs, set(measure.ALLOCATOR_LEGS) - {"system"})

    def test_it_declares_the_two_mechanisms_only_its_own_legs_reach(self):
        # The preamble, where the per-table structure is paid, and the CLI's
        # `query` path, which the four no-match legs run. Neither is in
        # `peak-rss`'s declaration, and a figure that cannot say what
        # invalidates it is one nobody has thought about.
        fig = self._fig()
        for path in (*measure.PREAMBLE, *measure.QUERY_CLI):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_the_manual_s_per_table_claim_is_this_figure_s_consumer(self):
        # `peak-rss` cannot license it: `blocks4000` gives every table exactly
        # one `COPY` block, so per-table and per-block coincide in its inputs.
        self.assertIn("docs/manual/dump-inspection.md", self._fig().quoted_by)

    def test_a_taken_attribution_declares_its_borrow(self):
        """The obligation the instrument leaves for `M74`, the sweep that publishes this.

        Its `parse` reference row runs `peak-rss`'s `blocks500` and
        `blocks4000` shapes, so the two must share a reading rather than take
        one each — the doc currently carries both, disagreeing. But declaring
        the edge while this entry is untaken closes no part of that item and would
        refuse `peak-rss`'s own standalone sitting, a figure the doc already
        carries, with no sweep yet to cure it. So the edge is declared in the
        change that moves this entry into `FIGURES`, and that is what this
        asserts rather than leaves to a comment."""
        fig = self._fig()
        if fig in measure.UNTAKEN:
            self.assertEqual(fig.shares, ())
        else:
            self.assertIn("peak-rss", [s.source for s in fig.shares])


def _peak_rss_specs() -> list:
    """`peak-rss`'s specs, without running the figure."""
    return [
        measure.RunSpec("pgdq", name, "parse-rss", "warm", name) for name in measure._RSS_ROWS
    ]


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


class BorrowGraph(unittest.TestCase):
    """The sharing relation, declared on `Figure` and read by the harness.

    Its whole reason to be declared is the *transitive* case: a note written at
    a `session.borrow` call site names the source it just asked for, and behind
    the allocator table that is two figures where the honest set is four."""

    def _session(self, readings=None):
        session = measure.Session.__new__(measure.Session)
        session.readings = dict(readings or {})
        return session

    def test_requires_is_read_off_the_declared_borrows(self):
        # One list, not two: a second declaration of the same fact drifts, and
        # the one that drifts is the one no run function reads.
        fig = measure.FIGURES_BY_ID["allocator"]
        self.assertEqual(
            fig.requires, ("census-brace-free", "nested-end-to-end")
        )

    def test_every_borrow_names_a_known_figure(self):
        for fig in measure.EVERY_FIGURE:
            for shared in fig.shares:
                with self.subTest(figure=fig.id, source=shared.source):
                    self.assertIn(shared.source, measure.EVERY_BY_ID)
                    self.assertTrue(shared.what)

    def test_every_republished_spec_is_one_its_source_takes(self):
        """A borrow is a dictionary lookup that silently returns nothing.

        So every declared key is checked against the specs the source figure
        actually sweeps — the failure it guards against is a rename on one side
        producing a table that measures its own reference column and says it
        shared it."""
        taken = {
            "census-brace-free": {
                spec.key("census-brace-free")
                for regime in ("cold", "warm")
                for spec in measure._census_specs("control", regime)
            },
            "nested-end-to-end": {
                spec.key("nested-end-to-end") for spec in measure._nested_specs()
            },
            "per-block-quadratic": {
                measure.RunSpec(
                    binary, name, "parse-cache-out", "warm", ""
                ).key("per-block-quadratic")
                for name, _ in measure._QUADRATIC_ROWS
                for binary in ("before", "pgdq")
            },
        }
        for fig in measure.EVERY_FIGURE:
            for shared in fig.shares:
                for spec in shared.republished:
                    with self.subTest(figure=fig.id, spec=spec.key(shared.source)):
                        self.assertIn(shared.source, taken)
                        self.assertIn(spec.key(shared.source), taken[shared.source])

    def test_the_closure_is_transitive(self):
        # The four the history entry names: `allocator` borrows two, and both
        # throughput tables borrow the census reading in turn.
        self.assertEqual(
            measure.sharing_closure("allocator"),
            [
                "census-brace-free",
                "scan-throughput-cold",
                "scan-throughput-warm",
                "nested-end-to-end",
            ],
        )

    def test_the_closure_is_symmetric(self):
        for fig in measure.EVERY_FIGURE:
            for other in measure.sharing_closure(fig.id):
                with self.subTest(figure=fig.id, other=other):
                    self.assertIn(fig.id, measure.sharing_closure(other))

    def test_a_consumed_reading_is_not_a_closure_edge(self):
        # `cross-file-floor` differences the nested sweep's reps into a per-row
        # cost. That is a derived quantity, not the nested table's number a
        # second time, so re-taking one does not put two numbers in the doc for
        # one measurement -- but it still orders the run.
        self.assertNotIn("cross-file-floor", measure.sharing_closure("nested-end-to-end"))
        self.assertEqual(
            measure.FIGURES_BY_ID["cross-file-floor"].requires, ("nested-end-to-end",)
        )

    def test_a_satisfied_borrow_is_copied_and_said_to_be_shared(self):
        source = measure.RunSpec("pgdq", "control", "parse", "warm", "")
        session = self._session({source.key("census-brace-free"): [1.0, 2.0]})
        note = measure.share_readings(session, "scan-throughput-warm")
        self.assertEqual(session.readings[source.key("scan-throughput-warm")], [1.0, 2.0])
        self.assertIn("Shared, not measured again", note)
        self.assertNotIn("Partial sweep", note)

    def test_an_unsatisfied_borrow_names_the_whole_closure(self):
        note = measure.share_readings(self._session(), "allocator")
        self.assertIn("**Partial sweep**", note)
        for fid in measure.sharing_closure("allocator"):
            with self.subTest(figure=fid):
                self.assertIn(fid, note)

    def test_nothing_is_faked_for_an_unsatisfied_borrow(self):
        # The figure measures it instead, which is what the note discloses.
        session = self._session()
        measure.share_readings(session, "allocator")
        self.assertEqual(session.readings, {})

    def test_closure_gaps_name_what_a_selection_leaves_out(self):
        gaps = measure.closure_gaps(measure.resolve_selection(["allocator"]))
        self.assertTrue(any(g.startswith("allocator —") for g in gaps))
        joined = " ".join(gaps)
        self.assertIn("scan-throughput-cold", joined)
        self.assertIn("scan-throughput-warm", joined)

    def test_a_whole_closure_leaves_no_gap(self):
        ids = ["allocator", *measure.sharing_closure("allocator")]
        self.assertEqual(measure.closure_gaps(measure.resolve_selection(ids)), [])

    def test_the_derived_direction_is_read_off_the_same_declaration(self):
        # The reverse of a non-republishing share, which the closure does not
        # carry: `cross-file-floor`'s row 1 is a difference over the nested
        # sweep's reps, so re-taking that sweep strands it.
        self.assertEqual(
            measure.derived_consumers("nested-end-to-end"),
            [("cross-file-floor", "row 1's per-rep differences")],
        )

    def test_the_edge_names_both_ends_and_the_quantity(self):
        # What `--check` prints: the consumer, the source whose reps it reads,
        # and what it makes of them.
        self.assertEqual(
            measure.derivation_edges(),
            [("cross-file-floor", "nested-end-to-end", "row 1's per-rep differences")],
        )

    def test_a_republished_share_is_not_a_derivation(self):
        # `scan-throughput-warm` publishes `census-brace-free`'s reading as its
        # own number, which is the closure's business and not this edge's.
        self.assertEqual(measure.derived_consumers("census-brace-free"), [])

    def test_re_taking_a_source_alone_names_the_table_it_strands(self):
        gaps = measure.derivation_gaps(measure.resolve_selection(["nested-end-to-end"]))
        self.assertEqual(len(gaps), 1)
        self.assertIn("nested-end-to-end", gaps[0])
        self.assertIn("cross-file-floor", gaps[0])

    def test_taking_the_consumer_too_leaves_no_derivation_gap(self):
        selection = measure.resolve_selection(["cross-file-floor"])
        self.assertIn("nested-end-to-end", [f.id for f in selection])
        self.assertEqual(measure.derivation_gaps(selection), [])

    def test_the_forward_direction_was_never_the_hazard(self):
        # Selection reads every share, republished or not, so a consumer always
        # drags its source in. Only the reverse needed naming.
        self.assertEqual(measure.derivation_gaps(measure.resolve_selection(["map-only"])), [])

    def test_alone_takes_exactly_what_is_named(self):
        got = [f.id for f in measure.resolve_selection(["allocator"], alone=True)]
        self.assertEqual(got, ["allocator"])

    def test_alone_refuses_a_reading_it_cannot_measure_for_itself(self):
        with self.assertRaises(SystemExit):
            measure.resolve_selection(["cross-file-floor"], alone=True)

    def test_a_partial_note_is_attributed_to_the_marker_above_it(self):
        text = (
            "<!-- figure: census-brace-free -->\nnothing here\n"
            "<!-- figure: allocator -->\n**Partial sweep**: measured here.\n"
        )
        self.assertEqual(
            measure.partial_sittings(text),
            [("allocator", measure.sharing_closure("allocator"))],
        )

    def test_a_doc_with_no_partial_note_reports_none(self):
        self.assertEqual(measure.partial_sittings("<!-- figure: allocator -->\n"), [])


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
            for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
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
        used = {
            n
            for f in measure.SELECTABLE
            for n in (*f.cold_inputs, *f.warm_inputs, *f.nvme_inputs)
        }
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

    What an entry waits on is a **commit to name** as its sitting, since a
    figure published outside a stamped sweep declares one inside its own marker
    and a sitting taken from a tree that carries the instrument uncommitted has
    none. `xz-decode-scaling` waited here for exactly that and left when the
    commit existed; the two `parallel-*` figures wait on it now."""

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
            for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
                with self.subTest(figure=fig.id, input=name):
                    self.assertIn(name, measure.INPUTS)
            for path in fig.depends:
                with self.subTest(figure=fig.id, path=path):
                    self.assertTrue((measure.REPO / path).exists(), path)

    def test_no_id_collides_with_a_figure(self):
        ids = [f.id for f in measure.ALL_FIGURES] + [f.id for f in measure.UNTAKEN]
        self.assertEqual(len(ids), len(set(ids)))


class ParallelFigures(unittest.TestCase):
    """The two `parallel-*` figures: the first in the register whose axis is
    `pgdq`'s own `--jobs`.

    Every assertion here is a way to get a plausible table of the wrong thing,
    which is the family this module already covers for the allocator legs and
    the census binary. Three of them matter most, and each fails silently
    without a test. A **clamped row** — a stated count the library quietly
    reduces because the budget cannot hold that many partitions — is a lower
    count wearing a higher label, and the resulting table is monotone and wrong.
    A **shape drifting off `SWEEP_JOBS`** puts a second worker count in the
    register with nothing saying so, which is the defect the worker-count
    apparatus rule closed one level up. And a **leg whose rate is per compressed byte** reads five times too
    slow under a heading that looks like the plain leg's.
    """

    def test_both_figures_are_taken_and_no_longer_untaken(self):
        # The sitting at `e29939c` moved both entries out of `UNTAKEN` and
        # into `FIGURES`, where the doc-side checks start applying.
        untaken = [f.id for f in measure.UNTAKEN]
        self.assertNotIn("parallel-scan-throughput", untaken)
        self.assertNotIn("parallel-peak-rss", untaken)
        taken = [f.id for f in measure.FIGURES]
        self.assertIn("parallel-scan-throughput", taken)
        self.assertIn("parallel-peak-rss", taken)

    def test_every_registered_job_count_has_a_shape_in_every_family(self):
        for family in measure.JOBS_AXIS:
            for jobs in measure.PARALLEL_JOBS:
                with self.subTest(family=family, jobs=jobs):
                    script = measure._script(f"{family}{jobs}")
                    self.assertIn(f"--jobs {jobs} ", script)
                    self.assertEqual(script.count("time "), 1)

    def test_a_job_count_the_figures_do_not_carry_is_an_error(self):
        # The shape is parsed rather than matched, so an unregistered count has
        # to be refused explicitly or it would run at whatever was typed and be
        # read as a row of the table.
        for command in ("parse-jobs-3", "parse-jobs-", "parse-jobs-all",
                        "query-typed-jobs-7", "parse-rss-jobs-x"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    measure._script(command)

    def test_a_family_declared_but_not_built_is_an_error(self):
        # `JOBS_AXIS` is what `_script` dispatches on and what `command_shapes`
        # enumerates, so a prefix added to it with no branch behind it would
        # otherwise produce a shape the reconciliation counts and the builder
        # cannot make.
        with unittest.mock.patch.object(
            measure, "JOBS_AXIS", (*measure.JOBS_AXIS, "query-strings-jobs-")
        ):
            with self.assertRaises(ValueError):
                measure._script("query-strings-jobs-4")

    def test_every_row_states_the_same_budget(self):
        # The axis is the worker count; a budget that moved with it would make
        # each row a different apparatus and the ratios a comparison of two
        # variables.
        for family in measure.JOBS_AXIS:
            for jobs in measure.PARALLEL_JOBS:
                with self.subTest(family=family, jobs=jobs):
                    self.assertIn(
                        f"--parallel-memory {measure.PARALLEL_BUDGET}",
                        measure._script(f"{family}{jobs}"),
                    )

    def test_the_budget_admits_the_widest_row_on_the_coarser_leg(self):
        # A block-decoding source charges one partition a decoded block plus a
        # chunk buffer, and `worker_count` divides the stated bytes by that. A
        # budget below `jobs x (block + chunk)` clamps the top rows silently.
        block = 24 * measure.MIB
        want = measure.PARALLEL_JOBS[-1] * (block + measure.CHUNK_DEFAULT)
        self.assertGreaterEqual(measure.PARALLEL_BUDGET, want)

    def test_the_container_holds_more_than_the_budget_it_states(self):
        # The library is told it may hold `PARALLEL_BUDGET`; the container has
        # to have room for that plus the decoder's dictionaries and the batches
        # in flight, or the figure OOMs instead of measuring.
        self.assertTrue(measure.PARALLEL_MEMORY.endswith("g"))
        self.assertGreater(
            int(measure.PARALLEL_MEMORY[:-1]) * measure.GIB, measure.PARALLEL_BUDGET
        )
        self.assertNotEqual(measure.PARALLEL_MEMORY, measure.Config().memory)

    def test_both_figures_declare_that_departure(self):
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                self.assertEqual(
                    measure.SELECTABLE_BY_ID[fid].memory, measure.PARALLEL_MEMORY
                )

    def test_the_baseline_row_is_one_job(self):
        # `--jobs 1` is `Parallelism::Serial` — the serial code path this
        # project ships, which is what a speedup is a speedup over.
        self.assertEqual(measure.PARALLEL_BASELINE, 1)
        self.assertEqual(measure.PARALLEL_JOBS[0], measure.PARALLEL_BASELINE)

    def test_the_range_runs_past_the_plain_path_ceiling(self):
        # `POOL_DEPTH` clamps the chunk pool to four slots, so a fifth fused
        # worker on a plain source waits. A table stopping at four would leave a
        # reader to infer the scan stopped scaling.
        self.assertGreater(measure.PARALLEL_JOBS[-1], 4)
        self.assertIn(4, measure.PARALLEL_JOBS)

    def test_the_counts_match_the_decode_figures(self):
        # `xz-decode-scaling` is the floor these are read against: it says what
        # the decoder alone does with N workers, and a row with no counterpart
        # there is a comparison nobody can make.
        self.assertEqual(measure.PARALLEL_JOBS, measure.DECODE_WORKERS)

    def test_a_compressed_legs_rate_is_per_plaintext_byte(self):
        # `control_xz` is a compression of `control`, so the plaintext volume is
        # a registered input's own size. Dividing by the compressed size would
        # state a rate five times too low under a heading that reads like the
        # plain leg's.
        for inp, _, _ in measure.PARALLEL_LEGS:
            with self.subTest(input=inp):
                plaintext = measure.PARALLEL_PLAINTEXT[inp]
                self.assertIn(plaintext, measure.INPUTS)
                self.assertEqual(measure.INPUTS[plaintext].suffix, ".sql")
        self.assertEqual(measure.PARALLEL_PLAINTEXT["control_xz"], "control")

    def test_a_compressed_leg_derives_from_the_plaintext_it_is_divided_by(self):
        # Not merely "some plain input of the same nominal size": the two must
        # be the same bytes, or the rate is per a volume the run never decoded.
        for inp, _, _ in measure.PARALLEL_LEGS:
            spec = measure.INPUTS[inp]
            if spec.suffix == ".xz":
                with self.subTest(input=inp):
                    self.assertEqual(spec.derives_from, measure.PARALLEL_PLAINTEXT[inp])

    def test_both_axes_are_crossed_in_the_throughput_legs(self):
        # Plain against `.xz` is whether a decoder is in front of the scan;
        # `parse` against a typed `query` is discovery against extraction. The
        # phase's rule is one line over those two, and a table missing a column
        # would confirm whichever half it kept.
        self.assertEqual(
            sorted({(inp, cmd) for inp, cmd, _ in measure.PARALLEL_LEGS}),
            sorted(
                (inp, cmd)
                for inp in ("control", "control_xz")
                for cmd in ("parse", "query-typed")
            ),
        )

    def test_the_rss_legs_differ_only_in_block_size(self):
        # The figure's whole content is that a compressed reader's per-worker
        # footprint is one decoded block, so a second variable between the legs
        # would make the two columns incomparable.
        legs = [measure.INPUTS[leg] for leg, _ in measure.PARALLEL_RSS_LEGS]
        self.assertEqual({s.derives_from for s in legs}, {"control"})
        self.assertEqual({s.generator for s in legs}, {"generate_xz_input.py"})
        sizes = {
            s.args[s.args.index("--block-size") + 1] if "--block-size" in s.args else "24MiB"
            for s in legs
        }
        self.assertEqual(sizes, {"24MiB", "128MiB"})

    def test_the_second_block_size_is_one_the_generator_admits(self):
        # The generator refuses a size no figure asks for, so a typo here is an
        # error rather than a valid `.xz` whose table row describes a shape
        # nobody registered.
        import generate_xz_input

        spec = measure.INPUTS["control_xz128"]
        self.assertIn(
            spec.args[spec.args.index("--block-size") + 1], generate_xz_input.BLOCK_SIZES
        )

    def test_the_compressed_inputs_have_their_own_nominal_size(self):
        cfg = measure.Config()
        for leg, _ in measure.PARALLEL_RSS_LEGS:
            with self.subTest(leg=leg):
                self.assertLess(measure.nominal_size(cfg, leg), cfg.size_gib * measure.GIB)

    def test_both_figures_read_the_parallel_gate(self):
        # `warm-parallel` gates on steal alone: a reading that occupies every
        # hardware thread is busy by construction, so `warm`'s row would discard
        # every rep of both these figures.
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                self.assertEqual(measure.SELECTABLE_BY_ID[fid].stage, "warm-parallel")

    def test_the_declared_paths_carry_the_leader_and_the_decoder(self):
        # These are the two mechanisms `--jobs` newly reaches; a figure about
        # them that declared neither would read green across the work that moves
        # it most.
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                depends = measure.SELECTABLE_BY_ID[fid].depends
                self.assertIn("pgdump_query/src/leader.rs", depends)
                self.assertIn("vendor/xz-seek/src/", depends)
                self.assertIn("pgdump_query/src/io.rs", depends)

    def test_a_stage_selection_does_not_reach_them(self):
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                self.assertNotIn(
                    "warm", measure.SELECTABLE_BY_ID[fid].stage.split("+")
                )


class PinnedWorkerCount(unittest.TestCase):
    """The other half of the `JOBS_AXIS` exemption.

    `worker_count_problems` catches a shape that pins *nothing*. This catches
    one that pins something else — a shape edited to `--jobs 4` for a sitting,
    or moved into a parallel family without being declared one. Both produce a
    table whose apparatus line is wrong about it, which no `--stale` can see:
    staleness says *re-take*, never *the apparatus moved underneath you*."""

    def test_nothing_outside_the_axis_states_another_count(self):
        self.assertEqual(measure.pinned_count_problems(), [])

    def test_a_shape_that_drifts_off_the_constant_is_reported(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdq parse --source /dump.sql --jobs 4"
        ):
            reported = measure.pinned_count_problems()
        self.assertTrue(reported)
        self.assertTrue(all("states --jobs 4" in line for line in reported))

    def test_the_axis_families_are_exempt(self):
        # They are the exemption, so nothing this reports may name one.
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdq parse --source /dump.sql --jobs 4"
        ):
            reported = measure.pinned_count_problems()
        for line in reported:
            with self.subTest(line=line):
                self.assertFalse(line.startswith(measure.JOBS_AXIS))
                self.assertFalse(line.startswith("decode-"))

    def test_check_fails_on_a_shape_that_drifts(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdq parse --source /dump.sql --jobs 4"
        ):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = measure.cmd_check(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("neither the apparatus's nor a", out.getvalue())

    def test_every_axis_family_has_a_figure_whose_rows_are_the_counts(self):
        """An exemption with no figure behind it is a shape that states an
        arbitrary count and calls it an axis."""
        wanted = set(measure.JOBS_AXIS)
        for fig in measure.EVERY_FIGURE + measure.UNTAKEN:
            if fig.stage != "warm-parallel":
                continue
            for spec in _figure_specs(fig.id):
                for family in list(wanted):
                    if spec.command.startswith(family):
                        wanted.discard(family)
        self.assertEqual(wanted, set())


def _figure_specs(fid: str) -> list:
    """The `RunSpec`s one parallel figure sweeps, without running it."""
    if fid == "parallel-scan-throughput":
        return measure._parallel_specs()
    if fid == "parallel-peak-rss":
        return measure._parallel_rss_specs()
    return []


class XzDecodeScaling(unittest.TestCase):
    """The decode-scaling figure: two `.xz` legs, seven worker counts.

    It is the register's first figure that runs no `pgdq` at all, its first
    compressed input, its first fourth regime and its first departure from the
    512 MB container — so what these hold is that each of those is *declared*
    rather than inherited, since every one of them fails by emitting a
    perfectly plausible table of something else.
    """

    def test_both_legs_are_compressed_inputs(self):
        # Staging, eviction and stamping all key on the file name, so an input
        # staged as `.sql` would collide with the plain dump it is a
        # compression of and the two would evict each other.
        for leg, _ in measure.DECODE_LEGS:
            with self.subTest(leg=leg):
                self.assertEqual(measure.INPUTS[leg].suffix, ".xz")
                self.assertTrue(measure.input_file(leg).endswith(".xz"))

    def test_a_plain_input_is_still_a_sql_file(self):
        self.assertEqual(measure.input_file("control"), "control.sql")

    def test_the_generated_leg_is_a_compression_of_the_control(self):
        # Not a second generation of similar rows: the two would be different
        # bytes behind one register, and the perf generator's own changes would
        # reach the plain figures and not this one.
        self.assertEqual(measure.INPUTS["control_xz"].derives_from, "control")

    def test_a_derived_input_folds_its_source_stamp_in(self):
        # Otherwise a change to `generate_perf_data.py` regenerates every plain
        # input and leaves the compressed leg measuring pre-change rows.
        cfg = measure.Config()
        spec = measure.INPUTS["control_xz"]
        got = measure.input_stamp(spec, cfg)
        unfolded = hashlib.sha256()
        unfolded.update((measure.SCRIPTS / spec.generator).read_bytes())
        unfolded.update(repr(spec.argv(cfg, Path("OUT"))).encode())
        self.assertNotEqual(got, unfolded.hexdigest())

    def test_the_perf_generator_reaches_the_generated_leg(self):
        # The control leg is a *compression of* `control`, so a change to the
        # perf generator moves the bytes this figure decodes. `figures_touched`
        # walks the taken register, which is where this figure now sits, so the
        # declaration has to be reachable through it rather than merely present.
        touched = [f.id for f, _ in measure.figures_touched(["scripts/generate_perf_data.py"])]
        self.assertIn("xz-decode-scaling", touched)
        fig = measure.SELECTABLE_BY_ID["xz-decode-scaling"]
        self.assertIn("scripts/generate_perf_data.py", fig.depends)

    def test_the_decoder_and_the_instrument_are_declared(self):
        # No `pgdq` runs here, so none of the library's own paths can move this
        # figure and none of them is declared. What can is the decoder, the
        # binary that drives it, and the generators behind the two files.
        fig = measure.SELECTABLE_BY_ID["xz-decode-scaling"]
        self.assertIn("vendor/xz-seek/src/", fig.depends)
        self.assertIn("pgdump_query/examples/xz_decode.rs", fig.depends)
        self.assertIn("scripts/generate_xz_input.py", fig.depends)
        for path in fig.depends:
            self.assertFalse(path.startswith("pgdump_query/src/"), path)

    def test_a_compressed_input_has_its_own_nominal_size(self):
        # A dry run that guessed 3.00 GiB for a compressed leg would size the
        # tmpfs budget against a file several times larger than the one staged.
        cfg = measure.Config()
        for leg, _ in measure.DECODE_LEGS:
            with self.subTest(leg=leg):
                self.assertLess(measure.nominal_size(cfg, leg), cfg.size_gib * measure.GIB)

    def test_every_registered_worker_count_has_a_command_shape(self):
        for workers in measure.DECODE_WORKERS:
            with self.subTest(workers=workers):
                script = measure._script(f"decode-{workers}")
                self.assertIn(f"--workers {workers}", script)
                self.assertEqual(script.count("time "), 1)

    def test_the_instrument_stdout_is_not_redirected(self):
        # It is where the decoded byte count comes back, which is the rate's
        # denominator: a compressed input's plaintext volume is in its seek
        # table and nowhere the harness can `stat`.
        self.assertNotIn(">", measure._script("decode-8"))

    def test_a_worker_count_the_figure_does_not_carry_is_an_error(self):
        for command in ("decode-3", "decode-", "decode-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    measure._script(command)

    def test_the_baseline_row_is_one_worker(self):
        # Every cell is a ratio against it, and it is the serial path this
        # build ships today.
        self.assertEqual(measure.DECODE_BASELINE, 1)
        self.assertEqual(measure.DECODE_WORKERS[0], measure.DECODE_BASELINE)

    def test_the_parallel_regime_reads_from_tmpfs(self):
        # It is `warm` staging under a different gate, not a fourth device: a
        # regime names a device, and this one names the same one `warm` does.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = measure.Config(
                cache_dir=root / "ssd", warm_dir=root / "shm", nvme_dir=root / "nvme",
                dry_run=True,
            )
            stager = measure.Stager(cfg, lambda _m: None)
            stager.plan([measure.SELECTABLE_BY_ID["xz-decode-scaling"]])
            session = measure.Session(cfg, stager, lambda _m: None)
            got = session.input_path("control_xz", "warm-parallel")
            self.assertEqual(got, root / "shm" / "control_xz.xz")

    def test_the_parallel_regime_gates_steal_and_nothing_else(self):
        # A 24-worker decode drives the machine to the nineties by
        # construction, so `cpu_busy_pct` cannot gate it; `cpu_steal_pct` is
        # about a neighbour rather than about this run and still can.
        self.assertIsNone(
            measure.contention_verdict(
                {"cpu_busy_pct": 96.0, "psi_cpu_some_pct": 40.0}, "warm-parallel"
            )
        )
        self.assertIsNotNone(
            measure.contention_verdict({"cpu_steal_pct": 12.0}, "warm-parallel")
        )

    def test_the_figure_declares_more_memory_than_the_recorded_apparatus(self):
        # 24 decoded 24 MiB blocks, their compressed windows and their
        # dictionaries do not fit the register's 512 MB, and a figure that
        # departs from the recorded apparatus has to say so rather than inherit
        # it.
        fig = measure.SELECTABLE_BY_ID["xz-decode-scaling"]
        self.assertEqual(fig.memory, measure.DECODE_MEMORY)
        self.assertNotEqual(measure.DECODE_MEMORY, measure.Config().memory)

    #: Every figure permitted to depart from the recorded 512 MB, and nothing
    #: else. Each is a figure that holds N decoded blocks at once, which is the
    #: one reason the register admits: the departure is stated in that figure's
    #: own table.
    MEMORY_DEPARTURES = {
        "xz-decode-scaling",
        "parallel-scan-throughput",
        "parallel-peak-rss",
    }

    def test_every_other_figure_runs_under_the_recorded_memory(self):
        # A departure is a departure only while it is the exception, so this
        # holds the rest of the register to the recorded apparatus rather than
        # to whatever each figure happens to declare. `EVERY_FIGURE`, not
        # `ALL_FIGURES`: an untaken instrument is exactly where a departure
        # arrives unnoticed, since no table of its is in the doc to state it.
        for fig in measure.EVERY_FIGURE:
            if fig.id in self.MEMORY_DEPARTURES:
                continue
            with self.subTest(figure=fig.id):
                self.assertIsNone(fig.memory)

    def test_nothing_departs_without_being_named_here(self):
        declared = {f.id for f in measure.EVERY_FIGURE if f.memory is not None}
        self.assertEqual(declared, self.MEMORY_DEPARTURES)

    def test_the_koji_leg_takes_whole_streams_from_past_the_head(self):
        # koji's first 3 GiB of plaintext compresses about 56x against the
        # file's own 19.4x, and a decode rate is per plaintext byte, so a slice
        # cut at the head would report a rate for bytes unlike the rest of it.
        args = measure.INPUTS["koji_xz"].args
        self.assertIn("--from-offset", args)
        self.assertGreater(measure.KOJI_XZ_OFFSET, 0)
        self.assertEqual(str(measure.KOJI_XZ_STREAMS), args[args.index("--streams") + 1])

    def test_the_koji_leg_has_a_block_for_every_worker(self):
        # A range with fewer blocks than workers clamps, and the instrument
        # refuses a clamped run — so a stream count under the largest worker
        # count would make the top of the table unrunnable rather than wrong.
        self.assertGreaterEqual(measure.KOJI_XZ_STREAMS, measure.DECODE_WORKERS[-1])

    def test_a_stage_selection_does_not_reach_it(self):
        # `--stage warm` splits on `+`, so `warm-parallel` is its own stage and
        # not a `warm` figure with a suffix.
        self.assertNotIn("warm", measure.SELECTABLE_BY_ID["xz-decode-scaling"].stage.split("+"))

    # -- the koji leg's density ------------------------------------------
    #
    # The generator is the figure's apparatus as much as the instrument is, and
    # `scripts/generate_xz_input.py` is one of the figure's own `depends`, so
    # its gate is held here with the rest of them rather than in a test module
    # of its own.

    def test_the_published_slice_density_is_inside_the_band(self):
        # 15.70x is what the 20 GB region yields and what every rate this
        # figure publishes is quoted against. A band that did not contain it
        # would refuse the figure's own input.
        self.assertEqual(
            generate_xz_input.check_koji_density(205215196, 3221749760, 20_000_000_000),
            3221749760 / 205215196,
        )

    def test_kojis_head_is_refused(self):
        # The whole reason `--from-offset` exists: koji's first 3 GiB of
        # plaintext compresses 56.19x against the file's own 19.41x, and a
        # decode rate is a rate per plaintext byte, so a slice cut there is a
        # rate for other bytes under this figure's heading.
        with self.assertRaises(SystemExit) as raised:
            generate_xz_input.check_koji_density(57341052, 3221749760, 0)
        self.assertIn("56.19x", str(raised.exception))

    def test_a_sparser_region_is_refused_too(self):
        # The band is two-sided: koji sampled at twelve depths runs from 5.02x
        # to 33.05x, and both ends are bytes this figure is not taken on.
        with self.assertRaises(SystemExit):
            generate_xz_input.check_koji_density(3221749760 // 5, 3221749760, 16_000_000_000)

    def test_an_empty_slice_has_no_ratio(self):
        with self.assertRaises(SystemExit):
            generate_xz_input.check_koji_density(0, 0, 0)

    def test_the_totals_line_is_read_positionally(self):
        # `xz --list --robot` is tab-separated and positional, and the fields
        # this reads are the compressed and uncompressed totals — the second of
        # which is written down nowhere a `stat` can reach.
        got = generate_xz_input.parse_xz_totals(
            "name\tkoji_xz.xz\n"
            "file\t128\t128\t205215196\t3221749760\t0.064\tCRC64\t0\n"
            "totals\t128\t128\t205215196\t3221749760\t0.064\tCRC64\t0\t1\n"
        )
        self.assertEqual(got, (205215196, 3221749760))

    def test_the_table_quotes_the_band_the_slice_was_cut_within(self):
        # The clause is selected by the leg's own key, so a renamed input would
        # drop it silently; and the band is imported rather than respelled, so
        # the number the table prints is the number a slice was refused
        # against.
        self.assertIn("koji_xz", [leg for leg, _ in measure.DECODE_LEGS])
        self.assertEqual(measure.KOJI_RATIO_MIN, generate_xz_input.KOJI_RATIO_MIN)
        self.assertEqual(measure.KOJI_RATIO_MAX, generate_xz_input.KOJI_RATIO_MAX)

    def test_output_with_no_totals_line_is_refused(self):
        with self.assertRaises(SystemExit):
            generate_xz_input.parse_xz_totals("name\tkoji_xz.xz\n")


class Reported(unittest.TestCase):
    """`key=value` lines a timed binary prints about its own run."""

    def test_it_reads_the_keys_it_is_given(self):
        got = measure.parse_reported("workers=8\nplaintext=3221225472\ndelivered=3221225472\n")
        self.assertEqual(got["workers"], "8")
        self.assertEqual(got["delivered"], "3221225472")

    def test_anything_else_is_ignored_rather_than_refused(self):
        # A binary is free to print whatever else it likes; a parser that
        # refused would couple every instrument's output to this one's shape.
        self.assertEqual(measure.parse_reported("hello\nworkers=2\n= \n"), {"workers": "2"})

    def test_no_report_is_an_empty_report(self):
        self.assertEqual(measure.parse_reported(""), {})


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

    def test_a_blanket_entry_does_not_reach_a_declared_section(self):
        # Empty means every figure and no declared section. A blanket entry is
        # written about the readings a sweep takes, which are durations; koji's
        # are a byte-for-byte comparison of what a scan concludes, so excusing
        # it has to be its author's decision rather than an inheritance.
        got = measure.excused_paths(
            "koji",
            ["scripts/measure.py"],
            {"scripts/measure.py": ["bbb"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_a_declared_section_is_excused_by_being_named(self):
        # The permission itself is unchanged: the harness cannot re-take koji,
        # so an entry naming it is the only discharge short of an hour on the HDD.
        acks = (*self.ACKS, measure.Acknowledged(commit="ddd", figures=("koji",), why="comments"))
        got = measure.excused_paths(
            "koji",
            ["scripts/measure.py"],
            {"scripts/measure.py": ["ddd"]},
            set(),
            acks,
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

    def test_spentness_is_read_off_each_figure_s_own_base(self):
        # An entry recorded between the stamp and a figure taken later is spent
        # for that figure and live for every figure read from the stamp, so it
        # is deleted only when every figure it covers has moved past it.
        bases = {fid: "stamp" for fid in measure.ALL_BY_ID}
        bases["peak-rss"] = "later"
        acks = (
            measure.Acknowledged(commit="aaa", figures=("peak-rss", "map-only"), why="x"),
            measure.Acknowledged(commit="bbb", figures=("peak-rss",), why="x"),
        )
        # `aaa` is inside `map-only`'s range and behind `peak-rss`'s; `bbb`
        # covers only the figure that has moved past it.
        spent = measure.spent_acknowledgements(
            acks, bases, ancestor=lambda commit, rev: rev == "later"
        )
        self.assertEqual(spent, ["bbb"])

    def test_an_entry_covering_every_figure_needs_every_base_past_it(self):
        bases = {fid: "stamp" for fid in measure.ALL_BY_ID}
        bases["peak-rss"] = "later"
        acks = (measure.Acknowledged(commit="aaa", figures=(), why="x"),)
        self.assertEqual(
            measure.spent_acknowledgements(acks, bases, ancestor=lambda c, rev: rev == "later"),
            [],
        )
        self.assertEqual(
            measure.spent_acknowledgements(acks, bases, ancestor=lambda c, rev: True), ["aaa"]
        )

    def test_a_blanket_entry_s_spentness_ignores_the_declared_sections(self):
        # `--check` passes the union of both bases, so an entry *naming* koji is
        # spent against koji's own marker. A blanket entry covers no section, so
        # a section left behind may not hold it alive past every figure.
        bases = {fid: "later" for fid in measure.ALL_BY_ID}
        bases |= {oid: "stamp" for oid in measure.NOT_OURS}
        acks = (measure.Acknowledged(commit="aaa", figures=(), why="x"),)
        self.assertEqual(
            measure.spent_acknowledgements(acks, bases, ancestor=lambda c, rev: rev == "later"),
            ["aaa"],
        )
        named = (measure.Acknowledged(commit="aaa", figures=("koji",), why="x"),)
        self.assertEqual(
            measure.spent_acknowledgements(named, bases, ancestor=lambda c, rev: rev == "later"),
            [],
        )

    def test_a_figure_with_no_base_spends_nothing(self):
        acks = (measure.Acknowledged(commit="aaa", figures=("map-only",), why="x"),)
        self.assertEqual(
            measure.spent_acknowledgements(acks, {"map-only": None}, ancestor=lambda c, r: True),
            [],
        )


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
        self.assertIsNone(measure.Config().unpublishable_reason)

    def test_a_smaller_input_is_not(self):
        self.assertFalse(measure.Config(size_gib=0.05).publishable)

    def test_an_overridden_rep_count_is_not(self):
        self.assertFalse(measure.Config(reps_override=1).publishable)

    def test_a_diagnostic_sitting_is_not(self):
        # `--alone` is not an apparatus change; it is a selection that leaves a
        # shared reading measured by the figure that borrows it, which is the
        # same table the publication refusal declines to let a sitting take.
        self.assertFalse(measure.Config(alone=True).publishable)

    def test_each_reason_says_which_departure_it_is(self):
        # One sentence per departure, because the log line and the banner over
        # the tables both print it and a reader has to know which run this was.
        self.assertIn("--alone", measure.Config(alone=True).unpublishable_reason)
        self.assertIn("--dry-run", measure.Config(dry_run=True).unpublishable_reason)
        self.assertIn(
            "overrode the recorded apparatus",
            measure.Config(reps_override=1).unpublishable_reason,
        )

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


class RegisterBoundary(unittest.TestCase):
    """The doc carries sections the harness does not own, and the session stamp
    claims nothing about them. A figure announces itself with a marker and an
    apparatus line; a section outside the register announced itself with a
    prose sentence paragraphs from the numbers, which nothing checked."""

    DOC = measure.REPO / "docs/design/measurements.md"

    def _doc(self, tmp: str, text: str) -> Path:
        doc = Path(tmp) / "measurements.md"
        doc.write_text(text)
        return doc

    def test_the_doc_declares_every_section_the_harness_disowns(self):
        # Both ways: a declaration naming nothing, and a disowned section that
        # declares nothing, are the two halves of the same drift.
        declared = {i for i, _ in measure.outside_register_sections(self.DOC.read_text())}
        self.assertEqual(declared, set(measure.NOT_OURS))

    def test_no_declared_section_carries_a_figure_marker(self):
        for oid, figs in measure.outside_register_sections(self.DOC.read_text()):
            with self.subTest(section=oid):
                self.assertEqual(figs, [])

    def test_a_declaration_covers_its_own_subheadings(self):
        text = (
            "## Outside\n\n<!-- outside-register: koji -->\n\n"
            "### A subsection of it\n\n<!-- figure: map-only -->\n"
        )
        self.assertEqual(measure.outside_register_sections(text), [("koji", ["map-only"])])

    def test_a_declaration_stops_at_the_next_section(self):
        text = (
            "## Outside\n\n<!-- outside-register: koji -->\n\n"
            "## A figure of its own\n\n<!-- figure: map-only -->\n"
        )
        self.assertEqual(measure.outside_register_sections(text), [("koji", [])])

    def test_a_comment_inside_a_code_fence_does_not_end_a_section(self):
        # measurements.md publishes a `sh` block whose first line is `# maps to
        # EOF …`, which reads as a level-1 heading.
        text = (
            "## Outside\n\n<!-- outside-register: koji -->\n\n"
            "```sh\n# maps to EOF (the table never matches)\npgdq parse\n```\n\n"
            "<!-- figure: map-only -->\n"
        )
        self.assertEqual(measure.outside_register_sections(text), [("koji", ["map-only"])])

    def test_headings_skip_fenced_blocks(self):
        text = "# Real\n\n```sh\n# not a heading\n```\n\n## Also real\n"
        self.assertEqual([level for _, level in measure.headings(text)], [1, 2])

    def test_the_stamp_is_scoped_to_the_register(self):
        # The claim was "every figure below", over a document that prints
        # readings the harness did not take.
        self.assertIn("<!-- figure:", measure.session_stamp("deadbee", dirty=False))

    def test_the_stamp_s_scope_clause_is_not_read_as_a_figure_marker(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(tmp, measure.session_stamp("deadbee", dirty=False) + "\n")
            self.assertEqual(measure.markers_in(doc), [])

    def test_the_doc_s_own_stamp_is_scoped_too(self):
        head = self.DOC.read_text().split("## The apparatus")[0]
        self.assertIn("`<!-- figure: … -->` marker, and no other", head)


class OutsideInvalidation(unittest.TestCase):
    """A declared section carries the same invalidation edge a figure does.

    Being outside the register says the harness cannot re-take the readings. It
    was also saying, by omission, that nothing would ever be told when they
    went wrong: `--stale` walked `ALL_BY_ID`, so koji could not go red however
    far the scanner moved under it. `Outside.depends` is that edge, and the
    commit it is measured from lives in the section's own marker — in the
    document, because `session-drift` declares `scripts/measure.py` and a run's
    provenance recorded here would mark a figure stale for saying where another
    reading came from."""

    DOC = measure.REPO / "docs/design/measurements.md"

    def test_every_section_that_publishes_a_reading_declares_an_edge(self):
        # `benches` is the one that legitimately declares nothing, and it is
        # the same sentence that puts it outside the register: it quotes no
        # number in the doc, so there is nothing a diff could falsify.
        publishing = {oid for oid, o in measure.NOT_OURS.items() if o.depends}
        self.assertEqual(publishing, {"koji", "rss-attribution"})
        self.assertEqual(measure.NOT_OURS["benches"].depends, ())

    def test_koji_s_edge_covers_what_a_koji_run_concludes(self):
        # The block list, the offsets and the row/byte totals come off the read
        # path, the scanner, the map and the cache; the `info --detail` report
        # they are compared as text against comes off the preamble, the type
        # resolution and the CLI -- and that last one is not hypothetical, the
        # two compared reports differing by one line of user-defined-type
        # detail the report had gained in between.
        for path in (
            "pgdump_query/src/io.rs",
            "pgdump_query/src/scan.rs",
            "pgdump_query/src/map.rs",
            "pgdump_query/src/cache.rs",
            "pgdump_query/src/preamble.rs",
            "pgdump_query/src/resolve.rs",
            "pgdump_query/src/pgtype.rs",
            "pgdump_query-cli/src/main.rs",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    measure.declared_hits(measure.NOT_OURS["koji"], [path]), [path]
                )

    def test_the_attribution_s_edge_is_the_registered_instrument_s(self):
        # Read off the figure rather than copied: the readings differ from it
        # in provenance, not in what moves them, and two spellings of one edge
        # drift in the window before `M74` lands.
        self.assertEqual(
            measure.NOT_OURS["rss-attribution"].depends,
            measure.EVERY_BY_ID["rss-attribution"].depends,
        )

    def test_one_predicate_answers_for_a_figure_and_a_section(self):
        # `declared_hits` is what `--stale` intersects a diff with, and an
        # `Outside` declares its edge in the same field a `Figure` does so that
        # no second authority over "what moves a reading" is written.
        self.assertEqual(
            measure.declared_hits(measure.NOT_OURS["koji"], ["docs/design/roadmap.md"]), []
        )

    def test_the_marker_carries_the_commit_and_is_read_back(self):
        text = "<!-- outside-register: koji — taken at `f5768e7` — nothing here is a figure -->"
        self.assertEqual(measure.outside_sittings(text), {"koji": "f5768e7"})
        self.assertEqual(measure.OUTSIDE_RE.findall(text), ["koji"])

    def test_a_section_with_no_reading_declares_no_commit(self):
        text = "<!-- outside-register: benches — tripwires, not figures -->"
        self.assertEqual(measure.outside_sittings(text), {})

    def test_a_commit_in_the_prose_under_a_section_is_not_its_provenance(self):
        # Same reason a figure's sitting went inside its marker: a sibling can
        # go missing on its own, and its absence is silent.
        text = "<!-- outside-register: koji -->\n\nThe last run was at commit `f5768e7`.\n"
        self.assertEqual(measure.outside_sittings(text), {})

    def test_the_doc_s_own_declarations_all_stand(self):
        self.assertEqual(measure.outside_sitting_problems(self.DOC.read_text()), [])

    def _about(self, text: str, oid: str) -> list[str]:
        """The refusals naming one section. Every declared section is walked on
        every call — a fragment that omits one is a section declaring nothing —
        so a test about one of them says which."""
        return [line for line in measure.outside_sitting_problems(text) if line.startswith(oid)]

    def test_an_edge_with_no_commit_to_measure_it_from_is_refused(self):
        # The state koji was in before this landed, reached from the other
        # side: a live-looking edge that `--stale` has no range to intersect.
        problems = self._about("<!-- outside-register: koji -->", "koji")
        self.assertEqual(len(problems), 1)
        self.assertIn("no commit", problems[0])

    def test_a_commit_no_edge_reads_is_refused(self):
        problems = self._about(
            "<!-- outside-register: benches — taken at `f5768e7` -->", "benches"
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("no invalidation edge", problems[0])

    def test_a_commit_that_is_not_one_is_refused(self):
        problems = measure.outside_sitting_problems(
            self.DOC.read_text(), resolve=lambda rev: None
        )
        self.assertEqual(len(problems), 2)
        for line in problems:
            self.assertIn("not a commit in this repository", line)

    def test_a_provenance_line_for_a_section_nobody_disowns_is_refused(self):
        problems = self._about(
            "<!-- outside-register: koji — taken at `f5768e7` -->\n"
            "<!-- outside-register: gone — taken at `f5768e7` -->",
            "gone",
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("no section this harness disowns", problems[0])

    def test_a_declared_section_is_argued_from_its_own_marker(self):
        # Never from the session stamp, which is scoped to the register and
        # says nothing about a section outside it.
        bases = measure.outside_bases(self.DOC.read_text())
        self.assertEqual(set(bases), {"koji", "rss-attribution"})
        self.assertNotEqual(bases["koji"], measure.stamp_in(self.DOC.read_text()))

    def test_since_asks_one_question_of_every_section(self):
        bases = measure.outside_bases(self.DOC.read_text(), "deadbee")
        self.assertEqual(set(bases.values()), {"deadbee"})

    def test_an_acknowledgement_may_name_a_declared_section(self):
        # The harness cannot re-take koji, so an entry is the only discharge
        # that does not cost an hour on the HDD -- and `--check` must not read
        # that entry as naming nothing.
        acks = [measure.Acknowledged(commit="aaa", figures=("koji",), why="x")]
        unknown, _ = measure.acknowledgement_problems(
            acks, {**measure.ALL_BY_ID, **measure.NOT_OURS}, []
        )
        self.assertEqual(unknown, [])
        typo = [measure.Acknowledged(commit="aaa", figures=("kojii",), why="x")]
        unknown, _ = measure.acknowledgement_problems(
            typo, {**measure.ALL_BY_ID, **measure.NOT_OURS}, []
        )
        self.assertEqual(len(unknown), 1)


class Sittings(unittest.TestCase):
    """A figure may be published outside the sweep, and then its own marker
    carries the commit it was taken at. Every reader of the session stamp
    argues from that commit instead — half-applying it leaves a mechanism
    reasoning from a commit the doc itself says is not the figure's."""

    DOC = measure.REPO / "docs/design/measurements.md"

    def test_the_emitted_marker_declares_the_sitting_and_is_read_back(self):
        marker = measure.figure_marker("peak-rss", "7ee5db5")
        self.assertEqual(measure.figure_sittings(marker), {"peak-rss": "7ee5db5"})
        self.assertEqual(measure.MARKER_RE.findall(marker), ["peak-rss"])

    def test_a_figure_from_the_sweep_declares_nothing(self):
        # The datum is present only where it differs from the stamp: a figure
        # that declares nothing came from the sweep.
        self.assertEqual(measure.figure_sittings(measure.figure_marker("map-only")), {})

    def test_the_stamp_s_own_scope_clause_is_not_a_sitting(self):
        stamp = measure.session_stamp("deadbee", dirty=False)
        self.assertEqual(measure.figure_sittings(stamp), {})

    def test_a_sitting_is_read_only_out_of_its_own_marker(self):
        # A commit named in the prose *under* a figure is not its provenance;
        # the marker is, which is why the datum went inside it.
        text = (
            "<!-- figure: peak-rss — reproduce with `x` -->\n\n"
            "This figure was taken alone, at commit `7ee5db5`.\n"
        )
        self.assertEqual(measure.figure_sittings(text), {})

    def test_the_doc_carries_exactly_the_sittings_the_register_permits(self):
        problems = measure.sitting_problems(
            measure.figure_sittings(self.DOC.read_text()),
            measure.stamped_commit(self.DOC),
        )
        self.assertEqual(problems, [])

    def test_a_base_is_the_figure_s_own_sitting_where_it_declares_one(self):
        text = measure.session_stamp("aaaaaaa", dirty=False) + "\n" + measure.figure_marker(
            "peak-rss", "bbbbbbb"
        )
        bases = measure.figure_bases(text)
        self.assertEqual(bases["peak-rss"], "bbbbbbb")
        self.assertEqual(bases["map-only"], "aaaaaaa")
        self.assertEqual(set(bases), set(measure.ALL_BY_ID))

    def test_an_explicit_since_overrides_every_figure(self):
        # `--since` asks one deliberate question of the whole document.
        text = measure.session_stamp("aaaaaaa", dirty=False) + "\n" + measure.figure_marker(
            "peak-rss", "bbbbbbb"
        )
        bases = measure.figure_bases(text, "ccccccc")
        self.assertEqual(set(bases.values()), {"ccccccc"})

    def test_an_unstamped_doc_leaves_a_figure_with_no_base(self):
        bases = measure.figure_bases("# Measurements\n")
        self.assertIsNone(bases["map-only"])

    def test_a_figure_that_shares_a_reading_may_not_be_published_alone(self):
        # What one sitting buys is differencing, so the condition is the borrow
        # graph: `allocator`'s reference column *is* three other tables' rows.
        self.assertEqual(measure.entangled_with("peak-rss"), [])
        self.assertIn("census-brace-free", measure.entangled_with("allocator"))

    def test_a_derivation_entangles_in_both_directions(self):
        # Not a closure edge, and still an edge: `cross-file-floor`'s first row
        # is a difference over `nested-end-to-end`'s reps.
        self.assertIn("cross-file-floor", measure.entangled_with("nested-end-to-end"))
        self.assertIn("nested-end-to-end", measure.entangled_with("cross-file-floor"))

    def test_an_entangled_sitting_is_refused_by_the_doc_and_by_the_run(self):
        problems = measure.sitting_problems({"allocator": "bbbbbbb"}, "aaaaaaa")
        self.assertEqual(len(problems), 1)
        self.assertIn("census-brace-free", problems[0])
        refusals = measure.publication_refusals([measure.ALL_BY_ID["allocator"]])
        self.assertEqual(len(refusals), 1)
        self.assertIn("only a sweep", refusals[0])

    def test_a_figure_standing_in_no_edge_may_be_taken_on_its_own(self):
        self.assertEqual(measure.publication_refusals([measure.ALL_BY_ID["peak-rss"]]), [])

    def _cli_stderr(self, argv: list[str]) -> str:
        """`main` up to its first refusal, with nothing measured.

        `emit` is stubbed because the question is which refusals fire before
        it; a missing release binary is a refusal of its own and is allowed to
        be the one that ends the run."""
        err = io.StringIO()
        with unittest.mock.patch.object(measure, "emit", return_value=0):
            with contextlib.redirect_stderr(err), contextlib.suppress(SystemExit):
                measure.main(argv)
        return err.getvalue()

    def test_an_entangled_figure_is_refused_before_the_measurement_is_spent(self):
        self.assertIn(
            "publish outside the document's session stamp",
            self._cli_stderr(["--figure", "allocator"]),
        )

    def test_the_refusal_stops_firing_for_a_diagnostic_sitting(self):
        # Not an exemption clause: `--alone` marks the run unpublishable, so
        # the guard -- which asks only of a publishable run -- has no
        # publication left to refuse.
        self.assertNotIn(
            "publish outside the document's session stamp",
            self._cli_stderr(["--figure", "allocator", "--alone"]),
        )

    def test_a_diagnostic_sitting_is_not_told_to_fold_its_tables_in(self):
        reason = measure.Config(alone=True).unpublishable_reason
        lead = measure.partial_lead(1, "deadbee", dirty=False, allocator=None, unpublishable=reason)
        self.assertIn("do not enter the document", lead)
        self.assertNotIn("fold each table in", lead)

    def test_a_publishable_partial_sitting_still_says_how_to_fold_it_in(self):
        lead = measure.partial_lead(1, "deadbee", dirty=False, allocator=None, unpublishable=None)
        self.assertIn("fold each table in", lead)
        self.assertIn("does not stamp the document", lead)

    def test_a_sitting_that_repeats_the_stamp_is_a_marker_that_should_not_be_there(self):
        problems = measure.sitting_problems(
            {"peak-rss": "aaaaaaa"},
            "aaaaaaa",
            resolve=lambda rev: rev * 5,
            ancestor=lambda a, b: True,
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("stamp's own commit", problems[0])

    def test_a_sitting_older_than_the_stamp_is_refused(self):
        # An older sitting cannot legitimately exist — a sweep replaces every
        # table at once — so it is a marker a sweep left behind or a hand edit,
        # and either puts --stale back on the wrong commit.
        problems = measure.sitting_problems(
            {"peak-rss": "bbbbbbb"},
            "aaaaaaa",
            resolve=lambda rev: rev * 5,
            ancestor=lambda a, b: False,
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("does not descend", problems[0])

    def test_a_descendant_sitting_passes(self):
        self.assertEqual(
            measure.sitting_problems(
                {"peak-rss": "bbbbbbb"},
                "aaaaaaa",
                resolve=lambda rev: rev * 5,
                ancestor=lambda a, b: True,
            ),
            [],
        )

    def test_a_sitting_naming_no_commit_is_refused(self):
        problems = measure.sitting_problems(
            {"peak-rss": "bbbbbbb"}, "aaaaaaa", resolve=lambda rev: None
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("not a commit", problems[0])

    def test_a_sitting_with_no_stamp_to_be_outside_of_is_refused(self):
        problems = measure.sitting_problems({"peak-rss": "bbbbbbb"}, None)
        self.assertEqual(len(problems), 1)
        self.assertIn("session stamp names no commit", problems[0])

    def test_the_accounting_sentence_is_generated_not_counted(self):
        whole = measure.sitting_accounting([])
        self.assertIn(f"All {len(measure.ALL_FIGURES)} figures", whole)
        one = measure.sitting_accounting([("peak-rss", "7ee5db5")])
        self.assertIn(f"{len(measure.ALL_FIGURES) - 1} of the {len(measure.ALL_FIGURES)}", one)
        self.assertIn("`peak-rss` (`7ee5db5`)", one)
        self.assertIn("The other carries its own", one)
        two = measure.sitting_accounting([("a", "1234567"), ("b", "2345678")])
        self.assertIn("The other 2 carry their own", two)

    def test_the_doc_carries_the_sentence_the_harness_generates(self):
        # Hand-maintaining the count is what went wrong silently: the sentence
        # claiming every marker was false for as long as one figure stood
        # outside the sweep and said so only in prose.
        text = self.DOC.read_text()
        wanted = measure.sitting_accounting(sorted(measure.figure_sittings(text).items()))
        self.assertIn(" ".join(wanted.split()), " ".join(text.split()))

    def test_the_stamp_carries_that_sentence_too(self):
        stamp = measure.session_stamp("deadbee", dirty=False, outside=[("peak-rss", "7ee5db5")])
        self.assertIn(measure.sitting_accounting([("peak-rss", "7ee5db5")]), stamp)
        # And the stamp is still the one `stamped_commit` reads back: the
        # accounting's own shas must not be mistaken for the sitting's.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(stamp + "\n")
            self.assertEqual(measure.stamped_commit(doc), "deadbee")


class KojiRecipe(unittest.TestCase):
    """koji is never run from here, but the invocation is owned here — three
    hand-maintained copies is how a documented command was found that could
    not execute. Each assertion below is a mistake that has cost a run."""

    def _recipe(self, wrap=False, jobs=measure.SWEEP_JOBS) -> str:
        return measure.koji_recipe(measure.Config(), "pgdq-koji", wrap, jobs)

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

    def test_the_run_s_own_resident_set_can_be_read(self):
        # koji's record carried an RSS row that no run since has measured,
        # because the recipe captured no memory figure at all. `VmHWM` off the
        # host pid is the instrument: the cgroup's `memory.peak` is charged the
        # page cache of a 784 GB read and reports the limit.
        recipe = self._recipe()
        self.assertIn("VmHWM", recipe)
        self.assertIn("{{.State.Pid}}", recipe)

    def test_the_resident_set_capture_is_outside_the_exec_d_command(self):
        # It is read from the host while the scan runs; adding anything to the
        # container's command would cost `exec` and with it the interrupt guard.
        for line in self._recipe().splitlines():
            if "exec /pgdq parse" in line:
                with self.subTest(line=line):
                    self.assertNotIn("VmHWM", line)

    def test_the_wrap_recipe_stops_reports_resumes_and_compares(self):
        wrap = self._recipe(wrap=True)
        for fragment in ("nerdctl stop", "info --dqcache", "--detail", "cmp "):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, wrap)

    def test_the_wrap_resumes_the_identical_command(self):
        wrap = self._recipe(wrap=True)
        legs = [ln for ln in wrap.splitlines() if "exec /pgdq parse" in ln]
        self.assertEqual(len(legs), 2)
        self.assertEqual(legs[0], legs[1])

    def test_the_scan_states_its_worker_count(self):
        # Nothing this module builds inherits the CLI's `--jobs` default, koji
        # included — a 784 GB scan that cannot say how many workers read it is
        # not a regression check against anything.
        self.assertIn(f"--jobs {measure.SWEEP_JOBS}", self._recipe())

    def test_the_worker_count_is_the_callers_to_state(self):
        # koji's parallel leg is a leg at some count against a serial one, so
        # the count is a parameter here where every other invocation pins it.
        self.assertIn("--jobs 8", self._recipe(jobs=8))
        self.assertNotIn("--jobs 1 ", self._recipe(jobs=8))

    def test_the_wrap_legs_agree_on_the_count(self):
        # Resuming under a different arrangement is a second variable in a
        # check that has one. `test_the_wrap_resumes_the_identical_command`
        # holds the whole line; this says which part of it matters.
        legs = [ln for ln in self._recipe(wrap=True, jobs=8).splitlines() if "--jobs" in ln]
        self.assertEqual(len(legs), 2)
        for line in legs:
            with self.subTest(line=line):
                self.assertIn("--jobs 8", line)

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

    def test_every_profiled_invocation_states_its_worker_count(self):
        # The sixth silent failure: a sampling profile's buckets are per
        # thread, so a profile that inherited the CLI's default would attribute
        # a scan among workers the figure it explains never ran. The axis pair
        # states a count too -- its own, which is the only thing separating the
        # two profiles -- so what is asserted is that every recorded line pins
        # one, not that every one pins the same one.
        recorded = [ln for ln in self._recipe().splitlines() if ln.strip().startswith("-- ")]
        self.assertEqual(
            len(recorded),
            len(measure.PROFILE_INPUTS) * len(measure.PROFILE_SHAPES)
            + len(measure.PROFILE_AXIS),
        )
        axis = {f"--jobs {s.rpartition('-jobs-')[2]}" for s, _ in measure.PROFILE_AXIS}
        for line in recorded:
            with self.subTest(line=line):
                self.assertTrue(
                    f"--jobs {measure.SWEEP_JOBS}" in line
                    or any(f"{j} " in line for j in axis),
                    line,
                )

    def test_the_axis_pair_differs_only_in_its_worker_count(self):
        """A difference read bucket by bucket is only a difference if the two
        argvs are otherwise identical -- a budget or a cache path that moved
        with the count would put a second variable in the one reading this
        pair exists to isolate."""
        argvs = [
            measure.profile_argv(shape, "/dump.sql", "/tmp/x.dqcache")
            for shape, _ in measure.PROFILE_AXIS
        ]
        self.assertEqual(len(argvs), 2)
        first, second = argvs
        self.assertEqual(len(first), len(second))
        differing = [i for i, (a, b) in enumerate(zip(first, second)) if a != b]
        self.assertEqual(len(differing), 1, f"{first} vs {second}")
        self.assertEqual(first[differing[0] - 1], "--jobs")

    def test_the_axis_pair_is_taken_on_one_input_it_stages(self):
        # It is a pair, not a second cross product: profiling it over `arrays`
        # too would take readings nothing reads. Whatever input it names is
        # still copied in and removed again.
        cfg = measure.Config()
        recipe = measure.profile_recipe(cfg)
        for shape, name in measure.PROFILE_AXIS:
            with self.subTest(shape=shape):
                self.assertIn(f"profile-{shape}-{name}.data", recipe)
                self.assertIn(f"profile-{shape}-{name}.txt", recipe)
                self.assertIn(f"cp -n {cfg.cache_dir / f'{name}.sql'}", recipe)
                self.assertIn(str(cfg.warm_dir / f"{name}.sql"), recipe.splitlines()[-1])

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
        shapes = list(measure.PROFILE_SHAPES) + [s for s, _ in measure.PROFILE_AXIS]
        for shape in shapes:
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


class ColdNvme(unittest.TestCase):
    """The third device class, and the only instrument that can price a
    readahead, `fadvise` or chunk-size default.

    What has to hold is that it really is a third *device*: an input read out
    of the SSD cache, or out of page cache, would produce a plausible table of
    the wrong thing — which is the failure mode every check here is aimed at.
    """

    def test_the_figure_reads_from_the_nvme_and_not_the_ssd(self):
        fig = measure.FIGURES_BY_ID["scan-throughput-nvme"]
        self.assertEqual(fig.nvme_inputs, ("control", "large_object", "insert_run"))
        # A `cold_inputs` entry would be read in place off the SSD cache, which
        # is the device the cold table above already measures.
        self.assertEqual(fig.cold_inputs, ())
        self.assertEqual(fig.warm_inputs, ())

    def test_it_measures_the_same_three_shapes_and_a_floor(self):
        # The three tables are read against each other as ratios, so they have
        # to be the same rows over the same files.
        cold = measure.FIGURES_BY_ID["scan-throughput-cold"]
        self.assertEqual(
            measure.FIGURES_BY_ID["scan-throughput-nvme"].nvme_inputs, cold.cold_inputs
        )
        specs = measure._throughput_specs("cold-nvme")
        self.assertEqual([s.regime for s in specs], ["cold-nvme"] * 4)
        self.assertEqual(specs[-1].command, "dd")

    def test_the_nvme_regime_resolves_under_the_nvme_directory(self):
        cfg = measure.Config(dry_run=True, warm_budget=8)
        session = measure.Session(cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None)
        self.assertEqual(session.input_path("control", "cold-nvme").parent, cfg.nvme_dir)
        self.assertEqual(session.input_path("control", "cold").parent, cfg.cache_dir)
        self.assertEqual(session.input_path("control", "warm").parent, cfg.warm_dir)
        # Three regimes, three devices: two of them resolving to one directory
        # would be a table of the wrong thing that still formats correctly.
        self.assertEqual(len({cfg.nvme_dir, cfg.cache_dir, cfg.warm_dir}), 3)

    def test_a_copy_of_stale_bytes_is_replaced_rather_than_measured(self):
        # The SSD cache's own rule, one device along: a stamp that does not
        # match the generator's source is a file benchmarking pre-change bytes.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = measure.Config(cache_dir=root / "ssd", nvme_dir=root / "nvme")
            (root / "ssd").mkdir()
            (root / "ssd" / "control.sql").write_bytes(b"new bytes")
            (root / "ssd" / "control.stamp").write_text(
                measure.input_stamp(measure.INPUTS["control"], cfg) + "\n"
            )
            (root / "nvme").mkdir()
            (root / "nvme" / "control.sql").write_bytes(b"old bytes")
            (root / "nvme" / "control.stamp").write_text("a stamp from another generator\n")
            stager = measure.Stager(cfg, lambda _m: None)
            got = stager.nvme_path("control")
            self.assertEqual(got.read_bytes(), b"new bytes")

    def test_a_matching_copy_is_left_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = measure.Config(cache_dir=root / "ssd", nvme_dir=root / "nvme")
            stamp = measure.input_stamp(measure.INPUTS["control"], cfg)
            for name, body in (("ssd", b"new bytes"), ("nvme", b"already here")):
                (root / name).mkdir()
                (root / name / "control.sql").write_bytes(body)
                (root / name / "control.stamp").write_text(stamp + "\n")
            stager = measure.Stager(cfg, lambda _m: None)
            self.assertEqual(stager.nvme_path("control").read_bytes(), b"already here")

    def test_every_regime_is_gated_for_contention(self):
        # A regime with no limits gates nothing, so a new one nobody added a
        # row for would take every reading it was handed, however busy the
        # machine was. The set is derived rather than written out: the tuple
        # this used to iterate was missing `warm-parallel` from the day that
        # regime landed, so the fourth regime went unchecked by the test whose
        # whole subject it is.
        for regime in measure.registered_regimes():
            with self.subTest(regime=regime):
                self.assertTrue(measure.CONTENTION_LIMITS.get(regime))
        self.assertIn("warm-parallel", measure.registered_regimes())
        self.assertIsNotNone(
            measure.contention_verdict({"cpu_busy_pct": 99.0}, "cold-nvme")
        )
        self.assertIsNone(measure.contention_verdict({"cpu_busy_pct": 1.0}, "cold-nvme"))

    def test_the_regime_vocabulary_reconciles_both_ways(self):
        """`REGIMES`, `CONTENTION_LIMITS` and the figures' own `stage`
        declarations name the same four regimes.

        Each of the three is the one a half-landed regime would be missing
        from, and each fails silently on its own: a `stage` token nothing
        declares takes a reading off the wrong device, a `REGIMES` row with no
        gate takes one off a busy machine, and a gate for a regime no figure
        runs is a row nobody will ever notice is wrong."""
        declared = set(measure.REGIMES)
        self.assertEqual(set(measure.registered_regimes()), declared)
        self.assertEqual(set(measure.CONTENTION_LIMITS), declared)
        for name, regime in measure.REGIMES.items():
            with self.subTest(regime=name):
                self.assertIn(regime.area, measure.STAGING_AREAS)

    def test_a_stage_that_names_no_regime_is_declared_as_such(self):
        # `criterion` and `derived` are exempt because they read no staged
        # input; anything else exempted by hand would be a regime hidden from
        # the reconciliation above.
        for name in measure.NON_REGIME_STAGES:
            with self.subTest(stage=name):
                self.assertNotIn(name, measure.REGIMES)
                # And each is a stage some figure actually carries, so an
                # exemption cannot outlive the thing it exempts.
                self.assertTrue(
                    any(name in f.stage.split("+") for f in measure.EVERY_FIGURE)
                )

    def test_an_unknown_regime_is_refused_rather_than_resolved(self):
        """The defect this closed: `input_path` matched two names and returned
        the *warm* path for everything else, while the cache drop fired on any
        name starting with `cold` — so a `cold-parallel` regime added to a
        figure and nowhere else would have dropped the page cache and then read
        from tmpfs, publishing warm readings under a cold heading."""
        with self.assertRaises(ValueError) as raised:
            measure.regime_spec("cold-parallel")
        self.assertIn("cold-parallel", str(raised.exception))
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(dry_run=True, cache_dir=Path(tmp) / "ssd")
            session = measure.Session(cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None)
            with self.assertRaises(ValueError):
                session.input_path("control", "cold-parallel")

    def test_the_staging_area_a_regime_reads_is_the_one_it_declares(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = measure.Config(
                dry_run=True,
                cache_dir=root / "ssd",
                nvme_dir=root / "nvme",
                warm_dir=root / "shm",
                warm_budget=8.0,
            )
            session = measure.Session(cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None)
            areas = {
                "cold": root / "ssd",
                "cold-nvme": root / "nvme",
                "warm": root / "shm",
                "warm-parallel": root / "shm",
            }
            for regime, parent in areas.items():
                with self.subTest(regime=regime):
                    self.assertEqual(session.input_path("control", regime).parent, parent)

    def test_the_nvme_area_being_full_is_refused_before_any_run(self):
        # An empty staging area, so the check is against what still has to be
        # copied there rather than against whatever this machine happens to
        # have left behind from an earlier sitting.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(dry_run=True, nvme_dir=Path(tmp) / "nvme")
            stager = measure.Stager(cfg, lambda _m: None)
            figures = [measure.FIGURES_BY_ID["scan-throughput-nvme"]]
            stager.plan(figures)
            tiny = collections.namedtuple("usage", "total used free")(0, 0, 1024)
            with unittest.mock.patch.object(measure.shutil, "disk_usage", return_value=tiny):
                problems = stager._nvme_preflight(figures)
        self.assertTrue(any("cold-NVMe" in p for p in problems), problems)

    def test_a_figure_with_no_nvme_inputs_checks_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(dry_run=True, nvme_dir=Path(tmp) / "nvme")
            stager = measure.Stager(cfg, lambda _m: None)
            self.assertEqual(
                stager._nvme_preflight([measure.FIGURES_BY_ID["scan-throughput-warm"]]), []
            )
            # Not even a directory: a check that mkdir'd one for a sweep that
            # takes no NVMe figure would leave the area behind on every run.
            self.assertFalse((Path(tmp) / "nvme").exists())

    def test_a_cold_stage_selection_does_not_reach_another_device(self):
        # `--stage` splits on `+` rather than matching as a substring, so
        # `cold+warm` is selected by both and `cold-nvme` by neither.
        cold = [f.id for f in measure.FIGURES if "cold" in f.stage.split("+")]
        self.assertIn("scan-throughput-cold", cold)
        self.assertIn("census-brace-free", cold)
        self.assertNotIn("scan-throughput-nvme", cold)
        nvme = [f.id for f in measure.FIGURES if "cold-nvme" in f.stage.split("+")]
        self.assertEqual(sorted(nvme), ["chunk-size", "scan-throughput-nvme"])
        # `chunk-size` is the one figure taken in all three regimes, so it is
        # also the one that would catch a selection rule reading `cold` as a
        # prefix of `cold-nvme` in either direction.
        self.assertIn("chunk-size", cold)


class ChunkSize(unittest.TestCase):
    """The read chunk sweep — the one of the three I/O defaults that is a
    value, and the only one measurable without a second build.

    Every check here is aimed at the same failure: a table that formats
    perfectly while every row measured the default."""

    def test_the_flag_carries_the_size_into_the_command(self):
        for size in measure.CHUNK_SIZES:
            script = measure._script(f"parse-chunk-{size}")
            self.assertIn(f"--chunk-size {size}", script)
            self.assertIn("parse --source /dump.sql", script)

    def test_a_size_the_figure_does_not_carry_is_refused(self):
        # The failure this stops: a typo'd row that still builds a command
        # line, runs, and publishes a reading nobody asked for.
        with self.assertRaises(ValueError):
            measure._script("parse-chunk-999")
        with self.assertRaises(ValueError):
            measure._script("parse-chunk-big")

    def test_the_default_row_is_the_shipped_default(self):
        # The table's ratios are against this row, and the doc's conclusion is
        # about the constant the library ships. A drift between the two would
        # publish a comparison against a value nothing runs.
        self.assertIn(measure.CHUNK_DEFAULT, measure.CHUNK_SIZES)
        source = (measure.REPO / "pgdump_query/src/scan.rs").read_text()
        self.assertIn(
            f"pub const DEFAULT_CHUNK_SIZE: usize = 1 << {measure.CHUNK_DEFAULT.bit_length() - 1};",
            source,
        )

    def test_the_sweep_brackets_the_read_path_pool_ceiling(self):
        # Above `io::POOL_MAX_BYTES` the buffer pool stops keeping the buffer,
        # so every chunk is allocated and zeroed afresh. The sweep has to hold
        # a row either side of it or the table cannot say so.
        source = (measure.REPO / "pgdump_query/src/io.rs").read_text()
        match = re.search(r"POOL_MAX_BYTES: usize = (\d+) << 20", source)
        assert match is not None, "the read path no longer names a pool ceiling"
        ceiling = int(match.group(1)) << 20
        self.assertIn(ceiling, measure.CHUNK_SIZES)
        self.assertTrue(any(s > ceiling for s in measure.CHUNK_SIZES))

    def test_every_row_is_the_same_binary_over_the_same_file(self):
        specs = measure._chunk_specs()
        self.assertEqual({s.binary for s in specs}, {"pgdq"})
        self.assertEqual({s.input for s in specs}, {"control"})
        self.assertEqual(
            len(specs), len(measure.CHUNK_SIZES) * len(measure.CHUNK_REGIMES)
        )

    def test_it_declares_an_input_on_each_of_the_three_devices(self):
        fig = measure.FIGURES_BY_ID["chunk-size"]
        self.assertEqual(fig.warm_inputs, ("control",))
        self.assertEqual(fig.cold_inputs, ("control",))
        self.assertEqual(fig.nvme_inputs, ("control",))
        self.assertEqual(sorted(fig.stage.split("+")), ["cold", "cold-nvme", "warm"])

    def test_a_size_is_spelled_in_whole_units(self):
        for size in measure.CHUNK_SIZES:
            self.assertRegex(measure.fmt_chunk(size), r"^\d+ (KiB|MiB)$")
        with self.assertRaises(ValueError):
            measure.fmt_chunk(1000)


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


class SubstreamAnnotation(unittest.TestCase):
    """The sub-stream count belongs to the two typed-`query` legs and no other.

    It is stated per cell rather than footnoted once, so a cell that carries it
    is making a claim about *that* leg. The count was once hand-applied to a
    pasted table and landed one column left of the leg it described — on
    `.xz`, `parse`, which plans no sub-streams at all — which is why the
    mapping is asserted here rather than read off the table by eye.
    """

    def test_only_typed_query_legs_are_annotated(self):
        annotated = {
            (inp, family)
            for inp, family, _ in measure.PARALLEL_LEGS
            if family == "query-typed"
        }
        self.assertEqual(annotated, {("control", "query-typed"), ("control_xz", "query-typed")})
        for inp, family, label in measure.PARALLEL_LEGS:
            if family != "query-typed":
                self.assertNotIn(
                    "typed `query`", label, f"{label} is not a typed-query leg but reads like one"
                )

    def test_every_annotated_leg_has_a_cap(self):
        for inp, family, _ in measure.PARALLEL_LEGS:
            if family == "query-typed":
                self.assertIn(inp, measure.QUERY_SUBSTREAM_CAP)

    def test_a_cap_is_never_above_the_largest_job_count(self):
        # A cap at or above the largest `--jobs` would annotate every row with
        # its own label and say nothing.
        for inp, cap in measure.QUERY_SUBSTREAM_CAP.items():
            self.assertLess(cap, measure.PARALLEL_JOBS[-1], inp)

    def test_the_xz_cap_is_below_the_plain_one(self):
        # A decoded block is larger than a chunk buffer, so the same budget
        # affords fewer `.xz` sub-streams. If this ever inverts, the arithmetic
        # in `QUERY_SUBSTREAM_CAP`'s comment has stopped describing the code.
        self.assertLess(
            measure.QUERY_SUBSTREAM_CAP["control_xz"], measure.QUERY_SUBSTREAM_CAP["control"]
        )


class Scaffolding(unittest.TestCase):
    """`tables.md` carries lines addressed to the session folding a table in.

    Pasting a whole section drags them into `measurements.md`, where they read
    as part of the table's own commentary. `--check` refuses that.
    """

    def test_a_clean_document_has_none(self):
        self.assertEqual(measure.scaffolding_in("# doc\n\nsome prose\n"), [])

    def test_a_pasted_fold_in_note_is_caught(self):
        found = measure.scaffolding_in(
            "# doc\n\n**The fold-in must also re-read**, because these repeat it: `x`.\n"
        )
        self.assertEqual(len(found), 1)
        self.assertIn("line 3", found[0])

    def test_the_document_carries_none(self):
        doc = (measure.REPO / "docs/design/measurements.md").read_text()
        self.assertEqual(measure.scaffolding_in(doc), [])

    def test_every_scaffolding_marker_is_something_emit_writes(self):
        # A marker that no longer matches what the harness emits would police
        # nothing, silently.
        source = inspect.getsource(measure)
        for marker in measure.SCAFFOLDING:
            self.assertIn(marker, source)


class Render(unittest.TestCase):
    """`--render` rebuilds a sitting's tables from its `raw.json`, measuring
    nothing — which is how a presentation-only renderer change is folded in."""

    def test_a_replay_session_refuses_to_measure(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = {"readings": {}, "input_sizes": {}, "runs": []}
            session = measure.ReplaySession(
                measure.Config(), raw, Path(tmp), lambda _m: None
            )
            spec = measure.RunSpec(
                binary="pgdq", input="control", command="parse", regime="warm", label="x"
            )
            with self.assertRaises(AssertionError):
                session.take(spec, 0)
            with self.assertRaises(AssertionError):
                session.drop_caches()
            # A sweep is a no-op rather than an error: a renderer calls it, and
            # the readings it would take are already loaded.
            self.assertIsNone(session.sweep("f", [spec], 5))

    def test_a_staged_input_is_a_sparse_file_of_the_recorded_size(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = {"readings": {}, "input_sizes": {"control": 4096}, "runs": []}
            session = measure.ReplaySession(
                measure.Config(), raw, Path(tmp), lambda _m: None
            )
            path = session.input_path("control", "warm")
            self.assertEqual(path.stat().st_size, 4096)
            self.assertEqual(
                measure.file_size(measure.Config(), path, "control"), 4096
            )

    def test_an_input_the_sitting_never_sized_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(
                measure.Config(), {"readings": {}, "input_sizes": {}, "runs": []},
                Path(tmp), lambda _m: None,
            )
            with self.assertRaises(KeyError):
                session.input_path("control", "warm")

    def test_a_sitting_without_the_recorded_fields_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "raw.json").write_text(json.dumps({"figures": [], "commit": "abc"}))
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = measure.render(measure.Config(), run_dir)
            self.assertEqual(rc, 2)
            self.assertIn("predates --render", err.getvalue())

    def test_a_missing_raw_json_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = measure.render(measure.Config(), Path(tmp))
            self.assertEqual(rc, 2)

    def test_a_sitting_records_what_a_render_needs(self):
        # The fields `render` reads must be the fields `emit` writes. Both
        # lists live in the source, so a field added to one and not the other
        # is caught here rather than at the next fold-in.
        source = inspect.getsource(measure.emit)
        for field in ("allocator", "whole_sweep", "header", "input_sizes", "rss", "reported"):
            self.assertIn(f'"{field}"', source, f"emit does not record {field}")


class SubstreamAnnotationLandsOnTheRightColumn(unittest.TestCase):
    """The renderer itself, over synthetic readings.

    The classes above assert the *mapping*; this one asserts the table. The
    defect this pins put the `.xz` sub-stream counts in the `.xz`, `parse`
    column — every number correct, attached to the wrong leg — which no
    assertion about `PARALLEL_LEGS` alone would have caught.
    """

    def _render(self):
        figure = "parallel-scan-throughput"
        specs = measure._parallel_specs()
        raw = {
            "readings": {s.key(figure): [1.0, 1.0, 1.0, 1.0, 1.0] for s in specs},
            "input_sizes": {"control": 3221227790, "control_xz": 591190020},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(
                measure.Config(), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = figure
            return measure.run_parallel_scan_throughput(session)

    def test_the_annotation_is_in_the_typed_query_columns_only(self):
        body = self._render()
        rows = [r for r in body.splitlines() if r.startswith("| ")]
        header = [c.strip() for c in rows[0].strip("|").split("|")]
        typed = {i for i, c in enumerate(header) if "typed `query`" in c}
        self.assertEqual(len(typed), 2, header)

        annotated_columns = set()
        for row in rows[2:]:  # skip header and the |---| separator
            cells = [c.strip() for c in row.strip("|").split("|")]
            for i, cell in enumerate(cells):
                if "sub-stream" in cell:
                    annotated_columns.add(i)
        self.assertEqual(
            annotated_columns,
            typed,
            "the sub-stream count must appear in the typed-`query` columns and no others",
        )

    def test_rows_at_or_below_four_carry_no_annotation(self):
        body = self._render()
        for row in body.splitlines():
            if row.startswith("| 1 ") or row.startswith("| 2 ") or row.startswith("| 4 "):
                self.assertNotIn("sub-stream", row)

    def test_an_annotated_cell_states_the_lesser_of_jobs_and_the_cap(self):
        body = self._render()
        header = None
        for row in body.splitlines():
            if not row.startswith("| "):
                continue
            cells = [c.strip() for c in row.strip("|").split("|")]
            if header is None:
                header = cells
                continue
            if cells[0].startswith("-") or not cells[0][0].isdigit():
                continue
            jobs = int(cells[0].split()[0])
            for i, cell in enumerate(cells):
                if "sub-stream" in cell:
                    inp = "control_xz" if "`.xz`" in header[i] else "control"
                    want = min(jobs, measure.QUERY_SUBSTREAM_CAP[inp])
                    self.assertIn(f"· {want} sub-stream", cell)
