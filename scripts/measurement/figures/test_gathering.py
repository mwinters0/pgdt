"""Tests of `measurement.figures.gathering`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import borrowing
from measurement import config
from measurement import parsing
from measurement import shapes
from measurement.figures import gathering
from measurement.figures import register


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
                fig = register.SELECTABLE_BY_ID[fid]
                self.assertIn(fig, register.FIGURES)
                self.assertNotIn(fig, register.UNTAKEN)
                self.assertEqual(fig.shares, ())
                self.assertEqual(borrowing.entangled_with(fid), [])

    def test_the_gathering_figure_is_the_scan_throughput_inputs_and_the_arrays_file(self):
        # The throughput inputs, whole: two of them hold no `COPY` row, and that
        # is a row of the table rather than a reason to drop it. The arrays file
        # beside them is where the census splits every row, so its row and the
        # control's bracket the census the data level prices.
        fig = register.SELECTABLE_BY_ID["statistics-gathering"]
        warm = register.FIGURES_BY_ID["scan-throughput-warm"].warm_inputs
        self.assertEqual(set(fig.warm_inputs), set(warm) | {"arrays"})
        self.assertEqual(fig.warm_inputs[:2], ("control", "arrays"))
        self.assertEqual(fig.memory, shapes.STATISTICS_MEMORY)
        self.assertNotEqual(shapes.STATISTICS_MEMORY, config.Config().memory)

    def test_the_group_size_stated_is_the_librarys_default(self):
        src = (config.REPO / "crates/pgdt-mapping/src/statistics/request.rs").read_text()
        self.assertEqual(shapes.ROW_GROUP_SIZE, 1 << 20)
        self.assertIn("pub const ROW_GROUP_DEFAULT_SIZE_BYTES: u64 = 1 << 20;", src)

    def test_a_gathering_legs_parse_states_its_request(self):
        for leg, flags in shapes.STATISTICS_LEGS:
            with self.subTest(leg=leg):
                script = shapes._script(f"{shapes.STATISTICS_FAMILY}{leg}-rss")
                self.assertIn(flags, script)
                self.assertIn(f"time {parsing.PEAK_RSS} /pgdt parse", script)

    def test_the_two_gathering_legs_differ_by_the_level_alone(self):
        (metadata, metadata_flags), (data, data_flags) = shapes.STATISTICS_LEGS
        self.assertEqual((metadata, data), ("metadata", "data"))
        self.assertEqual(metadata_flags, shapes.NO_STATISTICS)
        self.assertEqual(data_flags, shapes.GATHER_STATISTICS)
        a = shapes._script(f"{shapes.STATISTICS_FAMILY}{metadata}-rss")
        b = shapes._script(f"{shapes.STATISTICS_FAMILY}{data}-rss")
        self.assertEqual(a.replace(metadata_flags, data_flags), b)

    def test_the_metadata_leg_is_the_register_parse_in_the_figures_container(self):
        # Its own leg rather than `peak-rss`'s borrowed, for the container; the
        # argv is the same, so the two cannot drift apart unseen.
        metadata = shapes._script(f"{shapes.STATISTICS_FAMILY}metadata-rss")
        self.assertEqual(metadata, shapes._script("parse-rss"))

    def test_the_exemption_admits_the_gathering_request_and_nothing_else(self):
        # Outside the declared families a `parse` stating `GATHER_STATISTICS`
        # is still one that gathers, and is reported.
        script = f"time /pgdt parse --source /dump.sql {shapes.GATHER_STATISTICS} --jobs 1"
        with unittest.mock.patch.object(shapes, "_script", lambda c: script):
            reported = shapes.statistics_flag_problems()
        families = shapes.GATHERING_FAMILIES
        self.assertEqual(
            set(families),
            {
                shapes.STATISTICS_FAMILY,
                shapes.STRICT_PARSE,
                shapes.PRUNING_FAMILY,
                shapes.DYNFILTER_FAMILY,
                shapes.PARALLEL_SCAN,
                *shapes.DATA_LEVEL_QUERIES,
                shapes.LONG_RSS_FAMILY,
                shapes.LONG_FACTOR_FAMILY,
            },
        )
        self.assertEqual(
            sorted(reported),
            sorted(c for c in shapes.command_shapes() if not c.startswith(families)),
        )

    def test_the_pruning_builder_is_untimed_and_states_the_request(self):
        for name in shapes.PRUNING_FILTERS:
            for leg in shapes.PRUNING_LEGS:
                with self.subTest(filter=name, leg=leg):
                    script = shapes._script(f"{shapes.PRUNING_FAMILY}{name}-{leg}")
                    builder, _, timed = script.partition(" && ")
                    self.assertIn("/pgdt parse", builder)
                    self.assertIn(shapes.GATHER_STATISTICS, builder)
                    self.assertNotIn("time ", builder)
                    self.assertTrue(timed.startswith("time /pgdt query"))
                    self.assertEqual(script.count("time "), 1)
                    self.assertIn(f"--statistics {leg} ", timed)

    def test_no_shape_queries_a_cache_the_metadata_level_wrote(self):
        # Such a query re-reads its table for the census inside the timer, so
        # it times the census rather than what its figure names.
        for command in shapes.command_shapes():
            script = shapes._script(command)
            with self.subTest(command=command):
                self.assertFalse(
                    any(shapes.NO_STATISTICS in run for run in shapes._PARSE_RUN.findall(script))
                    and "/pgdt query" in script,
                    script,
                )

    def test_the_two_pruning_legs_differ_by_the_flag_alone(self):
        for name in shapes.PRUNING_FILTERS:
            with self.subTest(filter=name):
                a = shapes._script(f"{shapes.PRUNING_FAMILY}{name}-none")
                b = shapes._script(f"{shapes.PRUNING_FAMILY}{name}-all")
                self.assertEqual(a.replace("--statistics none", "--statistics all"), b)

    def test_the_filters_name_the_generators_columns(self):
        import generate_pruning_bench as gen

        range_expr = shapes.PRUNING_FILTERS["range"][0]
        self.assertTrue(range_expr.startswith(f"{shapes._PERF_SCALARS[0]} "))
        column, literal = shapes.PRUNING_FILTERS["dictionary"][0].split("=")
        self.assertEqual(column, gen.COLUMN[0])
        self.assertIn(literal, [gen.label(i) for i in range(gen.LABELS)])

    def test_the_unnarrowed_filter_is_a_midrange_equality_on_a_uniform_smallint(self):
        # What makes its statistics unable to narrow it is the draw: a
        # `smallint` over its whole range, per row (`random_row`), with `0` in
        # the middle of it. What `parse` keeps for the column at the figure's
        # group size, and that the query skips no group, is
        # `perf_generator_fidelity.rs`'s, which states the same filter.
        import generate_perf_data as perf

        expr = shapes.PRUNING_FILTERS[shapes.PRUNING_UNNARROWED][0]
        column, literal = expr.split("=")
        self.assertEqual(dict(perf.COLUMNS)[column], "smallint")
        self.assertEqual(int(literal), 0)
        fidelity = (config.REPO / "crates/pgdt/tests/perf_generator_fidelity.rs").read_text()
        self.assertIn(f'"{expr}"', fidelity)
        self.assertIn(str(shapes.ROW_GROUP_SIZE), fidelity)

    def test_the_query_notes_are_read_as_the_cli_prints_them(self):
        got = parsing.parse_query_notes(self.QUERY_STDERR)
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
            parsing.parse_query_notes("no rows found for public.perf in /dump.sql\n"),
            {"rows_returned": "0"},
        )
        self.assertEqual(parsing.parse_query_notes("real 0m1.0s\n"), {})

    def test_the_phrases_read_are_the_librarys_and_the_clis(self):
        # The regexes key on wording two crates print; a rewording there would
        # leave every leg reading as one that skipped nothing, which the
        # renderer refuses — loudly, but a sitting late.
        stream = (config.REPO / "crates/pgdt-query/src/stream/note.rs").read_text()
        cli = (config.REPO / "crates/pgdt/src/query.rs").read_text()
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
        problems = gathering.pruning_problems(self._reported(**override))
        self.assertEqual(len(problems), 1, problems)
        return problems[0]

    def test_a_sitting_that_prunes_passes(self):
        self.assertEqual(gathering.pruning_problems(self._reported()), [])

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
        self.assertIn(shapes.PRUNING_UNNARROWED, shapes.PRUNING_FILTERS)
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


if __name__ == "__main__":
    unittest.main()
