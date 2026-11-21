"""Tests of `measurement.figures.micro`."""

from __future__ import annotations

import dataclasses
import json
import release
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import host
from measurement import parsing
from measurement import staging
from measurement import timing
from measurement.builds import image
from measurement.figures import micro


class ResumableReader(unittest.TestCase):
    """`resumable-reader-micro`: which form a resumable reader takes. Its
    verdict is the deliverable, so what decides it is pinned here before any
    reading exists: the one-run rows alone; a noise the `async` walk is read
    against that is the wider of the pull's own scatter and the intervals,
    widened by how far its gap moves in the layout leg; and the push machine
    taken only where it beats the `async` walk past a noise formed alike."""

    @staticmethod
    def _est(median, half=0.0):
        return parsing.Estimate(median, median - half, median + half)

    def _forms(self, pull, again, walk, push, half):
        return micro.ResumableForms(
            self._est(pull, half), self._est(again, half), self._est(walk, half), self._est(push, half)
        )

    def _row(self, literal="array_4", feed="whole", pull=100.0, again=100.0, walk=100.0,
             push=90.0, half=0.0, aligned=None):
        shipped = self._forms(pull, again, walk, push, half)
        return micro.ResumableRow(
            literal, feed, 49, shipped,
            shipped if aligned is None else self._forms(*aligned, half),
        )  # fmt: skip

    def _rows(self, **whole):
        return [
            self._row(literal=literal, feed=feed, **(whole if feed == "whole" else {}))
            for literal, _, _ in micro.RESUMABLE_LITERALS
            for feed, _ in micro.RESUMABLE_FEEDS
        ]

    def test_the_estimate_is_criterions_median_and_its_interval(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp) / "x" / "new"
            d.mkdir(parents=True)
            (d / "benchmark.json").write_text(json.dumps({"full_id": "resumable/int8/pull/whole"}))
            (d / "estimates.json").write_text(json.dumps({"median": {
                "point_estimate": 20.0,
                "confidence_interval": {"lower_bound": 19.0, "upper_bound": 22.0},
            }}))  # fmt: skip
            got = parsing.criterion_estimate(Path(tmp), "resumable/int8/pull/whole")
        self.assertEqual(got, parsing.Estimate(20.0, 19.0, 22.0))
        self.assertEqual(got.half_width, 1.5)

    def test_noise_is_the_wider_of_the_pulls_scatter_and_the_intervals(self):
        # The two pull timings 10 ns apart, intervals of ±2: the scatter wins.
        self.assertEqual(self._row(pull=95.0, again=105.0, half=2.0).noise_ns, 10.0)
        # Timings agreeing, intervals of ±3: the pull's and the walk's summed.
        self.assertEqual(self._row(half=3.0).noise_ns, 6.0)

    def test_noise_widens_by_how_far_the_gap_moves_in_the_layout_leg(self):
        # Shipped async − pull is +0; aligned it is +7, the pull having moved
        # alone, so the noise of ±3 + ±3 widens by 7.
        row = self._row(half=3.0, aligned=(93.0, 93.0, 100.0, 90.0))
        self.assertEqual(row.layout_async_ns, 7.0)
        self.assertEqual(row.noise_ns, 13.0)
        # A gap that holds still across the builds, both forms having moved
        # together, adds nothing however far each moved.
        self.assertEqual(self._row(half=3.0, aligned=(150.0, 150.0, 150.0, 140.0)).noise_ns, 6.0)

    def test_the_gaps_are_read_off_the_shipped_build(self):
        # A walk past its noise in the shipped build stays past it, however
        # far inside it the layout leg reads, but for the layout term.
        row = self._row(walk=110.0, half=1.0, aligned=(100.0, 100.0, 101.0, 90.0))
        self.assertEqual(row.shipped.async_gap, 10.0)
        self.assertEqual(row.noise_ns, 2.0 + 9.0)
        self.assertTrue(row.async_within)
        row = self._row(walk=110.0, half=1.0, aligned=(100.0, 100.0, 108.0, 90.0))
        self.assertFalse(row.async_within)

    def test_push_noise_is_formed_as_the_async_noise_is(self):
        # push − async is −10 shipped and −4 aligned: ±2 + ±2 widened by 6.
        row = self._row(push=90.0, half=2.0, aligned=(100.0, 100.0, 100.0, 96.0))
        self.assertEqual(row.layout_push_ns, 6.0)
        self.assertEqual(row.push_noise_ns, 10.0)
        self.assertFalse(row.push_pays)
        self.assertTrue(self._row(push=80.0, half=2.0, aligned=(100.0, 100.0, 100.0, 86.0)).push_pays)
        # The sitting's drift bounds it from below, as it bounds the walk's.
        self.assertEqual(self._row(pull=90.0, again=110.0, push=90.0).push_noise_ns, 20.0)

    def test_async_within_its_noise_of_the_pull_is_the_async_form(self):
        rows = self._rows(walk=104.0, half=2.0)
        self.assertTrue(all(row.async_within for row in rows))
        self.assertIn("resume as `async` walks", micro.resumable_verdict(rows))

    def test_async_past_its_noise_with_push_paying_is_the_push_form(self):
        rows = self._rows(half=2.0)
        rows[2] = self._row(literal="array_4", walk=105.0, push=90.0, half=2.0)
        verdict = micro.resumable_verdict(rows)
        self.assertIn("hand-written push machines", verdict)
        self.assertIn("`array_4`", verdict)

    def test_async_past_its_noise_with_push_not_paying_is_still_the_async_form(self):
        rows = self._rows(half=2.0)
        rows[2] = self._row(literal="array_4", walk=105.0, push=102.0, half=2.0)
        verdict = micro.resumable_verdict(rows)
        self.assertIn("resume as `async` walks, neither form holding today's price", verdict)
        self.assertIn("`array_4`", verdict)
        # Push paying on one row the walk misses and not on another is the
        # walk: push is taken only where it pays on every one.
        rows[4] = self._row(literal="array_50", walk=105.0, push=90.0, half=2.0)
        verdict = micro.resumable_verdict(rows)
        self.assertIn("neither form holding today's price", verdict)
        self.assertIn("not cheaper than it past theirs on `array_4`**", verdict)

    def test_push_paying_where_the_walk_is_within_decides_nothing(self):
        rows = self._rows(walk=101.0, push=50.0, half=2.0)
        self.assertTrue(all(row.push_pays for row in rows))
        self.assertIn("resume as `async` walks", micro.resumable_verdict(rows))

    def test_a_row_fed_a_byte_a_run_decides_nothing(self):
        rows = [
            self._row(literal=literal, feed=feed, walk=1000.0 if feed == "bytes" else 100.0)
            for literal, _, _ in micro.RESUMABLE_LITERALS
            for feed, _ in micro.RESUMABLE_FEEDS
        ]
        self.assertIn("resume as `async` walks", micro.resumable_verdict(rows))
        self.assertNotIn("**no**", micro.render_resumable(rows).split("| a run a byte |")[1])

    def test_no_one_run_row_is_no_verdict(self):
        with self.assertRaises(ValueError):
            micro.resumable_verdict([self._row(feed="bytes")])

    @staticmethod
    def _write_estimates(root, median):
        for n, (literal, _, _) in enumerate(micro.RESUMABLE_LITERALS):
            for feed, _ in micro.RESUMABLE_FEEDS:
                for form in micro.RESUMABLE_FORMS:
                    d = root / f"{n}_{literal}_{form}_{feed}" / "new"
                    d.mkdir(parents=True)
                    full_id = f"resumable/{literal}/{form}/{feed}"
                    (d / "benchmark.json").write_text(json.dumps({"full_id": full_id}))
                    (d / "estimates.json").write_text(json.dumps({"median": {
                        "point_estimate": median,
                        "confidence_interval": {"lower_bound": median - 1, "upper_bound": median + 1},
                    }}))  # fmt: skip

    def test_every_id_the_figure_reads_is_one_the_bench_times(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, aligned = Path(tmp) / "shipped", Path(tmp) / "aligned"
            self._write_estimates(root, 50.0)
            self._write_estimates(aligned, 70.0)
            rows = micro.resumable_rows(root, aligned)
        self.assertEqual(len(rows), len(micro.RESUMABLE_LITERALS) * len(micro.RESUMABLE_FEEDS))
        # Each build's estimates are read from its own root.
        self.assertTrue(all(row.shipped.pull.median == 50.0 for row in rows))
        self.assertTrue(all(row.aligned.pull.median == 70.0 for row in rows))
        table = micro.render_resumable(rows)
        self.assertIn("resume as `async` walks", table)
        # The renderer's ids are the bench's: each literal, form and feed is
        # spelled in its source, and each literal's byte count is its text's.
        bench = (config.REPO / "crates/pgdt-codec/benches/resumable/main.rs").read_text()
        for literal, _, _ in micro.RESUMABLE_LITERALS:
            self.assertIn(f'"{literal}",', bench)
        for form in micro.RESUMABLE_FORMS:
            self.assertIn(f'"{form}"', bench)
        for feed, _ in micro.RESUMABLE_FEEDS:
            self.assertIn(f'("{feed}", ', bench)
        self.assertIn('"-1234567890".to_owned()', bench)
        self.assertEqual(dict((lit, n) for lit, _, n in micro.RESUMABLE_LITERALS)["int8"], 11)

    def test_a_dry_run_records_where_it_would_run_and_runs_nothing(self):
        cfg = config.Config(dry_run=True)
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        with unittest.mock.patch.object(release, "ensure_image") as built:
            got = micro.run_resumable_reader_micro(session)
        built.assert_not_called()
        self.assertIn("dry run", got)
        self.assertEqual(
            session.places["resumable-reader-micro"],
            {release.image_tag(release.image_inputs())},
        )
        self.assertEqual(
            micro.RESUMABLE_BENCH.step(),
            ["bench", "--package=pgdt-codec", "--bench=resumable", "--filter=resumable"],
        )

    def test_the_layout_leg_is_the_same_bench_aligned_in_a_target_dir_of_its_own(self):
        leg = micro.RESUMABLE_ALIGNED_BENCH
        self.assertEqual(dataclasses.replace(leg, rustflags=""), micro.RESUMABLE_BENCH)
        self.assertEqual(leg.rustflags, "-C llvm-args=-align-all-functions=6")
        cfg = config.Config()
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        ran = []
        with unittest.mock.patch.object(release, "ensure_image", return_value="t"), \
                unittest.mock.patch.object(release, "prepare_state"), \
                unittest.mock.patch.object(session, "ran_in"), \
                unittest.mock.patch.object(host, "run", lambda argv, **_k: ran.append(argv)), \
                unittest.mock.patch.object(micro, "render_resumable", lambda rows: "table"), \
                unittest.mock.patch.object(micro, "resumable_rows", lambda *roots: roots):
            micro.run_resumable_reader_micro(session)
        own = image.image_target_dir(cfg, micro.RESUMABLE_ALIGNED_DIR)
        shipped, aligned = ran
        self.assertNotIn(f"{own.resolve()}:", " ".join(shipped))
        self.assertIn(f"{own.resolve()}:", " ".join(aligned))
        self.assertIn(f"--rustflags={leg.rustflags}", aligned)
        # A bench stating flags in the release state's target dir is refused.
        with self.assertRaises(ValueError):
            micro.bench_in_image(session, "resumable-reader-micro", leg)


class BenchConstants(unittest.TestCase):
    """The micro table's byte counts are computed from the bench's own literal
    shape, so they cannot drift out of agreement with it silently."""

    def test_array_literal_lengths(self):
        self.assertEqual(micro.array_literal_bytes(4), 49)
        self.assertEqual(micro.array_literal_bytes(50), 601)

    def test_record_literal_length(self):
        self.assertEqual(len(micro._RECORD_LITERAL), 42)

    def test_the_table_rows_carry_those_lengths(self):
        self.assertEqual([row[1] for row in micro._MICRO_ROWS], [49, 601, 42])


if __name__ == "__main__":
    unittest.main()
