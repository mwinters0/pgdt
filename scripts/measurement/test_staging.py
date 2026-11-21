"""Tests of `measurement.staging`."""

from __future__ import annotations

import inspect
import os
import shutil
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import emit
from measurement import inputs
from measurement import replay
from measurement import runspec
from measurement import staging
from measurement import timing
from measurement.figures import nested
from measurement.figures import register
from measurement.figures import throughput


class Eviction(unittest.TestCase):
    """The tmpfs budget is smaller than the union of the inputs, so the sweep
    is staged. What must never happen is evicting something the step in hand
    still needs, or thrashing an input the next step wants."""

    def _stager(self, budget: float | None):
        cfg = config.Config(dry_run=True, warm_budget=budget)
        return staging.Stager(cfg, lambda _msg: None)

    def test_an_input_nothing_wants_again_goes_first(self):
        stager = self._stager(None)
        stager.needs = {"control": [0, 3], "arrays": [1], "composite": [3]}
        stager._staged = {"control": 3 * config.GIB, "arrays": 3 * config.GIB}
        stager._make_room(3 * config.GIB, step=3)
        self.assertNotIn("arrays", stager._staged)
        self.assertIn("control", stager._staged)

    def test_what_the_current_step_needs_is_never_the_victim(self):
        stager = self._stager(3)
        stager.needs = {"control": [0]}
        stager._staged = {"control": 3 * config.GIB}
        with self.assertRaises(staging.StagingError):
            stager._make_room(3 * config.GIB, step=0)

    def test_a_budget_too_small_for_one_input_is_an_error(self):
        stager = self._stager(1)
        stager.needs = {}
        with self.assertRaises(staging.StagingError):
            stager._make_room(3 * config.GIB, step=0)

    def test_room_already_free_evicts_nothing(self):
        stager = self._stager(None)
        stager.needs = {"control": [0, 5]}
        stager._staged = {"control": 3 * config.GIB}
        stager._make_room(3 * config.GIB, step=1)
        self.assertEqual(list(stager._staged), ["control"])

    def test_a_split_figure_evicts_its_own_earlier_group(self):
        # The point of a split: the second group's input displaces the first
        # group's, which no later step wants, rather than refusing because the
        # figure as a whole still names it.
        stager = self._stager(None)
        fig = register.FIGURES_BY_ID["statistics-gathering"]
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
        cfg = config.Config(dry_run=True, warm_budget=budget)
        return staging.Stager(cfg, lambda _msg: None)

    def test_the_bound_is_two_full_size_inputs_and_the_slack(self):
        cfg = config.Config()
        self.assertEqual(staging.WARM_FULL_INPUTS, 2)
        self.assertEqual(staging.WARM_SLACK, 64 * config.MIB)
        self.assertEqual(
            staging.warm_bound(cfg), int(2 * cfg.size_gib * config.GIB) + 64 * config.MIB
        )
        self.assertEqual(self._stager().budget(), staging.warm_bound(cfg))

    def test_the_bound_does_not_follow_the_figures_selected(self):
        # The defect it replaces: a budget computed from the largest figure
        # grew with whatever a slice added, past a quota nothing checked.
        small, every = self._stager(), self._stager()
        small.plan([register.FIGURES_BY_ID["peak-rss"]])
        every.plan(register.FIGURES)
        self.assertEqual(small.budget(), every.budget())

    def test_every_warm_set_fits_the_bound(self):
        # Real sizes where the inputs exist, so a generator's overshoot is
        # counted; nominal ones otherwise.
        stager = self._stager()
        stager.plan(register.EVERY_FIGURE)
        budget = stager.budget()
        for fig in register.EVERY_FIGURE:
            for gi, group in enumerate(fig.staging_groups):
                with self.subTest(figure=fig.id, group=group):
                    self.assertLessEqual(stager.group_need[(fig.id, gi)], budget)

    def test_a_generator_overshooting_its_target_does_not_break_the_bound(self):
        # The real defect behind the slack: three inputs whose nominal sum was
        # exactly the budget overshot it by 6,799 bytes, and the sweep died
        # twenty minutes in.
        stager = self._stager()
        stager.plan(register.EVERY_FIGURE)
        self.assertGreater(stager.budget() - max(stager.group_need.values()), 64 * 1024)

    def test_a_split_figure_covers_its_warm_set(self):
        # An input is measured in a second sweep only where a declared
        # subtraction pairs it with that sweep's other input: a repeat nothing
        # reads is a full-size input's reps spent for no reading.
        paired = {
            (s.source, frozenset(s.inputs)) for fig in register.EVERY_FIGURE for s in fig.subtracts
        }
        for fig in register.EVERY_FIGURE:
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
                                    register.subtraction_sweep(fig.id, name, other) == gi
                                    for other in group
                                    if other != name
                                    and (fig.id, frozenset((name, other))) in paired
                                ),
                                f"{name} is in sweep {gi} beside nothing it is paired with",
                            )

    def test_the_figures_the_bound_splits(self):
        # One sweep per input where no reading subtracts one input from
        # another; `nested-end-to-end` measures control beside each nested
        # file, because every reading across its files is against control.
        self.assertEqual(
            {f.id: f.warm_groups for f in register.EVERY_FIGURE if f.warm_groups},
            {
                "scan-throughput-warm": (("control",), ("large_object",), ("insert_run",)),
                "nested-end-to-end": (("control", "composite"), ("control", "arrays")),
                "statistics-gathering": (
                    ("control",), ("arrays",), ("large_object",), ("insert_run",)
                ),
                # Control beside each writer's file, its floor read against
                # the pass in every sweep.
                "gzip-first-pass": tuple(
                    ("control", leg) for leg, _, _ in register.GZPASS_WRITERS
                ),
                # A kind's lengths at both counts together, its secant and
                # residuals subtracting them; then the bound beside the
                # secant's longer end, the tmpfs holding no more.
                "long-line-resident": tuple(
                    group
                    for kind in ("text", "escaped", "bytea", "int8array", "textarray", "jsonb")
                    for group in (
                        tuple(f"long_{kind}_{length}" for length in ("32", "128", "288", "512")),
                        (f"long_{kind}_512", f"long_{kind}_bound"),
                    )
                ),
                # The same lengths, and the bound alone: nothing subtracts
                # across the two.
                "purge-delay-resident": tuple(
                    group
                    for kind in ("text", "escaped", "bytea", "int8array", "textarray", "jsonb")
                    for group in (
                        tuple(f"long_{kind}_{length}" for length in ("32", "128", "288", "512")),
                        (f"long_{kind}_bound",),
                    )
                ),
            },
        )

    def test_every_declared_subtraction_lies_in_one_sweep(self):
        # The rule the split is held to: two inputs a reading subtracts are
        # measured together, whichever figure reads them.
        declared = [(fig.id, s) for fig in register.EVERY_FIGURE for s in fig.subtracts]
        self.assertTrue(declared)
        for reader, s in declared:
            with self.subTest(reader=reader, source=s.source, inputs=s.inputs):
                source = register.EVERY_BY_ID[s.source]
                self.assertNotEqual(*s.inputs)
                homes = [g for g in source.sweeps if set(s.inputs) <= set(g)]
                self.assertEqual(len(homes), 1, source.sweeps)
                requires = register.EVERY_BY_ID[reader].requires
                self.assertTrue(reader == s.source or s.source in requires)

    def test_the_nested_figure_and_its_consumer_declare_what_they_read(self):
        self.assertEqual(
            register.subtraction_sweep("nested-end-to-end", "control", "composite"), 0
        )
        self.assertEqual(register.subtraction_sweep("nested-end-to-end", "arrays", "control"), 1)
        self.assertEqual(register.subtraction_sweep("cross-file-floor", "control", "control43"), 0)

    def test_an_undeclared_cross_input_reading_is_refused(self):
        # composite and arrays are in no one sweep, and nothing declares them.
        for a, b in (("composite", "arrays"), ("control", "control43")):
            with self.subTest(pair=(a, b)):
                with self.assertRaises(ValueError):
                    register.subtraction_sweep("nested-end-to-end", a, b)
        with self.assertRaises(ValueError):
            nested._per_row_diffs(None, "nested-end-to-end", "composite", "arrays")

    def test_a_declared_pair_no_one_sweep_holds_is_refused(self):
        fig = register.EVERY_BY_ID["nested-end-to-end"]
        stray = runspec.Subtraction("nested-end-to-end", ("composite", "arrays"), "x")
        with unittest.mock.patch.object(fig, "subtracts", (*fig.subtracts, stray)):
            with self.assertRaises(ValueError) as caught:
                register.subtraction_sweep("nested-end-to-end", "composite", "arrays")
        self.assertIn("0 sweeps", str(caught.exception))

    def test_an_explicit_budget_only_caps_it_lower(self):
        self.assertEqual(self._stager(4).budget(), 4 * config.GIB)
        self.assertEqual(self._stager(100).budget(), staging.warm_bound(config.Config()))

    def test_a_warm_set_too_big_for_the_budget_is_refused_before_any_run(self):
        stager = self._stager(1)
        stager.plan(register.FIGURES)
        problems = stager.preflight(register.FIGURES)
        self.assertTrue(any("over the" in p for p in problems))


