"""Tests of `measurement.gating`."""

from __future__ import annotations

import dataclasses
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import gating
from measurement import memory_model
from measurement import replay
from measurement import staging
from measurement import timing
from measurement.builds import leg_binaries
from measurement.figures import gate_table
from measurement.figures import parallel
from measurement.figures import register
from measurement.figures import reserve


class GateLegs(unittest.TestCase):
    """The gate's `system` legs: every reading taken against a memory limit is
    read on the platform allocator's build too, in the same sitting, so a
    failure can be told the allocator's from the code's."""

    def test_the_gate_build_is_an_allocator_leg_and_not_the_shipped_one(self):
        self.assertIn(gating.GATE_LEG, leg_binaries.ALLOCATOR_LEGS)
        self.assertNotEqual(gating.GATE_LEG, leg_binaries.ALLOCATOR_LEGS[0])
        self.assertEqual(gating.GATE_BINARY, f"alloc:{gating.GATE_LEG}")

    def test_the_gate_binary_is_the_allocator_figures_build(self):
        cfg = config.Config()
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        with unittest.mock.patch.object(
            leg_binaries, "ensure_allocator_binary", lambda _c, leg, _l: Path(f"/{leg}")
        ):
            self.assertEqual(session.binary_path(gating.GATE_BINARY), Path(f"/{gating.GATE_LEG}"))

    def test_a_twin_differs_from_its_leg_by_the_binary_alone(self):
        for spec in (*reserve._reserve_flagless_specs(), *parallel._parallel_rss_specs()):
            with self.subTest(leg=spec.label):
                twin = gating.gate_twin(spec)
                self.assertEqual(twin.binary, gating.GATE_BINARY)
                self.assertEqual(
                    dataclasses.replace(twin, binary=spec.binary, label=spec.label), spec
                )
                self.assertNotEqual(twin.key("f"), spec.key("f"))

    def test_only_the_shipped_build_has_a_twin(self):
        spec = reserve._reserve_instrument_specs()[0]
        with self.assertRaises(ValueError):
            gating.gate_twin(spec)

    def test_the_reserve_twins_are_the_flagless_axis_and_keep_its_licence(self):
        twins = reserve._reserve_gate_specs()
        self.assertEqual(
            [dataclasses.replace(t, binary="pgdt", label="") for t in twins],
            [dataclasses.replace(s, label="") for s in reserve._reserve_flagless_specs()],
        )
        for twin in twins:
            with self.subTest(leg=twin.label):
                self.assertTrue(timing.kill_tolerant(twin.command))

    def test_both_gate_figures_declare_the_features_that_select_the_build(self):
        for fid in ("reserve", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                self.assertIn("crates/pgdt/Cargo.toml", register.EVERY_BY_ID[fid].depends)

    def test_the_verdicts(self):
        self.assertEqual(gate_table.gate_verdict(True, False)[1], True)
        for shipped, twin in ((True, True), (False, True), (False, False), (None, False), (True, None)):
            with self.subTest(shipped=shipped, twin=twin):
                self.assertFalse(gate_table.gate_verdict(shipped, twin)[1])
        self.assertIn("both", gate_table.gate_verdict(True, True)[0])
        self.assertIn("not judged", gate_table.gate_verdict(None, True)[0])

    def _section(
        self,
        killed: dict[str, int],
        twins: bool = True,
        figure: str = "reserve",
        shipped_kib: float = 400_000.0,
        twin_kib: float = 400_000.0,
        limit: int = 1 << 30,
    ):
        specs = reserve._reserve_flagless_specs()[:2]
        every = [*specs, *(gating.gate_twin(s) for s in specs)] if twins else specs
        raw = {
            "readings": {s.key(figure): [1.0] * 3 for s in every},
            "rss": {
                s.key(figure): [shipped_kib if s in specs else twin_kib]
                * (3 - killed.get(s.key(figure), 0))
                for s in every
            },
            "killed": {k: n for k, n in killed.items() if n},
            "input_sizes": {},
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            legs = [(s.label, s, limit) for s in specs]
            return gate_table.gate_section(session, figure, legs), specs

    def test_reserve_reports_the_librarys_margin_and_no_other_figure_does(self):
        self.assertEqual(
            gate_table.REPORTED_MARGIN_PERCENT, {"reserve": memory_model.LIBRARY_MEMORY_MARGIN_PERCENT}
        )

    def test_a_reserve_leg_under_the_margin_on_the_shipped_build_alone_passes_and_says_so(self):
        # 900,000 KiB of a 1 GiB limit leaves 14.2%: no rep killed, so the
        # gate passes it, and the shortfall is reported beside the verdict.
        body, _ = self._section({}, shipped_kib=900_000.0)
        self.assertIn("**The gate passes**", body)
        self.assertNotIn("blocks**", body)
        self.assertIn("| 878.91 MiB · head 14.2% | 390.62 MiB · head 61.9% | passes |", body)
        self.assertIn("| **shipped build short** |", body)
        self.assertIn("2 leg(s) leave less than the 20% margin on the shipped build alone", body)
        self.assertIn("blocks nothing", body)
        self.assertIn("A leg fails where it is killed.", body)

    def test_a_reserve_leg_under_the_margin_on_both_builds_passes_and_files_nothing(self):
        body, _ = self._section({}, shipped_kib=900_000.0, twin_kib=900_000.0)
        self.assertIn("**The gate passes**", body)
        self.assertNotIn("failed on both legs", body)
        self.assertIn("| both short |", body)
        self.assertNotIn("on the shipped build alone —", body)

    def test_the_margin_line_itself_is_met(self):
        # 1000 MiB, so 80% is a whole number of KiB and the comparison is
        # exact at the line: leaving exactly the margin meets it, as
        # `margin_allowance`'s ceiling does, and a KiB more is short. The gate
        # passes either way.
        limit = 1000 << 20
        at_line = limit * (100 - memory_model.LIBRARY_MEMORY_MARGIN_PERCENT) // 100 // 1024
        body, _ = self._section({}, shipped_kib=float(at_line), limit=limit)
        self.assertIn("**The gate passes**", body)
        self.assertIn("| met |", body)
        body, _ = self._section({}, shipped_kib=float(at_line + 1), limit=limit)
        self.assertIn("**The gate passes**", body)
        self.assertIn("| **shipped build short** |", body)

    def test_a_killed_reserve_leg_inside_the_margin_still_blocks(self):
        # The survivors leave the margin; the kill alone fails the leg.
        specs = reserve._reserve_flagless_specs()[:2]
        body, _ = self._section({specs[0].key("reserve"): 1})
        self.assertIn("**The gate blocks**: 1 leg(s)", body)
        self.assertIn("| met |", body)

    def test_the_margin_notes(self):
        self.assertEqual(gate_table.margin_note(None, None), "—")
        self.assertEqual(gate_table.margin_note(False, False), "met")
        self.assertEqual(gate_table.margin_note(False, None), "met where read")
        self.assertEqual(gate_table.margin_note(True, True), "both short")
        self.assertEqual(gate_table.margin_note(True, False), "**shipped build short**")
        self.assertEqual(gate_table.margin_note(True, None), "**shipped build short**")
        self.assertEqual(gate_table.margin_note(False, True), f"`{gating.GATE_LEG}` short")

    def test_parallel_peak_rss_fails_on_a_kill_alone(self):
        # The same reading the reserve gate reports short passes here with no
        # margin column: the container is that figure's contract.
        body, _ = self._section({}, figure="parallel-peak-rss", shipped_kib=900_000.0)
        self.assertIn("**The gate passes**", body)
        self.assertNotIn("margin", body)
        self.assertIn("A leg fails where it is killed, its container", body)
        specs = reserve._reserve_flagless_specs()[:2]
        body, _ = self._section({specs[0].key("parallel-peak-rss"): 1}, figure="parallel-peak-rss")
        self.assertIn("**The gate blocks**: 1 leg(s)", body)

    def test_a_kill_on_the_shipped_build_alone_blocks(self):
        specs = reserve._reserve_flagless_specs()[:2]
        body, _ = self._section({specs[0].key("reserve"): 3})
        self.assertIn("**The gate blocks**: 1 leg(s)", body)
        self.assertIn("fails on the shipped build alone", body)
        self.assertIn("D13", body)

    def test_a_kill_on_both_legs_is_filed_and_does_not_block(self):
        specs = reserve._reserve_flagless_specs()[:2]
        twin = gating.gate_twin(specs[0])
        body, _ = self._section({specs[0].key("reserve"): 2, twin.key("reserve"): 3})
        self.assertIn("**The gate passes**", body)
        self.assertIn("1 failed on both legs", body)
        # A partly-killed leg prints its survivors' worst beside the kill.
        self.assertIn("survivors' worst 390.62 MiB · head 61.9%", body)

    def test_every_cell_is_the_worst_rep_and_its_headroom(self):
        body, _ = self._section({})
        self.assertIn("390.62 MiB · head 61.9%", body)
        self.assertIn("**The gate passes**", body)

    def test_a_sitting_with_no_twin_is_not_judged_rather_than_passed(self):
        body, _ = self._section({}, twins=False)
        self.assertIn("not judged", body)
        self.assertNotIn("passes", body)

    def test_replay_builds_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(), {"readings": {}}, Path(tmp), lambda _m: None
            )
            with unittest.mock.patch.object(
                leg_binaries, "ensure_allocator_binary", side_effect=AssertionError("built")
            ):
                session.build_before_sweep(reserve._reserve_gate_specs())

    def test_a_live_session_builds_the_gate_leg_before_the_sweep(self):
        cfg = config.Config()
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        built = []
        with unittest.mock.patch.object(
            leg_binaries, "ensure_allocator_binary", lambda _c, leg, _l: built.append(leg) or Path("/x")
        ):
            session.build_before_sweep(
                [*reserve._reserve_flagless_specs(), *reserve._reserve_gate_specs()]
            )
        self.assertEqual(built, [gating.GATE_LEG])


if __name__ == "__main__":
    unittest.main()
