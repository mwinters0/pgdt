#!/usr/bin/env python3
"""Unit tests for row_density.py, run with
`uv run python -m unittest test_row_density`.

Synthetic `info --json` documents only: the reading itself runs over caches
`pgdq` writes, which are not committed, so what is pinned here is the
arithmetic -- the pairwise ladder, the quantiles the spec's bound rests on, the
choice, the criterion and the selection.
"""

from __future__ import annotations

import io
import json
import random
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from tempfile import TemporaryDirectory

import row_density as rd


def block(rows, *, table="t", schema="public", group_size=1 << 20, data_bytes=None, database=None, row_bytes=None):
    """A block whose every group holding rows spans a whole group, or
    `row_bytes` a row where stated; an empty group spans nothing."""
    total_bytes = data_bytes if data_bytes is not None else group_size * len(rows)

    def spanned(r):
        if not r:
            return 0
        return r * row_bytes if row_bytes is not None else group_size

    return {
        "header": {"schema": schema, "table": table, "columns": ["id"]},
        "database": database,
        "header_offset": 100,
        "data_offset": 200,
        "terminator_offset": 200 + total_bytes,
        "end_offset": 203 + total_bytes,
        "row_count": sum(rows),
        "statistics": None if rows is None else {
            "group_size": group_size,
            "groups": [{"rows": r, "bytes": spanned(r)} for r in rows],
            "columns": [None],
        },
    }


#: 51 groups of the minimum among 100, the rest empty: the upper middle group,
#: the 51st smallest, holds the minimum at the gathered size.
HALF_EMPTY = [1024, 0] * 49 + [1024, 1024]


def info(*blocks, tables=None):
    spans = [{"body": {"Data": {"Copy": b}}} for b in blocks]
    spans.insert(0, {"body": "Other"})
    return {"spans": spans, "metadata": {"databases": [{"tables": tables or {}}]}}


class Ladder(unittest.TestCase):
    def test_pairs_sum_from_the_start_and_an_odd_group_stands_alone(self):
        self.assertEqual(rd.coarsen([1, 2, 3, 4, 5]), [3, 7, 5])
        self.assertEqual(rd.ladder([1, 2, 3, 4, 5]), [[1, 2, 3, 4, 5], [3, 7, 5], [10, 5], [15]])

    def test_every_size_keeps_the_blocks_rows(self):
        rows = [random.Random(7).randrange(0, 50) for _ in range(333)]
        for level in rd.ladder(rows):
            self.assertEqual(sum(level), sum(rows))
        self.assertEqual(len(rd.ladder(rows)[-1]), 1)

    def test_one_group_is_its_own_ladder(self):
        self.assertEqual(rd.ladder([9]), [[9]])
        self.assertEqual(rd.ladder([]), [])


class LibraryDefault(unittest.TestCase):
    def test_the_default_minimum_is_the_librarys(self):
        src = (Path(__file__).resolve().parent.parent / "pgdump_query/src/statistics.rs").read_text()
        self.assertEqual(rd.DEFAULT_MIN_ROWS, 1 << 10)
        self.assertIn("pub const STATISTICS_GROUP_DEFAULT_MIN_ROWS: u64 = 1 << 10;", src)


class Quantile(unittest.TestCase):
    def test_nearest_rank(self):
        values = [1, 2, 3, 4]
        self.assertEqual(rd.quantile(values, 0.5), 2)  # the lower median
        self.assertEqual(rd.quantile([1, 2, 3], 0.5), 2)
        self.assertEqual(rd.quantile(values, 0.1), 1)
        self.assertEqual(rd.quantile(values, 0.9), 4)
        self.assertEqual(rd.quantile([5], 0.0), 5)

    def test_no_groups_is_refused(self):
        with self.assertRaises(ValueError):
            rd.quantile([], 0.5)
        with self.assertRaises(ValueError):
            rd.min_group([], 0.5)

    def test_the_minimums_median_is_the_upper_middle_group(self):
        # Even counts are where the two differ: the 3rd smallest of four, not
        # the 2nd, so at most half the groups fall short.
        self.assertEqual(rd.min_group([1, 2, 3, 4], 0.5), 3)
        self.assertEqual(rd.min_group([1, 2, 3], 0.5), 2)
        self.assertEqual(rd.min_group([5], 0.5), 5)
        # A lower quantile reported beside it stays nearest-rank.
        self.assertEqual(rd.min_group([1, 2, 3, 4], 0.25), 1)


