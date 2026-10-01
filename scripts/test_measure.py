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
import dataclasses
import hashlib
import inspect
import io
import json
import re
import subprocess
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
        # The preamble table's first column has no header.
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
    #: `;` or an `&&`. Not a bare `time ` anywhere in the string — the perf
    #: table has a column named `v_time`, so a projection's flags carry that
    #: substring without timing anything.
    TIMER = re.compile(r"(?:\A|; |&& )time ")

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

    def unjoined_builder(self, script: str) -> bool | None:
        """Whether `script` runs a builder its timed command does not follow by
        `&&`; `None` where nothing ahead of the timer runs `pgdt`, `rm -f`
        being no builder."""
        timer = self.TIMER.search(script)
        ahead = script[: timer.start()]
        if "/pgdt " not in ahead:
            return None
        return "; " in ahead or not script[timer.start():].startswith("&& time ")

    def test_every_builder_joins_its_timed_command_by_and(self):
        # A `;` would run the timed command after a builder that failed, timing
        # whatever it does over no cache -- a cold map and its save, published
        # as a read over the cache.
        verdicts = {c: self.unjoined_builder(measure._script(c)) for c in measure.command_shapes()}
        self.assertEqual([c for c, unjoined in verdicts.items() if unjoined], [])
        self.assertIn(False, verdicts.values())

    def test_a_builder_joined_by_a_semicolon_is_caught(self):
        self.assertTrue(self.unjoined_builder(
            "/pgdt parse --source /dump.sql >/dev/null; time /pgdt info >/dev/null"
        ))
        self.assertTrue(self.unjoined_builder(
            "/pgdt parse >/dev/null; rm -f /x && time /pgdt info >/dev/null"
        ))
        self.assertIsNone(self.unjoined_builder("rm -f /x; time /pgdt parse >/dev/null"))

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

    `pgdt --jobs`' default has moved underneath the published figures without
    one command shape changing, and now depends on the source, which is the
    failure this reconciles against. That it agrees with `SWEEP_JOBS` on some
    source is not a reason to inherit it. `--stale` cannot see
    it either: staleness says *re-take*, never *the apparatus moved underneath
    you*."""

    def test_every_shape_states_a_worker_count(self):
        self.assertEqual(measure.worker_count_problems(), [])

    def test_a_shape_that_inherits_one_is_reported(self):
        # The check must fail loudly, since the shape it would pass still runs
        # and still produces a table. Every shape but the two declared
        # exemptions: `dd`, which is not a run of ours, and the flagless family,
        # whose reading is the count a flagless run resolves —
        # `flagless_flag_problems` is what holds *that* family to stating
        # nothing, so neither exemption leaves a shape unchecked.
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql"
        ):
            reported = measure.worker_count_problems()
        self.assertEqual(
            sorted(reported),
            sorted(
                c
                for c in measure.command_shapes()
                if c != "dd" and not c.startswith(measure.RESERVE_FLAGLESS)
            ),
        )

    def test_check_fails_on_a_shape_that_inherits_one(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql"
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
            named = getattr(measure, name)
            # A named prefix is one string or a tuple of them; `set()` over the
            # first would enumerate its *characters*, which resolves to a set of
            # single letters and fails this reconciliation on every one of them.
            prefixes |= {named} if isinstance(named, str) else set(named)
        shapes = set(measure.command_shapes())
        self.assertTrue(exact)
        self.assertTrue(prefixes)
        self.assertEqual(exact - shapes, set())
        for prefix in prefixes:
            with self.subTest(prefix=prefix):
                self.assertTrue([s for s in shapes if s.startswith(prefix)])

    def test_the_only_exempt_shape_runs_no_binary_of_ours(self):
        # `dd` is the device floor. Anything else claiming the exemption would
        # be a pgdt run measuring whatever the machine had.
        for command in measure._NO_WORKERS:
            with self.subTest(command=command):
                self.assertNotIn("/pgdt", measure._script(command))

    def test_the_decode_instrument_pins_its_own_spelling(self):
        # It is not `pgdt`, so it has no `--jobs`; `--workers` is the same
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


class StatisticsFlag(unittest.TestCase):
    """Every `pgdt parse` the harness runs gathers no statistics, but for the
    statistics figures' own (`StatisticsFigures`).

    `parse` gathers by default, which reads every value of every column, so a
    shape inheriting that default would re-time the figure it belongs to the
    day the default moved — the same failure `WorkerCount` reconciles against,
    for a second flag."""

    def test_every_parse_shape_states_none(self):
        self.assertEqual(measure.statistics_flag_problems(), [])

    def test_a_shape_that_gathers_is_reported(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 1"
        ):
            reported = measure.statistics_flag_problems()
        self.assertEqual(sorted(reported), sorted(measure.command_shapes()))

    def test_a_later_parse_in_the_same_script_is_checked_too(self):
        # A builder ahead of the timed command, as `info-cache-rss` has, is a
        # `parse` of its own.
        for join in ("&&", ";"):
            script = (
                f"/pgdt parse --source /dump.sql {measure.NO_STATISTICS} >/dev/null{join} "
                "time /pgdt parse --source /dump.sql >/dev/null"
            ).replace("null&&", "null &&")
            with self.subTest(join=join), unittest.mock.patch.object(
                measure, "_script", lambda c: script
            ):
                self.assertTrue(measure.statistics_flag_problems())

    def test_the_preamble_shapes_are_exempt_and_state_none(self):
        # `--preamble-only` reads no row, and the CLI refuses a statistics flag
        # beside it.
        for command in ("parse-preamble", "parse-preamble-rss"):
            with self.subTest(command=command):
                script = measure._script(command)
                self.assertIn("--preamble-only", script)
                self.assertNotIn("--statistics", script)

    def test_check_fails_on_a_shape_that_gathers(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 1"
        ):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = measure.cmd_check(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("with statistics gathered", out.getvalue())

    def test_the_argvs_outside_the_sweep_state_none_too(self):
        # Untimed or recorded rather than timed, and each one's reading is
        # read against a figure that states it.
        flag = measure.NO_STATISTICS.split()
        argvs = [
            measure.profile_argv("parse", "/dump.sql", "/tmp/x.dtcache"),
            *(measure.heaptrack_argv(shape, "/d", "/c") for shape, _ in measure.HEAPTRACK_AXIS),
        ]
        for argv in argvs:
            with self.subTest(argv=argv):
                at = argv.index(flag[0])
                self.assertEqual(argv[at : at + 2], flag)
        for function in (measure.Stager.profile, measure.count_saves):
            with self.subTest(function=function.__name__):
                self.assertIn("*NO_STATISTICS.split()", inspect.getsource(function))

    def test_the_koji_legs_state_none(self):
        # koji's check is that a parallel scan writes the serial cache, which a
        # gathering leg would answer with the serial scan compared to itself.
        for wrap in (False, True):
            recipe = measure.koji_recipe(measure.Config(), "pgdt-koji", wrap, 8)
            legs = [line for line in recipe.splitlines() if "/pgdt parse" in line]
            with self.subTest(wrap=wrap):
                self.assertTrue(legs)
                self.assertTrue(all(measure.NO_STATISTICS in leg for leg in legs), legs)


class StatisticsFigures(unittest.TestCase):
    """`statistics-gathering` and `statistics-pruning`, the rule above's one
    exemption: figures whose subject is the gathering.

    What these hold is that the exemption is stated rather than inherited, that
    the two legs of every row differ by the statistics flag alone — and the leg
    over a cache written without statistics by that cache alone — and that a
    pruning sitting whose pruned leg skipped nothing refuses rather than
    publishing a table saying pruning buys nothing."""

    #: What `pgdt query` printed over a 24 MiB `pruning` input, verbatim.
    QUERY_STDERR = (
        "2026-09-14T23:28:26.079040999Z  INFO scan started bytes=1 jobs=1 memory_bytes=67108864\n"
        "note: row-group statistics rule out 3060 of 3072 group(s), so 3208646636 of the "
        "3221227176 byte(s) of rows this table holds are not read\n"
        "note: reading stopped early in 1 block(s) sorted past the filter's bound, so a "
        "further 429055 byte(s) of rows are not read\n"
        "3000 row(s)\n"
    )

    def test_both_are_taken_and_borrow_nothing(self):
        # Standing in no sharing edge is what lets each publish from its own
        # commit, outside a sweep; taken, each is in `FIGURES`, where the
        # doc-side checks apply.
        for fid in ("statistics-gathering", "statistics-pruning"):
            with self.subTest(figure=fid):
                fig = measure.SELECTABLE_BY_ID[fid]
                self.assertIn(fig, measure.FIGURES)
                self.assertNotIn(fig, measure.UNTAKEN)
                self.assertEqual(fig.shares, ())
                self.assertEqual(measure.entangled_with(fid), [])

    def test_the_gathering_figure_is_the_scan_throughput_inputs_and_the_arrays_file(self):
        # The throughput inputs, whole: two of them hold no `COPY` row, and that
        # is a row of the table rather than a reason to drop it. The arrays file
        # beside them is where the census splits every row, so its row and the
        # control's bracket the census the data level prices.
        fig = measure.SELECTABLE_BY_ID["statistics-gathering"]
        warm = measure.FIGURES_BY_ID["scan-throughput-warm"].warm_inputs
        self.assertEqual(set(fig.warm_inputs), set(warm) | {"arrays"})
        self.assertEqual(fig.warm_inputs[:2], ("control", "arrays"))
        self.assertEqual(fig.memory, measure.STATISTICS_MEMORY)
        self.assertNotEqual(measure.STATISTICS_MEMORY, measure.Config().memory)

    def test_the_group_size_stated_is_the_librarys_default(self):
        src = (measure.REPO / "pgdump_query/src/statistics.rs").read_text()
        self.assertEqual(measure.ROW_GROUP_SIZE, 1 << 20)
        self.assertIn("pub const ROW_GROUP_DEFAULT_SIZE_BYTES: u64 = 1 << 20;", src)

    def test_a_gathering_legs_parse_states_its_request(self):
        for leg, flags in measure.STATISTICS_LEGS:
            with self.subTest(leg=leg):
                script = measure._script(f"{measure.STATISTICS_FAMILY}{leg}-rss")
                self.assertIn(flags, script)
                self.assertIn(measure.rss_wrapper(measure.platform.machine()), script)

    def test_the_two_gathering_legs_differ_by_the_level_alone(self):
        (metadata, metadata_flags), (data, data_flags) = measure.STATISTICS_LEGS
        self.assertEqual((metadata, data), ("metadata", "data"))
        self.assertEqual(metadata_flags, measure.NO_STATISTICS)
        self.assertEqual(data_flags, measure.GATHER_STATISTICS)
        a = measure._script(f"{measure.STATISTICS_FAMILY}{metadata}-rss")
        b = measure._script(f"{measure.STATISTICS_FAMILY}{data}-rss")
        self.assertEqual(a.replace(metadata_flags, data_flags), b)

    def test_the_metadata_leg_is_the_register_parse_in_the_figures_container(self):
        # Its own leg rather than `peak-rss`'s borrowed, for the container; the
        # argv is the same, so the two cannot drift apart unseen.
        metadata = measure._script(f"{measure.STATISTICS_FAMILY}metadata-rss")
        self.assertEqual(metadata, measure._script("parse-rss"))

    def test_the_exemption_admits_the_gathering_request_and_nothing_else(self):
        # Outside the declared families a `parse` stating `GATHER_STATISTICS`
        # is still one that gathers, and is reported.
        script = f"time /pgdt parse --source /dump.sql {measure.GATHER_STATISTICS} --jobs 1"
        with unittest.mock.patch.object(measure, "_script", lambda c: script):
            reported = measure.statistics_flag_problems()
        families = measure.GATHERING_FAMILIES
        self.assertEqual(
            set(families),
            {
                measure.STATISTICS_FAMILY,
                measure.PRUNING_FAMILY,
                measure.DYNFILTER_FAMILY,
                f"{measure.PARALLEL_SCAN}-jobs-",
                *measure.DATA_LEVEL_QUERIES,
            },
        )
        self.assertEqual(
            sorted(reported),
            sorted(c for c in measure.command_shapes() if not c.startswith(families)),
        )

    def test_the_pruning_builder_is_untimed_and_states_the_request(self):
        for name in measure.PRUNING_FILTERS:
            for leg in measure.PRUNING_LEGS:
                with self.subTest(filter=name, leg=leg):
                    script = measure._script(f"{measure.PRUNING_FAMILY}{name}-{leg}")
                    builder, _, timed = script.partition(" && ")
                    self.assertIn("/pgdt parse", builder)
                    self.assertIn(measure.GATHER_STATISTICS, builder)
                    self.assertNotIn("time ", builder)
                    self.assertTrue(timed.startswith("time /pgdt query"))
                    self.assertEqual(script.count("time "), 1)
                    self.assertIn(f"--statistics {leg} ", timed)

    def test_no_shape_queries_a_cache_the_metadata_level_wrote(self):
        # Such a query re-reads its table for the census inside the timer, so
        # it times the census rather than what its figure names.
        for command in measure.command_shapes():
            script = measure._script(command)
            with self.subTest(command=command):
                self.assertFalse(
                    any(measure.NO_STATISTICS in run for run in measure._PARSE_RUN.findall(script))
                    and "/pgdt query" in script,
                    script,
                )

    def test_the_two_pruning_legs_differ_by_the_flag_alone(self):
        for name in measure.PRUNING_FILTERS:
            with self.subTest(filter=name):
                a = measure._script(f"{measure.PRUNING_FAMILY}{name}-none")
                b = measure._script(f"{measure.PRUNING_FAMILY}{name}-all")
                self.assertEqual(a.replace("--statistics none", "--statistics all"), b)

    def test_the_filters_name_the_generators_columns(self):
        import generate_pruning_bench as gen

        range_expr = measure.PRUNING_FILTERS["range"][0]
        self.assertTrue(range_expr.startswith(f"{measure._PERF_SCALARS[0]} "))
        column, literal = measure.PRUNING_FILTERS["dictionary"][0].split("=")
        self.assertEqual(column, gen.COLUMN[0])
        self.assertIn(literal, [gen.label(i) for i in range(gen.LABELS)])

    def test_the_unnarrowed_filter_is_a_midrange_equality_on_a_uniform_smallint(self):
        # What makes its statistics unable to narrow it is the draw: a
        # `smallint` over its whole range, per row (`random_row`), with `0` in
        # the middle of it. What `parse` keeps for the column at the figure's
        # group size, and that the query skips no group, is
        # `perf_generator_fidelity.rs`'s, which states the same filter.
        import generate_perf_data as perf

        expr = measure.PRUNING_FILTERS[measure.PRUNING_UNNARROWED][0]
        column, literal = expr.split("=")
        self.assertEqual(dict(perf.COLUMNS)[column], "smallint")
        self.assertEqual(int(literal), 0)
        fidelity = (measure.REPO / "pgdt/tests/perf_generator_fidelity.rs").read_text()
        self.assertIn(f'"{expr}"', fidelity)
        self.assertIn(str(measure.ROW_GROUP_SIZE), fidelity)

    def test_the_query_notes_are_read_as_the_cli_prints_them(self):
        got = measure.parse_query_notes(self.QUERY_STDERR)
        self.assertEqual(
            got,
            {
                "skipped_groups": "3060",
                "groups": "3072",
                "skipped_bytes": "3208646636",
                "bytes": "3221227176",
                "stopped_blocks": "1",
                "unread_bytes": "429055",
                "rows_returned": "3000",
            },
        )
        self.assertEqual(
            measure.parse_query_notes("no rows found for public.perf in /dump.sql\n"),
            {"rows_returned": "0"},
        )
        self.assertEqual(measure.parse_query_notes("real 0m1.0s\n"), {})

    def test_the_phrases_read_are_the_librarys_and_the_clis(self):
        # The regexes key on wording two crates print; a rewording there would
        # leave every leg reading as one that skipped nothing, which the
        # renderer refuses — loudly, but a sitting late.
        stream = (measure.REPO / "pgdump_query/src/stream.rs").read_text()
        cli = (measure.REPO / "pgdt/src/main.rs").read_text()
        self.assertIn(
            '"row-group statistics rule out {skipped_groups} of {groups} group(s), so \\', stream
        )
        self.assertIn("note: reading stopped early in {} block(s)", cli)
        self.assertIn('eprintln!("{rows} row(s)");', cli)
        self.assertIn('eprintln!("no rows found for {table} in {origin}");', cli)

    def _reported(self, **override):
        good = {
            "range-none": {"rows_returned": "3000"},
            "range-all": {"rows_returned": "3000", "skipped_groups": "12", "groups": "20"},
            "dictionary-none": {"rows_returned": "3000"},
            "dictionary-all": {"rows_returned": "3000", "skipped_groups": "5", "groups": "20"},
            "unnarrowed-none": {"rows_returned": "12"},
            "unnarrowed-all": {"rows_returned": "12", "skipped_groups": "0", "groups": "20"},
        }
        good.update(override)
        return good

    def _refused(self, **override):
        """The one refusal `override` earns, by the filter it names — so a
        test passes on the refusal it was written for rather than on any."""
        problems = measure.pruning_problems(self._reported(**override))
        self.assertEqual(len(problems), 1, problems)
        return problems[0]

    def test_a_sitting_that_prunes_passes(self):
        self.assertEqual(measure.pruning_problems(self._reported()), [])

    def test_a_pruned_leg_that_skipped_nothing_is_refused(self):
        got = self._refused(**{"range-all": {"rows_returned": "3000"}})
        self.assertTrue(got.startswith("range: "), got)
        got = self._refused(
            **{"dictionary-all": {"rows_returned": "3000", "skipped_groups": "0", "groups": "20"}}
        )
        self.assertTrue(got.startswith("dictionary: "), got)

    def test_an_unpruned_leg_that_skipped_is_refused(self):
        got = self._refused(**{"dictionary-none": {"rows_returned": "3000", "skipped_groups": "1"}})
        self.assertTrue(got.startswith("dictionary: "), got)
        got = self._refused(**{"unnarrowed-none": {"rows_returned": "12", "skipped_groups": "0"}})
        self.assertTrue(got.startswith("unnarrowed: "), got)

    def test_legs_returning_different_rows_are_refused(self):
        got = self._refused(**{"range-none": {"rows_returned": "2999"}})
        self.assertTrue(got.startswith("range: "), got)
        got = self._refused(**{"range-none": {}})
        self.assertTrue(got.startswith("range: "), got)

    def test_the_unnarrowed_leg_is_the_one_admitted_to_skip_nothing(self):
        self.assertIn(measure.PRUNING_UNNARROWED, measure.PRUNING_FILTERS)
        # Not having consulted statistics at all is not skipping nothing: the
        # note is printed only where they were consulted.
        got = self._refused(**{"unnarrowed-all": {"rows_returned": "12"}})
        self.assertIn("consulting no statistics", got)

    def test_an_unnarrowed_leg_that_skipped_or_stopped_is_refused(self):
        skipped = {"rows_returned": "12", "skipped_groups": "1", "groups": "20"}
        got = self._refused(**{"unnarrowed-all": skipped})
        self.assertTrue(got.startswith("unnarrowed: "), got)
        stopped = {
            "rows_returned": "12",
            "skipped_groups": "0",
            "groups": "20",
            "unread_bytes": "4096",
        }
        got = self._refused(**{"unnarrowed-all": stopped})
        self.assertTrue(got.startswith("unnarrowed: "), got)


class DataLevelQueries(unittest.TestCase):
    """The query figures time a `pgdt query` over a data-level cache one
    untimed `parse` wrote in the same container, with `--statistics none`
    (`measure.DATA_LEVEL_QUERIES`), so neither a mapping pass nor pruning
    enters the reading."""

    #: The figures whose rows are those shapes, `allocator`'s query rows
    #: included, and `parallel-scan-throughput`, whose provider legs read the
    #: cache the same builder request writes (`ParallelScanThroughputProvider`).
    FIGURES = (
        "nested-end-to-end",
        "cross-file-floor",
        "projection-widths",
        "predicate-terms",
        "allocator",
        "parallel-scan-throughput",
    )

    def shapes(self) -> list[str]:
        return [
            c for c in measure.command_shapes() if c.startswith(measure.DATA_LEVEL_QUERIES)
        ]

    def test_every_one_queries_the_cache_its_builder_wrote_ahead_of_the_timer(self):
        for command in self.shapes():
            script = measure._script(command)
            with self.subTest(command=command):
                self.assertTrue(script.startswith(measure.DATA_LEVEL_BUILDER), script)
                builder, _, timed = script.partition(" && ")
                self.assertNotIn("time ", builder)
                self.assertIn(measure.GATHER_STATISTICS, builder)
                self.assertIn(f"--jobs {measure.SWEEP_JOBS} ", builder)
                self.assertTrue(timed.startswith("time /pgdt query "), timed)
                self.assertIn(measure.DATA_LEVEL_QUERY, timed)
                self.assertNotIn("--dtcache none", script)

    def test_every_query_figure_row_is_one_of_them(self):
        specs = [
            *measure._nested_specs(),
            *(measure.RunSpec("pgdt", "arrays", f"query-project-{w}", "warm", "")
              for w in measure.PROJECTION_WIDTHS),
            *(measure.RunSpec("pgdt", "control", f"query-where-{s}", "warm", "")
              for s in measure.PREDICATE_SHAPES),
            *(measure.RunSpec("pgdt", "control", c, "warm", "")
              for c, _, _ in measure._ALLOCATOR_SHAPES if c.startswith("query")),
        ]
        for spec in specs:
            with self.subTest(command=spec.command):
                self.assertIn(spec.command, self.shapes())

    def test_only_the_map_shapes_keep_no_cache(self):
        # `query-nomatch*` times the map and its saves, which a built cache
        # would leave nothing of.
        uncached = {
            c for c in measure.command_shapes() if "--dtcache none" in measure._script(c)
        }
        self.assertEqual(uncached, {"query-nomatch", "query-nomatch-rss"})

    def test_every_query_figure_declares_what_its_cache_holds(self):
        for fid in self.FIGURES:
            with self.subTest(figure=fid):
                depends = measure.SELECTABLE_BY_ID[fid].depends
                self.assertLessEqual(set(measure.CACHED_QUERY), set(depends))
                self.assertEqual(len(depends), len(set(depends)))


class DynamicFilterFigures(unittest.TestCase):
    """`dynamic-filter-join` and `dynamic-filter-topk`: the register's first
    figures timing `datafusion-cli-pgdump`, the second timed program.

    Each way a leg can depart from what its table says is held here, because
    each produces a plausible table of something else: a run inheriting
    DataFusion's `target_partitions` while the untimed builder's `--jobs 1`
    satisfies the worker-count check, two legs differing by more than the
    producer's flag, an image or allocator the prose does not name, and legs
    answering differently under the timer."""

    SHAPES = measure.dynfilter_shapes()

    def test_both_are_taken_and_borrow_nothing(self):
        for fid in ("dynamic-filter-join", "dynamic-filter-topk"):
            with self.subTest(figure=fid):
                fig = measure.SELECTABLE_BY_ID[fid]
                self.assertIn(fig, measure.FIGURES)
                self.assertEqual(fig.shares, ())
                self.assertEqual(measure.entangled_with(fid), [])
                self.assertEqual(fig.warm_inputs, ("dynfilter",))
                self.assertIsNone(fig.memory)

    def test_a_join_reports_the_rows_it_matched(self):
        # The table's last column is `result_first`, the answer's first cell;
        # `count(p.v_text)` there would drop the matched rows whose payload is
        # NULL and report fewer than the join matched.
        for name, (sql, _) in measure.DYNFILTER_QUERIES["join"].items():
            with self.subTest(query=name):
                self.assertTrue(sql.startswith("SELECT count(*), count(p.v_text) FROM "))

    def test_every_leg_runs_the_second_program_under_its_producers_flag(self):
        for figure, flag in measure.DYNFILTER_FLAGS.items():
            for command in measure.dynfilter_shapes(figure):
                with self.subTest(command=command):
                    script = measure._script(command)
                    leg = command.rpartition("-")[2]
                    self.assertIn(f"{flag}={measure.DYNFILTER_LEGS[leg]} {measure.DFCLI} ", script)
                    self.assertEqual(script.count("time "), 1)
                    self.assertEqual(script.count(f"{measure.DFCLI} "), 1)
                    for other in set(measure.DYNFILTER_FLAGS.values()) - {flag}:
                        self.assertNotIn(other, script)

    def test_each_leg_of_a_row_differs_from_the_next_by_one_lever(self):
        # Off to on is the producer's flag alone; on to rows the setting's
        # `SET` alone, run before the query so the answer read back is the
        # query's. A third difference would price something no column names.
        self.assertEqual(list(measure.DYNFILTER_LEGS), ["off", "on", measure.DYNFILTER_ROWS_LEG])
        for figure, queries in measure.DYNFILTER_QUERIES.items():
            flag = measure.DYNFILTER_FLAGS[figure]
            for name in queries:
                with self.subTest(figure=figure, query=name):
                    off, on, rows = (
                        measure._script(f"{measure.DYNFILTER_FAMILY}{figure}-{name}-{leg}")
                        for leg in measure.DYNFILTER_LEGS
                    )
                    self.assertEqual(off.replace(f"{flag}=false", f"{flag}=true"), on)
                    self.assertNotIn(measure.DYNFILTER_ROWS_SQL, on)
                    stated = f"-q -c '{measure.DYNFILTER_ROWS_SQL}' -c "
                    self.assertEqual(rows.replace(stated, "-q -c "), on)

    def test_the_rows_leg_states_a_setting_the_provider_takes(self):
        # `SET` of a key the provider does not register fails the run rather
        # than timing the default under the rows leg's name; held against the
        # provider's own source, as the allocator is below.
        settings = (measure.REPO / "datafusion-pgdump/src/settings.rs").read_text()
        key, _, value = measure.DYNFILTER_ROWS_SQL.removeprefix("SET ").partition(" = ")
        self.assertEqual(key.partition(".")[0], "pgdump")
        self.assertIn(f'"{key.partition(".")[2]}" => {{', settings)
        self.assertEqual(value, "true")

    def test_the_builder_is_untimed_and_writes_where_dump_looks(self):
        # `--dump` reads the cache beside the dump, and `datafusion-cli-pgdump`
        # takes no cache path; the builder gathers what the scan prunes by.
        for command in self.SHAPES:
            with self.subTest(command=command):
                builder, _, timed = measure._script(command).partition(" && ")
                self.assertTrue(builder.startswith("/pgdt parse --source /dump.sql "))
                self.assertIn("--dtcache /dump.sql.dtcache ", builder)
                self.assertIn(measure.GATHER_STATISTICS, builder)
                self.assertTrue(timed.startswith("time "))
                self.assertIn(f"--dump {measure.DFCLI_CATALOG}=/dump.sql ", timed)

    def test_every_run_of_the_second_program_states_its_partitions(self):
        for command in self.SHAPES:
            with self.subTest(command=command):
                self.assertEqual(
                    measure._dfcli_partitions(measure._script(command)),
                    {str(measure.SWEEP_JOBS)},
                )
        self.assertEqual(measure.worker_count_problems(), [])
        self.assertEqual(measure.pinned_count_problems(), [])

    def test_a_run_inheriting_its_partitions_is_reported_though_the_builder_states_jobs(self):
        script = (
            "/pgdt parse --source /dump.sql --jobs 1 >/dev/null && "
            f"time {measure.DFCLI} --dump b=/dump.sql -c 'SELECT 1'"
        )
        with unittest.mock.patch.object(measure, "_script", lambda c: script):
            self.assertEqual(sorted(measure.worker_count_problems()), sorted(
                c for c in measure.command_shapes()
                if c != "dd" and not c.startswith(measure.RESERVE_FLAGLESS)
            ))
        stated = script.replace("time ", f"time {measure.DFCLI_PARTITIONS}=4 ")
        with unittest.mock.patch.object(measure, "_script", lambda c: stated):
            got = measure.pinned_count_problems()
        self.assertTrue(any(f"{measure.DFCLI_PARTITIONS}=4" in line for line in got), got)

    def test_the_sql_survives_the_shells_quoting(self):
        # Each statement is single-quoted into `bash -c`.
        statements = [sql for q in measure.DYNFILTER_QUERIES.values() for sql, _ in q.values()]
        for sql in [*statements, measure.DYNFILTER_ROWS_SQL, measure.STARTUP_SQL]:
            with self.subTest(sql=sql):
                self.assertNotIn("'", sql)

    def test_the_queries_name_the_generators_tables_and_columns(self):
        import generate_dynamic_filter_bench as gen

        tables = {t.removeprefix("public.") for t, _ in gen.BUILD_TABLES}
        columns = {name for name, _ in (*measure.perf.COLUMNS, *gen.COLUMNS)}
        joins = measure.DYNFILTER_QUERIES["join"]
        for name, (sql, _) in joins.items():
            with self.subTest(query=name):
                table = sql.split(f"JOIN {measure.DFCLI_CATALOG}.public.")[1].split(" ")[0]
                probe_key = sql.rsplit(" = ", 1)[0].rsplit("p.", 1)[1]
                self.assertIn(table, tables)
                self.assertIn(probe_key, columns)
        topk = measure.DYNFILTER_QUERIES["topk"]["unsorted"][0]
        self.assertIn(f"ORDER BY p.{gen.COLUMNS[0][0]} ", topk)

    def test_the_second_program_and_its_image_are_what_the_table_says(self):
        # The table names `mimalloc` and the image; both are facts about the
        # binary and the config, held here rather than trusted.
        main = (measure.REPO / "datafusion-cli-pgdump/src/main.rs").read_text()
        self.assertIn("static GLOBAL: MiMalloc = MiMalloc;", main)
        self.assertNotEqual(measure.Config().dfcli_image, measure.Config().image)
        self.assertEqual(measure.DFCLI_RELEASE_BIN.name, measure.DFCLI.lstrip("/"))

    def test_a_dry_run_builds_nothing_and_names_the_binary(self):
        said = []
        with unittest.mock.patch.object(measure, "_DFCLI_BUILT", False), \
                unittest.mock.patch.object(measure, "run") as ran:
            got = measure.ensure_dfcli_binary(measure.Config(dry_run=True), said.append)
        self.assertEqual(got, measure.DFCLI_RELEASE_BIN)
        ran.assert_not_called()
        self.assertTrue(any("would build" in line for line in said), said)

    def _reported(self, figure, **override):
        good = {
            f"{name}-{leg}": {"result_rows": "1", "result_digest": f"h-{name}"}
            for name in measure.DYNFILTER_QUERIES[figure]
            for leg in measure.DYNFILTER_LEGS
        }
        good.update(override)
        return good

    def test_legs_answering_alike_pass(self):
        for figure in measure.DYNFILTER_QUERIES:
            with self.subTest(figure=figure):
                self.assertEqual(measure.dynfilter_problems(figure, self._reported(figure)), [])

    def test_legs_answering_differently_are_refused(self):
        got = measure.dynfilter_problems(
            "join", self._reported("join", **{"costing-on": {"result_rows": "1", "result_digest": "x"}})
        )
        self.assertEqual(len(got), 1, got)
        self.assertTrue(got[0].startswith("costing: "), got)
        got = measure.dynfilter_problems("topk", self._reported("topk", **{"unsorted-off": {}}))
        self.assertEqual(len(got), 1, got)
        # The rows leg is held to the answer too, not only the two the flag
        # separates.
        got = measure.dynfilter_problems(
            "topk",
            self._reported("topk", **{"unsorted-rows": {"result_rows": "1", "result_digest": "x"}}),
        )
        self.assertEqual(len(got), 1, got)

    def test_an_empty_answer_is_refused(self):
        empty = {"result_rows": "0", "result_digest": "e"}
        got = measure.dynfilter_problems(
            "join",
            self._reported("join", **{f"clustered-{leg}": empty for leg in measure.DYNFILTER_LEGS}),
        )
        self.assertEqual(got, ["clustered: the query returned no row"])

    def test_the_answer_is_read_off_the_legs_own_lines(self):
        got = measure.parse_reported(
            "result_rows=1\nresult_first=100\nresult_digest=" + "a" * 64 + "\n"
        )
        self.assertEqual(got, {"result_rows": "1", "result_first": "100", "result_digest": "a" * 64})


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
        reference = [s for s in specs if s.command == "parse" and s.binary == "pgdt"]
        self.assertEqual(len(reference), 1)

    def test_every_other_leg_is_its_own_build(self):
        specs = measure._allocator_specs("system")
        binaries = {s.binary for s in specs if s.command == "parse"}
        self.assertEqual(binaries, {"pgdt", "alloc:jemalloc", "alloc:mimalloc"})

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
                self.assertIn("pgdt", shipped)
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
                self.assertIn("time /pgdt", measure._script(command))

    def test_each_shape_names_a_figure_that_takes_its_reference_reading(self):
        # The borrow is what keeps one number in the doc per measurement, and
        # it silently does nothing if the source figure never takes that spec.
        order = [f.id for f in measure.FIGURES]
        for command, _, source in measure._ALLOCATOR_SHAPES:
            with self.subTest(command=command):
                self.assertIn(source, measure.FIGURES_BY_ID)
                self.assertLess(order.index(source), order.index("allocator"))
        throughput = measure._throughput_specs("warm")
        self.assertIn(
            measure.RunSpec("pgdt", "control", "parse", "warm", "").key("scan-throughput-warm"),
            {s.key("scan-throughput-warm") for s in throughput},
        )
        nested = measure._nested_specs()
        for command in ("query-strings", "query-typed"):
            with self.subTest(command=command):
                self.assertIn(
                    measure.RunSpec("pgdt", "control", command, "warm", "").key(
                        "nested-end-to-end"
                    ),
                    {s.key("nested-end-to-end") for s in nested},
                )

    def test_a_version_string_yields_its_allocator(self):
        with unittest.mock.patch.object(
            measure, "run", return_value="pgdt 0.1.0 (allocator: mimalloc)\n"
        ):
            self.assertEqual(measure.binary_allocator(Path("/pgdt")), "mimalloc")

    def test_a_binary_that_names_no_allocator_is_an_error(self):
        # Not a default: a binary too old to report it would otherwise be
        # published as the reference leg under a name nothing checked.
        with unittest.mock.patch.object(measure, "run", return_value="pgdt 0.1.0\n"):
            with self.assertRaises(RuntimeError):
                measure.binary_allocator(Path("/pgdt"))

    def test_an_instrumented_build_is_refused_rather_than_timed(self):
        # The failure this prevents is silent: a counting `#[global_allocator]`
        # still answers `(allocator: system)`, so without the refusal an
        # introspection build reads as the shipped binary and every reading
        # taken on it is published as a figure of something else.
        with unittest.mock.patch.object(
            measure,
            "run",
            return_value="pgdt 0.1.0 (allocator: system) (instrument: counting-allocator)\n",
        ):
            with self.assertRaises(RuntimeError) as raised:
                measure.binary_allocator(Path("/pgdt"))
        self.assertIn("counting-allocator", str(raised.exception))

    def test_an_unknown_leg_is_never_built(self):
        with self.assertRaises(ValueError):
            measure.ensure_allocator_binary(measure.Config(), "tcmalloc", lambda _: None)

    def test_a_leg_builds_with_no_default_features_and_its_own_target_dir(self):
        # Both flags are load-bearing. Without `--no-default-features` the
        # reference leg stops being the platform allocator the day one is
        # adopted, so the figure stops being re-takeable at the moment it
        # matters; without its own `--target-dir` a `--features` build
        # overwrites `target/release/pgdt` and every other figure in the same
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
                (built / "pgdt").write_text("#!/bin/true\n")
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
            self.assertEqual(out, cfg.out_dir / "pgdt-alloc-jemalloc")

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
                (built / "pgdt").write_text("#!/bin/true\n")
                return ""

            with unittest.mock.patch.object(measure, "run", fake_run), \
                 unittest.mock.patch.object(
                     measure, "binary_allocator", return_value="system"
                 ):
                with self.assertRaises(RuntimeError):
                    measure.ensure_allocator_binary(cfg, "mimalloc", lambda _: None)
            self.assertFalse((cfg.out_dir / "pgdt-alloc-mimalloc").exists())

    def test_a_leg_left_over_from_an_earlier_session_is_rebuilt(self):
        # The failure this stops is silent and total: `runs/pgdt-alloc-<leg>`
        # survives between sessions, so short-circuiting on its existence times
        # a leg built from last week's source against a reference built from
        # today's, and the leg still answers `--version` with its own allocator
        # name, so nothing downstream notices.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )
            stale = cfg.out_dir / "pgdt-alloc-jemalloc"
            stale.parent.mkdir(parents=True, exist_ok=True)
            stale.write_text("last session's binary\n")
            calls = []

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                calls.append(list(argv))
                built = cfg.alloc_build_root / "jemalloc" / "release"
                built.mkdir(parents=True, exist_ok=True)
                (built / "pgdt").write_text("#!/bin/true\n")
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
            measure.Config().out_dir / "pgdt-alloc-jemalloc",
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


class Glibc(unittest.TestCase):
    """Which glibc a figure ran under: both images pinned by digest, each place
    a figure's program runs asked for its glibc, and the answer named in the
    stamp or in the figure's own marker (`measurements.md`, "The apparatus")."""

    DOC = measure.REPO / "docs/design/measurements.md"
    PIN = re.compile(r"[^@\s]+:[^@\s]+@sha256:[0-9a-f]{64}")

    @unittest.skipIf(
        {"PGDT_MEASURE_IMAGE", "PGDT_MEASURE_DFCLI_IMAGE"} & set(measure.os.environ),
        "an image is overridden in this environment",
    )
    def test_both_images_are_pinned_by_digest(self):
        # A tag moves under the register; a digest is the image a figure ran in.
        cfg = measure.Config()
        self.assertRegex(cfg.image, self.PIN)
        self.assertRegex(cfg.dfcli_image, self.PIN)

    def test_prose_names_an_image_by_its_tag(self):
        self.assertEqual(measure.image_name("postgres:16@sha256:" + "0" * 64), "postgres:16")
        self.assertEqual(measure.image_name("archlinux:base"), "archlinux:base")

    def test_an_image_is_asked_in_a_container_and_the_host_is_asked_directly(self):
        cfg = measure.Config()
        asked = []

        def fake(argv, **_kwargs):
            asked.append(list(argv))
            return "glibc 2.41\n"

        with unittest.mock.patch.object(measure, "run", fake):
            self.assertEqual(measure.glibc_of(cfg, "some:image@sha256:ab"), "2.41")
            self.assertEqual(measure.glibc_of(cfg, measure.HOST), "2.41")
        self.assertEqual(
            asked[0],
            [*cfg.container_argv(), "run", "--rm", "some:image@sha256:ab",
             "getconf", "GNU_LIBC_VERSION"],
        )
        self.assertEqual(asked[1], ["getconf", "GNU_LIBC_VERSION"])

    def test_a_place_that_is_no_glibc_is_refused(self):
        # musl's `getconf` has no GNU_LIBC_VERSION and exits non-zero.
        failed = subprocess.CalledProcessError(1, ["getconf"])
        with unittest.mock.patch.object(measure, "run", side_effect=failed):
            with self.assertRaises(RuntimeError) as raised:
                measure.glibc_of(measure.Config(), "alpine:3")
        self.assertIn("alpine:3", str(raised.exception))
        with unittest.mock.patch.object(measure, "run", return_value="musl\n"):
            with self.assertRaises(RuntimeError):
                measure.glibc_of(measure.Config(), "alpine:3")

    def test_the_stamp_names_the_glibc_and_still_reads_back(self):
        stamp = measure.session_stamp("deadbee", dirty=True, allocator="system", glibc="2.41")
        self.assertIn("under the `system` allocator and glibc 2.41.", stamp)
        self.assertEqual(measure.stamp_in(stamp), "deadbee")
        self.assertEqual(measure.stamp_glibc(stamp), "2.41")
        self.assertIsNone(measure.stamp_glibc(measure.session_stamp("deadbee", dirty=False)))
        self.assertIn(", under glibc 2.41", measure.taken_against(False, None, "2.41"))

    def test_a_partial_sitting_says_its_glibc_too(self):
        lead = measure.partial_lead(1, "deadbee", False, "system", None, "2.41")
        self.assertIn("glibc 2.41", lead)

    def test_a_marker_names_its_glibc_beside_its_sitting_and_both_read_back(self):
        marker = measure.figure_marker("map-only", "7ee5db5", glibc="2.44")
        self.assertEqual(measure.figure_sittings(marker), {"map-only": "7ee5db5"})
        self.assertEqual(measure.marker_glibcs(marker), {"map-only": "2.44"})
        alone = measure.figure_marker("nested-decode-micro", glibc="2.41 and 2.44")
        self.assertEqual(measure.figure_sittings(alone), {})
        self.assertEqual(measure.marker_glibcs(alone), {"nested-decode-micro": "2.41 and 2.44"})

    def test_a_marker_names_its_glibc_only_where_the_stamp_does_not_speak_for_it(self):
        # A figure of the sweep run in the register's image: the stamp speaks.
        self.assertIsNone(measure.marker_glibc(True, "2.41", "2.41"))
        # Run elsewhere — the second program's image, or the host.
        self.assertEqual(measure.marker_glibc(True, "2.41", "2.44"), "2.44")
        # A sitting of its own names its glibc beside its commit, whatever it is.
        self.assertEqual(measure.marker_glibc(False, "2.41", "2.41"), "2.41")
        # Nothing asked, nothing named: `--dry-run`, or a sitting older than this.
        self.assertIsNone(measure.marker_glibc(False, "2.41", None))

    def test_several_places_are_named_together(self):
        glibcs = {"a": "2.44", "b": "2.41", "c": "2.41"}
        self.assertEqual(measure.glibc_named(["a", "b", "c"], glibcs), "2.41 and 2.44")
        self.assertIsNone(measure.glibc_named(["a"], {}))

    def test_a_sweep_records_where_the_program_ran_and_not_the_floor(self):
        cfg = measure.Config(dry_run=True)
        session = measure.Session(cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        leg = measure.RunSpec("dfcli", "dynfilter", "dfcli-dynamic-filter-topk-unsorted-on",
                              "warm", "")
        floor = measure.RunSpec("none", "dynfilter", "dd", "warm", "dd floor")
        session.sweep("dynamic-filter-topk", [leg, floor], 1)
        self.assertEqual(session.places["dynamic-filter-topk"], {cfg.dfcli_image})
        # `--dry-run` asks nothing: it must not need root or a runtime.
        self.assertEqual(session.glibcs, {})
        measure.run_nested_decode_micro(session)
        self.assertEqual(session.places["nested-decode-micro"], {measure.HOST})

    def test_a_place_is_asked_once_and_before_its_first_run(self):
        cfg = measure.Config()
        session = measure.Session(cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None)
        with unittest.mock.patch.object(measure, "glibc_of", return_value="2.41") as asked:
            session.ran_in("f", cfg.image)
            session.ran_in("g", cfg.image)
        asked.assert_called_once_with(cfg, cfg.image)
        self.assertEqual(session.places, {"f": {cfg.image}, "g": {cfg.image}})

    def test_a_render_names_what_the_sitting_recorded_and_asks_nothing(self):
        render = inspect.getsource(measure.render)
        self.assertIn('raw.get("figure_glibc"', render)
        self.assertIn("marker_glibc(", render)
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(
                measure.Config(), {"readings": {}, "runs": []}, Path(tmp), lambda _m: None
            )
            with unittest.mock.patch.object(measure, "glibc_of") as asked:
                session.ran_in("nested-decode-micro", measure.HOST)
            asked.assert_not_called()

    def test_the_check_wants_a_glibc_in_the_stamp_and_in_every_sitting_marker(self):
        text = (
            measure.session_stamp("aaaaaaa", dirty=False, allocator="system") + "\n"
            + measure.figure_marker("map-only", "bbbbbbb") + "\n"
            + measure.figure_marker("peak-rss", "ccccccc", glibc="2.41") + "\n"
        )
        problems = measure.glibc_problems(text)
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("session stamp", problems[0])
        self.assertIn("map-only", problems[1])

    def test_the_doc_names_every_glibc_it_must(self):
        self.assertEqual(measure.glibc_problems(self.DOC.read_text()), [])
        self.assertIsNotNone(measure.stamp_glibc(self.DOC.read_text()))

    def test_a_drift_taken_outside_the_stamp_names_the_sweeps_glibc(self):
        with tempfile.TemporaryDirectory() as tmp:
            dirs = []
            for name in ("a", "b"):
                d = Path(tmp) / name
                d.mkdir()
                (d / "raw.json").write_text(
                    json.dumps(
                        {"commit": "abc1234", "date": "2026-09-27", "glibc": "2.41",
                         "readings": {"f/x": [1.0]}}
                    )
                )
                dirs.append(str(d))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                measure.cmd_drift(*dirs)
        self.assertEqual(measure.marker_glibcs(out.getvalue()), {"session-drift": "2.41"})


class ShippedBinary(unittest.TestCase):
    """The binary every `pgdt` figure is timed against, built rather than
    found.

    The failure each of these holds shut is the same one and it is silent: a
    sitting emits a full table, a session stamp naming `git rev-parse HEAD`,
    and a verdict about a library that was never executed. Only one figure in
    the register — the reserve instrument, whose resolved budgets fingerprint
    the charge model — could have noticed from its own numbers.
    """

    def setUp(self):
        # Module state, so one test's build would otherwise satisfy the next.
        measure._PGDT_BUILT = False

    def _cfg(self, **kw):
        return measure.Config(bin_pgdt=measure.CARGO_RELEASE_BIN, **kw)

    def test_the_default_binary_is_the_one_cargo_writes(self):
        # `ensure_pgdt_binary` compares the two to decide whether it may claim
        # to have built what it is about to time, so a `Config` default that
        # drifted from cargo's output path would turn the build into a no-op
        # and put the old check back with no sign of it.
        self.assertEqual(measure.Config().bin_pgdt, measure.CARGO_RELEASE_BIN)

    def test_the_shipped_binary_is_built_once_per_process(self):
        calls = []

        def fake_run(argv, cwd=None, capture=False, quiet=False):
            calls.append((list(argv), cwd))
            return ""

        with unittest.mock.patch.object(measure, "run", fake_run):
            out = measure.ensure_pgdt_binary(self._cfg(), lambda _: None)
            measure.ensure_pgdt_binary(self._cfg(), lambda _: None)
        self.assertEqual(out, measure.CARGO_RELEASE_BIN)
        self.assertEqual(len(calls), 1)
        argv, cwd = calls[0]
        self.assertEqual(
            argv, ["cargo", "build", "--release", "-p", "pgdt"]
        )
        self.assertEqual(cwd, measure.REPO)
        # No `--target-dir` and no `--features`: this is the shipped build, and
        # either one would make it a different binary from the one the recipes
        # in measurements.md and CONTRIBUTING.md describe.
        self.assertNotIn("--target-dir", argv)
        self.assertNotIn("--features", argv)

    def test_an_existing_binary_is_rebuilt_anyway(self):
        # The whole point, and it is asserted as an absence: nothing on the
        # path to the build consults the file. `target/release/pgdt` survives
        # between sessions, so short-circuiting on its existence is what timed
        # a charge model three commits stale against the harness's repaired
        # mirror of it and reported the difference as an over-bill.
        calls = []

        def fake_run(argv, cwd=None, capture=False, quiet=False):
            calls.append(list(argv))
            return ""

        def never(self):
            raise AssertionError("the build short-circuited on the file's existence")

        with unittest.mock.patch.object(measure, "run", fake_run), \
             unittest.mock.patch.object(Path, "exists", never):
            measure.ensure_pgdt_binary(self._cfg(), lambda _: None)
        self.assertEqual(len(calls), 1)

    def test_a_dry_run_announces_and_builds_nothing(self):
        # It measures nothing and must run where no binary exists.
        said = []

        def fake_run(argv, cwd=None, capture=False, quiet=False):
            raise AssertionError("a dry run built the binary")

        with unittest.mock.patch.object(measure, "run", fake_run):
            measure.ensure_pgdt_binary(self._cfg(dry_run=True), said.append)
        self.assertEqual(len(said), 1)
        self.assertIn("dry-run", said[0])

    def test_an_overridden_binary_is_never_built(self):
        # `cargo build --release` writes exactly one path, so a harness that
        # ran it and then timed some other file would be asserting a
        # provenance it does not have. There the binary is the caller's.
        def fake_run(argv, cwd=None, capture=False, quiet=False):
            raise AssertionError("the harness built a binary it does not own")

        with tempfile.TemporaryDirectory() as tmp:
            elsewhere = Path(tmp) / "pgdt"
            with unittest.mock.patch.object(measure, "run", fake_run):
                out = measure.ensure_pgdt_binary(
                    measure.Config(bin_pgdt=elsewhere), lambda _: None
                )
            self.assertEqual(out, elsewhere)


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
    *instrument* must report pgdt's peak and not the wrapper's or the client's
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
        self.assertIn("/pgdt parse --source /dump.sql --dtcache /tmp/x.dtcache", script)

    def test_the_redirection_takes_pgdt_s_stdout_and_not_the_reading(self):
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
        # Otherwise a pgdt that died would be reported as a resident set.
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
    something else, which is the family the allocator legs already have tests
    for. Two matter most. A leg that is not
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
        self.assertEqual(measure._ATTRIBUTION_LEGS[0][1:], ("pgdt", "parse-rss"))
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
        self.assertIn("docs/manual/dump-inspection.md", self._fig().also_quoted_by)

    def test_a_taken_attribution_declares_its_borrow(self):
        """A taken attribution borrows its reference row from `peak-rss`.

        Its `parse` reference row runs `peak-rss`'s `blocks500` and
        `blocks4000` shapes, so the two share a reading rather than take one
        each — two readings of one run put two numbers in the doc a section
        apart. An entry in `UNTAKEN` declares no edge, since the edge would
        entangle `peak-rss` with a figure no sweep takes; a taken one declares
        it, and that is what this asserts rather than leaves to a comment."""
        fig = self._fig()
        if fig in measure.UNTAKEN:
            self.assertEqual(fig.shares, ())
        else:
            self.assertIn("peak-rss", [s.source for s in fig.shares])


def _peak_rss_specs() -> list:
    """`peak-rss`'s specs, without running the figure."""
    return [
        measure.RunSpec("pgdt", name, "parse-rss", "warm", name) for name in measure._RSS_ROWS
    ]


class Selection(unittest.TestCase):
    def test_a_shared_reading_pulls_its_source_in(self):
        got = [f.id for f in measure.resolve_selection(["allocator"])]
        self.assertIn("scan-throughput-warm", got)

    def test_the_source_runs_first(self):
        got = [f.id for f in measure.resolve_selection(["allocator"])]
        self.assertLess(got.index("scan-throughput-warm"), got.index("allocator"))

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
    the reserve table that is one figure where the honest set is two."""

    def _session(self, readings=None, rss=None):
        session = measure.Session.__new__(measure.Session)
        session.readings = dict(readings or {})
        session.rss = dict(rss or {})
        return session

    def test_requires_is_read_off_the_declared_borrows(self):
        # One list, not two: a second declaration of the same fact drifts, and
        # the one that drifts is the one no run function reads.
        fig = measure.FIGURES_BY_ID["allocator"]
        self.assertEqual(
            fig.requires, ("scan-throughput-warm", "nested-end-to-end")
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
            "scan-throughput-warm": {
                spec.key("scan-throughput-warm") for spec in measure._throughput_specs("warm")
            },
            "nested-end-to-end": {
                spec.key("nested-end-to-end") for spec in measure._nested_specs()
            },
            "per-block-quadratic": {
                measure.RunSpec(
                    "pgdt", name, "parse-cache-out", "warm", ""
                ).key("per-block-quadratic")
                for name, _ in measure._QUADRATIC_ROWS
            },
            # The two resident figures borrow from this one, which is what made
            # `Session.borrow` carry an RSS reading. Both are keyed against the
            # same `parse-rss` rows, so a rename on either side reads here as a
            # figure measuring its own reference cell and calling it shared.
            "peak-rss": {spec.key("peak-rss") for spec in _peak_rss_specs()},
        }
        for fig in measure.EVERY_FIGURE:
            for shared in fig.shares:
                for spec in shared.republished:
                    with self.subTest(figure=fig.id, spec=spec.key(shared.source)):
                        self.assertIn(shared.source, taken)
                        self.assertIn(spec.key(shared.source), taken[shared.source])

    def test_the_closure_is_transitive(self):
        # `reserve` borrows `peak-rss` alone, and `rss-attribution` borrows the
        # same readings in turn, so re-taking `reserve` moves all three tables.
        self.assertEqual(measure.FIGURES_BY_ID["reserve"].requires, ("peak-rss",))
        self.assertEqual(
            measure.sharing_closure("reserve"), ["peak-rss", "rss-attribution"]
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
        source = measure.RunSpec("pgdt", "control", "parse", "warm", "")
        readings = {
            spec.key(shared.source): [1.0, 2.0]
            for shared in measure.FIGURES_BY_ID["allocator"].shares
            for spec in shared.republished
        }
        session = self._session(readings)
        note = measure.share_readings(session, "allocator")
        self.assertEqual(session.readings[source.key("allocator")], [1.0, 2.0])
        self.assertIn("Shared, not measured again", note)
        self.assertNotIn("Partial sweep", note)

    def test_a_resident_set_crosses_the_share_with_the_wall_clock(self):
        """The half `reserve`'s borrow needs, and the half that fails as a `KeyError`.

        `has` reads `readings`, so a borrow that copied the duration alone
        would report the spec satisfied and then leave the borrowing figure's
        renderer asking `get_rss` for a key nothing wrote — an hour into a
        sweep, not at the declaration."""
        source = measure.RunSpec("pgdt", "control", "parse", "warm", "")
        session = self._session(
            {source.key("scan-throughput-warm"): [1.0, 2.0]},
            {source.key("scan-throughput-warm"): [5.9, 6.0]},
        )
        measure.share_readings(session, "allocator")
        self.assertEqual(session.rss[source.key("allocator")], [5.9, 6.0])

    def test_a_source_with_no_resident_reading_leaves_the_key_absent(self):
        # An absent `rss` key means "this shape carries no RSS wrapper", which
        # `sweep` is careful to distinguish from an empty one. Manufacturing an
        # empty list here would turn the first fact into the second.
        source = measure.RunSpec("pgdt", "control", "parse", "warm", "")
        session = self._session({source.key("scan-throughput-warm"): [1.0, 2.0]})
        measure.share_readings(session, "allocator")
        self.assertNotIn(source.key("allocator"), session.rss)

    def test_an_all_killed_source_leg_crosses_as_an_empty_list(self):
        # The other side of the same distinction: the source opened the key and
        # every rep was censored. That is a fact about the leg and it travels.
        source = measure.RunSpec("pgdt", "control", "parse", "warm", "")
        session = self._session(
            {source.key("scan-throughput-warm"): [1.0]},
            {source.key("scan-throughput-warm"): []},
        )
        measure.share_readings(session, "allocator")
        self.assertEqual(session.rss[source.key("allocator")], [])

    def test_no_republished_spec_is_an_instrument_leg(self):
        """Why `borrow` copies two channels and not four.

        `reported` and `instrument` are read by the figure that *declared* the
        leg, and `RunSpec.instrument` is that declaration rather than a
        property of the run. A borrow that carried them would hand a borrower a
        report it never asked the harness to find — sound only while no shared
        spec is such a leg, which is what this holds."""
        for fig in measure.EVERY_FIGURE:
            for shared in fig.shares:
                for spec in shared.republished:
                    with self.subTest(figure=fig.id, spec=spec.key(shared.source)):
                        self.assertFalse(spec.instrument)

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
        # Selection pulls `peak-rss` in, which `reserve` borrows; the table
        # borrowing the same readings is what it leaves out.
        gaps = measure.closure_gaps(measure.resolve_selection(["reserve"]))
        self.assertTrue(any(g.startswith("reserve —") for g in gaps))
        self.assertIn("rss-attribution", " ".join(gaps))

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
        # `allocator` publishes `scan-throughput-warm`'s reading as its own
        # number, which is the closure's business and not this edge's.
        self.assertEqual(measure.derived_consumers("scan-throughput-warm"), [])

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
            "<!-- figure: scan-throughput-warm -->\nnothing here\n"
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
    `pgdt`'s own `--jobs`.

    Every assertion here is a way to get a plausible table of the wrong thing,
    which is the family this module already covers for the allocator legs.
    Three of them matter most, and each fails silently
    without a test. A **clamped row** — a stated count the library quietly
    reduces because the budget cannot hold that many partitions — is a lower
    count wearing a higher label, and the resulting table is monotone and wrong.
    A **shape drifting off `SWEEP_JOBS`** puts a second worker count in the
    register with nothing saying so, which is the defect the worker-count
    apparatus rule closed one level up. And a **leg whose rate is per compressed byte** reads five times too
    slow under a heading that looks like the plain leg's.
    """

    def test_both_figures_are_taken_and_no_longer_untaken(self):
        # Both are in `FIGURES`, where the doc-side checks apply.
        untaken = [f.id for f in measure.UNTAKEN]
        self.assertNotIn("parallel-scan-throughput", untaken)
        self.assertNotIn("parallel-peak-rss", untaken)
        taken = [f.id for f in measure.FIGURES]
        self.assertIn("parallel-scan-throughput", taken)
        self.assertIn("parallel-peak-rss", taken)

    def test_every_registered_job_count_has_a_shape_in_every_family(self):
        # The provider family's count is its `target_partitions`; the `--jobs`
        # it states is its untimed builder's.
        for family in measure.JOBS_AXIS:
            for jobs in measure.PARALLEL_JOBS:
                with self.subTest(family=family, jobs=jobs):
                    script = measure._script(f"{family}{jobs}")
                    if family.startswith(measure.PARALLEL_SCAN):
                        self.assertEqual(measure._dfcli_partitions(script), {str(jobs)})
                    else:
                        self.assertIn(f"--jobs {jobs} ", script)
                    self.assertEqual(script.count("time "), 1)

    def test_a_job_count_the_figures_do_not_carry_is_an_error(self):
        # The shape is parsed rather than matched, so an unregistered count has
        # to be refused explicitly or it would run at whatever was typed and be
        # read as a row of the table.
        for command in ("parse-jobs-3", "parse-jobs-", "parse-jobs-all",
                        "dfcli-query-typed-jobs-7", "parse-rss-jobs-x"):
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
        # A provider leg states it as the setting `pgdt --memory` is the flag
        # for, ahead of its query in the same process.
        allowance = measure.stated_allowance(measure.PARALLEL_BUDGET)
        for family in measure.JOBS_AXIS:
            for jobs in measure.PARALLEL_JOBS:
                with self.subTest(family=family, jobs=jobs):
                    stated = (
                        f"'SET pgdump.memory = {allowance}'"
                        if family.startswith(measure.PARALLEL_SCAN)
                        else f"--memory {allowance}"
                    )
                    self.assertIn(stated, measure._script(f"{family}{jobs}"))

    def test_the_budget_admits_the_widest_row_on_the_coarser_leg(self):
        # A block-decoding source charges each reader its block, the chunk
        # buffer and the decoder's own retention, and the readers together the
        # block pool's retention list — `WorkerMemory::at`, which
        # `worker_count` solves the stated bytes against. A budget below that
        # at the widest row clamps the top rows silently, which 1 GiB would.
        block = 24 * measure.MIB
        # `xz_seek::Reader::decode_footprint` on this shape: an 8 MiB LZMA2
        # dictionary, the 1 MiB input chunk, and `liblzma`'s 34,592 B of state.
        decoder = 8 * measure.MIB + measure.CHUNK_DEFAULT + 34_592
        jobs = measure.PARALLEL_JOBS[-1]
        want = jobs * (block + measure.CHUNK_DEFAULT + decoder) + (
            max(measure.LIBRARY_POOL_DEPTH, jobs) - 1
        ) * block
        self.assertGreaterEqual(measure.PARALLEL_BUDGET, want)
        self.assertLess(1 << 30, want, "1 GiB would clamp the widest row")

    def test_the_container_is_the_budget_plus_a_fixed_headroom(self):
        # `container > budget` holds against a literal `3g` whatever the budget
        # does: raising the budget 1 GiB -> 2 GiB under it halved the headroom
        # and still passed. Block-pool retention is a function of the budget,
        # so the widest `control_xz128` row went from 2110 MiB to 3067 MiB
        # against a 3072 MiB limit. Pin the rule, not the inequality.
        self.assertTrue(measure.PARALLEL_MEMORY.endswith("g"))
        self.assertEqual(
            int(measure.PARALLEL_MEMORY[:-1]) * measure.GIB,
            measure.PARALLEL_BUDGET + measure.PARALLEL_HEADROOM,
        )
        self.assertNotEqual(measure.PARALLEL_MEMORY, measure.Config().memory)

    def test_the_headroom_clears_the_widest_measured_row(self):
        # The reading that forced the rule: `control_xz128` at `--jobs 24` and a
        # 2 GiB budget peaked at 3067 MiB, of which 2 GiB is the budget itself.
        # A headroom below what the unbudgeted terms actually cost buys an OOM
        # an hour into a sweep rather than a reading.
        measured_above_budget = 3067 * measure.MIB - (2 << 30)
        self.assertGreater(measure.PARALLEL_HEADROOM, measured_above_budget)

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
                for cmd in ("parse", measure.PARALLEL_SCAN)
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


class Reserve(unittest.TestCase):
    """The instrument behind the budget rule's one number.

    Every assertion here is a way to get a plausible table of the wrong thing,
    which is the family this module already covers for the `--jobs` axis. Four
    matter most and each fails silently. An **arena setting that never
    reached the process** — written where the wrapper takes it as an argument
    rather than as its environment — gives two legs that agree, which reads as
    "the cap buys nothing" rather than as an instrument that set nothing. A
    **budget the shape does not carry** would run at whatever was typed and be
    read as the row it is labelled with. A **worker count drifting onto
    `SWEEP_JOBS`** would read the stated axis at a count the flagless `.xz`
    default never recommends on this machine. And a **`Shared` edge missing
    from the taken figure** leaves two readings of `peak-rss`'s `control` run in
    the doc a section apart.
    """

    def _fig(self):
        return measure.SELECTABLE_BY_ID["reserve"]

    def _shape(self, token="unset", budget=None):
        return f"{measure.RESERVE_FAMILY}{token}-{budget or measure.RESERVE_BUDGETS[0]}"

    def test_every_registered_leg_has_a_shape(self):
        shapes = set(measure.command_shapes())
        for token, _, _ in measure.RESERVE_ARENAS:
            for budget in measure.RESERVE_BUDGETS:
                with self.subTest(arena=token, budget=budget):
                    self.assertIn(self._shape(token, budget), shapes)

    def test_the_arena_setting_is_the_processs_environment(self):
        # `perl` is `exec`ed by the wrapper, so an assignment in front of it is
        # inherited by pgdt. In front of `/pgdt` it would be a further argument
        # to `perl` and would set nothing at all.
        for token, value, _ in measure.RESERVE_ARENAS:
            with self.subTest(arena=token):
                script = measure._script(self._shape(token))
                if not value:
                    self.assertNotIn("MALLOC_ARENA_MAX", script)
                    continue
                self.assertIn(f"time MALLOC_ARENA_MAX={value} perl", script)

    def test_one_leg_sets_nothing(self):
        # The shipped default has to survive the operator who followed no
        # recommendation, so an uncapped leg is not optional decoration.
        unset = [token for token, value, _ in measure.RESERVE_ARENAS if not value]
        self.assertEqual(unset, ["unset"])

    def test_no_leg_caps_at_or_above_the_worker_count(self):
        # A cap at or above the arena count cannot bind, and a leg pricing one
        # reads as "the cap buys nothing". The arena count is `readers + 2`
        # from two readers up, and 2 at one, rather than the worker count, so this bound is looser than inertness
        # — a `readers + 1` cap is below it and unpriced — but a cap at the
        # worker count or above is a leg no sitting needs.
        for token, value, _ in measure.RESERVE_ARENAS:
            if value:
                with self.subTest(arena=token):
                    self.assertLess(int(value), measure.RESERVE_JOBS)

    def test_the_capped_leg_is_the_tightest_an_operator_would_set(self):
        # The pair is uncapped against the tightest plausible cap, and both are
        # load-bearing: a one-leg figure cannot report a null.
        capped = [value for _, value, _ in measure.RESERVE_ARENAS if value]
        self.assertEqual(capped, ["2"])
        self.assertEqual(len(measure.RESERVE_ARENAS), 2)

    def test_a_budget_or_arena_the_figure_does_not_carry_is_an_error(self):
        # Parsed rather than matched, so an unregistered value has to be
        # refused explicitly or it runs and is read as a row of the table.
        with self.assertRaises(ValueError):
            measure._script(self._shape(budget=99))
        with self.assertRaises(ValueError):
            measure._script(self._shape(token="sixteen"))

    def test_the_axis_starts_at_the_librarys_own_default(self):
        # The smallest budget is the library's own default, which is also the
        # number a run that states nothing gets.
        self.assertEqual(measure.RESERVE_BUDGETS[0], measure.LIBRARY_DEFAULT_BUDGET)
        self.assertEqual(sorted(measure.RESERVE_BUDGETS), list(measure.RESERVE_BUDGETS))

    def test_every_budget_is_a_whole_mebibyte(self):
        for budget in measure.RESERVE_BUDGETS:
            with self.subTest(budget=budget):
                self.assertEqual(measure._fmt_budget(budget), f"{budget >> 20} MiB")

    def test_every_shape_states_the_figures_own_worker_count(self):
        for token, _, _ in measure.RESERVE_ARENAS:
            with self.subTest(arena=token):
                script = measure._script(self._shape(token))
                self.assertIn(f"--jobs {measure.RESERVE_JOBS} ", script)
                self.assertEqual(script.count("time "), 1)
        self.assertNotEqual(measure.RESERVE_JOBS, measure.SWEEP_JOBS)
        self.assertEqual(measure.RESERVE_JOBS, measure.PARALLEL_JOBS[-1])

    def test_the_family_is_a_declared_axis_rather_than_an_unpinned_shape(self):
        # Both halves of the worker-count reconciliation: the family states a
        # count, and it is exempt from `SWEEP_JOBS` because it declares itself.
        self.assertEqual(measure.worker_count_problems(), [])
        self.assertEqual(measure.pinned_count_problems(), [])

    def test_the_shape_name_says_it_carries_a_resident_reading(self):
        # `time_run` reads the wrapper's report only for a shape whose name
        # contains `rss`; without it every reading here would come back `None`
        # and the table would fail on a missing key rather than on a wrong one.
        self.assertIn("rss", measure.RESERVE_FAMILY)

    def test_both_sources_are_read(self):
        # A reserve that is a property of the source is not a constant, so the
        # loosest shape is read beside the tightest.
        self.assertEqual([n for n, _ in measure.RESERVE_INPUTS], ["control_xz", "control"])

    def test_the_serial_baseline_is_peak_rsss_own_row(self):
        # Spec for spec, which is what makes it the borrow this figure declares
        # when it is published rather than a second reading of one measurement.
        base = measure._RESERVE_BASELINE
        # By key, which is what identifies a reading: the label is what the
        # table prints and is free to differ between two figures naming one run.
        keys = [s.key("peak-rss") for s in _peak_rss_specs()]
        self.assertIn(base.key("peak-rss"), keys)
        self.assertEqual(base.input, "control")
        self.assertIn("control", measure._RSS_ROWS)

    def test_it_declares_no_share_while_it_is_untaken(self):
        """The same obligation `rss-attribution` carries, and for the same reason.

        An entry in `UNTAKEN` declares no edge, since the edge would entangle
        `peak-rss` with a figure no sweep takes; a taken one borrows the
        `control` run, which `Session.borrow` carries with its resident set."""
        fig = self._fig()
        if fig in measure.UNTAKEN:
            self.assertEqual(fig.shares, ())
        else:
            self.assertIn("peak-rss", [s.source for s in fig.shares])

    def test_the_baselines_input_is_staged(self):
        # It is one of the two already; naming it anyway is what survives the
        # day it is not.
        self.assertIn(measure._RESERVE_BASELINE.input, self._fig().warm_inputs)
        self.assertEqual(
            len(self._fig().warm_inputs), len(set(self._fig().warm_inputs))
        )

class CompressedAccount(unittest.TestCase):
    """The three families `reserve` grew when the compressed path got an account.

    The figure publishes a **pair** beside its model check — a fixed term and a
    per-reader term — so every assertion here is a way to get a plausible pair
    of the wrong thing. Four fail silently and are the reason this class
    exists. A **flagless leg that states a flag** measures a stated arrangement
    under a heading that says discovered, which `worker_count_problems` cannot
    see because that family is exempt from it. Two **legs differing only in
    their container limit** would share a reading, the limit not being an argv
    fact — which is `_attribution_specs`' failure with a new cause. A **fit over
    one point** is an intercept asserted as a measurement, which is the
    extrapolation `.claude/skills/evidence/SKILL.md`'s second rule names. And a
    **mechanism leg at its
    own limit or block size** would read as a mechanism moving a term when what
    moved was the arrangement.
    """

    def _fig(self):
        return measure.SELECTABLE_BY_ID["reserve"]

    # -- the flagless axis -------------------------------------------------

    def test_the_flagless_axis_runs_at_both_block_sizes(self):
        # A compressed reader's charge bills a unit, and the pool a unit a
        # reader past `POOL_DEPTH`, so a unit-shaped error in it is multiplied by
        # the reader count: a fit at one block size
        # cannot tell a term that scales with the unit from one that does not.
        self.assertEqual(
            [name for name, _, _ in measure.RESERVE_FLAGLESS_INPUTS],
            ["control_xz", "control_xz128"],
        )
        for name, _, unit in measure.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(input=name):
                self.assertEqual(measure.INPUTS[name].suffix, ".xz")
                self.assertIn(name, self._fig().warm_inputs)
                self.assertGreater(unit, 0)
        units = {unit for _, _, unit in measure.RESERVE_FLAGLESS_INPUTS}
        self.assertEqual(len(units), len(measure.RESERVE_FLAGLESS_INPUTS))

    def test_the_block_sizes_are_the_ones_the_other_compressed_figure_reads(self):
        # Two block sizes is a claim about files, not about this figure, so the
        # pair is the one `parallel-peak-rss` already publishes — a third size
        # here would be a second answer to the same question.
        self.assertEqual(
            [name for name, _, _ in measure.RESERVE_FLAGLESS_INPUTS],
            [name for name, _ in measure.PARALLEL_RSS_LEGS],
        )

    def test_the_flagless_shapes_state_neither_flag(self):
        # The exemption `_NO_FLAGS` opens is from *stating a count*; a shape
        # that quietly acquired one would still pass the count reconciliation,
        # being exempt, and would publish a stated arrangement as a discovered
        # one.
        self.assertEqual(measure.flagless_flag_problems(), [])
        for token, _, _ in measure.RESERVE_ARENAS:
            with self.subTest(arena=token):
                script = measure._script(f"{measure.RESERVE_FLAGLESS}{token}")
                self.assertNotIn("--jobs", script)
                self.assertNotIn("--memory", script)
                self.assertIn("/pgdt parse --source /dump.sql", script)

    def test_a_flagless_shape_that_states_a_flag_is_reported(self):
        # The check must fail loudly: the shape it would pass still runs and
        # still produces a row.
        with unittest.mock.patch.object(
            measure,
            "_script",
            lambda c: "time /pgdt parse --source /dump.sql --jobs 4 --memory 99",
        ):
            reported = measure.flagless_flag_problems()
        self.assertEqual(
            sorted(reported),
            sorted(
                f"{c} states --jobs 4, --memory 99"
                for c in measure.command_shapes()
                if c.startswith(measure.RESERVE_FLAGLESS)
            ),
        )

    def test_the_exemption_is_declared_at_both_ends(self):
        # Declared as its own prefix rather than appended to `_NO_WORKERS`,
        # whose rule is "not a run of ours" — this is exactly a run of ours,
        # stating nothing on purpose.
        self.assertIn(measure.RESERVE_FLAGLESS, measure._NO_FLAGS)
        self.assertNotIn(measure.RESERVE_FLAGLESS, measure._NO_WORKERS)
        self.assertEqual(measure.worker_count_problems(), [])
        self.assertEqual(measure.pinned_count_problems(), [])

    def test_the_limit_rides_on_the_spec_and_two_legs_never_share_a_reading(self):
        # The container limit is a `nerdctl run` argument, so it cannot be in
        # the command shape; these legs share one shape and are told apart by
        # `RunSpec.memory`, which `RunSpec.key` has to carry or they collapse
        # into one another's reps.
        specs = measure._reserve_flagless_specs()
        self.assertEqual(
            len(specs),
            len(measure.RESERVE_FLAGLESS_INPUTS) * len(measure.RESERVE_LIMITS),
        )
        keys = [s.key("reserve") for s in specs]
        self.assertEqual(len(set(keys)), len(keys))
        tokens = {token for token, _ in measure.RESERVE_LIMITS}
        for spec in specs:
            with self.subTest(leg=spec.label):
                self.assertIn(spec.memory, tokens)
                self.assertIn(f"m={spec.memory}", spec.key("reserve"))

    def test_a_spec_that_states_no_limit_keys_as_it_always_did(self):
        # Appended rather than always present, so a past sitting's `raw.json`
        # still renders: every spec outside this figure keys exactly as before.
        spec = measure.RunSpec("pgdt", "control", "parse-rss", "warm", "x")
        self.assertEqual(spec.key("peak-rss"), "peak-rss/pgdt/control/parse-rss/warm")
        self.assertEqual(spec.memory, None)

    def test_nothing_outside_this_figure_states_a_per_spec_limit(self):
        # A per-spec limit exists because one figure's axis *is* the limit.
        # Anywhere else it would be a container departure no table states.
        others = [
            *measure._reserve_specs(),
            *_peak_rss_specs(),
            *measure._parallel_rss_specs(),
            *measure._parallel_specs(),
            *measure._decode_specs(),
            *(spec for _, small, big in measure._attribution_specs() for spec in (small, big)),
            measure._RESERVE_BASELINE,
        ]
        for spec in others:
            with self.subTest(leg=spec.label):
                self.assertIsNone(spec.memory)

    def test_the_limits_span_the_curve_rather_than_its_worst_end(self):
        # 512 MiB is the bottom of the range, where the rule's own headroom is
        # worst; a set clustered there would fit a line through the worst end
        # alone.
        self.assertEqual(measure.RESERVE_LIMITS[0], ("512m", 512 << 20))
        byte_values = [n for _, n in measure.RESERVE_LIMITS]
        self.assertEqual(sorted(byte_values), byte_values)
        self.assertGreaterEqual(len(measure.RESERVE_LIMITS), 3)

    def test_the_pool_term_is_billed_at_every_reader_count(self):
        # *Rejected:* `max(0, POOL_DEPTH - jobs) x unit`, which clamps to zero
        # at four readers or more, so only limits under the clamp evaluate it.
        # Stated as what the pool holds it is billed everywhere, and past
        # `POOL_DEPTH` it is the larger half of the charge — so there is no
        # window to miss.
        for _name, label, unit in measure.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(block_size=label):
                for jobs in (1, 2, 4, 5, 24):
                    self.assertEqual(
                        measure.pool_bytes(unit, jobs),
                        (max(measure.LIBRARY_POOL_DEPTH, jobs) - 1) * unit,
                    )
                    self.assertGreater(measure.pool_bytes(unit, jobs), 0)
                # And the charge is exactly one unit under what the two-unit
                # per-reader term plus a decaying floor billed, at every count.
                for jobs in (1, 2, 3, 4, 9, 24):
                    was = jobs * (measure.reader_bytes(unit) + unit) + max(
                        0, measure.LIBRARY_POOL_DEPTH - jobs
                    ) * unit
                    self.assertEqual(measure.charge_bytes(unit, jobs), was - unit)

    def test_the_registered_axis_can_reach_three_distinct_reader_counts(self):
        # Fit-ability is asked of the same registered limits, because an
        # axis that can only ever publish a secant should fail before a sitting
        # is spent rather than after. Necessary and not sufficient — a host with
        # few cores collapses distinct fits onto one count, which is the
        # per-sitting residue the secant covers.
        self.assertEqual(measure.reserve_axis_problems(), [])
        for _name, label, unit in measure.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(block_size=label):
                counts = {
                    measure.afforded_readers(unit, measure.discovered_budget(limit))
                    for _, limit in measure.RESERVE_LIMITS
                } - {0}
                self.assertGreaterEqual(len(counts), measure.RESERVE_FIT_MIN_COUNTS)

    def test_the_check_fires_where_the_axis_cannot_be_fitted(self):
        # The two-sided half of the fit-ability check: dropping the limits that
        # afford the 128 MiB family its distinct counts has to turn it red, and
        # the line has to say which counts are left.
        kept = tuple(
            row for row in measure.RESERVE_LIMITS if row[0] not in ("1536m", "2g")
        )
        with unittest.mock.patch.object(measure, "RESERVE_LIMITS", kept):
            problems = measure.reserve_axis_problems()
        fitness = [line for line in problems if "distinct reader count(s)" in line]
        self.assertEqual(len(fitness), 1)
        self.assertIn("128 MiB blocks", fitness[0])
        self.assertIn(f"under the {measure.RESERVE_FIT_MIN_COUNTS}", fitness[0])

    def test_check_fails_where_the_axis_cannot_be_fitted(self):
        # Wired into `--check`, not only available to be called: the whole point
        # is that an axis that can only ever publish a secant should fail before
        # a sitting is spent rather than after.
        kept = tuple(
            row for row in measure.RESERVE_LIMITS if row[0] not in ("1536m", "2g")
        )
        with unittest.mock.patch.object(measure, "RESERVE_LIMITS", kept):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = measure.cmd_check(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("distinct reader counts a two-term fit needs", out.getvalue())
        self.assertIn("distinct reader count(s)", out.getvalue())

    def test_check_fails_where_the_axis_cannot_be_fitted(self):
        # The same wiring for the fit-ability half: one refusal, two questions,
        # and `--check` has to carry both or the second is a function nobody
        # calls.
        kept = tuple(
            row for row in measure.RESERVE_LIMITS if row[0] not in ("1536m", "2g")
        )
        with unittest.mock.patch.object(measure, "RESERVE_LIMITS", kept):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = measure.cmd_check(measure.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("distinct reader count(s)", out.getvalue())

    def test_a_stated_allowance_leaves_the_budget_the_figure_registered(self):
        # `--memory` states resident, so a figure registered against a buffer
        # budget has to state that budget plus the reserve or it measures a
        # different apparatus under the same heading.
        for budget in (*measure.RESERVE_BUDGETS, *measure.RESERVE_STEP_BUDGETS,
                       measure.PARALLEL_BUDGET):
            with self.subTest(budget=budget):
                allowance = measure.stated_allowance(budget)
                self.assertEqual(
                    allowance - measure.LIBRARY_MEMORY_RESERVE,
                    budget,
                    "the carve's cap has to land back on the registered budget",
                )
        # And the step pair stays one byte apart, which is the whole of what
        # that family measures.
        step = [measure.stated_allowance(b) for b in measure.RESERVE_STEP_BUDGETS]
        self.assertEqual(abs(step[0] - step[1]), 1)

    def test_the_discovered_budget_is_the_library_rule_mirrored(self):
        # `limit.bytes.saturating_sub(MEMORY_RESERVE)`, read off the source
        # rather than trusted, since the whole point of the check above is that
        # it reasons in the library's own arithmetic.
        src = (measure.REPO / "pgdump_query/src/io.rs").read_text()
        self.assertIn("allowance.saturating_sub(MEMORY_RESERVE)", src)
        self.assertEqual(
            measure.discovered_budget(512 << 20), (512 << 20) - measure.LIBRARY_MEMORY_RESERVE
        )
        # Saturating, not negative: a container at or under the reserve.
        self.assertEqual(measure.discovered_budget(measure.LIBRARY_MEMORY_RESERVE), 0)
        self.assertEqual(measure.discovered_budget(0), 0)

    def test_every_flagless_leg_is_read_under_the_uncapped_arena(self):
        # The shipped constant comes from the operator who capped nothing; the
        # capped arrangement is a mechanism leg, not the axis.
        shape = f"{measure.RESERVE_FLAGLESS}{measure.RESERVE_UNCAPPED}"
        for spec in measure._reserve_flagless_specs():
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.command, shape)
        self.assertNotIn("MALLOC_ARENA_MAX", measure._script(shape))

    # -- the mechanism legs ------------------------------------------------

    def test_the_mechanism_legs_are_one_block_size_and_one_allocation(self):
        # Crossing them with the limits and the block sizes buys a second cross
        # of an expensive axis for no question anybody asked.
        legs = measure._reserve_mechanism_specs()
        self.assertEqual(len(legs), 1)
        for label, spec in legs:
            with self.subTest(leg=label):
                self.assertEqual(spec.input, measure.RESERVE_MECHANISM_INPUT)
                self.assertEqual(spec.memory, measure.RESERVE_MECHANISM_LIMIT)
        self.assertIn(
            measure.RESERVE_MECHANISM_LIMIT, [token for token, _ in measure.RESERVE_LIMITS]
        )
        self.assertIn(
            measure.RESERVE_MECHANISM_INPUT,
            [name for name, _, _ in measure.RESERVE_FLAGLESS_INPUTS],
        )

    def test_the_reference_is_a_leg_of_the_axis_rather_than_a_re_take(self):
        # Each mechanism leg is one reading, compared against the flagless leg
        # at the same input and limit — which the axis above already measures,
        # so nothing here is measured twice.
        reference = measure.RunSpec(
            "pgdt",
            measure.RESERVE_MECHANISM_INPUT,
            f"{measure.RESERVE_FLAGLESS}{measure.RESERVE_UNCAPPED}",
            "warm-parallel",
            "",
            memory=measure.RESERVE_MECHANISM_LIMIT,
        )
        keys = [s.key("reserve") for s in measure._reserve_flagless_specs()]
        self.assertIn(reference.key("reserve"), keys)

    def test_no_leg_of_this_figure_swaps_the_allocator(self):
        # The two allocator legs are **dropped, not re-aimed**: jemalloc and
        # mimalloc do not have glibc's dynamic mmap threshold, so swapping them
        # removes the mechanism under test instead of measuring it. A later
        # session re-adding one would be re-running the sitting whose
        # foreseeable outcome was "the three legs could not attribute it".
        every = [
            *measure._reserve_flagless_specs(),
            *measure._reserve_instrument_specs(),
            *(s for _, s in measure._reserve_mechanism_specs()),
            *measure._reserve_step_specs(),
            *measure._reserve_specs(),
            measure._RESERVE_BASELINE,
        ]
        for spec in every:
            with self.subTest(leg=spec.label):
                self.assertFalse(spec.binary.startswith("alloc:"))

    def test_the_instrument_legs_are_the_flagless_shape_on_the_instrument_build(self):
        # The attribution and the check must be the *same arrangement* measured
        # two ways, so the shape is the flagless one and the only difference is
        # the binary — which `key` carries, so neither can be read as a rep of
        # the other.
        flagless = {s.key("reserve"): s for s in measure._reserve_flagless_specs()}
        legs = measure._reserve_instrument_specs()
        self.assertEqual(len(legs), len(measure.RESERVE_INSTRUMENT_LIMITS) + 1)
        for spec in legs:
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.binary, "introspect")
                self.assertTrue(spec.instrument)
                self.assertEqual(spec.input, measure.RESERVE_MECHANISM_INPUT)
                self.assertTrue(spec.command.startswith(measure.RESERVE_FLAGLESS))
                self.assertNotIn(spec.key("reserve"), flagless)
        # Every limit of the axis, so the instrument's decomposition can be read
        # against the black-box fit leg for leg rather than at one cell.
        uncapped = [s for s in legs if s.command == measure._flagless_shape()]
        self.assertEqual(
            [s.memory for s in uncapped], list(measure.RESERVE_INSTRUMENT_LIMITS)
        )
        # And the capped leg, at the same pair the mechanism row takes, so the
        # black-box delta and the introspective one describe one arrangement.
        capped = [s for s in legs if s.command == measure._flagless_shape(measure.RESERVE_CAPPED)]
        self.assertEqual([s.memory for s in capped], [measure.RESERVE_MECHANISM_LIMIT])

    def test_an_instrument_leg_is_kill_tolerant_like_the_axis_it_mirrors(self):
        # It runs the arrangement the rule aims *at* the allocation, so it sits
        # against its own ceiling exactly as the flagless axis does — and the
        # report is written at exit, so a kill is also the one legitimate
        # absence of the report `RunSpec.instrument` otherwise makes an error.
        for spec in measure._reserve_instrument_specs():
            with self.subTest(leg=spec.label):
                self.assertTrue(measure.kill_tolerant(spec.command))

    def test_the_arena_leg_is_the_flagless_shape_with_the_cap_and_nothing_else(self):
        # One mechanism at a time: the capped leg differs from the reference by
        # the environment the wrapper `exec`s into and by nothing on the command
        # line.
        arena = next(
            label for token, _, label in measure.RESERVE_ARENAS if token == measure.RESERVE_CAPPED
        )
        spec = dict(measure._reserve_mechanism_specs())[arena]
        self.assertEqual(spec.binary, "pgdt")
        script = measure._script(spec.command)
        self.assertIn("time MALLOC_ARENA_MAX=2 perl", script)
        self.assertNotIn("--jobs", script)
        self.assertNotIn("--memory", script)

    # -- the path step -----------------------------------------------------

    def test_the_step_is_one_byte_either_side_of_what_a_reader_costs(self):
        # `block_path_afforded` is the comparison, so the pair straddles it and
        # differs in nothing else. A wider gap would be a budget change with a
        # path change inside it — and a pair straddling `reader_bytes` instead
        # would run the streaming decoder on *both* legs, one reader's share of
        # the pool's retention list being inside `BlockCache::affordable`.
        afforded, declined = measure.RESERVE_STEP_BUDGETS
        self.assertEqual(afforded, measure.charge_bytes(measure.RESERVE_MECHANISM_UNIT, 1))
        self.assertEqual(declined, afforded - 1)
        self.assertTrue(measure.block_path_afforded(measure.RESERVE_MECHANISM_UNIT, afforded))
        self.assertFalse(measure.block_path_afforded(measure.RESERVE_MECHANISM_UNIT, declined))
        self.assertGreater(afforded, measure.reader_bytes(measure.RESERVE_MECHANISM_UNIT))
        for spec in measure._reserve_step_specs():
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.input, measure.RESERVE_MECHANISM_INPUT)
                self.assertEqual(spec.memory, measure.RESERVE_MECHANISM_LIMIT)
                self.assertIn(f"--jobs {measure.RESERVE_JOBS}", measure._script(spec.command))

    def test_the_step_budgets_fit_inside_the_allocation_they_run_in(self):
        # A stated budget above the container's own limit would measure the
        # container, not the path.
        limit = dict(measure.RESERVE_LIMITS)[measure.RESERVE_MECHANISM_LIMIT]
        for budget in measure.RESERVE_STEP_BUDGETS:
            with self.subTest(budget=budget):
                self.assertLess(budget, limit)

    def test_a_step_budget_the_figure_does_not_carry_is_an_error(self):
        with self.assertRaises(ValueError):
            measure._script(f"{measure.RESERVE_STEP_FAMILY}99")

    # -- the reader charge, mirrored ---------------------------------------

    def test_the_reader_charge_is_the_librarys_own_three_terms(self):
        # Hand-computed on `QUERY_SUBSTREAM_CAP`'s argument, so the mirror is
        # checked here rather than trusted: one block slot — the block being
        # decoded, which is the block then retained — the chunk buffer
        # and the decoder's retention, which is 34.03 MiB at koji's block size.
        self.assertEqual(measure.reader_bytes(24 << 20), 35_686_176)
        self.assertEqual(
            measure.reader_bytes(128 << 20),
            (128 << 20) + measure.LIBRARY_CHUNK_BYTES + measure.XZ_DECODE_FOOTPRINT,
        )
        self.assertEqual(measure.LIBRARY_CHUNK_BYTES, measure.CHUNK_DEFAULT)

    def test_the_mirrored_chunk_size_is_the_librarys_default(self):
        # The one term of the three that is a library constant rather than a
        # property of the file or of the decoder.
        src = (measure.REPO / "pgdump_query/src/scan.rs").read_text()
        self.assertIn(
            f"pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = {measure.LIBRARY_CHUNK_BYTES >> 20} << 20;",
            src,
        )

    def test_the_mechanism_unit_is_read_off_the_input_registry(self):
        # Written twice, the unit the step's budgets are computed from and the
        # unit the flagless table prints would be free to disagree.
        self.assertEqual(
            measure.RESERVE_MECHANISM_UNIT,
            next(
                unit
                for name, _, unit in measure.RESERVE_FLAGLESS_INPUTS
                if name == measure.RESERVE_MECHANISM_INPUT
            ),
        )

    # -- the block-path line, mirrored -------------------------------------

    def test_the_path_line_is_the_charge_at_one_reader_not_the_reader_term(self):
        # `BlockCache::affordable` compares the budget against what one reader
        # *costs the rule* — the per-reader term plus that one reader's share of
        # the pool's retention list. `reader_bytes` alone is a different line,
        # and at both registered block sizes the two are far apart, so a mirror
        # comparing against it labels whole cells with the wrong mechanism
        # rather than getting an edge case wrong.
        for unit in (24 << 20, 128 << 20):
            with self.subTest(unit=unit):
                line = measure.charge_bytes(unit, 1)
                self.assertEqual(line, measure.reader_bytes(unit) + 3 * unit)
                self.assertTrue(measure.block_path_afforded(unit, line))
                self.assertFalse(measure.block_path_afforded(unit, line - 1))
                # The number the old mirror compared against is now well inside
                # the declined side.
                self.assertFalse(measure.block_path_afforded(unit, measure.reader_bytes(unit)))

    def test_the_path_line_is_the_comparison_the_library_makes(self):
        # Mirrored by hand on `QUERY_SUBSTREAM_CAP`'s argument, so the source is
        # read here rather than the arithmetic trusted: `affordable` asks
        # `worker_memory(..).at(1) <= budget`, and `worker_memory` is the
        # per-reader term `pooling`ed at `POOL_DEPTH`.
        src = (measure.REPO / "pgdump_query/src/io.rs").read_text()
        self.assertIn(
            "self.worker_memory(chunk_bytes, decode_bytes).at(1) <= budget",
            src,
        )
        self.assertIn(".pooling(self.unit as u64, POOL_DEPTH)", src)
        self.assertIn(f"const POOL_DEPTH: usize = {measure.LIBRARY_POOL_DEPTH};", src)

    def test_the_renderer_asks_the_path_question_in_one_way_only(self):
        # Sites asking the question different ways make a table whose cells
        # silently change mechanism while reading as one series. A comparison of a budget
        # against `reader_bytes` anywhere in the renderer is that failure
        # returning.
        source = inspect.getsource(measure.run_reserve)
        self.assertNotIn("budget >= reader_bytes", source)
        self.assertNotIn("budget < reader_bytes", source)
        self.assertNotIn("budget >= charge_bytes", source)
        self.assertNotIn("budget < charge_bytes", source)
        # Five: the instrument account's line drops a declined leg
        # by the same question, because a leg holding no block pool has no
        # retention list for `_depooled` to subtract.
        self.assertEqual(source.count("block_path_afforded("), 5)

    def test_the_smallest_allocation_splits_the_two_inputs_across_the_line(self):
        # The CLI grants
        # `limit - MEMORY_RESERVE`, so a `512m` container affords 128 MiB
        # against a line of 106.03 MiB at 24 MiB blocks and 522.03 at 128. The
        # bottom row of the axis therefore carries one cell of each path, which
        # is exactly the case a table printing them as one series gets wrong.
        token, limit = measure.RESERVE_LIMITS[0]
        self.assertEqual(token, "512m")
        granted = limit - measure.LIBRARY_MEMORY_RESERVE
        paths = {
            label: measure.block_path_afforded(unit, granted)
            for _name, label, unit in measure.RESERVE_FLAGLESS_INPUTS
        }
        self.assertEqual(paths, {"24 MiB blocks": True, "128 MiB blocks": False})

    # -- the charge model, and the cells it was seeded from ----------------

    #: Readings from the reserve constant's five-build grid, off
    #: `control_xz128` at a 384 MiB reserve (`docs/design/decisions.md`,
    #: "I/O, memory and parallelism"): reader count, worst rep in MiB, the
    #: budget the run reported under the charge that build carried, and the
    #: residual computed by hand against that charge.
    #:
    #: The model is *seeded* from them rather than fitted to them: every number
    #: in the middle two columns is arithmetic the harness does at the cell,
    #: and this is the check that it reproduces what was computed by hand — up
    #: to the one unit that build's charge double-counted, which is a statement
    #: the seeding makes rather than a reason to re-seed.
    SEED_UNIT = 128 << 20
    SEED_CELLS = (
        (2, 802.1, 532.1, 14.0),
        (3, 939.9, 798.1, 13.8),
        (4, 1077.7, 1064.1, 13.6),
        (5, 1342.6, 1330.2, 12.4),
        (6, 1607.5, 1596.2, 11.3),
    )

    def test_the_model_reproduces_the_readings_it_was_seeded_from(self):
        for jobs, worst, budget, residual in self.SEED_CELLS:
            with self.subTest(jobs=jobs):
                billed, pool, unnamed = measure.charge_model(
                    self.SEED_UNIT, jobs, worst * measure.MIB
                )
                # The seed's budget column is what that build's rule resolved:
                # two units a reader, with the pool unbilled. Today's charge
                # bills the pool and one unit a reader, so today's bill is that
                # column plus the pool less one unit — and the residual computed
                # by hand rises by exactly that unit, which is the double count
                # seen from the readings rather than from the source.
                self.assertAlmostEqual(
                    (billed - pool + jobs * self.SEED_UNIT) / measure.MIB, budget, places=1
                )
                self.assertEqual(pool, measure.pool_bytes(self.SEED_UNIT, jobs))
                self.assertAlmostEqual(
                    unnamed / measure.MIB, residual + self.SEED_UNIT / measure.MIB, places=1
                )
                self.assertIsNone(
                    measure.charge_model_problem(self.SEED_UNIT, jobs, worst * measure.MIB)
                )

    def test_the_pool_term_is_named_rather_than_left_in_the_remainder(self):
        # The finding the check exists to surface, and what the charge bills: at
        # two readers of a 128 MiB block file the pool retains 384 MiB no
        # per-reader term carries, which is nearly three times the remainder
        # left over. Folded into that remainder it would read as a term nothing
        # accounts for, and as a breach on a 512 MiB-block file.
        jobs, worst, _, _ = self.SEED_CELLS[0]
        billed, pool, unnamed = measure.charge_model(
            self.SEED_UNIT, jobs, worst * measure.MIB
        )
        self.assertGreater(pool, 2 * unnamed)
        self.assertEqual(pool, (measure.LIBRARY_POOL_DEPTH - 1) * self.SEED_UNIT)
        # And it never goes away: past the depth the pool clamps to, every
        # further reader brings a slot of it, which is what stops it being a
        # floor.
        self.assertEqual(
            measure.pool_bytes(self.SEED_UNIT, measure.LIBRARY_POOL_DEPTH),
            (measure.LIBRARY_POOL_DEPTH - 1) * self.SEED_UNIT,
        )
        self.assertEqual(measure.pool_bytes(self.SEED_UNIT, 99), 98 * self.SEED_UNIT)

    def test_an_over_bill_is_a_fault_and_says_which_way_it_went(self):
        # The half a grid search over reserve constants cannot report: a charge
        # that is too large shows up there as headroom.
        billed = measure.charge_bytes(self.SEED_UNIT, 4)
        fault = measure.charge_model_problem(self.SEED_UNIT, 4, billed - 32 * measure.MIB)
        assert fault is not None
        self.assertEqual(fault.band, measure.BAND_OVER_BILL)
        self.assertIn("over-billed", fault.text)
        self.assertIn("fewer readers than the allocation affords", fault.text)
        # And it refutes: the `bound` band alone is released, an over-bill
        # carrying no remedy the sitting discharges.
        self.assertTrue(fault.refutes)

    def test_a_remainder_above_the_reserve_is_a_fault_and_names_the_constant(self):
        billed = measure.charge_bytes(self.SEED_UNIT, 4)
        held = billed + measure.LIBRARY_MEMORY_RESERVE + measure.MIB
        fault = measure.charge_model_problem(self.SEED_UNIT, 4, held)
        assert fault is not None
        self.assertEqual(fault.band, measure.BAND_RULE)
        self.assertIn("unnamed", fault.text)
        self.assertIn("MEMORY_RESERVE", fault.text)
        self.assertIn("rule does not hold", fault.text)
        # And exactly at the reserve it is the *inner* fault only: the criterion
        # is what the constant promises, not a margin inside it, so the rule
        # still holds there and the bound is what is named.
        at_reserve = measure.charge_model_problem(
            self.SEED_UNIT, 4, billed + measure.LIBRARY_MEMORY_RESERVE
        )
        assert at_reserve is not None
        self.assertIn("MEMORY_UNPOOLED_BOUND", at_reserve.text)
        self.assertNotIn("rule does not hold", at_reserve.text)

    def test_the_bound_band_alone_is_released(self):
        # A fault carries its band so the verdict can read them by name. A bound
        # fault is released — the remainder is inside `MEMORY_RESERVE` and the
        # sitting re-derives the constant it overran — and every other band
        # refutes, the over-bill included: it cannot be apparatus scatter and
        # it leaves the sitting nothing to repair.
        billed = measure.charge_bytes(self.SEED_UNIT, 4)
        bars = {
            measure.BAND_OVER_BILL: billed - measure.MIB,
            measure.BAND_BOUND: billed + measure.LIBRARY_MEMORY_UNPOOLED_BOUND + measure.MIB,
            measure.BAND_RULE: billed + measure.LIBRARY_MEMORY_RESERVE + measure.MIB,
        }
        for band, held in bars.items():
            with self.subTest(band=band):
                fault = measure.charge_model_problem(self.SEED_UNIT, 4, held)
                assert fault is not None
                self.assertEqual(fault.band, band)
                self.assertEqual(fault.refutes, band != measure.BAND_BOUND)

    def test_a_band_with_no_stance_recorded_refutes_and_is_reported(self):
        # The rule is an enumeration, so a fault line added later does not
        # inherit the released half by being unmentioned. `refutes` defaults to
        # refuting, and `--check` names the band until somebody records a
        # stance for it.
        self.assertIsNotNone(measure.band_refutes("a-line-nobody-has-argued"))
        self.assertTrue(measure.ChargeFault("a-line-nobody-has-argued", "…").refutes)
        self.assertEqual(measure.charge_band_problems(), [])
        with unittest.mock.patch.object(measure, "BAND_INVENTED", "invented", create=True):
            reported = measure.charge_band_problems()
        self.assertEqual(len(reported), 1, reported)
        self.assertIn("BAND_INVENTED", reported[0])
        self.assertIn("BAND_STANCE", reported[0])

    def test_every_refuting_band_says_why_it_refutes(self):
        # The verdict prints the band's clause beside the cells, so a stance
        # cannot be recorded without the reason being written in the same place
        # — where one sentence over the whole refuting stanza would assert a
        # single band's reason over every cell in it.
        for band, why in measure.BAND_STANCE.items():
            with self.subTest(band=band):
                if why is None:
                    self.assertIsNone(measure.band_refutes(band))
                else:
                    self.assertTrue(why.strip())
                    self.assertEqual(measure.band_refutes(band), why)
        self.assertIsNone(measure.BAND_STANCE[measure.BAND_BOUND])

    def test_the_bound_is_re_derived_on_the_grid_it_was_read_off(self):
        # The shipped 256 MiB is the next 64 MiB step above the five-build
        # grid's 238.6 MiB worst remainder. The re-derivation is that arithmetic
        # over whatever sitting is in hand, which is what lets a cell above the
        # bound publish with its finding instead of owing a re-take.
        self.assertEqual(
            measure.rederived_unpooled_bound(238.6 * measure.MIB),
            measure.LIBRARY_MEMORY_UNPOOLED_BOUND,
        )
        # On a step exactly, the step itself covers it; above it, the next one.
        step = measure.UNPOOLED_BOUND_STEP
        self.assertEqual(measure.rederived_unpooled_bound(4 * step), 4 * step)
        self.assertEqual(measure.rederived_unpooled_bound(4 * step + 1), 5 * step)
        # And it is never zero: a sitting whose worst remainder is negative is
        # an over-bill throughout, not a bound of nothing.
        self.assertEqual(measure.rederived_unpooled_bound(-1.0), step)

    def test_the_two_ceilings_are_distinguishable_at_every_cell(self):
        # The reason for two lines rather than one: between them the
        # allocation holds and the number the count is predicted against is
        # wrong; above the outer one the allocation does not. A single threshold
        # cannot say which, whichever of the two it is set at.
        billed = measure.charge_bytes(self.SEED_UNIT, 4)
        inner = measure.LIBRARY_MEMORY_UNPOOLED_BOUND
        outer = measure.LIBRARY_MEMORY_RESERVE
        self.assertLess(inner, outer)
        # Exactly at the bound: inside both, so no fault at all.
        self.assertIsNone(measure.charge_model_problem(self.SEED_UNIT, 4, billed + inner))
        between = measure.charge_model_problem(self.SEED_UNIT, 4, billed + inner + measure.MIB)
        assert between is not None
        self.assertIn("finding about the bound, not the rule", between.text)
        above = measure.charge_model_problem(self.SEED_UNIT, 4, billed + outer + measure.MIB)
        assert above is not None
        self.assertIn("rule does not hold", above.text)

    def test_the_mirrored_pool_depth_and_constants_are_the_librarys_own(self):
        # All three are hardcoded on `QUERY_SUBSTREAM_CAP`'s argument, so the
        # mirror is checked here rather than trusted. The two byte constants
        # especially: they are the model's two upper bounds, so one that moved
        # in the library and not here would check the rule against a promise it
        # no longer makes.
        src = (measure.REPO / "pgdump_query/src/io.rs").read_text()
        self.assertIn(f"const POOL_DEPTH: usize = {measure.LIBRARY_POOL_DEPTH};", src)
        self.assertIn(
            f"pub const MEMORY_RESERVE: u64 = {measure.LIBRARY_MEMORY_RESERVE >> 20} << 20;",
            src,
        )
        self.assertIn(
            "pub const MEMORY_UNPOOLED_BOUND: u64 = "
            f"{measure.LIBRARY_MEMORY_UNPOOLED_BOUND >> 20} << 20;",
            src,
        )
        # And the margin predicts with the second rather than the first.
        # *Rejected:* one number for both — the criterion is then applied twice,
        # and the doubling is invisible in every reading the harness takes.
        self.assertIn("saturating_sub(MEMORY_UNPOOLED_BOUND)", src)

    def test_the_recommendation_and_the_affordability_charge_are_one_number(self):
        # What `reader_bytes`' docstring asserts, and what the model rests on:
        # the charge the rule solves an allowance against is the charge the gate
        # then compares a budget to. Split, they are two numbers 7 MiB apart
        # and the mirror is a model of neither. Both come off one composition
        # site, which is what is pinned here.
        src = (measure.REPO / "pgdump_query/src/io.rs").read_text()
        self.assertIn(
            "fn default_worker_memory(&self) -> Option<WorkerMemory> {\n"
            "        self.budget.block_worker_memory()",
            src,
        )
        self.assertIn(
            "fn affordable(&self, chunk_bytes: u64, decode_bytes: u64, budget: u64) -> bool {\n"
            "        self.worker_memory(chunk_bytes, decode_bytes).at(1) <= budget",
            src,
        )

    def test_the_library_bills_the_pool_term_the_model_names(self):
        # `charge_bytes` is `WorkerMemory::at` mirrored, so the pool term's
        # shape has to be the library's: a pool that clamped somewhere else
        # would make every cell of the check arithmetic about a rule nothing
        # implements.
        src = (measure.REPO / "pgdump_query/src/io.rs").read_text()
        self.assertIn(
            "(self.pool_depth.max(workers).saturating_sub(1) as u64)"
            ".saturating_mul(self.pool_unit)",
            src,
        )
        self.assertIn(".pooling(self.unit as u64, POOL_DEPTH)", src)

    # -- the resolution, read back off the run -----------------------------

    def test_the_resolved_arrangement_is_read_off_the_runs_own_log(self):
        # The count a flagless run resolves exists nowhere else: the harness
        # cannot compute it without reimplementing the rule under test.
        log = (
            "2026-09-11T03:33:02Z  INFO running inside a stated memory allocation "
            "limit_bytes=536870912 limit_read_from=/sys/fs/cgroup/memory.max "
            "jobs_flag=(not stated) memory_flag=(not stated)\n"
            "2026-09-11T03:33:02Z  INFO resolved the arrangement "
            "jobs=3 (recommended by the source; lowered from 24 by the allocation) "
            "memory_bytes=204576096 (discovered: /sys/fs/cgroup/memory.max states a "
            "limit of 536870912 byte(s))\n"
            "2026-09-11T03:33:02Z  INFO preamble scan started bytes=3221227790 "
            "chunk_size=1048576 jobs=3 memory_bytes=204576096\n"
            "2026-09-11T03:33:18Z  INFO scan started bytes=3221227790 resumed_from=425 "
            "chunk_size=1048576 jobs=3 memory_bytes=204576096\n"
            "maxrss_kib=485786\n"
        )
        self.assertEqual(
            measure.parse_resolution(log),
            {"resolved_jobs": "3", "resolved_budget": "204576096"},
        )

    def test_the_line_it_reads_still_carries_that_pair_in_that_order(self):
        # The flagless axis rests on a log line, which is the one input here
        # that is not a constant: a field renamed or reordered in `stream.rs`
        # turns every reader count into a missing key, and the figure would fail
        # a sitting in rather than at its first second.
        src = (measure.REPO / "pgdump_query/src/stream.rs").read_text()
        event = src[: src.index('"scan started"')]
        jobs = event.rindex("jobs = ")
        budget = event.rindex("memory_bytes = ")
        self.assertLess(jobs, budget)
        self.assertNotIn("\n\n", event[jobs:])

    def test_a_run_that_reports_no_arrangement_reports_nothing(self):
        # Every shape that is not a scan, `dd` and the decode instrument among
        # them. `{}` rather than a guess: a renderer that wants the pair says so
        # by failing on its absence.
        self.assertEqual(measure.parse_resolution("real 0m1.000s\n"), {})

    # -- the fit ------------------------------------------------------------

    def test_the_fit_recovers_a_line_exactly(self):
        fixed, per_reader = measure._least_squares([(1, 110.0), (2, 120.0), (4, 140.0)])
        self.assertAlmostEqual(fixed, 100.0)
        self.assertAlmostEqual(per_reader, 10.0)

    def test_a_fit_through_one_reader_count_is_refused(self):
        # An intercept asserted as a measurement is the extrapolation the
        # evidence skill's second rule names, and the failure this figure
        # exists to stop repeating.
        with self.assertRaises(ValueError):
            measure._least_squares([(3, 474.0), (3, 503.0)])

    # A block size of zero makes `pool_bytes` vanish, so the three guard tests
    # below are about the guard alone and not about `_depooled`'s subtraction; the
    # de-pooling has its own tests after them.
    NO_POOL = 0

    def test_the_publication_guard_sits_in_front_of_the_arithmetic(self):
        # Every fitted line in this harness crosses `_fit_or_secant`, so
        # a fourth call site cannot skip the rule by not remembering it.
        fixed, slope = measure._fit_or_secant(
            [(1, 110.0), (2, 120.0), (4, 140.0)], self.NO_POOL
        )
        self.assertAlmostEqual(fixed, 100.0)
        self.assertAlmostEqual(slope, 10.0)

    def test_below_the_guard_the_slope_survives_and_the_intercept_does_not(self):
        # The slope is the same number either way — at two distinct abscissae
        # the least-squares slope *is* the secant between the group means — so
        # withholding the intercept withholds the model's claim and nothing the
        # axis measured.
        points = [(1, 110.0), (1, 114.0), (4, 200.0)]
        fixed, slope = measure._fit_or_secant(points, self.NO_POOL)
        self.assertIsNone(fixed)
        self.assertAlmostEqual(slope, (200.0 - 112.0) / 3)
        self.assertAlmostEqual(slope, measure._least_squares(points)[1])

    def test_a_secant_needs_two_points_as_much_as_a_fit_does(self):
        with self.assertRaises(ValueError):
            measure._fit_or_secant([(3, 474.0), (3, 503.0)], self.NO_POOL)

    # -- the kink the fit must not cross (`_depooled`) ----------------------

    def test_the_term_taken_off_each_point_is_the_librarys_own(self):
        # Not a second spelling of the pool's arithmetic: `_depooled` subtracts
        # `pool_bytes`, which is `WorkerMemory::pool_bytes` mirrored, so the
        # thing removed from the ordinate is exactly the thing the charge bills.
        unit = measure.RESERVE_MECHANISM_UNIT
        for jobs in (1, 2, 3, 4, 5, 11, 29):
            with self.subTest(jobs=jobs):
                (_, left), = measure._depooled([(jobs, 1000.0)], unit)
                self.assertAlmostEqual(
                    1000.0 - left, measure.pool_bytes(unit, jobs) / measure.MIB
                )

    def test_the_fit_recovers_the_two_unknown_terms_across_the_kink(self):
        # The synthetic mechanism, exactly: a fixed cost, a per-reader cost and
        # the pool's retention list, read at counts either side of `POOL_DEPTH`.
        # A line over the remainder recovers the two terms the sitting does not
        # know; a line over the resident set does not, and cannot, because the
        # quantity it is fitting has a bend in it.
        unit, fixed, per_reader = measure.RESERVE_MECHANISM_UNIT, 120.0, 31.0
        counts = [1, 2, 11, 12, 20, 29]
        held = [
            (k, fixed + per_reader * k + measure.pool_bytes(unit, k) / measure.MIB)
            for k in counts
        ]
        got_fixed, got_per_reader = measure._fit_or_secant(held, unit)
        self.assertAlmostEqual(got_fixed, fixed)
        self.assertAlmostEqual(got_per_reader, per_reader)
        raw_fixed, raw_per_reader = measure._least_squares(held)
        # 25 MiB of it at these counts, which is the bend measured from the true
        # fixed term; the published 49 MiB is the same bias measured from the
        # above-kink regime's own intercept of minus one unit.
        self.assertGreater(raw_fixed - fixed, 20.0)
        self.assertGreater(raw_per_reader, per_reader)

    def test_the_bias_a_straight_line_across_the_kink_carries(self):
        # The arithmetic behind the published bias, over the charge's held-unit
        # values alone, at
        # the reader counts the registered axis resolves. It is why the
        # subtraction exists, and it is checkable without a sitting.
        for unit, counts, bias, slope_units in (
            (24 << 20, [1, 2, 11, 12, 20, 29], 49.0, 1.90),
            (128 << 20, [1, 2, 4, 6], 421.0, 1.37),
        ):
            with self.subTest(unit=unit):
                held = [
                    (k, (k * unit + measure.pool_bytes(unit, k)) / measure.MIB)
                    for k in counts
                ]
                intercept, slope = measure._least_squares(held)
                # The regime the axis mostly sits in holds `2k - 1` units, so a
                # faithful line there has an intercept of minus one unit.
                self.assertAlmostEqual(
                    intercept + unit / measure.MIB, bias, delta=1.0
                )
                self.assertAlmostEqual(slope / (unit / measure.MIB), slope_units, places=2)
                # And the remainder is a line through the origin with a slope of
                # exactly one unit, at every count on both sides of the bend.
                self.assertEqual(
                    [round(y, 6) for _, y in measure._depooled(held, unit)],
                    [round(k * unit / measure.MIB, 6) for k in counts],
                )

    def test_no_line_is_fitted_without_crossing_the_de_pooling(self):
        # The same argument `RESERVE_FIT_MIN_COUNTS` makes about the guard: the
        # subtraction is a property of the mechanism, not of one renderer, so
        # `_least_squares` is reached through `_fit_or_secant` and nowhere else.
        source = (measure.REPO / "scripts/measure.py").read_text()
        calls = [
            ln for ln in source.splitlines()
            if "_least_squares(" in ln and not ln.lstrip().startswith("def ")
        ]
        self.assertEqual(
            [ln.strip() for ln in calls],
            ["fixed, slope = _least_squares(remainder)"],
            "a fitted line that does not cross `_depooled` publishes the charge's own bend "
            "as an intercept (`_depooled`)",
        )

    # -- the edges it owes --------------------------------------------------

    def test_the_only_reading_it_shares_is_peak_rsss_control_row(self):
        """What it owes `peak-rss` when it is published, and what it does not owe
        `rss-attribution`.

        The edge is mechanical rather than asserted: a shared reading is two
        figures keying the same run, so the overlap is computable. `peak-rss`
        carries exactly one of these runs — the serial-default baseline.
        `rss-attribution` carries none, and that is a reading of the leg set
        rather than an omission: its every leg runs over the two block-count
        shapes whose axis it is, where nothing here does. What the three share
        is a *sitting*, which one sweep takes."""
        mine = {
            spec.key("")
            for spec in (
                *measure._reserve_specs(),
                *measure._reserve_flagless_specs(),
                *(s for _, s in measure._reserve_mechanism_specs()),
                *measure._reserve_step_specs(),
                measure._RESERVE_BASELINE,
            )
        }
        peak = {spec.key("") for spec in _peak_rss_specs()}
        attribution = {
            spec.key("")
            for _, small, big in measure._attribution_specs()
            for spec in (small, big)
        }
        self.assertEqual(mine & peak, {measure._RESERVE_BASELINE.key("")})
        self.assertEqual(mine & attribution, set())


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
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 4"
        ):
            reported = measure.pinned_count_problems()
        self.assertTrue(reported)
        self.assertTrue(all("states --jobs 4" in line for line in reported))

    def test_the_axis_families_are_exempt(self):
        # They are the exemption, so nothing this reports may name one.
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 4"
        ):
            reported = measure.pinned_count_problems()
        for line in reported:
            with self.subTest(line=line):
                self.assertFalse(line.startswith(measure.JOBS_AXIS))
                self.assertFalse(line.startswith("decode-"))

    def test_check_fails_on_a_shape_that_drifts(self):
        with unittest.mock.patch.object(
            measure, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 4"
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

    It is the register's first figure that runs no `pgdt` at all, its first
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
        # No `pgdt` runs here, so none of the library's own paths can move this
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
    #: else. Each is a figure that holds more than the recorded container
    #: affords, or may: N decoded blocks at once, or statistics nothing bills.
    #: The departure is stated in that figure's own table.
    MEMORY_DEPARTURES = {
        "xz-decode-scaling",
        "parallel-scan-throughput",
        "parallel-peak-rss",
        # The reserve's largest stated budget is itself larger than the
        # recorded 512 MB, so the container that holds a run of it cannot be
        # the recorded one; its `.xz` legs hold N decoded blocks besides.
        "reserve",
        # Its spec gives it a generous limit of its own, so that no kill
        # interrupts the phase while what statistics hold is unbounded (`KD28`).
        "statistics-gathering",
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


class InstrumentReport(unittest.TestCase):
    """The introspection build's own report, out of the file it writes.

    The transport is a file rather than a block on a shared stream, so the
    parse is `parse_reported`'s and the questions here are about the channel:
    that the variable naming it matches the binary's, that a leg declaring the
    instrument fails loudly when no report arrives, and that the readings are
    filed per rep rather than collapsed to the last one.
    """

    #: A report shaped as the instrument writes one: the two scope keys, the
    #: prose note between them, the totals, and the raw XML underneath.
    REPORT = (
        "instrument=counting-allocator\n"
        "live_scope=rust-global-alloc\n"
        "live_bytes=76876\n"
        "live_peak_bytes=209822121\n"
        "# `live_*` counts only what passed through Rust's `GlobalAlloc`.\n"
        "# their difference is not retention.\n"
        "glibc_scope=whole-process\n"
        "mallinfo_arena=131768320\n"
        "malloc_system_max=404201472\n"
        "# malloc_info\n"
        '<system type="max" size="135168"/>\n'
        "# end malloc_info\n"
    )

    def test_it_reads_the_keys_and_ignores_the_prose(self):
        got = measure.parse_reported(self.REPORT)
        self.assertEqual(got["live_peak_bytes"], "209822121")
        self.assertEqual(got["malloc_system_max"], "404201472")
        # The scope labels are part of the report, not commentary on it: a
        # consumer differencing a Rust-only count against a whole-process one
        # is what they exist to make visible.
        self.assertEqual(got["live_scope"], "rust-global-alloc")
        self.assertEqual(got["glibc_scope"], "whole-process")

    def test_the_harnesss_own_reading_cannot_reach_it(self):
        # `maxrss_kib` is `rss_wrapper`'s, and it is a `key=value` line by this
        # same grammar. The file is why it cannot land here: one writer by
        # construction, where the bracketed stderr block it replaced was a
        # framing protocol over a stream two processes wrote to.
        self.assertNotIn("maxrss_kib", measure.parse_reported(self.REPORT))

    def test_the_variable_matches_the_binarys(self):
        # Two constants in two languages. A rename on one side that missed the
        # other would leave the instrument writing nowhere the harness looks —
        # which is now an error rather than an empty dict, but only because the
        # name is right.
        source = (measure.REPO / "pgdt/src/introspect.rs").read_text()
        self.assertIn(f'pub const OUT_VAR: &str = "{measure.INSTRUMENT_OUT_VAR}";', source)
        # `datafusion-cli-pgdump`'s introspection build reads the same one.
        source = (measure.REPO / "datafusion-cli-pgdump/src/pgdump.rs").read_text()
        self.assertIn(
            f'const INTROSPECT_OUT_VAR: &str = "{measure.INSTRUMENT_OUT_VAR}";', source
        )

    def test_no_leg_that_does_not_declare_it_is_given_the_variable(self):
        # An env var leaves the command shape identical, which is the whole
        # reason it is not a flag — but it must still reach only the legs that
        # asked for it, or a default build's runs acquire a mount for nothing.
        self.assertFalse(any(s.instrument for s in measure._reserve_flagless_specs()))

    def test_a_declaring_leg_is_pointed_at_the_instrument_build_and_nothing_else_is(self):
        # A leg that declares the instrument and runs the default binary
        # produces no report, which `_read_instrument` can only report as "one
        # of three apparatus faults" — and the pairing is the one of the three
        # that can be checked before the sitting starts.
        for spec in measure._reserve_instrument_specs():
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.instrument, spec.binary == "introspect")
        for spec in [
            *measure._reserve_flagless_specs(),
            *(s for _, s in measure._reserve_mechanism_specs()),
            *measure._reserve_step_specs(),
            *measure._reserve_specs(),
        ]:
            with self.subTest(leg=spec.label):
                self.assertNotEqual(spec.binary, "introspect")
                self.assertFalse(spec.instrument)

    def test_a_build_that_names_no_instrument_is_refused(self):
        # The mirror of `binary_allocator`'s refusal, pointed the other way:
        # there an instrumented build must not be timed, here a leg that
        # declares the instrument must carry one.
        with unittest.mock.patch.object(
            measure, "run", return_value="pgdt 0.1.0 (allocator: system)\n"
        ):
            with self.assertRaises(RuntimeError) as caught:
                measure.binary_instrument(Path("/pgdt"))
        self.assertIn("--features introspect", str(caught.exception))

    def test_the_instrument_build_is_read_back_and_names_itself(self):
        with unittest.mock.patch.object(
            measure,
            "run",
            return_value="pgdt 0.1.0 (allocator: system) (instrument: counting-allocator)\n",
        ):
            self.assertEqual(
                measure.binary_instrument(Path("/pgdt")), "counting-allocator"
            )

    def test_the_dictionary_term_is_inside_the_reader_charge(self):
        # The one term an attribution adds back by hand: `liblzma` allocates it
        # through C `malloc`, so the counting allocator cannot see it and glibc
        # cannot separate it. It is part of what `reader_bytes` already charges,
        # never a term beside it — a decomposition that added it twice would
        # under-report retention by 8 MiB a reader.
        self.assertLess(measure.XZ_DICT_BYTES, measure.XZ_DECODE_FOOTPRINT)
        self.assertEqual(measure.XZ_DICT_BYTES, 8 << 20)
        # What is left of the footprint once the dictionary comes out is
        # `xz_seek`'s 1 MiB input chunk plus `liblzma`'s own 34,592 B of state.
        self.assertEqual(
            measure.XZ_DECODE_FOOTPRINT - measure.XZ_DICT_BYTES,
            measure.LIBRARY_CHUNK_BYTES + 34_592,
        )

    def _session(self, out_root):
        cfg = measure.Config()
        session = measure.Session(
            cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None, out_root
        )
        session.figure_id = "reserve"
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        return session

    TIMED = "maxrss_kib=68228\n\nreal\t0m0.012s\nuser\t0m0.008s\nsys\t0m0.004s\n"
    NO_KILL = "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\noom_group_kill 0\n"

    def _instrumented(self):
        spec = dataclasses.replace(measure._reserve_flagless_specs()[0], instrument=True)
        return spec

    def _run(self, session, spec, write_report: bool):
        """One fake run, optionally writing the report the container would."""

        def fake_run(argv, **_kwargs):
            if write_report:
                out = [a for a in argv if a.startswith(f"{measure.INSTRUMENT_OUT_VAR}=")]
                self.assertEqual(len(out), 1, argv)
                name = out[0].split("=", 1)[1].rpartition("/")[2]
                (session.out_root / measure.INSTRUMENT_DIR / name).write_text(self.REPORT)
            return subprocess.CompletedProcess(
                argv, 0, stdout="", stderr=self.TIMED + self.NO_KILL
            )

        with unittest.mock.patch.object(measure.subprocess, "run", fake_run):
            return session.time_run(spec)

    def test_a_declared_leg_that_reports_nothing_is_an_error(self):
        # The three apparatus faults this catches — built without the feature,
        # pointed at the default binary, a write that failed — all used to read
        # as an empty dict, and an empty column an hour later is what that
        # looks like.
        with tempfile.TemporaryDirectory() as tmp:
            session = self._session(Path(tmp))
            with self.assertRaises(RuntimeError) as caught:
                self._run(session, self._instrumented(), write_report=False)
        self.assertIn("declares the instrument", str(caught.exception))
        self.assertIn("--features introspect", str(caught.exception))

    def test_the_report_is_read_back_and_kept_beside_the_readings(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = self._session(Path(tmp))
            spec = self._instrumented()
            self._run(session, spec, write_report=True)
            self.assertEqual(session._last_instrument["live_peak_bytes"], "209822121")
            record = session.records[0]
            self.assertEqual(record["instrument"]["glibc_scope"], "whole-process")
            # The file stays, under the sitting's own directory, because the
            # `malloc_info` XML in it is per-arena detail no summary carries.
            kept = Path(tmp) / record["instrument_report"]
            self.assertTrue(kept.exists())
            self.assertIn("malloc_info", kept.read_text())

    def test_the_command_shape_is_untouched_by_the_variable(self):
        # The argv a figure records must be the argv it would run without the
        # instrument; the variable and the mount are `nerdctl` arguments.
        plain = measure._script(measure._reserve_flagless_specs()[0].command)
        self.assertEqual(plain, measure._script(self._instrumented().command))
        self.assertNotIn(measure.INSTRUMENT_OUT_VAR, plain)

    def test_the_readings_are_filed_per_rep_and_not_collapsed(self):
        # `reported` keeps one dict per spec, the last rep's, because what it
        # holds is identical across reps. `live_peak_bytes` is not: it is a
        # reading, and a spread of them is what an attribution reads.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(dry_run=True)
            session = measure.Session(
                cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None, Path(tmp)
            )
            session.figure_id = "reserve"
            session.input_path = lambda name, regime: Path("/dev/null")
            spec = self._instrumented()
            session.sweep("reserve", [spec], reps=3)
            peaks = [r["live_peak_bytes"] for r in session.instrument_reports("reserve", spec)]
        self.assertEqual(len(peaks), 3)
        self.assertGreater(len(set(peaks)), 1, peaks)

    def test_a_leg_with_no_instrument_is_absent_rather_than_empty(self):
        # The same split `get_rss` makes: an absent key means this leg carries
        # no instrument, an empty list means every reading it took was
        # censored.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(dry_run=True)
            session = measure.Session(
                cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None, Path(tmp)
            )
            session.figure_id = "reserve"
            session.input_path = lambda name, regime: Path("/dev/null")
            spec = measure._reserve_flagless_specs()[0]
            session.sweep("reserve", [spec], reps=1)
            with self.assertRaises(KeyError):
                session.instrument_reports("reserve", spec)


class Staleness(unittest.TestCase):
    def test_a_file_under_a_declared_directory_counts(self):
        touched = dict(
            (f.id, hits) for f, hits in measure.figures_touched(["pgdump_query/src/map.rs"])
        )
        self.assertIn("scan-throughput-warm", touched)
        self.assertIn("statistics-gathering", touched)

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


class CommentOnlyCommits(unittest.TestCase):
    """The one oracle the harness decides for itself.

    It replaced three consecutive commits on `main` whose whole content was
    telling `--stale` that the commit before them had moved nothing, so what
    these tests hold is the direction it fails in: anything the scanner cannot
    place is *not* comment-only, because an oracle that has to be trusted is
    worth less than the red it clears.
    """

    def test_a_rust_doc_comment_is_a_comment(self):
        got = measure._rust_comment_lines("/// doc\n//! inner\n// plain\nfn f() {}\n")
        self.assertEqual(got, {1, 2, 3})

    def test_a_blank_line_carries_nothing(self):
        self.assertEqual(measure._rust_comment_lines("fn f() {}\n\n"), {2})

    def test_a_whole_line_block_comment_is_read(self):
        got = measure._rust_comment_lines("/*\n * prose\n */\nfn f() {}\n")
        self.assertEqual(got, {1, 2, 3})

    def test_a_block_delimiter_beside_code_defeats_the_scanner(self):
        # Where the comment starts is a lexing question -- a `/*` inside a
        # string literal reads the same -- so the file is not read at all.
        self.assertIsNone(measure._rust_comment_lines('let s = "/*";\n'))

    def test_an_unclosed_block_defeats_the_scanner(self):
        self.assertIsNone(measure._rust_comment_lines("/*\n prose\n"))

    def test_a_python_hash_and_docstring_are_comments(self):
        src = '# lead\ndef f():\n    """Doc.\n\n    More.\n    """\n    return 1\n'
        self.assertEqual(measure._python_comment_lines(src), {1, 3, 4, 5, 6})

    def test_an_assigned_triple_quote_is_a_value(self):
        # `x = """..."""` is data the program reads, not prose, so the scanner
        # refuses the file rather than excusing an edit to it.
        self.assertIsNone(measure._python_comment_lines('x = """body"""\n'))

    def test_markdown_is_comment_all_the_way_down(self):
        self.assertEqual(measure.comment_lines("docs/x.md", "a\nb\n"), {1, 2})

    def test_a_suffix_the_oracle_does_not_read_is_unanalyzable(self):
        self.assertIsNone(measure.comment_lines("Cargo.toml", "[package]\n"))

    def test_hunk_headers_give_both_sides_ranges(self):
        diff = "@@ -3,2 +3,0 @@\n@@ -10 +8,3 @@\n"
        self.assertEqual(measure.hunk_ranges(diff), ([3, 4, 10], [8, 9, 10]))

    def test_an_unresolvable_commit_is_not_comment_only(self):
        self.assertFalse(measure.comment_only_commit("no-such-rev", "pgdump_query/src/io.rs"))

    def test_a_path_the_commit_did_not_touch_is_not_comment_only(self):
        # Silence is not an excuse: saying "comment-only" about a path with no
        # hunks in it would be answering a question nobody asked.
        self.assertFalse(measure.comment_only_commit("HEAD", "docs/design/measurements.md.missing"))


class CommentOnlyStaleness(unittest.TestCase):
    """What the oracle does to `--stale`'s two path predicates."""

    ACKS = ()

    def test_a_comment_only_commit_settles_a_path_with_no_entry(self):
        got = measure.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
            comment_only=lambda c, p: True,
        )
        self.assertEqual(got, ["scripts/generate_perf_data.py"])

    def test_a_comment_only_commit_is_not_what_holds_a_path_red(self):
        # An inert entry names the commits actually blocking it, and a commit
        # the oracle settled is not one of them.
        acks = (measure.Acknowledged(commit="aaa", figures=("nested-end-to-end",), why="additive"),)
        got = measure.inert_excuses(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "bbb", "ccc"]},
            acks,
            comment_only=lambda c, p: c == "bbb",
        )
        self.assertEqual(got, [("scripts/generate_perf_data.py", ["aaa"], ["ccc"])])

    def test_the_oracle_is_on_by_default(self):
        # The weaker answer must not be reachable by forgetting an argument:
        # a caller that passes no oracle gets the real one.
        import inspect

        for fn in (measure.excused_paths, measure.inert_excuses):
            with self.subTest(fn=fn.__name__):
                default = inspect.signature(fn).parameters["comment_only"].default
                self.assertIs(default, measure.comment_only_commit)


