"""Tests of `measurement.shapes`."""

from __future__ import annotations

import contextlib
import generate_perf_data as perf
import inspect
import io
import re
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import check
from measurement import config
from measurement import inputs
from measurement import memory_model
from measurement import replay
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement.builds import leg_binaries
from measurement.figures import declaration
from measurement.figures import nested
from measurement.figures import parallel
from measurement.figures import query
from measurement.figures import register
from measurement.recipes import heaptrack
from measurement.recipes import koji
from measurement.recipes import profile


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
            *(f"query-project-{w}" for w in shapes.PROJECTION_WIDTHS),
            *(f"query-where-{s}" for s in shapes.PREDICATE_SHAPES),
            "dd",
        ):
            with self.subTest(command=command):
                self.assertEqual(len(self.TIMER.findall(shapes._script(command))), 1)

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
        verdicts = {c: self.unjoined_builder(shapes._script(c)) for c in shapes.command_shapes()}
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
                self.assertNotIn("2>", shapes._script(command))

    def test_the_cache_removal_sits_outside_the_timer(self):
        script = shapes._script("parse-cache-out")
        self.assertLess(script.index("rm -f"), script.index("time "))

    def test_an_unknown_command_is_an_error(self):
        with self.assertRaises(ValueError):
            shapes._script("no-such-command")

    def test_a_width_the_register_does_not_carry_is_an_error(self):
        # The command shape is parsed rather than matched, so an unregistered
        # width has to be refused explicitly or it would run a query with no
        # projection flags at all and be read as a width.
        for command in ("query-project-7", "query-project-", "query-project-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)


class WorkerCount(unittest.TestCase):
    """A worker count is apparatus, so nothing here inherits the CLI's.

    `pgdt --jobs`' default has moved underneath the published figures without
    one command shape changing, and now depends on the source, which is the
    failure this reconciles against. That it agrees with `SWEEP_JOBS` on some
    source is not a reason to inherit it. `--stale` cannot see
    it either: staleness says *re-take*, never *the apparatus moved underneath
    you*."""

    def test_every_shape_states_a_worker_count(self):
        self.assertEqual(shapes.worker_count_problems(), [])

    def test_a_shape_that_inherits_one_is_reported(self):
        # The check must fail loudly, since the shape it would pass still runs
        # and still produces a table. Every shape but the declared exemptions:
        # `_NO_WORKERS`, which runs no `pgdt`, and the flagless family, whose
        # reading is the count a flagless run resolves —
        # `flagless_flag_problems` is what holds *that* family to stating
        # nothing, so neither exemption leaves a shape unchecked.
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql"
        ):
            reported = shapes.worker_count_problems()
        self.assertEqual(
            sorted(reported),
            sorted(
                c
                for c in shapes.command_shapes()
                if c not in shapes._NO_WORKERS
                and not c.startswith(shapes.RESERVE_FLAGLESS)
            ),
        )

    def test_a_builder_beside_the_sql_shell_states_its_own(self):
        # The shell's count is its partitions, which does not pin the `parse`
        # ahead of it; one running no builder needs nothing more.
        shell = f"time {shapes.DFCLI_PARTITIONS}=4 {shapes.SQL_SHELL} --source x=/dump.sql"
        for script, inherits in (
            (shell, False),
            (f"/pgdt parse --source /dump.sql --jobs 1 >/dev/null && {shell}", False),
            (f"/pgdt parse --source /dump.sql >/dev/null && {shell}", True),
        ):
            with self.subTest(script=script), unittest.mock.patch.object(
                shapes, "_script", lambda c: script
            ):
                reported = shapes.worker_count_problems()
                self.assertEqual(bool(reported), inherits)

    def test_check_fails_on_a_shape_that_inherits_one(self):
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql"
        ):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = check.cmd_check(config.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("inheriting a worker count", out.getvalue())

    def test_the_enumeration_covers_every_branch_of_the_builder(self):
        """`command_shapes` is a second list of what `_script` accepts, and a
        second list drifts. This is what stops a shape added there from being
        exempted from the reconciliation by not being enumerated here."""
        src = inspect.getsource(shapes._script)
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
            named = getattr(shapes, name)
            # A named prefix is one string or a tuple of them; `set()` over the
            # first would enumerate its *characters*, which resolves to a set of
            # single letters and fails this reconciliation on every one of them.
            prefixes |= {named} if isinstance(named, str) else set(named)
        shapes_ = set(shapes.command_shapes())
        self.assertTrue(exact)
        self.assertTrue(prefixes)
        self.assertEqual(exact - shapes_, set())
        for prefix in prefixes:
            with self.subTest(prefix=prefix):
                self.assertTrue([s for s in shapes_ if s.startswith(prefix)])

    def test_an_exempt_shape_runs_no_pgdt(self):
        # `dd` is the device floor and the inflate instrument has one thread
        # by construction. Anything else claiming the exemption would be a
        # `pgdt` run measuring whatever count the machine had. `/pgdt` is the
        # mount point every figure's binary takes, so what is refused is a
        # `pgdt` subcommand there, or the SQL shell.
        subcommand = re.compile(r"/pgdt (?:parse|query|info|sql)\b")
        for command in shapes._NO_WORKERS:
            with self.subTest(command=command):
                script = shapes._script(command)
                self.assertIsNone(subcommand.search(script), script)
                self.assertNotIn(shapes.SQL_SHELL, script)

    def test_the_inflate_shapes_are_exempt_and_nothing_else_new(self):
        # Admitted on purpose, one name per backend or engine, beside `dd`.
        self.assertEqual(
            shapes._NO_WORKERS,
            ("dd", *shapes.INFLATE_SHAPES, *shapes.GZSEEK_SHAPES, *shapes.GZSPACING_SHAPES),
        )

    def test_the_decode_instrument_pins_its_own_spelling(self):
        # It is not `pgdt`, so it has no `--jobs`; `--workers` is the same
        # statement in the instrument's own vocabulary, and the figure's whole
        # axis is that count.
        for workers in shapes.DECODE_WORKERS:
            with self.subTest(workers=workers):
                self.assertIn(f"--workers {workers}", shapes._script(f"decode-{workers}"))

    def test_the_untimed_profiling_parse_states_one_too(self):
        # Untimed, but its row counts are the divisor under every per-row
        # number in the document.
        src = inspect.getsource(staging.Stager.profile)
        self.assertIn('"--jobs", str(shapes.SWEEP_JOBS)', src)

    def test_the_traced_save_count_states_one_too(self):
        # Untimed as well, and its number is published — and `strace -f`
        # follows every thread a parallel mapping pass would spawn.
        src = inspect.getsource(leg_binaries.count_saves)
        self.assertIn('"--jobs", str(shapes.SWEEP_JOBS)', src)

    def test_the_rss_attribution_states_one_on_every_leg(self):
        # A resident set is exactly the quantity a worker count moves, each
        # worker holding read buffers of its own — so a leg inheriting the
        # CLI's default would attribute a growth this apparatus never measured.
        # `info` takes no such flag because it starts no workers; its shape
        # states the count on the builder that precedes it.
        for _, _, command in register._ATTRIBUTION_LEGS:
            with self.subTest(command=command):
                self.assertIn(f"--jobs {shapes.SWEEP_JOBS}", shapes._script(command))

    def test_the_apparatus_line_states_the_count_the_harness_pins(self):
        """The document's one apparatus line names the worker count, and the
        number in it is the harness's — a sentence describing the previous
        arrangement is exactly what this whole reconciliation is against."""
        doc = (config.REPO / "docs/design/measurements.md").read_text()
        section = doc.split("\n## The apparatus\n", 1)[1].split("\n## ", 1)[0]
        stated = set(re.findall(r"--jobs (\d+)", section))
        self.assertEqual(stated, {str(shapes.SWEEP_JOBS)})


class StatisticsFlag(unittest.TestCase):
    """Every `pgdt parse` the harness runs gathers no statistics, but for the
    statistics figures' own (`StatisticsFigures`).

    `parse` gathers by default, which reads every value of every column, so a
    shape inheriting that default would re-time the figure it belongs to the
    day the default moved — the same failure `WorkerCount` reconciles against,
    for a second flag."""

    def test_every_parse_shape_states_none(self):
        self.assertEqual(shapes.statistics_flag_problems(), [])

    def test_a_shape_that_gathers_is_reported(self):
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 1"
        ):
            reported = shapes.statistics_flag_problems()
        self.assertEqual(sorted(reported), sorted(shapes.command_shapes()))

    def test_a_later_parse_in_the_same_script_is_checked_too(self):
        # A builder ahead of the timed command, as `info-cache-rss` has, is a
        # `parse` of its own.
        for join in ("&&", ";"):
            script = (
                f"/pgdt parse --source /dump.sql {shapes.NO_STATISTICS} >/dev/null{join} "
                "time /pgdt parse --source /dump.sql >/dev/null"
            ).replace("null&&", "null &&")
            with self.subTest(join=join), unittest.mock.patch.object(
                shapes, "_script", lambda c: script
            ):
                self.assertTrue(shapes.statistics_flag_problems())

    def test_the_preamble_shapes_are_exempt_and_state_none(self):
        # `--preamble-only` reads no row, and the CLI refuses a statistics flag
        # beside it.
        for command in ("parse-preamble", "parse-preamble-rss"):
            with self.subTest(command=command):
                script = shapes._script(command)
                self.assertIn("--preamble-only", script)
                self.assertNotIn("--statistics", script)

    def test_check_fails_on_a_shape_that_gathers(self):
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 1"
        ):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = check.cmd_check(config.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("with statistics gathered", out.getvalue())

    def test_the_argvs_outside_the_sweep_state_none_too(self):
        # Untimed or recorded rather than timed, and each one's reading is
        # read against a figure that states it.
        flag = shapes.NO_STATISTICS.split()
        argvs = [
            profile.profile_argv("parse", "/dump.sql", "/tmp/x.pgdt"),
            *(heaptrack.heaptrack_argv(shape, "/d", "/c") for shape, _ in heaptrack.HEAPTRACK_SHAPES),
        ]
        for argv in argvs:
            with self.subTest(argv=argv):
                at = argv.index(flag[0])
                self.assertEqual(argv[at : at + 2], flag)
        for function in (staging.Stager.profile, leg_binaries.count_saves):
            with self.subTest(function=function.__name__):
                self.assertIn("*shapes.NO_STATISTICS.split()", inspect.getsource(function))

    def test_the_koji_legs_state_none(self):
        # koji's check is that a parallel scan writes the serial cache, which a
        # gathering leg would answer with the serial scan compared to itself.
        for wrap in (False, True):
            recipe = koji.koji_recipe(config.Config(), "pgdt-koji", wrap, 8)
            legs = [line for line in recipe.splitlines() if "/pgdt parse" in line]
            with self.subTest(wrap=wrap):
                self.assertTrue(legs)
                self.assertTrue(all(shapes.NO_STATISTICS in leg for leg in legs), legs)


class DataLevelQueries(unittest.TestCase):
    """The query figures time a `pgdt query` over a data-level cache one
    untimed `parse` wrote in the same container, with `--statistics none`
    (`shapes.DATA_LEVEL_QUERIES`), so neither a mapping pass nor pruning
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
            c for c in shapes.command_shapes() if c.startswith(shapes.DATA_LEVEL_QUERIES)
        ]

    def test_every_one_queries_the_cache_its_builder_wrote_ahead_of_the_timer(self):
        for command in self.shapes():
            script = shapes._script(command)
            with self.subTest(command=command):
                self.assertTrue(script.startswith(shapes.DATA_LEVEL_BUILDER), script)
                builder, _, timed = script.partition(" && ")
                self.assertNotIn("time ", builder)
                self.assertIn(shapes.GATHER_STATISTICS, builder)
                self.assertIn(f"--jobs {shapes.SWEEP_JOBS} ", builder)
                self.assertTrue(timed.startswith("time /pgdt query "), timed)
                self.assertIn(shapes.DATA_LEVEL_QUERY, timed)
                self.assertNotIn("--cache none", script)

    def test_every_query_figure_row_is_one_of_them(self):
        specs = [
            *nested._nested_specs(),
            *(runspec.RunSpec("pgdt", "arrays", f"query-project-{w}", "warm", "")
              for w in shapes.PROJECTION_WIDTHS),
            *(runspec.RunSpec("pgdt", "control", f"query-where-{s}", "warm", "")
              for s in shapes.PREDICATE_SHAPES),
            *(runspec.RunSpec("pgdt", "control", c, "warm", "")
              for c, _, _ in register._ALLOCATOR_SHAPES if c.startswith("query")),
        ]
        for spec in specs:
            with self.subTest(command=spec.command):
                self.assertIn(spec.command, self.shapes())

    def test_only_the_map_shapes_keep_no_cache(self):
        # `query-nomatch*` times the map and its saves, which a built cache
        # would leave nothing of.
        uncached = {
            c for c in shapes.command_shapes() if "--cache none" in shapes._script(c)
        }
        self.assertEqual(uncached, {"query-nomatch", "query-nomatch-rss"})

    def test_every_query_figure_declares_what_its_cache_holds(self):
        for fid in self.FIGURES:
            with self.subTest(figure=fid):
                depends = register.SELECTABLE_BY_ID[fid].depends
                self.assertLessEqual(set(declaration.CACHED_QUERY), set(depends))
                self.assertEqual(len(depends), len(set(depends)))


class PredicateShapes(unittest.TestCase):
    """Six predicates over one file, which is the whole instrument.

    Three things make the adjacent-row subtraction mean what the table says it
    means: the terms are OR'd and every one of them is false, so all of them
    are evaluated and no row survives to be decoded; the two depths differ in
    the column and not in the term count, so their difference is the walk; and
    every column named is one the generator writes, so the query does not fail
    three minutes into a sweep."""

    def test_every_shape_names_a_column_the_generator_writes(self):
        emitted = {name for name, _ in perf.COLUMNS}
        for shape, (column, _) in shapes.PREDICATE_SHAPES.items():
            with self.subTest(shape=shape):
                self.assertIn(column, emitted)

    def test_an_expression_carries_exactly_that_many_terms(self):
        for shape, (column, terms) in shapes.PREDICATE_SHAPES.items():
            with self.subTest(shape=shape):
                expr = shapes.predicate_expr(shape)
                self.assertEqual(expr.count(f"{column}="), terms)
                self.assertEqual(expr.count(" OR "), terms - 1)

    def test_the_terms_are_distinct(self):
        for shape in shapes.PREDICATE_SHAPES:
            with self.subTest(shape=shape):
                parts = shapes.predicate_expr(shape).split(" OR ")
                self.assertEqual(len(set(parts)), len(parts))

    def test_the_terms_are_disjoined_never_conjoined(self):
        # `And` stops at the first non-`True` conjunct, so a conjunction of N
        # false terms evaluates one of them and the table would read flat.
        for shape in shapes.PREDICATE_SHAPES:
            with self.subTest(shape=shape):
                self.assertNotIn(" AND ", shapes.predicate_expr(shape))

    def test_a_shape_the_register_does_not_carry_is_an_error(self):
        # The command shape is parsed rather than matched, so an unregistered
        # one has to be refused explicitly or it would run a query with no
        # filter at all and be read as a predicate.
        for command in ("query-where-deep-4", "query-where-", "query-where-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_every_run_is_strings_and_carries_its_filter(self):
        # Typed `=` decodes the literal against the column's own type, so
        # `zzz1` on an `integer` column is refused before the first row.
        for shape in shapes.PREDICATE_SHAPES:
            with self.subTest(shape=shape):
                script = shapes._script(f"query-where-{shape}")
                self.assertIn("--schema-mode strings", script)
                self.assertIn(f"--where '{shapes.predicate_expr(shape)}'", script)

    def test_the_two_depths_differ_only_in_the_column(self):
        deep, shallow = shapes.PREDICATE_SHAPES["deep-5"], shapes.PREDICATE_SHAPES["shallow-5"]
        self.assertEqual(deep[1], shallow[1])
        self.assertNotEqual(deep[0], shallow[0])

    def test_the_deep_column_is_deeper_than_the_shallow_one(self):
        order = [name for name, _ in perf.COLUMNS]
        deep = order.index(shapes.PREDICATE_SHAPES["deep-5"][0])
        shallow = order.index(shapes.PREDICATE_SHAPES["shallow-5"][0])
        self.assertGreater(deep, shallow)

    def test_the_table_rows_are_the_registered_shapes(self):
        shapes_ = [s for s, _, _ in query._PREDICATE_ROWS]
        self.assertEqual(sorted(shapes_), sorted(shapes.PREDICATE_SHAPES))

    def test_the_figure_is_taken_on_the_brace_free_control(self):
        fig = register.SELECTABLE_BY_ID["predicate-terms"]
        self.assertEqual(fig.warm_inputs, ("control",))
        self.assertIn("crates/pgdt-query/src/predicate.rs", fig.depends)


class ProjectionWidths(unittest.TestCase):
    """One file read at five widths, which is the whole instrument.

    Two things make the adjacent-row subtraction mean what the table says it
    means: each width is a superset of the one above it, so their difference is
    exactly the columns they differ by; and every name is one the generator
    actually writes, so the query does not fail three minutes into a sweep."""

    def test_each_width_names_that_many_columns(self):
        for width, names in shapes.PROJECTION_WIDTHS.items():
            with self.subTest(width=width):
                self.assertEqual(len(names), width)

    def test_no_width_repeats_a_column(self):
        for width, names in shapes.PROJECTION_WIDTHS.items():
            with self.subTest(width=width):
                self.assertEqual(len(set(names)), len(names))

    def test_every_name_is_a_column_the_generator_writes(self):
        emitted = {
            name
            for name, _ in (
                *perf.COLUMNS,
                *perf.ARRAY_COLUMNS,
                *perf.COMPOSITE_COLUMNS,
            )
        }
        for width, names in shapes.PROJECTION_WIDTHS.items():
            for name in names:
                with self.subTest(width=width, column=name):
                    self.assertIn(name, emitted)

    def test_each_width_contains_the_one_below_it(self):
        widths = sorted(shapes.PROJECTION_WIDTHS)
        for narrow, wide in zip(widths, widths[1:]):
            with self.subTest(narrow=narrow, wide=wide):
                self.assertLessEqual(
                    set(shapes.PROJECTION_WIDTHS[narrow]),
                    set(shapes.PROJECTION_WIDTHS[wide]),
                )

    def test_zero_columns_asks_for_no_columns_rather_than_nothing(self):
        # An empty repetition of `--column` is an unprojected query, which
        # would silently make the floor row the widest row.
        self.assertEqual(shapes.projection_flags(0), "--no-columns")
        script = shapes._script("query-project-0")
        self.assertIn("--no-columns", script)
        self.assertNotIn("--column", script)

    def test_a_width_repeats_the_flag_once_per_column(self):
        self.assertEqual(shapes.projection_flags(16).count("--column "), 16)
        self.assertNotIn(",", shapes.projection_flags(16))

    def test_every_run_is_typed(self):
        # `strings` builds every column the same cheap way, so a table taken
        # that way would measure nothing this figure is about.
        for width in shapes.PROJECTION_WIDTHS:
            with self.subTest(width=width):
                self.assertIn("--schema-mode typed", shapes._script(f"query-project-{width}"))

    def test_the_table_rows_are_the_registered_widths_ascending(self):
        widths = [w for w, _, _ in query._PROJECTION_ROWS]
        self.assertEqual(widths, sorted(shapes.PROJECTION_WIDTHS))

    def test_the_figure_is_taken_on_the_nineteen_column_file(self):
        fig = register.SELECTABLE_BY_ID["projection-widths"]
        self.assertEqual(fig.warm_inputs, ("arrays",))
        self.assertEqual(inputs.INPUTS["arrays"].args[:2], ("--arrays", "--composite"))


class PinnedWorkerCount(unittest.TestCase):
    """The other half of the `JOBS_AXIS` exemption.

    `worker_count_problems` catches a shape that pins *nothing*. This catches
    one that pins something else — a shape edited to `--jobs 4` for a sitting,
    or moved into a parallel family without being declared one. Both produce a
    table whose apparatus line is wrong about it, which no `--stale` can see:
    staleness says *re-take*, never *the apparatus moved underneath you*."""

    def test_nothing_outside_the_axis_states_another_count(self):
        self.assertEqual(shapes.pinned_count_problems(), [])

    def test_a_shape_that_drifts_off_the_constant_is_reported(self):
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 4"
        ):
            reported = shapes.pinned_count_problems()
        self.assertTrue(reported)
        self.assertTrue(all("states --jobs 4" in line for line in reported))

    def test_the_axis_families_are_exempt(self):
        # They are the exemption, so nothing this reports may name one.
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 4"
        ):
            reported = shapes.pinned_count_problems()
        for line in reported:
            with self.subTest(line=line):
                self.assertFalse(line.startswith(shapes.JOBS_AXIS))
                self.assertFalse(line.startswith("decode-"))

    def test_check_fails_on_a_shape_that_drifts(self):
        with unittest.mock.patch.object(
            shapes, "_script", lambda c: "time /pgdt parse --source /dump.sql --jobs 4"
        ):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = check.cmd_check(config.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("neither the apparatus's nor a", out.getvalue())

    def test_every_axis_family_has_a_figure_whose_rows_are_the_counts(self):
        """An exemption with no figure behind it is a shape that states an
        arbitrary count and calls it an axis."""
        wanted = set(shapes.JOBS_AXIS)
        for fig in register.EVERY_FIGURE + register.UNTAKEN:
            if "warm-parallel" not in fig.stage.split("+"):
                continue
            for spec in _figure_specs(fig.id):
                for family in list(wanted):
                    if spec.command.startswith(family):
                        wanted.discard(family)
        self.assertEqual(wanted, set())


def _figure_specs(fid: str) -> list:
    """The `RunSpec`s one parallel figure sweeps, without running it."""
    if fid == "parallel-scan-throughput":
        return parallel._parallel_specs()
    if fid == "parallel-peak-rss":
        return parallel._parallel_rss_specs()
    return []


class ParallelScanThroughputProvider(unittest.TestCase):
    """`parallel-scan-throughput`'s extraction legs run the provider.

    `pgdt query`'s in-order merge reads one sub-stream at a time past its
    first round (`KD57`), so a column of it timed the merge rather than the
    library's sub-streams. What these hold is that the replacement times a
    scan: every column decoded, every row read, no count answered from the
    statistics, and one answer at every count."""

    FAMILY = f"{shapes.PARALLEL_SCAN}-jobs-"

    def test_the_query_counts_every_column_of_the_generated_table(self):
        sql = shapes.PARALLEL_SCAN_SQL
        for name, _ in perf.COLUMNS:
            with self.subTest(column=name):
                self.assertIn(f"count({name})", sql)
        self.assertIn(f"FROM {shapes.DFCLI_CATALOG}.{perf.TABLE} ", sql)

    def test_a_filter_keeps_the_counts_from_being_answered(self):
        # Unfiltered, the plan node's exact NULL counts answer every
        # `count(<column>)` with no row read; a pushed filter makes them
        # estimates. `id` is written on every row, so the filter keeps them all.
        self.assertTrue(shapes.PARALLEL_SCAN_SQL.endswith(" WHERE id IS NOT NULL"))
        self.assertEqual(perf.COLUMNS[0][0], "id")

    def test_every_row_reads_the_cache_its_builder_wrote_ahead_of_the_timer(self):
        builders = set()
        for jobs in shapes.PARALLEL_JOBS:
            script = shapes._script(f"{self.FAMILY}{jobs}")
            with self.subTest(jobs=jobs):
                builder, _, rest = script.partition(" && ")
                builders.add(builder)
                self.assertNotIn("time ", builder)
                self.assertIn(shapes.GATHER_STATISTICS, builder)
                self.assertIn(f"--jobs {shapes.SWEEP_JOBS} ", builder)
                self.assertIn("--cache /dump.sql.pgdt ", builder)
                self.assertTrue(
                    rest.startswith(
                        f"time {shapes.DFCLI_PARTITIONS}={jobs} {shapes.SQL_SHELL} "
                        f"--source {shapes.DFCLI_CATALOG}=/dump.sql "
                    ),
                    rest,
                )
                self.assertNotIn("; ", script)
        self.assertEqual(len(builders), 1)

    def test_the_answer_is_read_back_outside_the_timer(self):
        script = shapes._script(f"{self.FAMILY}1")
        timed, _, after = script.partition(">/tmp/result.csv && ")
        self.assertEqual(after, shapes.DFCLI_ANSWER)
        self.assertNotIn("echo", timed)

    def test_the_allowance_is_set_before_the_query_in_one_process(self):
        _, argv = shapes.parallel_scan_invocation(8, "/dump.sql")
        allowance = memory_model.stated_allowance(shapes.PARALLEL_BUDGET)
        self.assertEqual(
            argv[-4:], ["-c", f"SET pgdump.memory = {allowance}", "-c", shapes.PARALLEL_SCAN_SQL]
        )

    def test_a_count_the_figure_does_not_carry_is_refused(self):
        with self.assertRaises(ValueError):
            shapes.parallel_scan_invocation(3, "/dump.sql")

    def test_only_the_provider_legs_run_the_second_program(self):
        provider = (self.FAMILY, f"{shapes.PARALLEL_SCAN_PREBUILT}-jobs-")
        for spec in parallel._parallel_specs():
            with self.subTest(command=spec.command):
                self.assertEqual(
                    spec.binary, "dfcli" if spec.command.startswith(provider) else "pgdt"
                )

    def test_one_answer_passes_and_anything_else_is_refused(self):
        one = {"result_rows": "1", "result_first": "814362", "result_digest": "ab"}
        self.assertEqual(parallel.parallel_answer_problems({"a": one, "b": one}), [])
        self.assertTrue(
            parallel.parallel_answer_problems({"a": one, "b": {**one, "result_digest": "cd"}})
        )
        self.assertTrue(parallel.parallel_answer_problems({"a": one, "b": {}}))
        self.assertTrue(parallel.parallel_answer_problems({}))
        self.assertTrue(
            parallel.parallel_answer_problems({"a": {**one, "result_first": "0"}})
        )

    def test_a_sitting_whose_legs_disagree_is_not_rendered(self):
        figure = "parallel-scan-throughput"
        specs = parallel._parallel_specs()
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
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = figure
            with self.assertRaises(RuntimeError):
                parallel.run_parallel_scan_throughput(session)

    def test_the_figure_declares_the_provider(self):
        depends = register.SELECTABLE_BY_ID["parallel-scan-throughput"].depends
        self.assertLessEqual(set(declaration.DATAFUSION), set(depends))


if __name__ == "__main__":
    unittest.main()