class Choice(unittest.TestCase):
    def test_uniform_rows_choose_the_first_size_reaching_the_minimum(self):
        # 300 rows a group: 600 at 2N, 1200 at 4N.
        choice = rd.choose(rd.ladder([300] * 64), 1024, 0.5)
        self.assertEqual((choice.doublings, choice.groups, choice.reaches), (2, 16, True))

    def test_dense_rows_stay_at_the_gathered_size(self):
        choice = rd.choose(rd.ladder([5000] * 10), 1024, 0.5)
        self.assertEqual((choice.doublings, choice.groups), (0, 10))

    def test_a_block_that_never_reaches_the_minimum_is_one_group(self):
        choice = rd.choose(rd.ladder([10] * 8), 1024, 0.5)
        self.assertEqual((choice.doublings, choice.groups, choice.reaches), (3, 1, False))

    def test_a_minimum_of_zero_coarsens_nothing(self):
        self.assertEqual(rd.choose(rd.ladder([0, 0, 1]), 0, 0.5).doublings, 0)

    def test_the_median_keeps_groups_under_twice_rows_over_the_minimum(self):
        # The spec's bound, whatever the distribution: clustered, empty-gapped
        # and heavy-tailed blocks alike. Under the upper middle group it is
        # reached, not merely approached -- half the groups at the minimum and
        # half empty sit exactly on it.
        rng = random.Random(20)
        for trial in range(400):
            shape = trial % 4
            n = rng.randrange(1, 700)
            if shape == 0:
                rows = [rng.randrange(0, 4000) for _ in range(n)]
            elif shape == 1:
                rows = [rng.choice([0, 0, 0, rng.randrange(1000, 3000)]) for _ in range(n)]
            elif shape == 2:
                rows = [int(rng.paretovariate(0.8)) for _ in range(n)]
            else:
                rows = [1024 if i % 2 else 0 for i in range(n)]
            minimum = rng.choice([1, 64, 1024, 5000])
            choice = rd.choose(rd.ladder(rows), minimum, 0.5)
            if choice.reaches:
                with self.subTest(trial=trial):
                    self.assertLessEqual(choice.groups, 2 * sum(rows) / minimum)

    def test_the_choice_is_monotone_in_size(self):
        # What the guarantee rests on: once the upper middle group holds the
        # minimum, no coarser size falls back below it -- so a block the length
        # cap left at some size reaches the same size the base distribution
        # chooses, or keeps the cap's where that is coarser. The odd tail is
        # the shape the nearest-rank median breaks it on.
        self.assertEqual(rd.choose(rd.ladder([5, 5, 5, 5, 0, 0, 0]), 5, 0.5).doublings, 0)
        self.assertEqual(rd.choose(rd.ladder([10, 10, 0, 0]), 5, 0.5).doublings, 0)
        rng = random.Random(41)
        coarsened = 0
        for trial in range(400):
            rows = [rng.choice([0, 0, rng.randrange(0, 12)]) for _ in range(rng.randrange(1, 40))]
            minimum = rng.randrange(0, 30)
            sizes = rd.ladder(rows)
            first = rd.choose(sizes, minimum, 0.5).doublings
            for capped, level in enumerate(sizes):
                with self.subTest(trial=trial, capped=capped):
                    reached = rd.choose(rd.ladder(level), minimum, 0.5).doublings
                    self.assertEqual(capped + reached, max(first, capped))
            coarsened += 0 < first < len(sizes) - 1
        self.assertGreater(coarsened, 50, "too few trials coarsened short of one group")

    def test_half_empty_groups_approach_the_bound(self):
        # Just over half the groups at the minimum and the rest empty: the one
        # shape the bound is tight for, and what "close" is meant to catch.
        result = rd.block_density(block(HALF_EMPTY, data_bytes=40 << 20), 1024)
        self.assertEqual(result["choices"][0]["groups"], 100)
        self.assertAlmostEqual(result["bound_ratio"], 100 / 102)
        self.assertGreaterEqual(result["bound_ratio"], rd.CLOSE_RATIO)
        self.assertTrue(result["close"])

    def test_uniform_density_lands_between_a_quarter_and_a_half_of_the_bound(self):
        for per_group in (1, 7, 100, 513, 1023, 1024, 1500, 2047):
            rows = [per_group] * 4096
            result = rd.block_density(block(rows, data_bytes=len(rows) << 20), 1024)
            if result["choices"][0]["reaches_minimum"] and result["choices"][0]["groups"] > 1:
                with self.subTest(per_group=per_group):
                    self.assertGreater(result["bound_ratio"], 0.25)
                    self.assertLessEqual(result["bound_ratio"], 0.5)
                    self.assertFalse(result["close"])