class Acknowledgements(unittest.TestCase):
    """A commit that touched a declared path and moved no reading.

    The mechanism's whole risk is that it becomes a way to wave staleness away,
    so what these tests hold are the refusals: a dirty path is never excused, a
    path is excused only when *every* commit that touched it is, and an excuse
    is scoped to the figures it names.
    """

    ACKS = (
        measure.Acknowledged(commit="aaa", figures=("nested-end-to-end",), why="additive"),
        measure.Acknowledged(commit="bbb", figures=(), why="touches no figure's subject"),
    )

    def test_a_path_whose_only_commit_is_excused_is_excused(self):
        got = measure.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, ["scripts/generate_perf_data.py"])

    def test_an_excuse_does_not_reach_a_figure_it_does_not_name(self):
        got = measure.excused_paths(
            "scan-throughput-warm",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_an_empty_figure_list_excuses_every_figure(self):
        got = measure.excused_paths(
            "scan-throughput-warm",
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
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "ccc"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_an_uncommitted_path_is_never_excused(self):
        # There is no commit to point at, so nobody has read the diff.
        got = measure.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            {"scripts/generate_perf_data.py"},
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_a_path_with_no_commits_in_range_is_not_excused(self):
        got = measure.excused_paths(
            "nested-end-to-end", ["scripts/generate_perf_data.py"], {}, set(), self.ACKS
        )
        self.assertEqual(got, [])

    def test_an_inert_entry_is_named_with_what_holds_the_path_red(self):
        # The failure this exists against: an entry that excuses one commit on
        # a path another commit also touched vanishes from every output, which
        # reads as a missing entry and has been mistaken for one.
        got = measure.inert_excuses(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "ccc"]},
            self.ACKS,
        )
        self.assertEqual(got, [("scripts/generate_perf_data.py", ["aaa"], ["ccc"])])

    def test_a_path_nothing_excuses_gets_no_commentary(self):
        got = measure.inert_excuses(
            "nested-end-to-end",
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
    is staged. What must never happen is evicting something the step in hand
    still needs, or thrashing an input the next step wants."""

    def _stager(self, budget: float | None):
        cfg = measure.Config(dry_run=True, warm_budget=budget)
        return measure.Stager(cfg, lambda _msg: None)

    def test_an_input_nothing_wants_again_goes_first(self):
        stager = self._stager(None)
        stager.needs = {"control": [0, 3], "arrays": [1], "composite": [3]}
        stager._staged = {"control": 3 * measure.GIB, "arrays": 3 * measure.GIB}
        stager._make_room(3 * measure.GIB, step=3)
        self.assertNotIn("arrays", stager._staged)
        self.assertIn("control", stager._staged)

    def test_what_the_current_step_needs_is_never_the_victim(self):
        stager = self._stager(3)
        stager.needs = {"control": [0]}
        stager._staged = {"control": 3 * measure.GIB}
        with self.assertRaises(measure.StagingError):
            stager._make_room(3 * measure.GIB, step=0)

    def test_a_budget_too_small_for_one_input_is_an_error(self):
        stager = self._stager(1)
        stager.needs = {}
        with self.assertRaises(measure.StagingError):
            stager._make_room(3 * measure.GIB, step=0)

    def test_room_already_free_evicts_nothing(self):
        stager = self._stager(None)
        stager.needs = {"control": [0, 5]}
        stager._staged = {"control": 3 * measure.GIB}
        stager._make_room(3 * measure.GIB, step=1)
        self.assertEqual(list(stager._staged), ["control"])

    def test_a_split_figure_evicts_its_own_earlier_group(self):
        # The point of a split: the second group's input displaces the first
        # group's, which no later step wants, rather than refusing because the
        # figure as a whole still names it.
        stager = self._stager(None)
        fig = measure.FIGURES_BY_ID["statistics-gathering"]
        stager.plan([fig])
        for gi, group in enumerate(fig.staging_groups):
            for name in group:
                stager.warm_path(name, stager.step_of(fig.id, gi))
            with self.subTest(group=group):
                self.assertTrue(set(group) <= set(stager._staged))
                self.assertLessEqual(stager._warm_bytes(), stager.budget())


class WarmBound(unittest.TestCase):
    """Measurement's RAM is bounded by design: a constant of the apparatus,
    which every figure's warm set is held to here, so a figure that outgrows
    it splits instead of growing the room (`measurements.md`, "The
    apparatus")."""

    def _stager(self, budget: float | None = None):
        cfg = measure.Config(dry_run=True, warm_budget=budget)
        return measure.Stager(cfg, lambda _msg: None)

    def test_the_bound_is_two_full_size_inputs_and_the_slack(self):
        cfg = measure.Config()
        self.assertEqual(measure.WARM_FULL_INPUTS, 2)
        self.assertEqual(measure.WARM_SLACK, 64 * measure.MIB)
        self.assertEqual(
            measure.warm_bound(cfg), int(2 * cfg.size_gib * measure.GIB) + 64 * measure.MIB
        )
        self.assertEqual(self._stager().budget(), measure.warm_bound(cfg))

    def test_the_bound_does_not_follow_the_figures_selected(self):
        # The defect it replaces: a budget computed from the largest figure
        # grew with whatever a slice added, past a quota nothing checked.
        small, every = self._stager(), self._stager()
        small.plan([measure.FIGURES_BY_ID["peak-rss"]])
        every.plan(measure.FIGURES)
        self.assertEqual(small.budget(), every.budget())

    def test_every_warm_set_fits_the_bound(self):
        # Real sizes where the inputs exist, so a generator's overshoot is
        # counted; nominal ones otherwise.
        stager = self._stager()
        stager.plan(measure.EVERY_FIGURE)
        budget = stager.budget()
        for fig in measure.EVERY_FIGURE:
            for gi, group in enumerate(fig.staging_groups):
                with self.subTest(figure=fig.id, group=group):
                    self.assertLessEqual(stager.group_need[(fig.id, gi)], budget)

    def test_a_generator_overshooting_its_target_does_not_break_the_bound(self):
        # The real defect behind the slack: three inputs whose nominal sum was
        # exactly the budget overshot it by 6,799 bytes, and the sweep died
        # twenty minutes in.
        stager = self._stager()
        stager.plan(measure.EVERY_FIGURE)
        self.assertGreater(stager.budget() - max(stager.group_need.values()), 64 * 1024)

    def test_a_split_figure_covers_its_warm_set(self):
        # An input is measured in a second sweep only where a declared
        # subtraction pairs it with that sweep's other input: a repeat nothing
        # reads is a full-size input's reps spent for no reading.
        paired = {
            (s.source, name)
            for fig in measure.EVERY_FIGURE
            for s in fig.subtracts
            for name in s.inputs
        }
        for fig in measure.EVERY_FIGURE:
            if not fig.warm_groups:
                continue
            with self.subTest(figure=fig.id):
                named = [name for group in fig.warm_groups for name in group]
                self.assertEqual(set(named), set(fig.warm_inputs))
                self.assertEqual(len(set(fig.warm_groups)), len(fig.warm_groups))
                for group in fig.warm_groups:
                    self.assertEqual(len(group), len(set(group)))
                for name in {n for n in named if named.count(n) > 1}:
                    for gi, group in enumerate(fig.warm_groups):
                        if name in group:
                            self.assertTrue(
                                any(
                                    measure.subtraction_sweep(fig.id, name, other) == gi
                                    for other in group
                                    if other != name and (fig.id, other) in paired
                                ),
                                f"{name} is in sweep {gi} beside nothing it is paired with",
                            )

    def test_the_figures_the_bound_splits(self):
        # One sweep per input where no reading subtracts one input from
        # another; `nested-end-to-end` measures control beside each nested
        # file, because every reading across its files is against control.
        self.assertEqual(
            {f.id: f.warm_groups for f in measure.EVERY_FIGURE if f.warm_groups},
            {
                "scan-throughput-warm": (("control",), ("large_object",), ("insert_run",)),
                "nested-end-to-end": (("control", "composite"), ("control", "arrays")),
                "statistics-gathering": (
                    ("control",), ("arrays",), ("large_object",), ("insert_run",)
                ),
            },
        )

    def test_every_declared_subtraction_lies_in_one_sweep(self):
        # The rule the split is held to: two inputs a reading subtracts are
        # measured together, whichever figure reads them.
        declared = [(fig.id, s) for fig in measure.EVERY_FIGURE for s in fig.subtracts]
        self.assertTrue(declared)
        for reader, s in declared:
            with self.subTest(reader=reader, source=s.source, inputs=s.inputs):
                source = measure.EVERY_BY_ID[s.source]
                self.assertNotEqual(*s.inputs)
                homes = [g for g in source.sweeps if set(s.inputs) <= set(g)]
                self.assertEqual(len(homes), 1, source.sweeps)
                requires = measure.EVERY_BY_ID[reader].requires
                self.assertTrue(reader == s.source or s.source in requires)

    def test_the_nested_figure_and_its_consumer_declare_what_they_read(self):
        self.assertEqual(
            measure.subtraction_sweep("nested-end-to-end", "control", "composite"), 0
        )
        self.assertEqual(measure.subtraction_sweep("nested-end-to-end", "arrays", "control"), 1)
        self.assertEqual(measure.subtraction_sweep("cross-file-floor", "control", "control43"), 0)

    def test_an_undeclared_cross_input_reading_is_refused(self):
        # composite and arrays are in no one sweep, and nothing declares them.
        for a, b in (("composite", "arrays"), ("control", "control43")):
            with self.subTest(pair=(a, b)):
                with self.assertRaises(ValueError):
                    measure.subtraction_sweep("nested-end-to-end", a, b)
        with self.assertRaises(ValueError):
            measure._per_row_diffs(None, "nested-end-to-end", "composite", "arrays")

    def test_a_declared_pair_no_one_sweep_holds_is_refused(self):
        fig = measure.EVERY_BY_ID["nested-end-to-end"]
        stray = measure.Subtraction("nested-end-to-end", ("composite", "arrays"), "x")
        with unittest.mock.patch.object(fig, "subtracts", (*fig.subtracts, stray)):
            with self.assertRaises(ValueError) as caught:
                measure.subtraction_sweep("nested-end-to-end", "composite", "arrays")
        self.assertIn("0 sweeps", str(caught.exception))

    def test_an_explicit_budget_only_caps_it_lower(self):
        self.assertEqual(self._stager(4).budget(), 4 * measure.GIB)
        self.assertEqual(self._stager(100).budget(), measure.warm_bound(measure.Config()))

    def test_a_warm_set_too_big_for_the_budget_is_refused_before_any_run(self):
        stager = self._stager(1)
        stager.plan(measure.FIGURES)
        problems = stager.preflight(measure.FIGURES)
        self.assertTrue(any("over the" in p for p in problems))


class Staging(unittest.TestCase):
    """A staging failure is the sweep's, not a figure's: what it must never
    leave behind is a partial file recorded as an input."""

    def _cfg(self, root: Path, **kw) -> measure.Config:
        return measure.Config(
            cache_dir=root / "ssd", warm_dir=root / "shm", nvme_dir=root / "nvme", **kw
        )

    def test_preflight_reserves_the_budget_and_gives_it_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stager = measure.Stager(self._cfg(root, warm_budget=1 / 1024), lambda _m: None)
            with unittest.mock.patch.object(
                measure.os, "posix_fallocate", wraps=measure.os.posix_fallocate
            ) as alloc:
                self.assertIsNone(stager.reserve(stager.budget()))
            self.assertEqual(alloc.call_args.args[1:], (0, measure.MIB))
            self.assertEqual(list((root / "shm").iterdir()), [])

    def test_a_reservation_the_quota_refuses_is_a_preflight_problem(self):
        # `statvfs` cannot see a per-user quota, so the free space read fine
        # and the sweep hit EDQUOT staging its eighteenth figure.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stager = measure.Stager(self._cfg(root), lambda _m: None)
            stager.plan([measure.FIGURES_BY_ID["peak-rss"]])
            quota = OSError(122, "Disk quota exceeded")
            with unittest.mock.patch.object(measure.os, "posix_fallocate", side_effect=quota):
                problems = stager.preflight([measure.FIGURES_BY_ID["peak-rss"]])
            self.assertTrue(any("could not reserve" in p for p in problems), problems)
            self.assertFalse((root / "shm" / measure.WARM_RESERVATION).exists())

    def test_what_is_already_staged_is_not_reserved_twice(self):
        with tempfile.TemporaryDirectory() as tmp:
            stager = measure.Stager(self._cfg(Path(tmp)), lambda _m: None)
            stager._staged = {"control": stager.budget()}
            with unittest.mock.patch.object(measure.os, "posix_fallocate") as alloc:
                self.assertIsNone(stager.reserve(stager.budget()))
            alloc.assert_not_called()

    def test_a_failed_copy_leaves_nothing_staged(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = self._cfg(root)
            stager = measure.Stager(cfg, lambda _m: None)
            (root / "ssd").mkdir()
            (root / "ssd" / "control.sql").write_bytes(b"x" * 4096)
            (root / "ssd" / "control.stamp").write_text(
                measure.input_stamp(measure.INPUTS["control"], cfg) + "\n"
            )

            def partial(src, dst):
                Path(dst).write_bytes(b"x" * 100)
                raise OSError(122, "Disk quota exceeded")

            with unittest.mock.patch.object(measure.shutil, "copyfile", side_effect=partial):
                with self.assertRaises(measure.StagingError):
                    stager.warm_path("control")
            self.assertNotIn("control", stager._staged)
            self.assertFalse((root / "shm" / "control.sql").exists())
            # And the next ask copies it whole rather than taking the stub.
            self.assertEqual(stager.warm_path("control").stat().st_size, 4096)
            self.assertIn("control", stager._staged)

    def test_a_staging_failure_aborts_the_sweep(self):
        source = inspect.getsource(measure.emit)
        handler = source.index("except StagingError")
        self.assertLess(handler, source.index("except Exception as exc:  # one figure failing"))
        self.assertIn("break", source[handler : handler + 800])


class SplitSweep(unittest.TestCase):
    """A split figure's specs run beside the specs over their own group, each
    group staged first."""

    def _session(self):
        cfg = measure.Config(dry_run=True)
        stager = measure.Stager(cfg, lambda _m: None)
        stager.plan(measure.FIGURES)
        return measure.Session(cfg, stager, lambda _m: None)

    def test_specs_go_to_their_group_in_group_order(self):
        fig = measure.FIGURES_BY_ID["nested-end-to-end"]
        parts = measure.split_specs(fig.id, measure._nested_specs(), fig.staging_groups)
        self.assertEqual([gi for gi, _ in parts], [0, 1])
        self.assertEqual({s.input for s in parts[0][1]}, {"control", "composite"})
        self.assertEqual({s.input for s in parts[1][1]}, {"control", "arrays"})
        # The second sweep's control is a reading of its own; the first's
        # keys as it always did, which is what the allocator table borrows.
        self.assertEqual({s.sweep for s in parts[0][1]}, {None})
        self.assertEqual(
            {(s.input, s.sweep) for s in parts[1][1]}, {("control", 1), ("arrays", None)}
        )
        keys = [s.key(fig.id) for s in measure._nested_specs()]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertIn("nested-end-to-end/pgdt/control/query-typed/warm/sweep=1", keys)
        self.assertIn("nested-end-to-end/pgdt/control/query-typed/warm", keys)

    def test_a_spec_names_only_a_later_group_staging_its_input(self):
        groups = measure.FIGURES_BY_ID["nested-end-to-end"].staging_groups
        for sweep in (0, 2):
            spec = measure.RunSpec("pgdt", "control", "query-typed", "warm", "x", sweep=sweep)
            with self.subTest(sweep=sweep):
                with self.assertRaises(ValueError):
                    measure.split_specs("nested-end-to-end", [spec], groups)
        arrays = measure.RunSpec("pgdt", "arrays", "query-typed", "warm", "x", sweep=0)
        with self.assertRaises(ValueError):
            measure.split_specs("nested-end-to-end", [arrays], groups)
        with self.assertRaises(ValueError):
            measure.in_sweep("nested-end-to-end", arrays, 0)

    def test_an_unsplit_figure_refuses_a_spec_naming_a_sweep(self):
        session = self._session()
        session.take = lambda spec, rep: 1.0
        spec = measure.RunSpec("pgdt", "control", "query-typed", "warm", "x", sweep=1)
        with self.assertRaises(ValueError):
            session.sweep("cross-file-floor", [spec], 1)

    def test_the_nested_table_reads_each_file_against_its_own_sweep(self):
        # Distinct controls per sweep, so a headline read against the wrong
        # one renders a different number.
        legs = {
            ("control", None): (1.0, 2.0),
            ("composite", None): (1.0, 2.5),
            ("control", 1): (1.1, 2.6),
            ("arrays", None): (1.1, 5.0),
            ("control43", None): (1.0, 2.0),
        }
        readings = {}
        for (name, sweep), (strings, typed) in legs.items():
            figure = "cross-file-floor" if name == "control43" else "nested-end-to-end"
            for command, value in (("query-strings", strings), ("query-typed", typed)):
                spec = measure.RunSpec("pgdt", name, command, "warm", "", sweep=sweep)
                readings[spec.key(figure)] = [value] * 5
        for command, value in (("query-strings", 1.0), ("query-typed", 2.0)):
            spec = measure.RunSpec("pgdt", "control", command, "warm", "")
            readings[spec.key("cross-file-floor")] = [value] * 5
        with tempfile.TemporaryDirectory() as tmp:
            cfg = measure.Config(dry_run=True, cache_dir=Path(tmp))
            session = measure.ReplaySession(
                cfg, {"readings": readings, "runs": []}, Path(tmp), lambda _m: None
            )
            table = measure.run_nested_end_to_end(session)
            rows = cfg.size_gib * measure.GIB // 4000
            self.assertIn("| control — 16 scalar columns, in `--composite`'s sweep |", table)
            self.assertIn(
                "| control — 16 scalar columns, in `--arrays --composite`'s sweep |", table
            )
            headline = ((5.0 - 1.1) - (2.6 - 1.1)) / rows * 1e6
            self.assertIn(f"**{headline:+.2f} µs/row**", table)
            self.assertIn("`--arrays --composite` 1.000×", table)
            self.assertIn("`--composite` 1.000×", table)
            floor = measure.run_cross_file_floor(session)
            share = ((2.5 - 1.0) - (2.0 - 1.0)) / rows * 1e6
            self.assertIn(f"**{share:+.2f} µs**", floor)

    def test_a_spec_no_group_stages_is_an_error(self):
        groups = measure.FIGURES_BY_ID["scan-throughput-warm"].staging_groups
        for spec in (
            measure.RunSpec("pgdt", "control", "parse", "cold", "off tmpfs"),
            measure.RunSpec("pgdt", "arrays", "parse", "warm", "no group"),
        ):
            with self.subTest(spec=spec.label):
                with self.assertRaises(ValueError):
                    measure.split_specs("scan-throughput-warm", [spec], groups)

    def test_each_group_is_staged_before_its_own_reps(self):
        session = self._session()
        order = []
        session.take = lambda spec, rep: order.append(("run", spec.input)) or 1.0
        stage = session.stage
        session.stage = lambda figure, gi: (order.append(("stage", gi)), stage(figure, gi))
        session.sweep("scan-throughput-warm", measure._throughput_specs("warm"), 2)
        self.assertEqual(
            [o for o in order if o[0] == "stage"], [("stage", 0), ("stage", 1), ("stage", 2)]
        )
        runs = [(i, o[1]) for i, o in enumerate(order) if o[0] == "run"]
        stages = [i for i, o in enumerate(order) if o[0] == "stage"]
        for (i, name) in runs:
            group = max(g for g, at in enumerate(stages) if at < i)
            self.assertEqual(
                (name,), measure.FIGURES_BY_ID["scan-throughput-warm"].staging_groups[group]
            )

    def test_each_warm_throughput_shape_has_its_own_floor(self):
        specs = measure._throughput_specs("warm")
        parsed = {s.input for s in specs if s.command == "parse"}
        floors = {s.input for s in specs if s.command == "dd"}
        self.assertEqual(parsed, floors)


class Consumers(unittest.TestCase):
    """`depends` is the edge into a figure; `consumers()` is the edge out, and
    it is computed from who names the figure rather than declared beside it. A
    declared list is corrected only by a session that happens to notice, which
    is how a retarget left every tuple pointing at a document that had been
    replaced."""

    def test_every_figure_has_a_consumer(self):
        for fig in measure.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertTrue(measure.consumers(fig))

    def test_every_consumer_exists(self):
        for fig in measure.ALL_FIGURES:
            for path in measure.consumers(fig):
                with self.subTest(figure=fig.id, path=path):
                    self.assertTrue((measure.REPO / path).exists(), path)

    def test_a_figure_does_not_list_the_doc_it_lives_in(self):
        # measurements.md is where the table goes, not somewhere that repeats
        # it; listing it would make every fold-in look like a cross-doc edit.
        for fig in measure.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertNotIn("docs/design/measurements.md", measure.consumers(fig))

    def test_a_document_naming_the_id_is_a_consumer(self):
        # `decisions.md` cites every figure it argues from by id, which is what
        # makes the computed edge possible at all.
        self.assertIn(
            "docs/design/decisions.md", measure.consumers(measure.FIGURES_BY_ID["allocator"])
        )

    def test_a_glob_reaches_the_family_it_spells(self):
        # `scan-throughput-*` is how one sentence cites three tables, and a
        # scan that only matched the exact id would read it as citing none.
        for fid in ("scan-throughput-cold", "scan-throughput-warm", "scan-throughput-nvme"):
            with self.subTest(figure=fid):
                self.assertIn(
                    "docs/design/decisions.md", measure.consumers(measure.FIGURES_BY_ID[fid])
                )

    def test_a_dated_history_entry_is_not_a_consumer(self):
        # An entry states what was true on its day and is never revised, so a
        # fold-in that re-read one could only make it untrue. Every figure is
        # named in some entry, so this would otherwise be the whole output.
        for fig in measure.ALL_FIGURES:
            for path in measure.consumers(fig):
                with self.subTest(figure=fig.id, path=path):
                    self.assertFalse(path.startswith("docs/status/history/"), path)

    def test_the_register_and_its_tests_are_not_consumers(self):
        # They name every figure because they *are* the declaration.
        for fig in measure.ALL_FIGURES:
            got = measure.consumers(fig)
            for path in ("scripts/measure.py", "scripts/test_measure.py"):
                with self.subTest(figure=fig.id, path=path):
                    self.assertNotIn(path, got)

    def test_the_declared_residue_is_carried_through(self):
        # The manual states `peak-rss`'s claim to a reader who will never see a
        # figure id, so no scan can find it and the tuple still holds it.
        got = measure.consumers(measure.FIGURES_BY_ID["peak-rss"])
        self.assertIn("docs/manual/dump-inspection.md", got)
        self.assertIn("README.md", got)

    def test_a_declared_consumer_is_not_repeated_by_the_scan(self):
        for fig in measure.ALL_FIGURES:
            got = measure.consumers(fig)
            with self.subTest(figure=fig.id):
                self.assertEqual(len(got), len(set(got)))


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
                "<!-- figure: nested-end-to-end — reproduce with `cd scripts && "
                "uv run measure.py --figure nested-end-to-end` -->\n",
            )
            self.assertEqual(measure.markers_in(doc), ["nested-end-to-end"])

    def test_a_heading_that_quotes_a_number_is_not_an_address(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(
                tmp,
                "## A typed query over nested columns costs 6.6 µs a row more than a string one\n\n"
                "<!-- figure: nested-end-to-end -->\n",
            )
            self.assertEqual(measure.markers_in(doc), ["nested-end-to-end"])

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
            "```sh\n# maps to EOF (the table never matches)\npgdt parse\n```\n\n"
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
        self.assertEqual(publishing, {"koji"})
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
            "pgdt/src/main.rs",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    measure.declared_hits(measure.NOT_OURS["koji"], [path]), [path]
                )

    def test_the_attribution_is_no_longer_a_section_the_harness_disowns(self):
        # A registered figure carries no `Outside` row and its section no
        # `outside-register` marker, which is the reconciliation `--check` makes
        # both ways.
        self.assertNotIn("rss-attribution", measure.NOT_OURS)
        self.assertIn("rss-attribution", measure.FIGURES_BY_ID)
        self.assertNotIn(
            "outside-register: rss-attribution",
            (measure.REPO / "docs/design/measurements.md").read_text(),
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
        self.assertEqual(len(problems), 1)
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
        self.assertEqual(set(bases), {"koji"})
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
    reasoning from a commit the doc itself says is not the figure's.

    **The cases below fabricate a sitting and name a real register entry as
    their example, so that entry's other properties are premises they do not
    state.** `test_the_unentangled_example_is_still_unentangled` states them
    instead, and is what a register move fails against — asserting the premise
    beside the test is the durable form of that lesson, because the failure
    then names the premise that went rather than a refusal these cases were
    never testing. A rule saying so elsewhere would be a third copy enforced by
    nothing. The shape is specific: it bites where an assertion picks **one
    element of a multi-cause result**, `sitting_problems` being the only such
    producer here — an example whose property is asserted whole, as at
    `test_a_figure_with_no_nvme_inputs_checks_nothing`, fails legibly on its
    own."""

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
        # graph: `allocator`'s reference column *is* two other tables' rows.
        self.assertEqual(measure.entangled_with("map-only"), [])
        self.assertIn("scan-throughput-warm", measure.entangled_with("allocator"))

    def test_the_unentangled_example_is_still_unentangled(self):
        """`map-only` is the figure the fabricated sittings below are written
        against, and it is load-bearing that it stands in no edge.

        `peak-rss` used to be that figure, and the register move that put
        `reserve` and `rss-attribution` into `FIGURES` gave it two — so seven
        assertions started failing on a refusal that fired before the one they
        were checking, which reads as a doc problem and is not one. Asserting
        the premise here is what makes the next such move fail with a sentence
        that says which premise went."""
        self.assertEqual(measure.entangled_with("map-only"), [])
        self.assertEqual(
            measure.entangled_with("peak-rss"), ["reserve", "rss-attribution"]
        )

    def test_a_derivation_entangles_in_both_directions(self):
        # Not a closure edge, and still an edge: `cross-file-floor`'s first row
        # is a difference over `nested-end-to-end`'s reps.
        self.assertIn("cross-file-floor", measure.entangled_with("nested-end-to-end"))
        self.assertIn("nested-end-to-end", measure.entangled_with("cross-file-floor"))

    def test_an_entangled_sitting_is_refused_by_the_doc_and_by_the_run(self):
        problems = measure.sitting_problems({"allocator": "bbbbbbb"}, "aaaaaaa")
        self.assertEqual(len(problems), 1)
        self.assertIn("scan-throughput-warm", problems[0])
        refusals = measure.publication_refusals([measure.ALL_BY_ID["allocator"]])
        self.assertEqual(len(refusals), 1)
        self.assertIn("only a sweep", refusals[0])

    def test_a_figure_standing_in_no_edge_may_be_taken_on_its_own(self):
        self.assertEqual(measure.publication_refusals([measure.ALL_BY_ID["map-only"]]), [])

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
            {"map-only": "aaaaaaa"},
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
            {"map-only": "bbbbbbb"},
            "aaaaaaa",
            resolve=lambda rev: rev * 5,
            ancestor=lambda a, b: False,
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("does not descend", problems[0])

    def test_a_descendant_sitting_passes(self):
        self.assertEqual(
            measure.sitting_problems(
                {"map-only": "bbbbbbb"},
                "aaaaaaa",
                resolve=lambda rev: rev * 5,
                ancestor=lambda a, b: True,
            ),
            [],
        )

    def test_a_sitting_naming_no_commit_is_refused(self):
        problems = measure.sitting_problems(
            {"map-only": "bbbbbbb"}, "aaaaaaa", resolve=lambda rev: None
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("not a commit", problems[0])

    def test_a_sitting_with_no_stamp_to_be_outside_of_is_refused(self):
        problems = measure.sitting_problems({"map-only": "bbbbbbb"}, None)
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
        return measure.koji_recipe(measure.Config(), "pgdt-koji", wrap, jobs)

    def test_pgdt_is_pid_one(self):
        # A compound command cannot be exec'd, so nothing may be appended to
        # report the exit status: `sh` would take the signal and not forward
        # it, the runtime's SIGKILL would follow, and the interrupt guard would
        # never run.
        self.assertIn("sh -c 'exec /pgdt parse", self._recipe())

    def test_nothing_follows_the_parse_inside_the_shell(self):
        for line in self._recipe().splitlines():
            if "exec /pgdt parse" in line:
                with self.subTest(line=line):
                    self.assertNotIn("; echo", line)

    def test_the_cgroup_limit_is_part_of_the_apparatus(self):
        self.assertIn("-m 512m --memory-swap 512m", self._recipe())

    def test_the_cache_lands_in_the_mounted_volume(self):
        # The dump is mounted read-only, so the colocated default would land in
        # the container's ephemeral layer and die with it — an hour of scanning
        # lost with no error, because the write itself succeeds.
        self.assertIn("--dtcache /out/", self._recipe())
        self.assertNotIn("--dtcache /dump.sql", self._recipe())

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
            if "exec /pgdt parse" in line:
                with self.subTest(line=line):
                    self.assertNotIn("VmHWM", line)

    def test_the_wrap_recipe_stops_reports_resumes_and_compares(self):
        wrap = self._recipe(wrap=True)
        for fragment in ("nerdctl stop", "info --dtcache", "--detail", "cmp "):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, wrap)

    def test_the_wrap_resumes_the_identical_command(self):
        wrap = self._recipe(wrap=True)
        legs = [ln for ln in wrap.splitlines() if "exec /pgdt parse" in ln]
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
        self.assertIn("target/profiling/pgdt", recipe)
        self.assertNotIn("target/release/pgdt", recipe)

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
        # a scan among workers the figure it explains never ran.
        lines = self._recipe().splitlines()
        recorded = [
            (lines[i - 1], ln) for i, ln in enumerate(lines) if ln.strip().startswith("-- ")
        ]
        self.assertEqual(
            len(recorded),
            len(measure.PROFILE_INPUTS) * len(measure.PROFILE_SHAPES)
            + len(measure.DFCLI_ACCOUNT_LEGS),
        )
        for record, line in recorded:
            with self.subTest(line=line):
                # `datafusion-cli-pgdump` states its count as the figure does,
                # in the environment `perf` hands on.
                self.assertTrue(
                    f"--jobs {measure.SWEEP_JOBS}" in line
                    or f"{measure.DFCLI_PARTITIONS}={measure.SWEEP_JOBS} " in record,
                    line,
                )

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
        wrapper = measure.rss_wrapper(measure.platform.machine())
        for shape in measure.PROFILE_SHAPES:
            with self.subTest(shape=shape):
                # A resident leg's wrapper is a second process a profile leaves
                # off, and nothing else of the timed line.
                # A query shape's cache builder runs ahead of the timer, and
                # is held to `profile_builder_argv` below.
                script = measure._script(shape).rpartition(" && ")[2]
                timed = script.replace(f"{wrapper} ", "", 1).split()
                # Drop `time /pgdt`, the trailing redirect, and the container's
                # own paths; what is left is the flags both must agree on.
                self.assertEqual(timed[:2], ["time", "/pgdt"])
                timed = [w for w in timed[2:] if w != ">/dev/null"]
                profiled = measure.profile_argv(shape, "/dump.sql", "/tmp/x.dtcache")
                self.assertEqual(profiled, timed)

    def test_a_profiled_query_is_preceded_by_the_builder_its_figure_runs(self):
        """A query shape is timed over a data-level cache, so its profile is
        taken over one too: the builder `_script` runs ahead of the timer,
        flag for flag, written after the cache is removed and before `perf
        record`, and never recorded itself."""
        cfg = measure.Config()
        lines = measure.profile_recipe(cfg).splitlines()
        binary = measure.REPO / "target/profiling/pgdt"
        for shape in measure.PROFILE_SHAPES:
            builder = measure.profile_builder_argv(shape, "/dump.sql", "/tmp/x.dtcache")
            with self.subTest(shape=shape):
                if not shape.startswith(measure.DATA_LEVEL_QUERIES):
                    self.assertIsNone(builder)
                    continue
                timed = measure._script(shape).partition(" && ")[0].split()
                self.assertEqual(timed[0], "/pgdt")
                self.assertEqual(builder, [w for w in timed[1:] if w != ">/dev/null"])
                for name in measure.PROFILE_INPUTS:
                    source = cfg.warm_dir / f"{name}.sql"
                    built = measure.profile_builder_argv(shape, source, cfg.warm_dir / "profile.dtcache")
                    record = next(
                        i for i, ln in enumerate(lines)
                        if ln.startswith(f"{measure.PERF} record")
                        and f"profile-{shape}-{name}.data" in ln
                    )
                    self.assertEqual(lines[record - 1], f"{binary} {' '.join(built)} >/dev/null")
                    self.assertTrue(lines[record - 2].startswith("rm -f "), lines[record - 2])

    def test_the_dfcli_pair_and_its_reading_run_the_timed_legs(self):
        """The costing row's pair, and the introspection build's runs of it,
        state what `_script` times: the same environment, arguments and SQL,
        read out of the timed line rather than restated, over a cache the
        figure's own `pgdt parse` writes where `--dump` looks."""
        cfg = measure.Config()
        recipe = measure.profile_recipe(cfg)
        figure, query = measure.DFCLI_ACCOUNT
        dump = cfg.warm_dir / "dynfilter.sql"
        self.assertIn(f"cp -n {cfg.cache_dir / 'dynfilter.sql'} {dump}", recipe)
        self.assertIn(
            f"parse --source {dump} --dtcache {dump}.dtcache --jobs {measure.SWEEP_JOBS} "
            f"{measure.GATHER_STATISTICS} >/dev/null",
            recipe,
        )
        profiled = str(measure.REPO / "target/profiling/datafusion-cli-pgdump")
        introspected = str(
            cfg.alloc_build_root / "dfcli-introspect/release/datafusion-cli-pgdump"
        )
        # The default leg against the rows leg: the setting alone apart, the
        # comparison "D93" reads.
        self.assertEqual(measure.DFCLI_ACCOUNT_LEGS, ("on", measure.DYNFILTER_ROWS_LEG))
        self.assertNotIn(f"{figure}-{query}-off", recipe)
        for leg in measure.DFCLI_ACCOUNT_LEGS:
            with self.subTest(leg=leg):
                timed = measure._script(f"{measure.DYNFILTER_FAMILY}{figure}-{query}-{leg}")
                run = timed.split(" && time ", 1)[1].split(" >/tmp/result.csv", 1)[0]
                env, _, rest = run.partition(f"{measure.DFCLI} ")
                rest = rest.replace("=/dump.sql ", f"={dump} ")
                self.assertIn(f"{env}{measure.PERF} record", recipe)
                self.assertIn(f"  -- {profiled} {rest} >/dev/null", recipe)
                for rep in range(1, measure.DFCLI_INTROSPECT_REPS + 1):
                    report = cfg.out_dir / f"introspect-dfcli-{figure}-{query}-{leg}-{rep}.txt"
                    self.assertIn(
                        f"{measure.INSTRUMENT_OUT_VAR}={report} {env}{introspected} {rest} "
                        ">/dev/null",
                        recipe,
                    )
        self.assertIn("--features introspect", recipe)
        self.assertIn(f"--target-dir {cfg.alloc_build_root / 'dfcli-introspect'}", recipe)
        self.assertIn(f"{dump}.dtcache", recipe.splitlines()[-1])

    def test_the_recipe_never_runs_anything(self):
        # The same rule koji's recipe obeys: this prints, and a session runs it
        # by hand. A harness that ran it would be taking a figure.
        with unittest.mock.patch.object(measure, "run") as ran:
            with unittest.mock.patch("sys.stdout"):
                measure.cmd_profile()
        ran.assert_not_called()


class HeaptrackRecipe(unittest.TestCase):
    """The libc-level instrument, printed here for the same reason the sampling
    profile is. Its failure mode is the profile's, not koji's: every mistake
    below returns a report that looks fine and describes something else — and
    one of them already has, an unexplained 67.11 MB that was a merged frame's
    summed peak read as one allocation."""

    def _recipe(self) -> str:
        return measure.heaptrack_recipe(measure.Config())

    def _records(self) -> list[str]:
        return [
            ln for ln in self._recipe().splitlines()
            if ln.strip().startswith(f"{measure.HEAPTRACK} --record-only")
        ]

    def _reports(self) -> list[str]:
        return [
            ln for ln in self._recipe().splitlines()
            if ln.strip().startswith(f"{measure.HEAPTRACK}_print")
        ]

    def test_the_profiling_binary_is_the_one_recorded(self):
        # heaptrack resolves symbols off either build, so this one fails
        # quietly: `release` yields a report with no `.rs:` reference anywhere
        # in it — 3,627 against 0 on the same recording — and every Rust frame
        # is a bare name with no file behind it.
        recipe = self._recipe()
        self.assertIn("target/profiling/pgdt", recipe)
        self.assertNotIn("target/release/pgdt", recipe)
        self.assertIn("--profile profiling", recipe)

    def test_frame_pointers_are_not_asked_for(self):
        # A stated non-requirement, not an omission: heaptrack unwinds
        # `.eh_frame` where `perf` needs frame pointers, so the flag buys
        # nothing and would fingerprint a second build of the same source.
        commands = [
            ln for ln in self._recipe().splitlines() if ln and not ln.startswith("#")
        ]
        for line in commands:
            with self.subTest(line=line):
                self.assertNotIn("force-frame-pointers", line)
                self.assertNotIn("RUSTFLAGS", line)
        self.assertTrue(any("--profile profiling" in ln for ln in commands))

    def test_the_gui_is_never_launched(self):
        # Without `--record-only` heaptrack hands the finished file to
        # `heaptrack_gui`, which need not be installed and, where it is, may
        # not start. The `.zst` is written either way, so the failure is
        # cosmetic — and a recipe ending in an error message is one a session
        # stops trusting.
        self.assertTrue(self._records())
        for line in self._records():
            with self.subTest(line=line):
                self.assertIn("--record-only", line)

    def test_the_report_does_not_merge_backtraces(self):
        # `heaptrack_print` merges by default and its own --help says the
        # merged peak consumption is not correct: a merged frame prints the
        # *summed* peak of every backtrace under it beside a call count
        # belonging to the merge, so one decoder's 8.39 MB can read as
        # "8.39M over 2 calls" and eight of them as 67.11 MB.
        self.assertTrue(self._reports())
        for line in self._reports():
            with self.subTest(line=line):
                self.assertIn("--merge-backtraces=0", line)

    def test_the_rust_frames_are_demangled(self):
        # This build's symbols are v0 and this heaptrack cannot demangle them:
        # its Rust support is post-1.5.0 and no binary of it references
        # `rustc_demangle`. Without the pipe the C frames read fine and the
        # Rust frames above them are noise.
        recipe = self._recipe()
        self.assertIn(measure.HEAPTRACK_DEMANGLE, recipe)
        piped = [ln for ln in recipe.splitlines() if ".txt" in ln]
        self.assertEqual(len(piped), len(measure.HEAPTRACK_AXIS) + 1)
        for line in piped:
            with self.subTest(line=line):
                self.assertIn(f"| {measure.HEAPTRACK_DEMANGLE} >", line)

    def test_each_recording_starts_from_no_cache(self):
        # `parse` resumes from a cache, so a pair sharing one would record a
        # full scan and then a no-op — two recordings whose difference is the
        # whole workload rather than the one byte of budget between them.
        lines = self._recipe().splitlines()
        starts = [i for i, ln in enumerate(lines) if ln.startswith(f"{measure.HEAPTRACK} --record-only")]
        self.assertEqual(len(starts), len(measure.HEAPTRACK_AXIS))
        for i in starts:
            with self.subTest(line=lines[i]):
                self.assertTrue(lines[i - 1].startswith("rm -f "), lines[i - 1])
                self.assertIn("heaptrack.dtcache", lines[i - 1])

    def test_the_pair_is_read_as_a_difference(self):
        # The whole reason there are two recordings: a single one names what a
        # run holds, and only the difference names what the admitted reader
        # added. Both recordings must appear in that one invocation.
        recipe = self._recipe()
        lines = recipe.splitlines()
        diff = [i for i, ln in enumerate(lines) if "--diff" in ln]
        self.assertEqual(len(diff), 1)
        (first, first_input), (second, second_input) = measure.HEAPTRACK_AXIS
        self.assertIn(f"heaptrack-{second}-{second_input}.zst", lines[diff[0]])
        self.assertIn(f"heaptrack-{first}-{first_input}.zst", lines[diff[0] - 1])

    def test_the_pair_differs_only_in_its_budget(self):
        """A difference read frame by frame is only a difference if the two
        argvs are otherwise identical — a worker count or a cache path that
        moved with the budget would put a second variable in the one reading
        this pair exists to isolate."""
        argvs = [
            measure.heaptrack_argv(shape, "/dump.sql", "/tmp/x.dtcache")
            for shape, _ in measure.HEAPTRACK_AXIS
        ]
        self.assertEqual(len(argvs), 2)
        first, second = argvs
        self.assertEqual(len(first), len(second))
        differing = [i for i, (a, b) in enumerate(zip(first, second)) if a != b]
        self.assertEqual(len(differing), 1, f"{first} vs {second}")
        self.assertEqual(first[differing[0] - 1], "--memory")
        at = differing[0]
        self.assertEqual(abs(int(first[at]) - int(second[at])), 1)

    def test_a_recorded_shape_is_the_shape_the_sweep_times(self):
        """The reconciliation that keeps an attribution readable against the
        figure it explains — `profile_argv`'s, over the wrapped shapes.

        `_script` builds a container command line with a timer and the RSS
        wrapper in front of it, so the two cannot be one function; a flag that
        moves in one and not the other gives a recording of something no figure
        measures, and nothing else would notice."""
        for shape, _ in measure.HEAPTRACK_AXIS:
            with self.subTest(shape=shape):
                script = measure._script(shape)
                head, sep, rest = script.partition("/pgdt ")
                self.assertTrue(sep, script)
                timed = [w for w in rest.split() if w != ">/dev/null"]
                recorded = measure.heaptrack_argv(shape, "/dump.sql", "/tmp/x.dtcache")
                self.assertEqual(recorded, timed)

    def test_every_recorded_invocation_states_its_worker_count(self):
        # The apparatus rule, and here it is also what makes the pair a pair:
        # the count a source recommends moves with the budget, so a recording
        # that inherited one would differ from its partner in two things.
        argv_lines = [
            ln for ln in self._recipe().splitlines() if "target/profiling/pgdt parse" in ln
        ]
        self.assertEqual(len(argv_lines), len(measure.HEAPTRACK_AXIS))
        for line in argv_lines:
            with self.subTest(line=line):
                self.assertIn(f"--jobs {measure.RESERVE_JOBS}", line)

    def test_the_input_is_staged_and_torn_down(self):
        # tmpfs is 16 G and this input does not fit beside a sweep's.
        cfg = measure.Config()
        recipe = measure.heaptrack_recipe(cfg)
        for _, name in measure.HEAPTRACK_AXIS:
            staged = cfg.warm_dir / measure.input_file(name)
            with self.subTest(input=name):
                self.assertIn(f"cp -n {cfg.cache_dir / measure.input_file(name)}", recipe)
                self.assertIn(f"--source {staged}", recipe)
                self.assertIn(str(staged), recipe.splitlines()[-1])

    def test_no_container_is_involved(self):
        # heaptrack multiplies allocation cost and resident set by its own
        # bookkeeping, so a recording under a cgroup would be a recording of
        # heaptrack meeting the limit. The cgroup belongs to the gate.
        recipe = self._recipe()
        self.assertNotIn("nerdctl", recipe)
        self.assertNotIn("--memory-swap", recipe)

    def test_the_step_numbering_has_no_hole(self):
        numbered = [
            int(ln.split(".")[0][2:]) for ln in self._recipe().splitlines()
            if re.match(r"^# \d+\. ", ln)
        ]
        self.assertEqual(numbered, list(range(len(numbered))))

    def test_the_recipe_never_runs_anything(self):
        # The rule koji's and the profile's recipes obey: this prints, and a
        # session runs it by hand.
        with unittest.mock.patch.object(measure, "run") as ran:
            with unittest.mock.patch("sys.stdout"):
                measure.cmd_heaptrack()
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

    def test_it_measures_the_same_three_shapes_and_their_floors(self):
        # The three tables are read against each other as ratios, so they have
        # to be the same rows over the same files.
        cold = measure.FIGURES_BY_ID["scan-throughput-cold"]
        self.assertEqual(
            measure.FIGURES_BY_ID["scan-throughput-nvme"].nvme_inputs, cold.cold_inputs
        )
        specs = measure._throughput_specs("cold-nvme")
        self.assertEqual({s.regime for s in specs}, {"cold-nvme"})
        self.assertEqual(
            [(s.input, s.command) for s in specs],
            [(s.input, s.command) for s in measure._throughput_specs("warm")],
        )

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
        # `chunk-size`'s `warm+cold+cold-nvme` is selected by `cold` and
        # `scan-throughput-nvme`'s `cold-nvme` is not.
        cold = [f.id for f in measure.FIGURES if "cold" in f.stage.split("+")]
        self.assertIn("scan-throughput-cold", cold)
        self.assertIn("chunk-size", cold)
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
            f"pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = 1 << {measure.CHUNK_DEFAULT.bit_length() - 1};",
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
        self.assertEqual({s.binary for s in specs}, {"pgdt"})
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
            a = self._sweep(tmp, "a", {"scan-throughput-warm/pgdt/control/parse/warm": [1.0, 1.0]})
            b = self._sweep(tmp, "b", {"scan-throughput-warm/pgdt/control/parse/warm": [1.1, 1.1]})
            table = measure.drift_table(a / "raw.json", b / "raw.json")
            self.assertIn("+10.0%", table)

    def test_a_faster_second_sweep_reads_negative(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self._sweep(tmp, "a", {"f/pgdt/control/parse/warm": [2.0]})
            b = self._sweep(tmp, "b", {"f/pgdt/control/parse/warm": [1.0]})
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
    """The sub-stream count belongs to the two provider legs and no other.

    It is stated per cell rather than footnoted once, so a cell that carries it
    is making a claim about *that* leg. The count was once hand-applied to a
    pasted table and landed one column left of the leg it described — on
    `.xz`, `parse`, which plans no sub-streams at all — which is why the
    mapping is asserted here rather than read off the table by eye.
    """

    def test_only_provider_legs_are_annotated(self):
        scan = measure.PARALLEL_SCAN
        annotated = {
            (inp, family) for inp, family, _ in measure.PARALLEL_LEGS if family == scan
        }
        self.assertEqual(annotated, {("control", scan), ("control_xz", scan)})
        for inp, family, label in measure.PARALLEL_LEGS:
            if family != scan:
                self.assertNotIn(
                    "provider", label, f"{label} is not a provider leg but reads like one"
                )

    def test_a_cap_belongs_to_a_provider_leg(self):
        # A cap for a leg nothing annotates is dead weight that reads as a
        # claim about the table. The dict may legitimately be empty — no leg's
        # count falls inside the axis at the budget stated today — so what is
        # asserted is the membership, not that anything is in it.
        typed = {
            inp for inp, family, _ in measure.PARALLEL_LEGS if family == measure.PARALLEL_SCAN
        }
        self.assertLessEqual(set(measure.QUERY_SUBSTREAM_CAP), typed)

    def test_an_empty_cap_says_so_in_the_prose(self):
        # The paragraph is emitted from the dict, so the two cannot disagree:
        # empty means the note says neither leg is clamped, and an entry means
        # it names the count. The dict is not empty at today's carving, so the
        # empty branch is reached by patching one in.
        with unittest.mock.patch.object(measure, "QUERY_SUBSTREAM_CAP", {}):
            empty = measure._substream_note()
        self.assertIn("neither provider leg reaches it", empty)
        self.assertNotIn("state the count they actually", empty)
        with unittest.mock.patch.object(measure, "QUERY_SUBSTREAM_CAP", {"control": 14}):
            clamped = measure._substream_note()
        self.assertIn("`14` on plain", clamped)
        self.assertNotIn("neither provider leg reaches it", clamped)

    def test_a_cap_is_never_above_the_largest_job_count(self):
        # A cap at or above the largest `--jobs` would annotate every row with
        # its own label and say nothing — which is why a leg the budget never
        # clamps inside the axis carries no entry at all rather than a number
        # past the top of it.
        for inp, cap in measure.QUERY_SUBSTREAM_CAP.items():
            self.assertLess(cap, measure.PARALLEL_JOBS[-1], inp)

    def test_only_the_plain_leg_carries_a_cap_at_this_budget(self):
        # `plan_partitions` charges the held batch's span only where the source
        # retains by the read chunk, and a block-decoding `XzSource` retains by
        # the partition — so the `.xz` leg is charged what one reader holds,
        # `34.03 MiB`, plus a 24 MiB unit of the pool's retention list for each
        # reader past four, and affords twenty-nine against the margin ceiling,
        # past the top of the axis. The plain leg recommends nothing, so it is
        # handed `DEFAULT_MEMORY_BUDGET` whatever `--memory` states (`D83`):
        # 64 MiB, spent on readers first and on the span with what is left
        # (`D84`), which seats every worker up to four and stops at seven once
        # the span is on its 1 MiB floor. The table's own paragraph is emitted
        # from this dict so it cannot disagree with it.
        self.assertEqual(measure.QUERY_SUBSTREAM_CAP, {"control": 7})


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
                binary="pgdt", input="control", command="parse", regime="warm", label="x"
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
        for field in (
            "allocator",
            "glibc",
            "figure_glibc",
            "whole_sweep",
            "header",
            "input_sizes",
            "rss",
            "reported",
        ):
            self.assertIn(f'"{field}"', source, f"emit does not record {field}")

    def test_a_render_skips_the_figures_the_sitting_failed(self):
        """A failed figure took no readings, so replaying it raises rather than
        rebuilding a table — and the sitting's own record says which those
        were."""
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "raw.json").write_text(
                json.dumps(
                    {
                        "commit": "abcdef1",
                        "figures": ["per-block-quadratic"],
                        "failures": [["per-block-quadratic", "before one_block exited 2"]],
                        "whole_sweep": False,
                        "header": ["# measure.py output"],
                        "allocator": None,
                        "input_sizes": {},
                        "readings": {},
                        "rss": {},
                        "reported": {},
                        "runs": [],
                    }
                )
            )
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = measure.render(measure.Config(), run_dir)
            self.assertEqual(rc, 0)
            self.assertIn("0 figure(s)", out.getvalue())


class StampingTheDocument(unittest.TestCase):
    """Which sittings may re-stamp `measurements.md`.

    The defect: `emit` decided this from the *selection*, before the first
    reading, so a sweep that selected 22 figures and took 20 emitted a full
    session stamp claiming every figure below came from it — over 20 tables,
    with the other two still standing in the document from an older sitting the
    stamp had just claimed."""

    def test_a_whole_sweep_that_took_everything_stamps(self):
        self.assertTrue(measure.stamps_the_document(True, []))

    def test_a_whole_sweep_that_lost_a_figure_does_not(self):
        self.assertFalse(
            measure.stamps_the_document(True, [("per-block-quadratic", "exited 2")])
        )

    def test_a_selection_short_of_the_sweep_never_stamps(self):
        self.assertFalse(measure.stamps_the_document(False, []))
        self.assertFalse(measure.stamps_the_document(False, [("x", "boom")]))

    def test_emit_decides_it_after_the_figure_loop_not_before(self):
        """The predicate has to be evaluated where `failures` is populated.

        Asserted on the source because the ordering *is* the fix: computing it
        early type-checks, runs, and reproduces the defect exactly."""
        source = inspect.getsource(measure.emit)
        self.assertLess(
            source.index("failures.append"),
            source.index("stamps_the_document"),
            "emit decides the stamp before a figure can have failed",
        )


class ParallelScanThroughputProvider(unittest.TestCase):
    """`parallel-scan-throughput`'s extraction legs run the provider.

    `pgdt query`'s in-order merge reads one sub-stream at a time past its
    first round (`KD57`), so a column of it timed the merge rather than the
    library's sub-streams. What these hold is that the replacement times a
    scan: every column decoded, every row read, no count answered from the
    statistics, and one answer at every count."""

    FAMILY = f"{measure.PARALLEL_SCAN}-jobs-"

    def test_the_query_counts_every_column_of_the_generated_table(self):
        sql = measure.PARALLEL_SCAN_SQL
        for name, _ in measure.perf.COLUMNS:
            with self.subTest(column=name):
                self.assertIn(f"count({name})", sql)
        self.assertIn(f"FROM {measure.DFCLI_CATALOG}.{measure.perf.TABLE} ", sql)

    def test_a_filter_keeps_the_counts_from_being_answered(self):
        # Unfiltered, the plan node's exact NULL counts answer every
        # `count(<column>)` with no row read; a pushed filter makes them
        # estimates. `id` is written on every row, so the filter keeps them all.
        self.assertTrue(measure.PARALLEL_SCAN_SQL.endswith(" WHERE id IS NOT NULL"))
        self.assertEqual(measure.perf.COLUMNS[0][0], "id")

    def test_every_row_reads_the_cache_its_builder_wrote_ahead_of_the_timer(self):
        builders = set()
        for jobs in measure.PARALLEL_JOBS:
            script = measure._script(f"{self.FAMILY}{jobs}")
            with self.subTest(jobs=jobs):
                builder, _, rest = script.partition(" && ")
                builders.add(builder)
                self.assertNotIn("time ", builder)
                self.assertIn(measure.GATHER_STATISTICS, builder)
                self.assertIn(f"--jobs {measure.SWEEP_JOBS} ", builder)
                self.assertIn("--dtcache /dump.sql.dtcache ", builder)
                self.assertTrue(
                    rest.startswith(
                        f"time {measure.DFCLI_PARTITIONS}={jobs} {measure.DFCLI} "
                        f"--dump {measure.DFCLI_CATALOG}=/dump.sql "
                    ),
                    rest,
                )
                self.assertNotIn("; ", script)
        self.assertEqual(len(builders), 1)

    def test_the_answer_is_read_back_outside_the_timer(self):
        script = measure._script(f"{self.FAMILY}1")
        timed, _, after = script.partition(">/tmp/result.csv && ")
        self.assertEqual(after, measure.DFCLI_ANSWER)
        self.assertNotIn("echo", timed)

    def test_the_allowance_is_set_before_the_query_in_one_process(self):
        _, argv = measure.parallel_scan_invocation(8, "/dump.sql")
        allowance = measure.stated_allowance(measure.PARALLEL_BUDGET)
        self.assertEqual(
            argv[-4:], ["-c", f"SET pgdump.memory = {allowance}", "-c", measure.PARALLEL_SCAN_SQL]
        )

    def test_a_count_the_figure_does_not_carry_is_refused(self):
        with self.assertRaises(ValueError):
            measure.parallel_scan_invocation(3, "/dump.sql")

    def test_only_the_provider_legs_run_the_second_program(self):
        for spec in measure._parallel_specs():
            with self.subTest(command=spec.command):
                self.assertEqual(
                    spec.binary, "dfcli" if spec.command.startswith(self.FAMILY) else "pgdt"
                )

    def test_one_answer_passes_and_anything_else_is_refused(self):
        one = {"result_rows": "1", "result_first": "814362", "result_digest": "ab"}
        self.assertEqual(measure.parallel_answer_problems({"a": one, "b": one}), [])
        self.assertTrue(
            measure.parallel_answer_problems({"a": one, "b": {**one, "result_digest": "cd"}})
        )
        self.assertTrue(measure.parallel_answer_problems({"a": one, "b": {}}))
        self.assertTrue(measure.parallel_answer_problems({}))
        self.assertTrue(
            measure.parallel_answer_problems({"a": {**one, "result_first": "0"}})
        )

    def test_a_sitting_whose_legs_disagree_is_not_rendered(self):
        figure = "parallel-scan-throughput"
        specs = measure._parallel_specs()
        answers = iter(range(len(specs)))
        raw = {
            "readings": {s.key(figure): [1.0] * 5 for s in specs},
            "reported": {
                s.key(figure): {
                    "result_rows": "1",
                    "result_first": "814362",
                    "result_digest": str(next(answers)),
                }
                for s in specs
                if s.binary == "dfcli"
            },
            "input_sizes": {"control": 3221227790, "control_xz": 591190020},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = figure
            with self.assertRaises(RuntimeError):
                measure.run_parallel_scan_throughput(session)

    def test_the_figure_declares_the_provider(self):
        depends = measure.SELECTABLE_BY_ID["parallel-scan-throughput"].depends
        self.assertLessEqual(set(measure.DATAFUSION), set(depends))


class SubstreamAnnotationLandsOnTheRightColumn(unittest.TestCase):
    """The renderer itself, over synthetic readings.

    The classes above assert the *mapping*; this one asserts the table. The
    defect this pins put the `.xz` sub-stream counts in the `.xz`, `parse`
    column — every number correct, attached to the wrong leg — which no
    assertion about `PARALLEL_LEGS` alone would have caught.

    **The cap is supplied here rather than read off the shipped constant**,
    which is empty at today's budget: a renderer asserted only against the
    arrangement that prints nothing would stop covering the placement the
    moment it stopped mattering, which is exactly when a later constant brings
    it back.
    """

    CAP = {"control": 14}

    def _render(self, cap=None):
        figure = "parallel-scan-throughput"
        specs = measure._parallel_specs()
        answer = {"result_rows": "1", "result_first": "814362", "result_digest": "ab"}
        raw = {
            "readings": {s.key(figure): [1.0, 1.0, 1.0, 1.0, 1.0] for s in specs},
            "reported": {s.key(figure): answer for s in specs if s.binary == "dfcli"},
            "input_sizes": {"control": 3221227790, "control_xz": 591190020},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(
                measure.Config(), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = figure
            with unittest.mock.patch.object(
                measure, "QUERY_SUBSTREAM_CAP", self.CAP if cap is None else cap
            ):
                return measure.run_parallel_scan_throughput(session)

    def test_the_shipped_cap_annotates_the_plain_provider_leg(self):
        # The plain provider leg is clamped to seven sub-streams once the
        # span is on its floor, so every one of its annotated cells carries
        # that count and no other column does. An empty dict annotates nothing
        # at all, which is what a leg no budget clamps inside the axis gets.
        body = self._render(cap=measure.QUERY_SUBSTREAM_CAP)
        cells = [r for r in body.splitlines() if r.startswith("| ")]
        self.assertTrue(cells)
        annotated = [r for r in cells if "sub-stream" in r]
        self.assertTrue(annotated, "the shipped cap annotates nothing")
        for row in annotated:
            self.assertEqual(row.count("sub-stream"), 1, row)
            self.assertIn("7 sub-streams", row)
        with unittest.mock.patch.object(measure, "QUERY_SUBSTREAM_CAP", {}):
            empty = self._render(cap={})
        for row in (r for r in empty.splitlines() if r.startswith("| ")):
            self.assertNotIn("sub-stream", row)

    def test_the_annotation_is_in_the_provider_columns_only(self):
        body = self._render()
        rows = [r for r in body.splitlines() if r.startswith("| ")]
        header = [c.strip() for c in rows[0].strip("|").split("|")]
        typed = {i for i, c in enumerate(header) if "provider" in c}
        self.assertEqual(len(typed), 2, header)
        # Only the legs a budget clamp actually reaches are annotated, and
        # which those are is `QUERY_SUBSTREAM_CAP`'s to say.
        capped = {
            i
            for i in typed
            if ("control_xz" if "`.xz`" in header[i] else "control") in self.CAP
        }
        self.assertTrue(capped, "no provider leg is capped, so this asserts nothing")

        annotated_columns = set()
        for row in rows[2:]:  # skip header and the |---| separator
            cells = [c.strip() for c in row.strip("|").split("|")]
            for i, cell in enumerate(cells):
                if "sub-stream" in cell:
                    annotated_columns.add(i)
        self.assertEqual(
            annotated_columns,
            capped,
            "the sub-stream count must appear in the capped provider columns and no others",
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
                    want = min(jobs, self.CAP[inp])
                    self.assertIn(f"· {want} sub-stream", cell)


class OomKillOracle(unittest.TestCase):
    """Telling an OOM kill from any other failure.

    The exit code cannot: `rss_wrapper` collapses every signal death to exit 1,
    and `--rm` has destroyed the container before `nerdctl inspect` could be
    asked. So the oracle is the container's own `memory.events`, read inside it
    after the timed command — and the three things that make that sound are
    asserted here, because each fails by returning a plausible answer about
    something else.
    """

    EVENTS = "low 0\nhigh 0\nmax 41\noom 2\noom_kill 1\noom_group_kill 0\n"

    def test_the_counter_is_read_off_memory_events(self):
        self.assertEqual(measure.parse_oom_kills(self.EVENTS), 1)

    def test_a_container_that_killed_nothing_answers_zero(self):
        self.assertEqual(measure.parse_oom_kills(self.EVENTS.replace("oom_kill 1", "oom_kill 0")), 0)

    def test_an_unreadable_counter_is_not_zero(self):
        # The distinction the whole oracle exists for: "nothing was killed" and
        # "the harness cannot tell" are different answers, and reading the
        # second as the first is how a kill becomes a silent apparatus failure.
        self.assertIsNone(measure.parse_oom_kills("real\t0m1.000s\n"))

    def test_the_oracle_runs_after_the_timed_command_and_preserves_its_status(self):
        # Inside the timer it would be part of the reading; without the saved
        # status the container would exit on `cat`'s success and every failure
        # would read as a pass.
        self.assertTrue(measure.OOM_ORACLE.startswith("; __st=$?;"))
        self.assertTrue(measure.OOM_ORACLE.rstrip().endswith("exit $__st"))
        self.assertIn("/sys/fs/cgroup/memory.events", measure.OOM_ORACLE)

    def test_it_writes_to_stderr_beside_the_other_two_reports(self):
        # `parse_reported` reads a binary's *stdout*; sending the counter there
        # would put `oom_kill 0` in front of it as a `key=value` line.
        self.assertIn(">&2", measure.OOM_ORACLE)

    def test_it_is_not_part_of_any_command_shape(self):
        # What a figure records as its shape must be exactly what was measured.
        for command in measure.command_shapes():
            with self.subTest(command=command):
                self.assertNotIn("memory.events", measure._script(command))

    def test_the_oracle_adds_a_command_but_never_a_timer(self):
        # `parse_bash_time` refuses more than one `real` line, so a shape that
        # acquired a second timed command would lose its figure — and a shape
        # whose timer count the oracle changed is exactly that.
        self.assertNotIn("time ", measure.OOM_ORACLE)
        for command in measure.command_shapes():
            with self.subTest(command=command):
                bare = measure._script(command)
                self.assertEqual((bare + measure.OOM_ORACLE).count("time "), bare.count("time "))


class KillLicence(unittest.TestCase):
    """Which families may lose a leg to the kernel and continue.

    The licence is narrow on purpose. `parallel-peak-rss` once measured
    3067 MiB inside a 3072 MiB container, and a kill there is the apparatus
    failure that being loud caught; the flagless family is the opposite case,
    since the rule under test aims resident at the allocation by construction.
    """

    def test_the_flagless_family_carries_it(self):
        for spec in measure._reserve_flagless_specs():
            with self.subTest(leg=spec.label):
                self.assertTrue(measure.kill_tolerant(spec.command))

    def test_the_mechanism_legs_carry_it_because_they_are_that_same_shape(self):
        for _, spec in measure._reserve_mechanism_specs():
            with self.subTest(leg=spec.label):
                self.assertTrue(measure.kill_tolerant(spec.command))

    def test_the_resident_figures_that_could_hide_a_near_miss_do_not(self):
        for spec in (*measure._parallel_rss_specs(), *_peak_rss_specs()):
            with self.subTest(leg=spec.label):
                self.assertFalse(measure.kill_tolerant(spec.command))

    def test_the_stated_families_of_the_same_figure_do_not(self):
        # They state a budget far below their container, so a kill there is the
        # apparatus and not the reading.
        for spec in (*measure._reserve_specs(), *measure._reserve_step_specs()):
            with self.subTest(leg=spec.label):
                self.assertFalse(measure.kill_tolerant(spec.command))

    def test_no_shape_outside_the_reserve_figure_is_tolerant(self):
        for command in measure.command_shapes():
            if command.startswith(measure.RESERVE_FLAGLESS):
                continue
            with self.subTest(command=command):
                self.assertFalse(measure.kill_tolerant(command))


class CensoredCells(unittest.TestCase):
    """The reserve table with a leg the kernel killed.

    A censored cell is a **bound on a peak the process never reached**, so what
    this holds is that no number is printed for it, that it leaves the fit, and
    that the figure says it must not be published. All three fail silently
    otherwise: a killed leg whose surviving reps were averaged publishes a
    median of the reps that stayed *under* the ceiling, which is exactly the
    number that makes a too-small reserve look adequate.
    """

    FIGURE = "reserve"
    #: The leg killed in these renders: the 128 MiB file in a 1 GiB allocation,
    #: which is the one an actual sitting lost.
    KILLED = ("control_xz128", "1g")
    #: The worst killed rep's `maxrss_bound_kib` is this plus `100 * rep`, so a
    #: renderer that took the first rep rather than the worst prints a number
    #: this test can tell apart.
    BOUND_KIB = 900_000.0

    def _specs(self):
        return {
            "flagless": measure._reserve_flagless_specs(),
            "instrument": measure._reserve_instrument_specs(),
            "mechanism": [s for _, s in measure._reserve_mechanism_specs()],
            "steps": measure._reserve_step_specs(),
            "stated": measure._reserve_specs(),
        }

    #: One instrument report, shaped as `introspect.rs` writes one. The
    #: quantities are monotone in the leg for the same reason the readings are,
    #: so a renderer that crossed two legs' reports prints a number these tests
    #: can tell apart.
    def _report(self, i):
        return {
            "instrument": "counting-allocator",
            "live_scope": "rust-global-alloc",
            "live_bytes": str(64 << 10),
            "live_peak_bytes": str((100 + 10 * i) * measure.MIB),
            "glibc_scope": "whole-process",
            "mallinfo_arena": str((150 + 10 * i) * measure.MIB),
            "mallinfo_hblkhd": "0",
            "mallinfo_uordblks": str((60 + 10 * i) * measure.MIB),
            "mallinfo_fordblks": str(90 * measure.MIB),
            "malloc_heaps": "6",
            "malloc_system_current": str((150 + 10 * i) * measure.MIB),
            "malloc_system_max": str((260 + 10 * i) * measure.MIB),
        }

    def _raw(self, killed_reps=3, total_reps=3):
        specs = self._specs()
        every = [
            *specs["flagless"],
            *specs["instrument"],
            *specs["mechanism"],
            *specs["steps"],
            *specs["stated"],
            measure._RESERVE_BASELINE,
        ]
        rss, reported, killed, instrument = {}, {}, {}, {}
        for i, spec in enumerate(every):
            key = spec.key(self.FIGURE)
            # Monotone in the leg, so the flagless fit is well conditioned and
            # the numbers below are never two legs' readings by accident.
            rss[key] = [100000.0 + 1000 * i + 10 * r for r in range(total_reps)]
            reported[key] = {
                "resolved_jobs": str(2 + i % 5),
                # Above `charge_bytes(128 MiB, 1)` — 522.0 MiB — so every
                # flagless leg of this fixture takes the block path and the fit
                # below it has legs to cover. That line carries one reader's
                # share of the pool's retention list (`block_path_afforded`).
                "resolved_budget": str(measure.charge_bytes(128 << 20, 1) + (1 << 20)),
            }
            if spec.instrument:
                instrument[key] = [self._report(i)] * total_reps
        # The killed reps' own records, exactly as `time_run`'s kill branch
        # writes them: the constraint line is rendered off `runs`, so a fixture
        # with an empty one would exercise only the nothing-was-reported branch.
        runs = []
        for spec in specs["flagless"]:
            if (spec.input, spec.memory) == self.KILLED:
                key = spec.key(self.FIGURE)
                killed[key] = killed_reps
                rss[key] = rss[key][: total_reps - killed_reps]
                for rep in range(killed_reps):
                    runs.append(
                        {
                            "figure": self.FIGURE,
                            "spec": dataclasses.asdict(spec),
                            "seconds": None,
                            "maxrss_kib": None,
                            "killed": True,
                            "maxrss_bound_kib": self.BOUND_KIB + 100 * rep,
                            "seconds_to_kill": 4.0 + rep,
                            "oom_kill": 1,
                            "exit": 1,
                        }
                    )
        return {
            "readings": {k: [1.0] * total_reps for k in rss},
            "rss": rss,
            "killed": killed,
            "reported": reported,
            "instrument": instrument,
            "input_sizes": {name: 1 << 30 for name in measure.INPUTS},
            "runs": runs,
        }

    def _render(self, **kwargs):
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(
                measure.Config(), self._raw(**kwargs), Path(tmp), lambda _m: None
            )
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                return measure.run_reserve(session), session

    def test_a_wholly_killed_leg_prints_no_number(self):
        body, _ = self._render()
        cells = [c.strip() for r in body.splitlines() if r.startswith("| ") for c in r.split("|")]
        killed = [c for c in cells if "OOM-killed" in c]
        self.assertTrue(killed, "the killed leg's cell says nothing about the kill")
        for cell in killed:
            self.assertNotIn("MiB", cell.split("·")[0])

    def test_a_killed_leg_is_out_of_the_fit_and_the_fit_says_so(self):
        body, _ = self._render()
        self.assertIn("out of the fit", body)
        self.assertIn("a bound on a peak the process never reached", body)

    def test_a_partly_killed_leg_keeps_its_reps_and_declares_the_kill(self):
        # A partly-censored leg's median understates, so the cell may show its
        # surviving reps only with the kill beside it — and it still leaves the
        # fit.
        body, _ = self._render(killed_reps=1)
        self.assertIn("1 rep(s) OOM-killed", body)
        self.assertIn("out of the fit", body)

    def test_a_killed_leg_is_printed_as_a_constraint_and_names_its_allocation(self):
        # The reading the fit cannot use and the figure must not lose: what the
        # kill still proves. Discarded, it left `KILL_TOLERANT` recording a
        # reading and the renderer throwing it away.
        body, _ = self._render()
        self.assertIn("What the killed legs still prove", body)
        self.assertIn("**not** fitted", body)
        self.assertIn(f"`-m {self.KILLED[1]}`", body)
        self.assertIn("did not fit 1,024 MiB", body)

    def test_the_constraint_carries_the_worst_reps_floor_and_calls_it_one(self):
        # `maxrss_bound_kib` and `seconds_to_kill` reached `raw.json` with
        # nothing reading them. The worst rep's pair, printed as one rep's, and
        # labelled a floor rather than a peak.
        body, _ = self._render()
        worst = self.BOUND_KIB + 100 * 2
        self.assertIn(measure.fmt_mib(worst), body)
        self.assertIn("6.0 s in", body)
        self.assertIn("a floor on the peak, not the peak", body)

    def test_a_censored_leg_with_no_recorded_bound_says_so_rather_than_guessing(self):
        # A sitting taken before the kill branch existed carries `killed` and no
        # `runs`. An empty bound list means *nothing is known*, which is not the
        # same as zero and must not print as a number.
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            raw["runs"] = []
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        self.assertIn("reported no bound before it died", body)
        self.assertIn("did not fit 1,024 MiB", body)

    def test_every_fit_states_the_window_it_covers(self):
        # `evidence` rule 2 with the sign flipped: a leg is censored exactly
        # when its resident ran closest to its ceiling, so a fit over what
        # survives is a fit over the legs that had room. Naming only what left
        # asks the reader to subtract from a tuple the table never prints.
        body, _ = self._render()
        section = body.split("Resident against the reader count")[1].split(
            "What the killed legs still prove"
        )[0]
        fits = [ln for ln in section.splitlines() if ln.startswith("- **")]
        self.assertEqual(len(fits), len(measure.RESERVE_FLAGLESS_INPUTS))
        for line in fits:
            self.assertTrue(
                any(w in line for w in ("it covers", "what is left is", "left cover")),
                f"the line states no window: {line}",
            )
            # Every window names allocations, not just reader counts.
            self.assertTrue(
                any(f"`{token}` at" in line for token, _ in measure.RESERVE_LIMITS),
                f"the window names no allocation: {line}",
            )

    def test_every_published_term_names_the_pool_it_excludes(self):
        # Both terms of every line here are the **remainder** — what a
        # leg held outside the block pool's retention list, which is subtracted
        # before the fit because it is known in advance and bends the line at
        # `POOL_DEPTH`. A reader who takes them for the whole of what a leg held
        # is out by that term: 72 MiB at one reader of 24 MiB blocks, 384 at
        # 128, and a unit a reader more above four.
        body, _ = self._render()
        section = body.split("Resident against the reader count")[1].split(
            "What the killed legs still prove"
        )[0]
        self.assertIn("(POOL_DEPTH.max(jobs) − 1) × unit", section)
        fits = [ln for ln in section.splitlines() if ln.startswith("- **")]
        self.assertEqual(len(fits), len(measure.RESERVE_FLAGLESS_INPUTS))
        for line in fits:
            self.assertIn("outside the pool", line)
        live = next(
            ln for ln in body.splitlines() if ln.startswith("**What the program itself held")
        )
        self.assertIn("outside the block pool", live)
        # And the comparison that reads the slope: both sides exclude the list,
        # `reader_bytes` being the per-worker term without it.
        self.assertIn("outside the pool plus", live)

    def test_the_no_line_branch_states_its_window_too(self):
        # "No line" is a claim about a window as much as a fit is, and it was the
        # branch that named only the declined legs. Reached by resolving one
        # reader count everywhere, which is what leaves a single point — below
        # even a secant, since a secant needs two abscissae as much as a fit
        # does.
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            for report in raw["reported"].values():
                report["resolved_jobs"] = "4"
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        no_line = [ln for ln in body.splitlines() if ln.startswith("- **") and "no line" in ln]
        self.assertEqual(len(no_line), len(measure.RESERVE_FLAGLESS_INPUTS))
        for line in no_line:
            self.assertIn("what is left is", line)
            self.assertTrue(
                any(f"`{token}` at" in line for token, _ in measure.RESERVE_LIMITS),
                f"the window names no allocation: {line}",
            )

    def _two_count_body(self):
        counts = [4, 4, 4, 4, 5, 5]
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            for key, report in raw["reported"].items():
                report["resolved_jobs"] = str(counts[sum(map(ord, key)) % len(counts)])
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                return measure.run_reserve(session)

    def test_a_two_point_family_publishes_a_secant_and_no_intercept(self):
        # The guard is three distinct reader counts, because a two-term model
        # passes exactly through two points and prints `±0 MiB` as though it
        # were a residual. What it withholds is the intercept: the slope
        # between two counts is a difference the axis measured and carries no
        # model claim, so it is published and the intercept is not.
        self.assertEqual(measure.RESERVE_FIT_MIN_COUNTS, 3)
        body = self._two_count_body()
        section = body.split("Resident against the reader count")[1].split(
            "What the killed legs still prove"
        )[0]
        lines = [ln for ln in section.splitlines() if ln.startswith("- **")]
        self.assertEqual(len(lines), len(measure.RESERVE_FLAGLESS_INPUTS))
        for line in lines:
            with self.subTest(line=line):
                self.assertIn("secant", line)
                self.assertIn("a reader **", line)
                # The slope is the remainder's too (`_depooled`).
                self.assertIn("outside the pool", line)
                self.assertIn("distinct reader count(s), under the 3", line)
                # The two things the guard withholds, and nothing else: no
                # intercept, and no residual value printed beside it.
                self.assertNotIn("fixed **", line)
                self.assertNotIn("residuals reach", line)
                # Named endpoints, so a reader knows which difference it is.
                self.assertIn(" → ", line)
                self.assertTrue(
                    any(f"`{token}`" in line for token, _ in measure.RESERVE_LIMITS),
                    f"the secant names no allocation: {line}",
                )

    def test_the_instrument_family_crosses_the_same_guard(self):
        # The guard is a property of the model, so it cannot live in one
        # renderer, or the instrument account admits an intercept at two
        # distinct counts while the flagless axis refuses one in the same
        # sitting.
        body = self._two_count_body()
        live = [ln for ln in body.splitlines() if "a reader" in ln and "secant" in ln]
        self.assertTrue(
            any("What a reader costs the program" in ln for ln in live),
            "the instrument account published something other than a secant",
        )
        line = next(ln for ln in live if "What a reader costs the program" in ln)
        self.assertNotIn("fixed **", line)
        self.assertNotIn("a residual of", line)
        # Named legs, not bare counts: a difference nobody can locate is a
        # difference nobody can re-take.
        self.assertIn(" → ", line)
        self.assertTrue(
            any(f"in {token}" in line for token in measure.RESERVE_INSTRUMENT_LIMITS),
            f"the secant names no leg: {line}",
        )
        # The comparison a censored sitting would otherwise lose silently: it
        # reads the slope alone, so a secant keeps it. Both sides exclude the
        # pool's retention list, which is what makes them commensurable
        # (`_depooled`).
        self.assertIn("`BlockCache::reader_bytes` bills", line)
        self.assertIn("outside the pool", line)

    def test_the_guard_is_above_the_arithmetic_floor_it_sits_on(self):
        # Two floors, deliberately: `_least_squares` refuses one point because
        # the line is undefined there, and the publication guard refuses two
        # because the residual is. Collapsing them would make the arithmetic
        # answer the publication question.
        self.assertGreater(measure.RESERVE_FIT_MIN_COUNTS, 2)
        fixed, slope = measure._least_squares([(1, 100.0), (2, 200.0)])
        self.assertAlmostEqual(fixed, 0.0)
        self.assertAlmostEqual(slope, 100.0)

    def test_the_figure_reports_its_kills_for_the_publication_bar(self):
        _, session = self._render()
        kills = session.figure_kills(self.FIGURE)
        self.assertEqual(len(kills), 1)
        self.assertEqual(sum(kills.values()), 3)

    def test_the_note_pasted_above_the_table_refuses_publication(self):
        _, session = self._render()
        note = measure.censored_note(session.figure_kills(self.FIGURE))
        self.assertIn("must not be published", note)
        self.assertIn("OOM-killed", note)

    # -- the attribution -------------------------------------------------

    def test_the_account_adds_the_decoder_dictionaries_back_by_hand(self):
        # `liblzma` allocates through C `malloc`, so the counting allocator
        # cannot see it and glibc cannot separate it. A decomposition that
        # subtracted the two families without this term charges 8 MiB a reader
        # to retention — which is the term the whole sitting is about.
        body, session = self._render()
        section = body.split("The account, term by term")[1].split("What the program itself")[0]
        self.assertIn("Decoder dictionaries", section)
        self.assertIn(f"{measure.XZ_DICT_BYTES:,} bytes", body)
        rows = [ln for ln in section.splitlines() if ln.startswith("| instrument")]
        self.assertEqual(len(rows), len(measure._reserve_instrument_specs()))
        # The column is `readers x dictionary` exactly, read off the
        # arrangement the run itself reported rather than off the limit — so
        # the fixture's own resolved counts are what it has to reproduce.
        readers = [
            int(session.reported[spec.key(self.FIGURE)]["resolved_jobs"])
            for spec in measure._reserve_instrument_specs()
        ]
        for row, count in zip(rows, readers):
            self.assertIn(
                measure._fmt_budget_bytes(count * measure.XZ_DICT_BYTES), row
            )

    def test_the_fit_is_stated_against_the_charge_the_source_bills(self):
        # The point of taking the account: what a reader costs the program plus
        # the C dictionary the counter is blind to, against what
        # `XzSource::block_reader_bytes` bills a sub-stream. It is the one
        # comparison that says whether the number the budget rule divides by is
        # the number a reader actually is.
        body, _ = self._render()
        line = next(ln for ln in body.splitlines() if ln.startswith("**What the program itself"))
        self.assertIn("Against the charge", line)
        self.assertIn(
            measure._fmt_budget_bytes(measure.reader_bytes(measure.RESERVE_MECHANISM_UNIT)),
            line,
        )

    def test_the_two_columns_that_can_leave_range_are_explained(self):
        # `RSS − heap high-water` goes negative and the `fordblks` share passes
        # 100% whenever two high-waters are taken at different instants. Both
        # read as apparatus faults unless the table says otherwise.
        body, _ = self._render()
        section = body.split("The account, term by term")[1].split("What the program itself")[0]
        self.assertIn("RSS − heap high-water", section)
        self.assertIn("non-simultaneity", section)

    def test_the_program_fit_is_evaluated_inside_its_own_window(self):
        # An intercept is a physical quantity only where the fit still holds
        # where the mechanism is simplest: a `403 MiB` intercept fitted over
        # 3–24 readers predicts 436 MiB at one reader, where the process holds
        # 62.9 (`.claude/skills/evidence/SKILL.md`, rule 2).
        body, _ = self._render()
        line = next(ln for ln in body.splitlines() if ln.startswith("**What the program itself"))
        self.assertIn("smallest arrangement in its own window", line)
        self.assertIn("residual", line)

    def test_the_remainder_carries_a_name_or_says_it_has_none(self):
        # Never a bare number carried across sessions: a residual with no owner
        # acquires a false one.
        body, _ = self._render()
        self.assertTrue(
            "**The remainder has a name**" in body or "**The remainder has no name here**" in body,
            "the account ends on neither a name nor an explicit no-name",
        )

    def test_the_check_states_the_arrangement_and_the_tolerance(self):
        # The exact half is whether the instrument build resolved the shipped
        # build's arrangement; the resident half is a stated tolerance, because
        # a different binary cannot be held to the shipped leg's own spread.
        body, _ = self._render()
        checks = [ln for ln in body.splitlines() if ln.startswith("- `-m ")]
        self.assertEqual(len(checks), len(measure.RESERVE_INSTRUMENT_LIMITS))
        for line in checks:
            self.assertIn("resolved", line)
            self.assertIn(f"{measure.INSTRUMENT_TOLERANCE_PCT}%", line)

    def test_a_censored_instrument_leg_prints_no_terms(self):
        # The report is written at exit, so a killed leg has none — and a
        # renderer that read a missing field as zero would print an account
        # summing to a resident set nobody measured.
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            spec = measure._reserve_instrument_specs()[0]
            key = spec.key(self.FIGURE)
            raw["instrument"][key] = []
            raw["rss"][key] = []
            raw["killed"][key] = 3
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        row = next(
            ln for ln in body.splitlines() if ln.startswith(f"| {spec.label} |")
        )
        self.assertIn("OOM-killed", row)
        self.assertNotIn("MiB", row)

    def test_an_untouched_sitting_carries_no_kill_and_no_note(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            raw["killed"] = {}
            for spec in measure._reserve_flagless_specs():
                key = spec.key(self.FIGURE)
                if len(raw["rss"][key]) < 3:
                    raw["rss"][key] = [100000.0, 100010.0, 100020.0]
            session = measure.ReplaySession(
                measure.Config(), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        self.assertNotIn("OOM-killed", body)
        self.assertEqual(session.figure_kills(self.FIGURE), {})


class ChargeModelSection(unittest.TestCase):
    """The model check the reserve renderer prints, cell by cell.

    What it is written against is the account done by hand: a sitting that
    reports forty headroom percentages and leaves the account to whoever reads
    the log afterwards. Three things fail silently here. A **criterion stated
    after the answer** reads as a description of whatever came back. A **pool
    term folded into the remainder** reads as a term nothing accounts for and
    as a breach on a larger block size, which is why the charge bills it. And a
    **declined or censored leg
    left in the table** evaluates the model at a leg that ran none of it.
    """

    FIGURE = CensoredCells.FIGURE
    KILLED = CensoredCells.KILLED
    BOUND_KIB = CensoredCells.BOUND_KIB

    # The reserve renderer's one fixture, borrowed rather than rebuilt: a
    # second raw dict is a second account of what a sitting records, and the
    # two drift.
    _specs = CensoredCells._specs
    _report = CensoredCells._report
    _raw = CensoredCells._raw
    _render = CensoredCells._render

    #: One flagless arrangement per input and limit: reader count and worst rep
    #: in MiB.
    #:
    #: The four original limits are the reserve constant's five-build grid at
    #: its r384 build — the 24 MiB rows are that build's headroom column
    #: inverted against its container limit, the 128 MiB rows the same readings
    #: `SEED_CELLS` carries. **`544m` and `1088m` are constructed**, no sitting
    #: having measured them: each is a one- or two-reader arrangement whose worst
    #: rep satisfies the criterion, chosen so the model table carries a
    #: one-reader cell at each block size, which no cell of the four-limit grid
    #: reached.
    #:
    #: **The budget a cell renders under is its own arrangement's charge, not
    #: its container's allowance** (`_seeded_body`), so a cell here takes the
    #: block path whatever `-m` it carries — which is why `control_xz128` at
    #: `512m` bills 660 MiB inside 512. The fixture is exercising the renderer's
    #: arithmetic over a reader count; which limits afford which count is
    #: `reserve_axis_problems`' question and is checked there.
    SEEDED = {
        ("control_xz", "512m"): (2, 251.9),
        ("control_xz", "544m"): (1, 240.0),
        ("control_xz", "1g"): (11, 817.2),
        ("control_xz", "1088m"): (12, 860.0),
        ("control_xz", "1536m"): (19, 1224.2),
        ("control_xz", "2g"): (24, 1515.5),
        ("control_xz128", "512m"): (2, 802.1),
        ("control_xz128", "544m"): (1, 760.0),
        ("control_xz128", "1g"): (3, 939.9),
        ("control_xz128", "1088m"): (2, 900.0),
        ("control_xz128", "1536m"): (4, 1077.7),
        ("control_xz128", "2g"): (5, 1342.6),
    }

    def _seeded_body(self, bumps=None):
        """The renderer over a fixture whose flagless legs are the five-build
        grid's own readings, nothing killed.

        `bumps` adds MiB to one leg's worst rep, which is how a cell is put in a
        chosen band of the criterion without inventing a second fixture."""
        raw = self._raw()
        raw["killed"] = {}
        units = {name: unit for name, _, unit in measure.RESERVE_FLAGLESS_INPUTS}
        for spec in measure._reserve_flagless_specs():
            jobs, worst = self.SEEDED[(spec.input, spec.memory)]
            worst += (bumps or {}).get((spec.input, spec.memory), 0.0)
            key = spec.key(self.FIGURE)
            raw["rss"][key] = [worst * 1024 - 20, worst * 1024 - 10, worst * 1024]
            raw["reported"][key] = {
                "resolved_jobs": str(jobs),
                "resolved_budget": str(measure.charge_bytes(units[spec.input], jobs)),
            }
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                return measure.run_reserve(session)

    def _unnamed_at(self, cell):
        """What the model leaves unnamed at one seeded cell, in bytes."""
        units = {name: unit for name, _, unit in measure.RESERVE_FLAGLESS_INPUTS}
        jobs, worst = self.SEEDED[cell]
        return worst * measure.MIB - measure.charge_bytes(units[cell[0]], jobs)

    def _bump_to(self, cell, unnamed):
        """MiB to add to one seeded cell's worst rep so that its unnamed
        remainder lands at `unnamed` bytes.

        Computed rather than written, because the bands are bounded on both
        sides: a bump stated as "the bound plus a megabyte" lands in whichever
        band the cell's own remainder plus that number falls in, which is the
        `rule` band as often as not."""
        return (unnamed - self._unnamed_at(cell)) / measure.MIB

    def _model_rows(self, body):
        section = body.split("The charge against what was held")[1].split(
            "What the process says it held"
        )[0]
        return [ln for ln in section.splitlines() if ln.startswith("| ")]

    def test_the_criterion_is_stated_before_the_answer(self):
        body, _ = self._render()
        section = body.split("The charge against what was held")[1].split(
            "What the process says it held"
        )[0]
        # The prose above the table is the criterion; whatever follows the last
        # row of it is the answer. Read that way rather than off the verdict's
        # opening words, which name the band and so are not one sentence.
        lines = section.splitlines()
        last_row = max(i for i, ln in enumerate(lines) if ln.startswith("| "))
        criterion = "\n".join(lines[:last_row])
        verdict = "\n".join(lines[last_row + 1 :]).strip()
        self.assertIn("non-negative", criterion)
        self.assertIn("MEMORY_RESERVE", criterion)
        # Both ceilings, and which finding each is: a section stating one of
        # them would read as a criterion while answering the other question.
        self.assertIn("MEMORY_UNPOOLED_BOUND", criterion)
        self.assertIn("the **bound** is wrong", criterion)
        self.assertIn("over-bill", criterion)
        # And that the column below says which of the three a cell crossed,
        # since that is what decides how the cell is read.
        self.assertIn("names the band", criterion)
        self.assertTrue(verdict, "the section states a criterion and never answers it")

    def test_the_seeded_readings_pass_and_the_floor_is_its_own_column(self):
        body = self._seeded_body()
        self.assertIn("The model holds at every cell above", body)
        rows = self._model_rows(body)
        # Eight flagless legs, and every one of them took the block path at the
        # budget its own reported arrangement carries.
        self.assertEqual(len(rows), len(self.SEEDED) + 1)
        pooled = next(r for r in rows if "128 MiB blocks, `-m 512m`" in r)
        # `(POOL_DEPTH - 1) x 128 MiB`, named rather than left in the remainder.
        self.assertIn(measure._fmt_budget_bytes(3 * (128 << 20)), pooled)
        self.assertIn(measure._fmt_budget_bytes(142.0 * measure.MIB)[:5], pooled)

    def test_the_pool_term_is_billed_at_every_cell_of_each_block_size(self):
        # The column exists to keep the pool out of the remainder, and a table
        # in which any cell reads `—` there is a cell whose bill has lost its
        # larger half. The term never clamps off, so every row of every block
        # size carries it.
        rows = self._model_rows(self._seeded_body())
        for _name, label, unit in measure.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(block_size=label):
                mine = [r for r in rows if r.startswith(f"| {label}, ")]
                self.assertTrue(mine, f"{label} has no cell at all")
                for row in mine:
                    readers = int(row.split("|")[2].strip().rstrip("r"))
                    self.assertIn(
                        measure._fmt_budget_bytes(
                            (max(measure.LIBRARY_POOL_DEPTH, readers) - 1) * unit
                        ),
                        row,
                    )

    def test_an_over_bill_is_named_in_those_words_and_refutes(self):
        # The default fixture holds ~100 MiB against a charge of several
        # hundred, which is the over-bill side of the criterion. It refutes —
        # it is never apparatus scatter, having to exceed the whole of the rest
        # of the process's footprint before the arithmetic reports it at all,
        # and the sitting discharges nothing by printing it.
        body, _ = self._render()
        self.assertIn("over-billed", body)
        self.assertIn("The model is refuted, and by these cells:", body)
        # And the refutation does not bar publication.
        self.assertIn("The table publishes all the same", body)
        # And the verdict says why *this* band refutes rather than asserting the
        # rule band's reason over every cell in the stanza.
        self.assertIn(measure.band_refutes(measure.BAND_OVER_BILL), body)
        self.assertNotIn("a finding rather than a refutation", body)

    def test_a_cell_above_the_bound_alone_publishes_with_its_finding(self):
        # Between the two lines the allocation holds and the number the
        # count is predicted against is wrong. The verdict says which, and
        # re-derives the bound from the sitting's own remainders rather than
        # owing a re-take.
        over = measure.LIBRARY_MEMORY_UNPOOLED_BOUND + measure.MIB
        bump = self._bump_to(("control_xz128", "1g"), over)
        body = self._seeded_body(bumps={("control_xz128", "1g"): bump})
        self.assertNotIn("The model is refuted", body)
        self.assertIn("a finding rather than a refutation", body)
        self.assertIn("MEMORY_UNPOOLED_BOUND", body)
        # The bumped cell is the sitting's worst remainder, rounded up on the
        # 64 MiB grid the constant was read off.
        rederived = measure.rederived_unpooled_bound(over)
        self.assertIn(
            f"re-derive `MEMORY_UNPOOLED_BOUND` at **{measure._fmt_budget_bytes(rederived)}**",
            body,
        )
        self.assertIn("| **bound** |", body)

    def test_a_cell_above_the_reserve_refutes_and_says_so(self):
        # The outer line: a remainder the reserve cannot cover is an
        # arrangement the discovery cannot keep inside its allocation.
        bump = self._bump_to(
            ("control_xz128", "1g"), measure.LIBRARY_MEMORY_RESERVE + measure.MIB
        )
        body = self._seeded_body(bumps={("control_xz128", "1g"): bump})
        self.assertIn("The model is refuted, and by these cells:", body)
        self.assertIn("The table publishes all the same", body)
        self.assertIn(measure.band_refutes(measure.BAND_RULE), body)
        self.assertIn("| **rule** |", body)

    def test_the_two_bands_are_reported_apart_in_one_sitting(self):
        # The case one sentence over both cannot state: a cell of each band. The
        # refutation is named on the rule cells alone, and the bound cell is
        # published beside it with its finding.
        body = self._seeded_body(
            bumps={
                ("control_xz128", "1g"): self._bump_to(
                    ("control_xz128", "1g"),
                    measure.LIBRARY_MEMORY_RESERVE + measure.MIB,
                ),
                ("control_xz", "2g"): self._bump_to(
                    ("control_xz", "2g"),
                    measure.LIBRARY_MEMORY_UNPOOLED_BOUND + measure.MIB,
                ),
            }
        )
        self.assertIn("The model is refuted, and by these cells:", body)
        self.assertIn("a finding rather than a refutation", body)
        refuting = body.split("The model is refuted")[1].split("Inside the rule")[0]
        self.assertIn("128 MiB blocks** at `-m 1g`", refuting)
        self.assertNotIn("24 MiB blocks** at `-m 2g`", refuting)
        # And no bound is re-derived here: a sitting in which the model is
        # refuted has a worst remainder that is not a reading a term of that
        # model may be sized to.
        self.assertNotIn("re-derive `MEMORY_UNPOOLED_BOUND`", body)
        self.assertIn("No bound is re-derived from this sitting", body)

    def test_a_declined_leg_is_absent_rather_than_evaluated(self):
        # The streaming fallback holds none of the model's terms, so a leg whose
        # reported budget cannot afford one reader is not a cell of it.
        raw = self._raw()
        raw["killed"] = {}
        spec = next(
            s
            for s in measure._reserve_flagless_specs()
            if (s.input, s.memory) == ("control_xz128", "512m")
        )
        for other in measure._reserve_flagless_specs():
            jobs, worst = self.SEEDED[(other.input, other.memory)]
            key = other.key(self.FIGURE)
            raw["rss"][key] = [worst * 1024]
            units = {n: u for n, _, u in measure.RESERVE_FLAGLESS_INPUTS}
            raw["reported"][key] = {
                "resolved_jobs": str(jobs),
                "resolved_budget": str(measure.charge_bytes(units[other.input], jobs)),
            }
        raw["reported"][spec.key(self.FIGURE)] = {
            "resolved_jobs": "1",
            "resolved_budget": str(measure.charge_bytes(128 << 20, 1) - 1),
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        rows = self._model_rows(body)
        self.assertEqual(len(rows), len(self.SEEDED))
        self.assertFalse([r for r in rows if "128 MiB blocks, `-m 512m`" in r])

    def test_no_cell_at_all_is_said_rather_than_read_as_a_pass(self):
        # "Every cell holds" and "there were no cells" are not the same claim,
        # and the second is what a sitting where everything declined produces.
        raw = self._raw()
        raw["killed"] = {}
        for spec in measure._reserve_flagless_specs():
            raw["reported"][spec.key(self.FIGURE)] = {
                "resolved_jobs": "1",
                "resolved_budget": "1",
            }
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        self.assertIn("The model was evaluated at no cell", body)
        self.assertNotIn("The model holds at every cell", body)
        self.assertEqual(len(self._model_rows(body)), 1)

    def test_a_censored_leg_is_absent_rather_than_evaluated(self):
        # A kill leaves a bound on a peak never reached, which is not a `held`
        # the model may be evaluated at — the exclusion the fit already makes.
        body, _ = self._render()
        rows = self._model_rows(body)
        self.assertFalse([r for r in rows if f"`-m {self.KILLED[1]}`" in r and "128 MiB" in r])

    # -- which path each cell ran ------------------------------------------

    def _flagless_rows(self, body):
        section = body.split("What a flagless scan resolves")[1].split(
            "Resident against the reader count"
        )[0]
        return [ln for ln in section.splitlines() if ln.startswith("| `-m ")]

    def test_every_flagless_cell_names_the_path_it_ran(self):
        # The block path and the streaming fallback hold different
        # things, so a column mixing them is two series printed as one — and an
        # unmarked cell cannot be told from a cell nobody checked. Every cell
        # says which, including the ones that took the block path.
        body = self._seeded_body()
        rows = self._flagless_rows(body)
        self.assertEqual(len(rows), len(measure.RESERVE_LIMITS))
        for row in rows:
            with self.subTest(row=row):
                for cell in row.split("|")[2:-1]:
                    self.assertTrue(
                        "*block path*" in cell or "*streaming*" in cell,
                        f"cell names no path: {cell}",
                    )

    def test_a_declined_cell_says_streaming_where_a_block_cell_does_not(self):
        # The seeded fixture's budgets are what each leg's own reported
        # arrangement carries, so every cell takes the block path; dropping one
        # leg's budget a byte below the line is the only difference.
        raw = self._raw()
        raw["killed"] = {}
        units = {n: u for n, _, u in measure.RESERVE_FLAGLESS_INPUTS}
        target = ("control_xz128", "512m")
        for spec in measure._reserve_flagless_specs():
            jobs, worst = self.SEEDED[(spec.input, spec.memory)]
            key = spec.key(self.FIGURE)
            raw["rss"][key] = [worst * 1024]
            budget = (
                measure.charge_bytes(units[spec.input], 1) - 1
                if (spec.input, spec.memory) == target
                else measure.charge_bytes(units[spec.input], jobs)
            )
            raw["reported"][key] = {"resolved_jobs": str(jobs), "resolved_budget": str(budget)}
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.ReplaySession(measure.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                measure, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = measure.run_reserve(session)
        row = next(r for r in self._flagless_rows(body) if r.startswith(f"| `-m {target[1]}`"))
        cells = row.split("|")[2:-1]
        # `RESERVE_FLAGLESS_INPUTS` order is the column order, and the declined
        # leg is the second of them.
        self.assertIn("*block path*", cells[0])
        self.assertIn("*streaming*", cells[1])
        self.assertNotIn("*block path*", cells[1])


class CensoredSittingsBarPublication(unittest.TestCase):
    """What `emit` does with a figure that lost a leg, and what `--render`
    reproduces."""

    def test_emit_bars_publication_and_names_the_figure(self):
        source = inspect.getsource(measure.emit)
        self.assertIn("NOT PUBLISHABLE — a leg was OOM-killed", source)
        self.assertIn("censored_note(kills)", source)
        self.assertIn("failures or censored", source)

    def test_an_untaken_instruments_sitting_can_be_re_rendered(self):
        """`render` reads `SELECTABLE_BY_ID`, which is wider than `FIGURES`.

        It looked its figures up in `FIGURES`, so a diagnostic sitting of an
        untaken instrument came back "unknown figure" — and an untaken
        instrument is the case that needs re-rendering most, being taken
        repeatedly while its renderer is still being written. `UNTAKEN` is
        empty today, so nothing in the register exercises the difference; the
        past sittings that need re-rendering are exactly the ones taken while
        it was not."""
        self.assertIn("by_id = SELECTABLE_BY_ID", inspect.getsource(measure.render))
        self.assertEqual(
            {f.id for f in measure.SELECTABLE},
            {f.id for f in measure.FIGURES} | {f.id for f in measure.UNTAKEN},
        )

    def test_a_sitting_records_its_kills_so_a_render_reproduces_the_cells(self):
        # The same reconciliation `test_a_sitting_records_what_a_render_needs`
        # makes for every other recorded field: `render` reads `killed` off the
        # sitting, so `emit` must write it.
        self.assertIn('"killed": session.killed', inspect.getsource(measure.emit))
        self.assertIn('raw.get("killed"', inspect.getsource(measure.ReplaySession.__init__))
        self.assertIn("censored_note(kills)", inspect.getsource(measure.render))


class TimeRunHandlesAKill(unittest.TestCase):
    """`Session.time_run`'s kill branch, which no other test reaches.

    It is the one path that only runs when something dies, so it is the one
    most likely to be wrong when it finally does — the previous sitting spent
    an hour finding that out.
    """

    EVENTS = "low 0\nhigh 0\nmax 22\noom 1\noom_kill 1\noom_group_kill 0\n"
    TIMED = "maxrss_kib=68228\n\nreal\t0m0.012s\nuser\t0m0.008s\nsys\t0m0.004s\n"

    def _session(self):
        cfg = measure.Config()
        with tempfile.TemporaryDirectory() as tmp:
            session = measure.Session(cfg, measure.Stager(cfg, lambda _m: None), lambda _m: None)
        session.figure_id = "reserve"
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        return session

    def _run(self, spec, stderr, returncode):
        session = self._session()
        proc = subprocess.CompletedProcess([], returncode, stdout="", stderr=stderr)
        with unittest.mock.patch.object(measure.subprocess, "run", lambda *a, **k: proc):
            return session, session.time_run(spec)

    def _flagless(self):
        return measure._reserve_flagless_specs()[0]

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
            self._run(measure._parallel_rss_specs()[0], self.TIMED + self.EVENTS, 1)
        self.assertIn("OOM-killed", str(caught.exception))

    def test_a_surviving_run_records_the_counter_beside_its_reading(self):
        session, seconds = self._run(
            self._flagless(), self.TIMED + self.EVENTS.replace("oom_kill 1", "oom_kill 0"), 0
        )
        self.assertFalse(session._last_killed)
        self.assertAlmostEqual(seconds, 0.012)
        self.assertEqual(session.records[0]["oom_kill"], 0)


class Arms(unittest.TestCase):
    """`--pin-cpus` and `--stage-binaries` (`M178`): an arrangement a leg is
    placed under — unpinned and staged by default, that being the recorded
    apparatus — and read against the recorded one leg by leg.

    Each way the experiment could read something other than what it claims is
    held here: a leg placed by a count it does not run at, a second arm leaking
    into a rendered table, an arm that is not the recorded apparatus passing as
    publishable, and the untimed additions reaching the timer."""

    # This machine's topology, as sysfs spells it: four L3 groups of six.
    GROUPS = [
        measure.parse_cpu_list(t)
        for t in ("0-2,12-14", "3-5,15-17", "6-8,18-20", "9-11,21-23")
    ]
    TIMED = "\nreal\t0m0.012345s\nuser\t0m0.008000s\nsys\t0m0.004000s\n"
    EVENTS = "oom_kill 0\n"

    def test_cpu_lists_round_trip(self):
        self.assertEqual(measure.parse_cpu_list("0-2,12-14"), frozenset({0, 1, 2, 12, 13, 14}))
        self.assertEqual(measure.parse_cpu_list("5"), frozenset({5}))
        self.assertEqual(measure.cpu_list({3, 4, 5, 15, 16, 17}), "3-5,15-17")
        self.assertEqual(measure.cpu_list({1, 3}), "1,3")

    def test_l3_groups_are_read_off_sysfs_by_lowest_cpu(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for cpu, shared in ((0, "0,2"), (1, "1,3"), (2, "0,2"), (3, "1,3")):
                d = root / f"cpu{cpu}/cache/index3"
                d.mkdir(parents=True)
                (d / "shared_cpu_list").write_text(shared + "\n")
            self.assertEqual(measure.l3_groups(root), [frozenset({0, 2}), frozenset({1, 3})])

    def test_the_placement_follows_the_stated_count(self):
        small, large = self.GROUPS[1], self.GROUPS[0] | self.GROUPS[1]
        self.assertEqual(measure.placement(1, self.GROUPS), small)
        self.assertEqual(measure.placement(6, self.GROUPS), small)
        self.assertEqual(measure.placement(7, self.GROUPS), large)
        self.assertEqual(measure.placement(12, self.GROUPS), large)
        # The whole machine's subject stays on the whole machine.
        self.assertIsNone(measure.placement(16, self.GROUPS))
        self.assertIsNone(measure.placement(None, self.GROUPS))
        # The harness never shares a group with a pinned leg.
        self.assertFalse(measure.harness_cpus(self.GROUPS) & large)

    def test_another_topology_is_refused_rather_than_guessed(self):
        self.assertIsNone(measure.pin_topology_problem(self.GROUPS))
        self.assertIn("4 equal L3 groups", measure.pin_topology_problem(self.GROUPS[:2]))
        uneven = [*self.GROUPS[:3], frozenset({9})]
        self.assertIsNotNone(measure.pin_topology_problem(uneven))

    def test_a_leg_is_placed_by_the_count_its_script_states(self):
        self.assertEqual(measure.stated_threads("dd"), 1)
        self.assertEqual(measure.stated_threads("parse"), measure.SWEEP_JOBS)
        self.assertEqual(measure.stated_threads("parse-jobs-24"), 24)
        self.assertEqual(measure.stated_threads("decode-16"), 16)
        self.assertEqual(measure.stated_threads(measure.DYNFILTER_STARTUP), measure.SWEEP_JOBS)
        for command in measure.dynfilter_shapes():
            self.assertEqual(measure.stated_threads(command), measure.SWEEP_JOBS)
        # Discovery stays unpinned: a cpuset would narrow what it reads.
        flagless = next(c for c in measure.command_shapes() if c.startswith(measure.RESERVE_FLAGLESS))
        self.assertIsNone(measure.stated_threads(flagless))
        # Every shape resolves, so no leg reaches a sitting unplaceable.
        for command in measure.command_shapes():
            measure.stated_threads(command)

    def test_the_recorded_apparatus_is_the_default_and_the_only_publishable_one(self):
        cfg = measure.Config()
        self.assertEqual(cfg.arms, (measure.Arm(),))
        self.assertTrue(cfg.publishable)
        self.assertEqual(cfg.arms[0].name, "unpinned+staged")
        self.assertTrue(measure.Config(pin_cpus="off", stage_binaries="on").publishable)
        for pin, stage in (("on", "on"), ("off", "off"), ("alternate", "on"), ("off", "alternate")):
            with self.subTest(pin=pin, stage=stage):
                self.assertFalse(measure.Config(pin_cpus=pin, stage_binaries=stage).publishable)
        # `alternate` puts the recorded arm first, so it is the one the tables render.
        both = measure.Config(pin_cpus="alternate", stage_binaries="on").arms
        self.assertEqual([a.name for a in both], ["unpinned+staged", "pinned+staged"])
        both = measure.Config(stage_binaries="alternate").arms
        self.assertEqual([a.name for a in both], ["unpinned+staged", "unpinned+unstaged"])
        with self.assertRaises(ValueError):
            measure.Config(pin_cpus="sometimes").arms

    def test_the_report_format_reads_to_the_microsecond_outside_the_shape(self):
        self.assertAlmostEqual(measure.parse_bash_time(self.TIMED), 0.012345)
        self.assertEqual(measure.parse_bash_cpu(self.TIMED), {"user": 0.008, "sys": 0.004})
        # The default's shape, which `TIME_RE` reads, at six places.
        self.assertIn("%6lR", measure.TIME_FORMAT)
        self.assertNotIn("time ", measure.TIME_FORMAT)

    def _session(self, **cfg):
        config = measure.Config(**cfg)
        with unittest.mock.patch.object(measure, "l3_groups", return_value=self.GROUPS):
            session = measure.Session(config, measure.Stager(config, lambda _m: None), lambda _m: None)
        session.figure_id = "dynamic-filter-topk"
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path(f"/bin/{which}")
        session.stage_binary = lambda src: Path("/dev/shm/pgdt/bin") / src.name
        session.place_harness = lambda cpus: setattr(session, "placed", cpus)
        return session

    def _argv(self, session, spec):
        proc = subprocess.CompletedProcess([], 0, stdout="startup_answer=1\n", stderr=self.TIMED + self.EVENTS)
        with unittest.mock.patch.object(measure.subprocess, "run", return_value=proc) as ran:
            session.time_run(spec)
        return ran.call_args[0][0]

    def test_a_pinned_staged_leg_mounts_from_tmpfs_and_reads_it_before_the_timer(self):
        session = self._session(pin_cpus="on", stage_binaries="on")
        spec = measure.RunSpec("dfcli", "dynfilter", measure.DYNFILTER_STARTUP, "warm", "")
        argv = self._argv(session, spec)
        self.assertEqual(argv[argv.index("--cpuset-cpus") + 1], "3-5,15-17")
        self.assertIn("/dev/shm/pgdt/bin/dfcli:/datafusion-cli-pgdump:ro", argv)
        script = argv[-1]
        self.assertTrue(script.startswith(measure.TIME_FORMAT + "cat /pgdt /datafusion-cli-pgdump >/dev/null; "))
        self.assertLess(script.index("cat /pgdt"), script.index("time "))
        self.assertEqual(session.placed, measure.harness_cpus(self.GROUPS))
        record = session.records[-1]
        self.assertEqual((record["arm"], record["cpuset"]), ("pinned+staged", "3-5,15-17"))
        self.assertEqual(record["cpu_seconds"], {"user": 0.008, "sys": 0.004})

    def test_the_recorded_apparatus_stages_and_adds_nothing_else(self):
        session = self._session()
        # A cold regime too: a `drop_caches` evicts a binary read off disk,
        # and none staged on tmpfs.
        for regime in ("warm", "cold"):
            with self.subTest(regime=regime):
                spec = measure.RunSpec("pgdt", "control", "parse", regime, "")
                session.drop_caches = lambda: None
                argv = self._argv(session, spec)
                self.assertNotIn("--cpuset-cpus", argv)
                self.assertIn("/dev/shm/pgdt/bin/pgdt:/pgdt:ro", argv)
                self.assertEqual(
                    argv[-1],
                    measure.TIME_FORMAT + "cat /pgdt >/dev/null; "
                    + measure._script("parse") + measure.OOM_ORACLE,
                )
        self.assertFalse(hasattr(session, "placed"))

    def test_an_unstaged_leg_mounts_where_cargo_left_it_and_reads_nothing_first(self):
        session = self._session(stage_binaries="off")
        argv = self._argv(session, measure.RunSpec("pgdt", "control", "parse", "warm", ""))
        self.assertIn("/bin/pgdt:/pgdt:ro", argv)
        self.assertEqual(
            argv[-1], measure.TIME_FORMAT + measure._script("parse") + measure.OOM_ORACLE
        )

    def test_an_unpinned_leg_under_the_pinned_arm_sends_the_harness_home(self):
        session = self._session(pin_cpus="on")
        argv = self._argv(session, measure.RunSpec("pgdt", "control", "parse-jobs-24", "warm", ""))
        self.assertNotIn("--cpuset-cpus", argv)
        self.assertEqual(session.placed, session._harness_home)

    def test_alternating_arms_file_the_second_arm_apart_from_the_tables(self):
        session = self._session(pin_cpus="alternate")
        taken = []

        def take(spec, rep):
            taken.append((rep, session.arm.name))
            session._last_killed = False
            session._last_stdout = {}
            session._last_instrument = {}
            session._last_rss = None
            return 1.0 if session.arm.pinned else 2.0

        session.take = take
        spec = measure.RunSpec("pgdt", "control", "parse", "warm", "")
        session.sweep("f", [spec], 2)
        # Each rep takes the leg under both arms, the first arm alternating.
        self.assertEqual(
            taken,
            [(0, "unpinned+staged"), (0, "pinned+staged"),
             (1, "pinned+staged"), (1, "unpinned+staged")],
        )
        key = spec.key("f")
        self.assertEqual(session.readings[key], [2.0, 2.0])
        self.assertEqual(session.arm_readings["pinned+staged"][key], [1.0, 1.0])
        self.assertEqual(session.arm_readings["unpinned+staged"][key], [2.0, 2.0])

    def _raw(self, first, second, commit="abc"):
        return {
            "commit": commit,
            "date": "2026-09-28",
            "arms": {
                "primary": "unpinned+staged",
                "names": ["unpinned+staged", "pinned+staged"],
                "readings": {"unpinned+staged": first, "pinned+staged": second},
            },
        }

    def test_one_sitting_sets_its_arms_beside_each_other(self):
        raw = self._raw({"f/a": [1.0, 1.1, 0.9]}, {"f/a": [0.5, 0.5, 0.5]})
        text = measure.arms_table(raw)
        self.assertIn("-50.00%", text)
        self.assertIn("spread wider than unpinned+staged's on 0 of 1", text)
        with self.assertRaises(ValueError):
            measure.arms_table({"commit": "abc", "date": "d"})

    def test_two_sittings_price_each_arms_drift_against_the_criterion(self):
        a = self._raw({"f/a": [1.0], "f/b": [1.0]}, {"f/a": [1.0], "f/b": [1.0]})
        b = self._raw({"f/a": [1.10], "f/b": [0.90]}, {"f/a": [1.04], "f/b": [0.96]})
        text = measure.arm_drift_table(a, b)
        self.assertIn("**10.00%**", text)
        self.assertIn("**4.00%**", text)
        self.assertIn("0.40×", text)

    def test_a_second_arm_never_reaches_a_censored_bound(self):
        session = self._session()
        spec = measure.RunSpec("pgdt", "control", "parse", "warm", "")
        want = dataclasses.asdict(spec)
        session.records = [
            {"killed": True, "figure": "f", "spec": want, "maxrss_bound_kib": 1,
             "seconds_to_kill": 1.0, "secondary_arm": False},
            {"killed": True, "figure": "f", "spec": want, "maxrss_bound_kib": 2,
             "seconds_to_kill": 2.0, "secondary_arm": True},
        ]
        self.assertEqual(session.censored_bounds("f", spec), [(1, 1.0)])

    def test_the_startup_leg_answers_before_its_table_does(self):
        script = measure._script(measure.DYNFILTER_STARTUP)
        builder, _, timed = script.partition(" && ")
        self.assertTrue(builder.startswith("/pgdt parse --source /dump.sql "))
        self.assertIn(measure.GATHER_STATISTICS, builder)
        self.assertTrue(timed.startswith(f"time {measure.DFCLI_PARTITIONS}={measure.SWEEP_JOBS} "))
        self.assertIn(f"'{measure.STARTUP_SQL}'", timed)
        self.assertEqual(script.count("time "), 1)
        self.assertIn(measure.DYNFILTER_STARTUP, measure.command_shapes())
        self.assertEqual(measure.statistics_flag_problems(), [])
