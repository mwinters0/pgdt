"""Tests of `measurement.figures.dynfilter`."""

from __future__ import annotations

import generate_perf_data as perf
import unittest
import unittest.mock

from measurement import test_support
from measurement import borrowing
from measurement import config
from measurement import parsing
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.figures import dynfilter
from measurement.figures import register


class DynamicFilterFigures(unittest.TestCase):
    """`dynamic-filter-join` and `dynamic-filter-topk`: the register's first
    figures timing `pgdt sql`, DataFusion's CLI.

    Each way a leg can depart from what its table says is held here, because
    each produces a plausible table of something else: a run inheriting
    DataFusion's `target_partitions` while the untimed builder's `--jobs 1`
    satisfies the worker-count check, two legs differing by more than the
    producer's flag, an image or allocator the prose does not name, and legs
    answering differently under the timer."""

    SHAPES = shapes.dynfilter_shapes()

    def test_both_are_taken_and_borrow_nothing(self):
        for fid in ("dynamic-filter-join", "dynamic-filter-topk"):
            with self.subTest(figure=fid):
                fig = register.SELECTABLE_BY_ID[fid]
                self.assertIn(fig, register.FIGURES)
                self.assertEqual(fig.shares, ())
                self.assertEqual(borrowing.entangled_with(fid), [])
                self.assertEqual(fig.warm_inputs, ("dynfilter",))
                self.assertIsNone(fig.memory)

    def test_a_join_reports_the_rows_it_matched(self):
        # The table's last column is `result_first`, the answer's first cell;
        # `count(p.v_text)` there would drop the matched rows whose payload is
        # NULL and report fewer than the join matched.
        for name, (sql, _) in shapes.DYNFILTER_QUERIES["join"].items():
            with self.subTest(query=name):
                self.assertTrue(sql.startswith("SELECT count(*), count(p.v_text) FROM "))

    def test_every_leg_runs_the_sql_shell_under_its_producers_flag(self):
        for figure, flag in shapes.DYNFILTER_FLAGS.items():
            for command in shapes.dynfilter_shapes(figure):
                with self.subTest(command=command):
                    script = shapes._script(command)
                    leg = command.rpartition("-")[2]
                    self.assertIn(f"{flag}={shapes.DYNFILTER_LEGS[leg]} {shapes.SQL_SHELL} ", script)
                    self.assertEqual(script.count("time "), 1)
                    self.assertEqual(script.count(f"{shapes.SQL_SHELL} "), 1)
                    for other in set(shapes.DYNFILTER_FLAGS.values()) - {flag}:
                        self.assertNotIn(other, script)

    def test_each_leg_of_a_row_differs_from_the_next_by_one_lever(self):
        # Off to on is the producer's flag alone; on to rows the setting's
        # `SET` alone, run before the query so the answer read back is the
        # query's. A third difference would price something no column names.
        self.assertEqual(list(shapes.DYNFILTER_LEGS), ["off", "on", shapes.DYNFILTER_ROWS_LEG])
        for figure, queries in shapes.DYNFILTER_QUERIES.items():
            flag = shapes.DYNFILTER_FLAGS[figure]
            for name in queries:
                with self.subTest(figure=figure, query=name):
                    off, on, rows = (
                        shapes._script(f"{shapes.DYNFILTER_FAMILY}{figure}-{name}-{leg}")
                        for leg in shapes.DYNFILTER_LEGS
                    )
                    self.assertEqual(off.replace(f"{flag}=false", f"{flag}=true"), on)
                    self.assertNotIn(shapes.DYNFILTER_ROWS_SQL, on)
                    stated = f"-q -c '{shapes.DYNFILTER_ROWS_SQL}' -c "
                    self.assertEqual(rows.replace(stated, "-q -c "), on)

    def test_the_rows_leg_states_a_setting_the_provider_takes(self):
        # `SET` of a key the provider does not register fails the run rather
        # than timing the default under the rows leg's name; held against the
        # provider's own source, as the allocator is below.
        settings = (config.REPO / "crates/datafusion-pgdump/src/settings.rs").read_text()
        key, _, value = shapes.DYNFILTER_ROWS_SQL.removeprefix("SET ").partition(" = ")
        self.assertEqual(key.partition(".")[0], "pgdump")
        self.assertIn(f'"{key.partition(".")[2]}" => {{', settings)
        self.assertEqual(value, "true")

    def test_the_builder_is_untimed_and_writes_where_dump_looks(self):
        # `--source` reads the cache beside the dump, and `pgdt sql` takes no
        # cache path; the builder gathers what the scan prunes by.
        for command in self.SHAPES:
            with self.subTest(command=command):
                builder, _, timed = shapes._script(command).partition(" && ")
                self.assertTrue(builder.startswith("/pgdt parse --source /dump.sql "))
                self.assertIn("--cache /dump.sql.pgdt ", builder)
                self.assertIn(shapes.GATHER_STATISTICS, builder)
                self.assertTrue(timed.startswith("time "))
                self.assertIn(f"--source {shapes.DFCLI_CATALOG}=/dump.sql ", timed)

    def test_every_run_of_the_sql_shell_states_its_partitions(self):
        for command in self.SHAPES:
            with self.subTest(command=command):
                self.assertEqual(
                    shapes._dfcli_partitions(shapes._script(command)),
                    {str(shapes.SWEEP_JOBS)},
                )
        self.assertEqual(shapes.worker_count_problems(), [])
        self.assertEqual(shapes.pinned_count_problems(), [])

    def test_a_run_inheriting_its_partitions_is_reported_though_the_builder_states_jobs(self):
        script = (
            "/pgdt parse --source /dump.sql --jobs 1 >/dev/null && "
            f"time {shapes.SQL_SHELL} --source b=/dump.sql -c 'SELECT 1'"
        )
        with unittest.mock.patch.object(shapes, "_script", lambda c: script):
            self.assertEqual(sorted(shapes.worker_count_problems()), sorted(
                c for c in shapes.command_shapes()
                if c not in shapes._NO_WORKERS and not c.startswith(shapes.RESERVE_FLAGLESS)
            ))
        stated = script.replace("time ", f"time {shapes.DFCLI_PARTITIONS}=4 ")
        with unittest.mock.patch.object(shapes, "_script", lambda c: stated):
            got = shapes.pinned_count_problems()
        self.assertTrue(any(f"{shapes.DFCLI_PARTITIONS}=4" in line for line in got), got)

    def test_the_sql_survives_the_shells_quoting(self):
        # Each statement is single-quoted into `bash -c`.
        statements = [sql for q in shapes.DYNFILTER_QUERIES.values() for sql, _ in q.values()]
        for sql in [*statements, shapes.DYNFILTER_ROWS_SQL, shapes.STARTUP_SQL]:
            with self.subTest(sql=sql):
                self.assertNotIn("'", sql)

    def test_the_queries_name_the_generators_tables_and_columns(self):
        import generate_dynamic_filter_bench as gen

        tables = {t.removeprefix("public.") for t, _ in gen.BUILD_TABLES}
        columns = {name for name, _ in (*perf.COLUMNS, *gen.COLUMNS)}
        joins = shapes.DYNFILTER_QUERIES["join"]
        for name, (sql, _) in joins.items():
            with self.subTest(query=name):
                table = sql.split(f"JOIN {shapes.DFCLI_CATALOG}.public.")[1].split(" ")[0]
                probe_key = sql.rsplit(" = ", 1)[0].rsplit("p.", 1)[1]
                self.assertIn(table, tables)
                self.assertIn(probe_key, columns)
        topk = shapes.DYNFILTER_QUERIES["topk"]["unsorted"][0]
        self.assertIn(f"ORDER BY p.{gen.COLUMNS[0][0]} ", topk)

    def test_the_sql_shell_is_the_timed_binary_in_the_one_image(self):
        # The shell is `pgdt`'s own `sql`: the binary every other figure times,
        # mounted where they mount it, in the image they run in, and no second
        # build.
        self.assertFalse(hasattr(config.Config(), "dfcli_image"))
        self.assertFalse(hasattr(timing.Session, "image_for"))
        self.assertEqual(shapes.SQL_SHELL, "/pgdt sql")
        cfg = config.Config(dry_run=True)
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        self.assertEqual(session.binary_path("dfcli"), cfg.bin_pgdt)
        self.assertFalse(test_support.harness_has("ensure_dfcli_binary"))

    def _reported(self, figure, **override):
        good = {
            f"{name}-{leg}": {"result_rows": "1", "result_digest": f"h-{name}"}
            for name in shapes.DYNFILTER_QUERIES[figure]
            for leg in shapes.DYNFILTER_LEGS
        }
        good.update(override)
        return good

    def test_legs_answering_alike_pass(self):
        for figure in shapes.DYNFILTER_QUERIES:
            with self.subTest(figure=figure):
                self.assertEqual(dynfilter.dynfilter_problems(figure, self._reported(figure)), [])

    def test_legs_answering_differently_are_refused(self):
        got = dynfilter.dynfilter_problems(
            "join", self._reported("join", **{"costing-on": {"result_rows": "1", "result_digest": "x"}})
        )
        self.assertEqual(len(got), 1, got)
        self.assertTrue(got[0].startswith("costing: "), got)
        got = dynfilter.dynfilter_problems("topk", self._reported("topk", **{"unsorted-off": {}}))
        self.assertEqual(len(got), 1, got)
        # The rows leg is held to the answer too, not only the two the flag
        # separates.
        got = dynfilter.dynfilter_problems(
            "topk",
            self._reported("topk", **{"unsorted-rows": {"result_rows": "1", "result_digest": "x"}}),
        )
        self.assertEqual(len(got), 1, got)

    def test_an_empty_answer_is_refused(self):
        empty = {"result_rows": "0", "result_digest": "e"}
        got = dynfilter.dynfilter_problems(
            "join",
            self._reported("join", **{f"clustered-{leg}": empty for leg in shapes.DYNFILTER_LEGS}),
        )
        self.assertEqual(got, ["clustered: the query returned no row"])

    def test_the_answer_is_read_off_the_legs_own_lines(self):
        got = parsing.parse_reported(
            "result_rows=1\nresult_first=100\nresult_digest=" + "a" * 64 + "\n"
        )
        self.assertEqual(got, {"result_rows": "1", "result_first": "100", "result_digest": "a" * 64})


if __name__ == "__main__":
    unittest.main()