class Staging(unittest.TestCase):
    """A staging failure is the sweep's, not a figure's: what it must never
    leave behind is a partial file recorded as an input."""

    def _cfg(self, root: Path, **kw) -> config.Config:
        return config.Config(
            cache_dir=root / "ssd", warm_dir=root / "shm", nvme_dir=root / "nvme", **kw
        )

    def test_preflight_reserves_the_budget_and_gives_it_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stager = staging.Stager(self._cfg(root, warm_budget=1 / 1024), lambda _m: None)
            with unittest.mock.patch.object(
                os, "posix_fallocate", wraps=os.posix_fallocate
            ) as alloc:
                self.assertIsNone(stager.reserve(stager.budget()))
            self.assertEqual(alloc.call_args.args[1:], (0, config.MIB))
            self.assertEqual(list((root / "shm").iterdir()), [])

    def test_a_reservation_the_quota_refuses_is_a_preflight_problem(self):
        # `statvfs` cannot see a per-user quota, so the free space read fine
        # and the sweep hit EDQUOT staging its eighteenth figure.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stager = staging.Stager(self._cfg(root), lambda _m: None)
            stager.plan([register.FIGURES_BY_ID["peak-rss"]])
            quota = OSError(122, "Disk quota exceeded")
            with unittest.mock.patch.object(os, "posix_fallocate", side_effect=quota):
                problems = stager.preflight([register.FIGURES_BY_ID["peak-rss"]])
            self.assertTrue(any("could not reserve" in p for p in problems), problems)
            self.assertFalse((root / "shm" / staging.WARM_RESERVATION).exists())

    def test_what_is_already_staged_is_not_reserved_twice(self):
        with tempfile.TemporaryDirectory() as tmp:
            stager = staging.Stager(self._cfg(Path(tmp)), lambda _m: None)
            stager._staged = {"control": stager.budget()}
            with unittest.mock.patch.object(os, "posix_fallocate") as alloc:
                self.assertIsNone(stager.reserve(stager.budget()))
            alloc.assert_not_called()

    def test_a_failed_copy_leaves_nothing_staged(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = self._cfg(root)
            stager = staging.Stager(cfg, lambda _m: None)
            (root / "ssd").mkdir()
            (root / "ssd" / "control.sql").write_bytes(b"x" * 4096)
            (root / "ssd" / "control.stamp").write_text(
                inputs.input_stamp(inputs.INPUTS["control"], cfg) + "\n"
            )

            def partial(src, dst):
                Path(dst).write_bytes(b"x" * 100)
                raise OSError(122, "Disk quota exceeded")

            with unittest.mock.patch.object(shutil, "copyfile", side_effect=partial):
                with self.assertRaises(staging.StagingError):
                    stager.warm_path("control")
            self.assertNotIn("control", stager._staged)
            self.assertFalse((root / "shm" / "control.sql").exists())
            # And the next ask copies it whole rather than taking the stub.
            self.assertEqual(stager.warm_path("control").stat().st_size, 4096)
            self.assertIn("control", stager._staged)

    def test_a_staging_failure_aborts_the_sweep(self):
        source = inspect.getsource(emit.emit)
        handler = source.index("except staging.StagingError")
        self.assertLess(handler, source.index("except Exception as exc:  # one figure failing"))
        self.assertIn("break", source[handler : handler + 800])


class SplitSweep(unittest.TestCase):
    """A split figure's specs run beside the specs over their own group, each
    group staged first."""

    def _session(self):
        cfg = config.Config(dry_run=True)
        stager = staging.Stager(cfg, lambda _m: None)
        stager.plan(register.FIGURES)
        return timing.Session(cfg, stager, lambda _m: None)

    def test_specs_go_to_their_group_in_group_order(self):
        fig = register.FIGURES_BY_ID["nested-end-to-end"]
        parts = staging.split_specs(fig.id, nested._nested_specs(), fig.staging_groups)
        self.assertEqual([gi for gi, _ in parts], [0, 1])
        self.assertEqual({s.input for s in parts[0][1]}, {"control", "composite"})
        self.assertEqual({s.input for s in parts[1][1]}, {"control", "arrays"})
        # The second sweep's control is a reading of its own; the first's
        # keys as it always did, which is what the allocator table borrows.
        self.assertEqual({s.sweep for s in parts[0][1]}, {None})
        self.assertEqual(
            {(s.input, s.sweep) for s in parts[1][1]}, {("control", 1), ("arrays", None)}
        )
        keys = [s.key(fig.id) for s in nested._nested_specs()]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertIn("nested-end-to-end/pgdt/control/query-typed/warm/sweep=1", keys)
        self.assertIn("nested-end-to-end/pgdt/control/query-typed/warm", keys)

    def test_a_spec_names_only_a_later_group_staging_its_input(self):
        groups = register.FIGURES_BY_ID["nested-end-to-end"].staging_groups
        for sweep in (0, 2):
            spec = runspec.RunSpec("pgdt", "control", "query-typed", "warm", "x", sweep=sweep)
            with self.subTest(sweep=sweep):
                with self.assertRaises(ValueError):
                    staging.split_specs("nested-end-to-end", [spec], groups)
        arrays = runspec.RunSpec("pgdt", "arrays", "query-typed", "warm", "x", sweep=0)
        with self.assertRaises(ValueError):
            staging.split_specs("nested-end-to-end", [arrays], groups)
        with self.assertRaises(ValueError):
            staging.in_sweep("nested-end-to-end", arrays, 0)

    def test_an_unsplit_figure_refuses_a_spec_naming_a_sweep(self):
        session = self._session()
        session.take = lambda spec, rep: 1.0
        spec = runspec.RunSpec("pgdt", "control", "query-typed", "warm", "x", sweep=1)
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
                spec = runspec.RunSpec("pgdt", name, command, "warm", "", sweep=sweep)
                readings[spec.key(figure)] = [value] * 5
        for command, value in (("query-strings", 1.0), ("query-typed", 2.0)):
            spec = runspec.RunSpec("pgdt", "control", command, "warm", "")
            readings[spec.key("cross-file-floor")] = [value] * 5
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(dry_run=True, cache_dir=Path(tmp))
            session = replay.ReplaySession(
                cfg, {"readings": readings, "runs": []}, Path(tmp), lambda _m: None
            )
            table = nested.run_nested_end_to_end(session)
            rows = cfg.size_gib * config.GIB // 4000
            self.assertIn("| control — 16 scalar columns, in `--composite`'s sweep |", table)
            self.assertIn(
                "| control — 16 scalar columns, in `--arrays --composite`'s sweep |", table
            )
            headline = ((5.0 - 1.1) - (2.6 - 1.1)) / rows * 1e6
            self.assertIn(f"**{headline:+.2f} µs/row**", table)
            self.assertIn("`--arrays --composite` 1.000×", table)
            self.assertIn("`--composite` 1.000×", table)
            floor = nested.run_cross_file_floor(session)
            share = ((2.5 - 1.0) - (2.0 - 1.0)) / rows * 1e6
            self.assertIn(f"**{share:+.2f} µs**", floor)

    def test_a_spec_no_group_stages_is_an_error(self):
        groups = register.FIGURES_BY_ID["scan-throughput-warm"].staging_groups
        for spec in (
            runspec.RunSpec("pgdt", "control", "parse", "cold", "off tmpfs"),
            runspec.RunSpec("pgdt", "arrays", "parse", "warm", "no group"),
        ):
            with self.subTest(spec=spec.label):
                with self.assertRaises(ValueError):
                    staging.split_specs("scan-throughput-warm", [spec], groups)

    def test_each_group_is_staged_before_its_own_reps(self):
        session = self._session()
        order = []
        session.take = lambda spec, rep: order.append(("run", spec.input)) or 1.0
        stage = session.stage
        session.stage = lambda figure, gi: (order.append(("stage", gi)), stage(figure, gi))
        session.sweep("scan-throughput-warm", throughput._throughput_specs("warm"), 2)
        self.assertEqual(
            [o for o in order if o[0] == "stage"], [("stage", 0), ("stage", 1), ("stage", 2)]
        )
        runs = [(i, o[1]) for i, o in enumerate(order) if o[0] == "run"]
        stages = [i for i, o in enumerate(order) if o[0] == "stage"]
        for (i, name) in runs:
            group = max(g for g, at in enumerate(stages) if at < i)
            self.assertEqual(
                (name,), register.FIGURES_BY_ID["scan-throughput-warm"].staging_groups[group]
            )

    def test_each_warm_throughput_shape_has_its_own_floor(self):
        specs = throughput._throughput_specs("warm")
        parsed = {s.input for s in specs if s.command == "parse"}
        floors = {s.input for s in specs if s.command == "dd"}
        self.assertEqual(parsed, floors)


if __name__ == "__main__":
    unittest.main()
