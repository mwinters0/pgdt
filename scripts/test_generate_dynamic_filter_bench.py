"""Tests for generate_dynamic_filter_bench.py: the control's rows with two join
keys appended, and the three build tables the figures join them against."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import generate_dynamic_filter_bench as gen
import generate_perf_data as perf


def _blocks(path: Path) -> tuple[str, dict[str, list[list[str]]]]:
    """The DDL before the first `COPY`, and each table's rows."""
    text = path.read_text()
    ddl, _, rest = text.partition("COPY ")
    blocks = {}
    for block in ("COPY " + rest).split("COPY ")[1:]:
        head, _, body = block.partition(" FROM stdin;\n")
        table = head.split(" ", 1)[0]
        data = body.split("\\.\n", 1)[0]
        blocks[table] = [line.split("\t") for line in data.splitlines()]
    return ddl, blocks


class GenerateDynamicFilterBench(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "dynfilter.sql"
            gen.generate(out, 1 << 20, seed=42)
            cls.ddl, cls.blocks = _blocks(out)
            control = Path(tmp) / "control.sql"
            perf.generate(control, 1 << 20, 42, arrays=False, composites=False)
            _, control_blocks = _blocks(control)
            cls.control = control_blocks[perf.TABLE]
        cls.probe = cls.blocks[perf.TABLE]

    def test_every_probe_row_is_the_controls_with_the_keys_appended(self):
        # Same seed, same draws: the keys are added without moving a byte of
        # what the control's rows hold.
        n = min(len(self.probe), len(self.control))
        self.assertGreater(n, 100)
        for row, control in zip(self.probe[:n], self.control[:n]):
            self.assertEqual(row[: -len(gen.COLUMNS)], control)
            row_id = int(row[0])
            self.assertEqual(row[-2:], [str(gen.u_key(row_id)), str(gen.bucket(row_id))])

    def test_all_ddl_precedes_the_data(self):
        # `pg_dump`'s order; the preamble is read before the first `COPY`.
        for table in (perf.TABLE, *(t for t, _ in gen.BUILD_TABLES)):
            with self.subTest(table=table):
                self.assertIn(f"CREATE TABLE {table} (", self.ddl)
                self.assertIn(table, self.blocks)

    def test_u_key_is_a_bijection_and_unsorted(self):
        keys = [gen.u_key(i) for i in range(1, 100_001)]
        self.assertEqual(len(set(keys)), len(keys))
        self.assertNotEqual(keys, sorted(keys))
        self.assertTrue(all(0 <= k < 1 << 32 for k in keys))
        self.assertEqual(gen.SCRAMBLE % 2, 1)

    def test_every_bucket_is_in_every_stretch_of_rows(self):
        # What makes the costing join's filter reject nothing: `every` holds
        # every bucket, and any `BUCKETS` consecutive rows hold them all.
        self.assertEqual({gen.bucket(i) for i in range(500, 500 + gen.BUCKETS)}, set(range(gen.BUCKETS)))
        every = self.blocks["public.every"]
        self.assertEqual(sorted(int(r[0]) for r in every), list(range(gen.BUCKETS)))

    def test_the_selective_joins_each_match_build_rows_probe_rows(self):
        ids = {int(r[0]) for r in self.probe}
        u_keys = {int(r[-2]) for r in self.probe}
        near = [int(r[0]) for r in self.blocks["public.near"]]
        scattered = [int(r[0]) for r in self.blocks["public.scattered"]]
        for name, keys, probe in (("near", near, ids), ("scattered", scattered, u_keys)):
            with self.subTest(table=name):
                self.assertEqual(len(keys), gen.BUILD_ROWS)
                self.assertEqual(len(set(keys)), gen.BUILD_ROWS)
                self.assertTrue(set(keys) <= probe)

    def test_near_is_a_run_at_the_middle_and_scattered_spans_the_table(self):
        rows = len(self.probe)
        near = [int(r[0]) for r in self.blocks["public.near"]]
        self.assertEqual(near, list(range(near[0], near[0] + gen.BUILD_ROWS)))
        self.assertLess(abs(near[0] + gen.BUILD_ROWS // 2 - rows // 2), 2)
        ids = gen.scattered_ids(rows)
        self.assertLess(ids[0], rows // gen.BUILD_ROWS + 1)
        self.assertGreater(ids[-1], rows - rows // gen.BUILD_ROWS - 1)

    def test_both_selective_joins_publish_an_in_list(self):
        # DataFusion 55 publishes membership as an `IN` list up to this many
        # distinct build keys; past it, a `hash_lookup` the scan cannot read.
        self.assertLessEqual(gen.BUILD_ROWS, gen.BUCKETS)
        self.assertEqual(gen.BUCKETS, 150)


if __name__ == "__main__":
    unittest.main()