class Width(unittest.TestCase):
    def test_a_wide_block_is_outside_the_criterion(self):
        # The median group's rows at 2 KiB each, however near the bound.
        wide = rd.block_density(block(HALF_EMPTY, row_bytes=2048), 1024)
        self.assertEqual(wide["median_group_row_bytes"], 2048)
        self.assertFalse(wide["reasonable_width"])
        self.assertFalse(wide["close"])

    def test_a_mean_lifted_by_wide_rows_does_not_hide_a_dense_median(self):
        # The shape the width judgement exists for: half the groups dense, the
        # other half one row of a MiB and a half each, so the mean row is far
        # past 1 KiB while the median group holds a KiB a row -- and the block
        # is as near the bound as the half-empty one.
        rows = [1024, 1] * 49 + [1024, 1024]
        groups = block(rows)
        for g in groups["statistics"]["groups"]:
            g["bytes"] = (3 << 19) if g["rows"] == 1 else 1 << 20
        result = rd.block_density(groups, 1024)
        self.assertGreater(result["mean_row_bytes"], 1024)
        self.assertEqual(result["median_group_row_bytes"], 1024)
        self.assertTrue(result["reasonable_width"])
        self.assertTrue(result["close"])

    def test_an_empty_median_group_is_not_of_reasonable_width(self):
        result = rd.block_density(block([0, 0, 0, 5000]), 1024)
        self.assertIsNone(result["median_group_row_bytes"])
        self.assertFalse(result["reasonable_width"])

    def test_the_median_group_is_nearest_rank_by_rows_ties_by_bytes(self):
        groups = [{"rows": 10, "bytes": 900}, {"rows": 4, "bytes": 400}, {"rows": 10, "bytes": 300},
                  {"rows": 2, "bytes": 8000}]
        # By (rows, bytes): (2, 8000), (4, 400), (10, 300), (10, 900); the 2nd is the median.
        self.assertEqual(rd.median_group_width(groups), 100)
        groups.append({"rows": 10, "bytes": 200})
        # Five groups: the 3rd, (10, 200).
        self.assertEqual(rd.median_group_width(groups), 20)


class Density(unittest.TestCase):

    def test_every_size_is_summarised(self):
        result = rd.block_density(block([1, 2, 3, 4]), 1)
        self.assertEqual([s["group_size"] for s in result["sizes"]], [1 << 20, 1 << 21, 1 << 22])
        self.assertEqual(result["sizes"][0]["p50"], 2)
        self.assertEqual(result["sizes"][1]["min"], 3)
        self.assertEqual(result["sizes"][2]["max"], 10)

    def test_blocks_with_rows_and_no_statistics_are_listed(self):
        empty = block([0], table="empty")
        empty["statistics"] = None
        empty["row_count"] = 0
        untracked = block([0], table="untracked")
        untracked["statistics"] = None
        untracked["row_count"] = 5
        result = rd.density(info(block([2000] * 3), empty, untracked), 1024)
        self.assertEqual([b["table"] for b in result["blocks"]], ["public.t"])
        self.assertEqual(result["untracked"], ["public.untracked"])

    def test_a_database_qualifies_the_table(self):
        self.assertEqual(rd.qualified(block([1], database="koji")), "koji:public.t")

    def test_the_verdict_names_what_is_close(self):
        dense = rd.block_density(block([4000] * 8, table="dense"), 1024)
        gapped = rd.block_density(block(HALF_EMPTY, table="gapped", data_bytes=40 << 20), 1024)
        self.assertIn("not met", rd.verdict([dense]))
        self.assertIn("MET", rd.verdict([dense, gapped]))
        self.assertIn("public.gapped", rd.verdict([dense, gapped]))
        sparse = rd.block_density(block([1] * 4, table="sparse", data_bytes=4 << 10), 1024)
        self.assertIn("nothing to judge", rd.verdict([sparse]))

    def test_main_writes_the_artifact_and_the_table(self):
        with TemporaryDirectory() as tmp:
            source = Path(tmp) / "info.json"
            source.write_text(json.dumps(info(block([300] * 16))))
            out = Path(tmp) / "nested" / "density.json"
            printed = io.StringIO()
            with redirect_stdout(printed):
                self.assertEqual(rd.main(["density", str(source), "--out", str(out)]), 0)
            written = json.loads(out.read_text())
            self.assertEqual(written[str(source)]["blocks"][0]["choices"][0]["groups"], 4)
            self.assertIn("criterion:", printed.getvalue())


class Selection(unittest.TestCase):
    def test_the_narrowest_fixed_width_column_first_among_equals(self):
        columns = [
            {"name": "note", "declared_type": "text"},
            {"name": "created", "declared_type": "timestamp with time zone"},
            {"name": "id", "declared_type": "integer"},
            {"name": "other", "declared_type": "integer"},
        ]
        self.assertEqual(rd.narrowest(columns)["name"], "id")

    def test_a_table_of_no_fixed_width_column_tracks_its_first(self):
        columns = [{"name": "a", "declared_type": "text"}, {"name": "b", "declared_type": "character varying(20)"}]
        self.assertEqual(rd.narrowest(columns)["name"], "a")

    def test_the_selection_names_one_column_a_table(self):
        tables = {
            "public.task": [{"name": "request", "declared_type": "text"}, {"name": "id", "declared_type": "integer"}],
            "public.flag": [{"name": "on", "declared_type": "boolean"}],
            "public.nothing": [],
        }
        self.assertEqual(rd.selection(info(tables=tables)), "public.flag.on,public.task.id")

    def test_a_name_a_selection_cannot_spell_is_refused(self):
        with self.assertRaises(ValueError):
            rd.selection(info(tables={"public.t": [{"name": "a.b", "declared_type": "integer"}]}))
        with self.assertRaises(ValueError):
            rd.selection(info())


if __name__ == "__main__":
    unittest.main()
