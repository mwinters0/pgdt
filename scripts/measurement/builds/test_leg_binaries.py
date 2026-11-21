"""Tests of `measurement.builds.leg_binaries`."""

from __future__ import annotations

import collections
import dataclasses
import release
import tempfile
import tomllib
import unittest
import unittest.mock
from pathlib import Path

from measurement import borrowing
from measurement import config
from measurement import emit
from measurement import gating
from measurement import host
from measurement import inputs
from measurement import memory_model
from measurement import replay
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import test_support
from measurement import timing
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import allocator
from measurement.figures import declaration
from measurement.figures import long_line
from measurement.figures import nested
from measurement.figures import purge_delay
from measurement.figures import register
from measurement.figures import release_profile
from measurement.figures import throughput
from measurement.staleness import doc_markers


def write_leg(cfg: "config.Config", leg: str) -> None:
    built = test_support.leg_binary(cfg, leg)
    built.parent.mkdir(parents=True, exist_ok=True)
    built.write_text("#!/bin/true\n")


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
        leg_binaries._ALLOC_BUILT.clear()
        leg_binaries._ALLOC_ANNOUNCED.clear()
        test_support.without_the_release_image(self)

    def test_the_reference_leg_is_the_shipped_binary(self):
        # Not a fourth build of the same source: two builds of one source
        # differ by ~10% from code layout alone, which is larger than the
        # effect being measured.
        specs = allocator._allocator_specs("system")
        reference = [s for s in specs if s.command == "parse" and s.binary == "pgdt"]
        self.assertEqual(len(reference), 1)

    def test_every_other_leg_is_its_own_build(self):
        specs = allocator._allocator_specs("system")
        binaries = {s.binary for s in specs if s.command == "parse"}
        self.assertEqual(binaries, {"pgdt", "alloc:jemalloc", "alloc:mimalloc"})

    def test_the_reference_is_read_off_the_binary_not_assumed(self):
        # Adopt a leg and it becomes the reference with no code change; assume
        # `system` instead and the figure would compare a leg against itself.
        for reference in leg_binaries.ALLOCATOR_LEGS:
            with self.subTest(reference=reference):
                columns = allocator.allocator_columns(reference)
                self.assertEqual(columns[0], reference)
                self.assertEqual(sorted(columns), sorted(leg_binaries.ALLOCATOR_LEGS))
                specs = allocator._allocator_specs(reference)
                shipped = {s.binary for s in specs if s.command == "parse"}
                self.assertIn("pgdt", shipped)
                self.assertNotIn(f"alloc:{reference}", shipped)

    def test_an_unknown_reference_is_an_error(self):
        with self.assertRaises(ValueError):
            allocator._allocator_specs("tcmalloc")

    def test_every_shape_is_run_on_every_leg(self):
        specs = allocator._allocator_specs("system")
        timed = collections.Counter(
            (s.command, s.binary) for s in specs if s.command != "dd"
        )
        self.assertEqual(len(timed), len(register._ALLOCATOR_SHAPES) * 3)
        self.assertEqual(set(timed.values()), {1})

    def test_the_floor_is_one_row_not_one_per_leg(self):
        # `dd` links no allocator, so three floor readings would be three
        # readings of one thing.
        specs = allocator._allocator_specs("system")
        floors = [s for s in specs if s.command == "dd"]
        self.assertEqual(len(floors), 1)
        self.assertEqual(floors[0].binary, "none")
        self.assertIs(floors[0], specs[-1])

    def test_every_shape_is_warm_on_the_control(self):
        for spec in allocator._allocator_specs("system"):
            with self.subTest(spec=spec.label):
                self.assertEqual(spec.regime, "warm")
                self.assertEqual(spec.input, "control")

    def test_the_shapes_are_the_three_the_baseline_quotes(self):
        commands = [c for c, _, _ in register._ALLOCATOR_SHAPES]
        self.assertEqual(commands, ["parse", "query-strings", "query-typed"])
        for command in commands:
            with self.subTest(command=command):
                # A shape the sweep does not time is a shape no figure can be
                # read against.
                self.assertIn("time /pgdt", shapes._script(command))

    def test_each_shape_names_a_figure_that_takes_its_reference_reading(self):
        # The borrow is what keeps one number in the doc per measurement, and
        # it silently does nothing if the source figure never takes that spec.
        order = [f.id for f in register.FIGURES]
        for command, _, source in register._ALLOCATOR_SHAPES:
            with self.subTest(command=command):
                self.assertIn(source, register.FIGURES_BY_ID)
                self.assertLess(order.index(source), order.index("allocator"))
        throughput_ = throughput._throughput_specs("warm")
        self.assertIn(
            runspec.RunSpec("pgdt", "control", "parse", "warm", "").key("scan-throughput-warm"),
            {s.key("scan-throughput-warm") for s in throughput_},
        )
        nested_ = nested._nested_specs()
        for command in ("query-strings", "query-typed"):
            with self.subTest(command=command):
                self.assertIn(
                    runspec.RunSpec("pgdt", "control", command, "warm", "").key(
                        "nested-end-to-end"
                    ),
                    {s.key("nested-end-to-end") for s in nested_},
                )

    def test_a_version_string_yields_its_allocator(self):
        with unittest.mock.patch.object(
            host, "run", return_value="pgdt 0.1.0 (allocator: mimalloc)\n"
        ):
            self.assertEqual(leg_binaries.binary_allocator(Path("/pgdt")), "mimalloc")

    def test_a_binary_that_names_no_allocator_is_an_error(self):
        # Not a default: a binary too old to report it would otherwise be
        # published as the reference leg under a name nothing checked.
        with unittest.mock.patch.object(host, "run", return_value="pgdt 0.1.0\n"):
            with self.assertRaises(RuntimeError):
                leg_binaries.binary_allocator(Path("/pgdt"))

    def test_an_instrumented_build_is_refused_rather_than_timed(self):
        # The failure this prevents is silent: a counting `#[global_allocator]`
        # still answers `(allocator: system)`, so without the refusal an
        # introspection build reads as the shipped binary and every reading
        # taken on it is published as a figure of something else.
        with unittest.mock.patch.object(
            host,
            "run",
            return_value="pgdt 0.1.0 (allocator: system) (instrument: counting-allocator)\n",
        ):
            with self.assertRaises(RuntimeError) as raised:
                leg_binaries.binary_allocator(Path("/pgdt"))
        self.assertIn("counting-allocator", str(raised.exception))

    def test_an_unknown_leg_is_never_built(self):
        with self.assertRaises(ValueError):
            leg_binaries.ensure_allocator_binary(config.Config(), "tcmalloc", lambda _: None)

    def test_a_leg_builds_with_no_default_features_and_its_own_target_dir(self):
        # Both flags are load-bearing. `pgdt` refuses two allocator features at
        # compile time, so without `--no-default-features` every leg but the
        # default's fails to build; without its own target dir a `--features`
        # build overwrites the shipped binary and every other figure in the
        # same sweep is timed under the wrong allocator. And it is built in the
        # release image, as the shipped binary is, so the two differ by the
        # allocator alone.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )
            calls = []

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                calls.append(list(argv))
                write_leg(cfg, "jemalloc")
                return ""

            with unittest.mock.patch.object(host, "run", fake_run), \
                 unittest.mock.patch.object(
                     leg_binaries, "binary_allocator", return_value="jemalloc"
                 ):
                out = leg_binaries.ensure_allocator_binary(cfg, "jemalloc", lambda _: None)
            self.assertEqual(len(calls), 1)
            argv = calls[0]
            self.assertEqual(argv[-1], "--inside")
            self.assertIn("pgdt-release:t", argv)
            self.assertIn("--no-default-features", argv)
            self.assertIn("--features=jemalloc", argv)
            self.assertIn(f"--target={release.NATIVE_TARGET}", argv)
            target = image.image_target_dir(cfg, "jemalloc").resolve()
            self.assertIn(f"{target}:/state/target", argv)
            self.assertEqual(out, cfg.out_dir / "pgdt-alloc-jemalloc")

    def test_every_leg_names_its_own_feature(self):
        # The platform allocator is a feature like the other two, not the
        # absence of one: `pgdt` refuses a build naming no allocator, so a
        # `system` leg built without `--features system` would not build.
        for leg in leg_binaries.ALLOCATOR_LEGS:
            with self.subTest(leg=leg), tempfile.TemporaryDirectory() as tmp:
                leg_binaries._ALLOC_BUILT.clear()
                cfg = config.Config(
                    out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
                )
                calls = []

                def fake_run(argv, cwd=None, capture=False, quiet=False, leg=leg, cfg=cfg):
                    calls.append(list(argv))
                    write_leg(cfg, leg)
                    return ""

                with unittest.mock.patch.object(host, "run", fake_run), \
                     unittest.mock.patch.object(leg_binaries, "binary_allocator", return_value=leg):
                    leg_binaries.ensure_allocator_binary(cfg, leg, lambda _: None)
                argv = calls[0]
                self.assertIn("--no-default-features", argv)
                self.assertIn(f"--features={leg}", argv)

    def test_the_first_leg_is_the_default_build_s(self):
        # A dry run asks no binary, so it names `ALLOCATOR_LEGS[0]` as the
        # shipped one; and `rss-attribution` runs the shipped binary beside the
        # other two. Both are wrong the day `pgdt`'s default moves and this
        # tuple does not.
        manifest = tomllib.loads((config.REPO / "crates" / "pgdt" / "Cargo.toml").read_text())
        default = manifest["features"]["default"]
        self.assertEqual(
            [f for f in default if f in leg_binaries.ALLOCATOR_LEGS], [leg_binaries.ALLOCATOR_LEGS[0]]
        )
        for leg in leg_binaries.ALLOCATOR_LEGS:
            with self.subTest(leg=leg):
                self.assertIn(leg, manifest["features"])
        self.assertEqual(
            allocator._allocator_reference(config.Config(dry_run=True)),
            leg_binaries.ALLOCATOR_LEGS[0],
        )

    def test_a_leg_whose_build_dropped_its_feature_is_refused(self):
        # The build succeeds and produces a working binary, so nothing else
        # would notice: the table would compare two identical binaries.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                write_leg(cfg, "mimalloc")
                return ""

            with unittest.mock.patch.object(host, "run", fake_run), \
                 unittest.mock.patch.object(
                     leg_binaries, "binary_allocator", return_value="system"
                 ):
                with self.assertRaises(RuntimeError):
                    leg_binaries.ensure_allocator_binary(cfg, "mimalloc", lambda _: None)
            self.assertFalse((cfg.out_dir / "pgdt-alloc-mimalloc").exists())

    def test_a_leg_left_over_from_an_earlier_session_is_rebuilt(self):
        # The failure this stops is silent and total: `runs/pgdt-alloc-<leg>`
        # survives between sessions, so short-circuiting on its existence times
        # a leg built from last week's source against a reference built from
        # today's, and the leg still answers `--version` with its own allocator
        # name, so nothing downstream notices.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(
                out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds"
            )
            stale = cfg.out_dir / "pgdt-alloc-jemalloc"
            stale.parent.mkdir(parents=True, exist_ok=True)
            stale.write_text("last session's binary\n")
            calls = []

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                calls.append(list(argv))
                write_leg(cfg, "jemalloc")
                return ""

            with unittest.mock.patch.object(host, "run", fake_run), \
                 unittest.mock.patch.object(
                     leg_binaries, "binary_allocator", return_value="jemalloc"
                 ):
                leg_binaries.ensure_allocator_binary(cfg, "jemalloc", lambda _: None)
                self.assertEqual(len(calls), 1)
                self.assertEqual(stale.read_text(), "#!/bin/true\n")
                # Twice in one process is one build: `cargo` is incremental,
                # but a build between two timed reps moves the second one.
                leg_binaries.ensure_allocator_binary(cfg, "jemalloc", lambda _: None)
                self.assertEqual(len(calls), 1)

    def test_an_allocator_leg_resolves_to_a_binary(self):
        session = timing.Session(config.Config(dry_run=True), None, lambda _: None)
        self.assertEqual(
            session.binary_path("alloc:jemalloc"),
            config.Config().out_dir / "pgdt-alloc-jemalloc",
        )

    def test_an_unknown_binary_is_still_an_error(self):
        session = timing.Session(config.Config(dry_run=True), None, lambda _: None)
        with self.assertRaises(ValueError):
            session.binary_path("alloc")

    def test_the_stamp_names_the_allocator_it_was_given(self):
        stamp = emit.session_stamp("deadbee", dirty=False, allocator="mimalloc")
        self.assertIn("mimalloc", stamp)
        # And still reads back as a stamp: the commit is what `--stale`
        # resolves, and the allocator sits after it.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(stamp + "\n")
            self.assertEqual(doc_markers.stamped_commit(doc), "deadbee")

    def test_a_stamp_with_no_binary_to_ask_names_no_allocator(self):
        self.assertNotIn("allocator", emit.session_stamp("deadbee", dirty=False))


def write_profile_leg(cfg: "config.Config", leg: str) -> None:
    built = test_support.profile_leg_binary(cfg, leg)
    built.parent.mkdir(parents=True, exist_ok=True)
    built.write_text("#!/bin/true\n")


class ReleaseProfile(unittest.TestCase):
    """The `release-profile` figure: three profiles, `allocator`'s three
    shapes and the provider's typed scan, one table, every leg built from the
    tree measured.

    Each assertion is a way to publish a plausible table of the wrong
    comparison: a leg built under the wrong profile or the wrong allocator, a
    leg borrowed from another build or another sitting, or a default leg that
    moves with `release` and so stops being the default."""

    def setUp(self):
        leg_binaries._PROFILE_BUILT.clear()
        leg_binaries._PROFILE_ANNOUNCED.clear()
        test_support.without_the_release_image(self)

    def _cfg(self, tmp: str) -> "config.Config":
        return config.Config(out_dir=Path(tmp) / "runs", alloc_build_root=Path(tmp) / "builds")

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish outside a sweep,
        # and twice in one phase.
        fig = register.SELECTABLE_BY_ID["release-profile"]
        self.assertIn(fig, register.FIGURES)
        self.assertEqual(fig.shares, ())
        self.assertEqual(borrowing.entangled_with("release-profile"), [])

    def test_the_shapes_are_the_allocator_figure_s_then_the_typed_provider_scan(self):
        specs = release_profile._release_profile_specs()
        timed = collections.Counter((s.command, s.binary) for s in specs if s.command != "dd")
        commands = [*(c for c, _, _ in register._ALLOCATOR_SHAPES), shapes.PARALLEL_SCAN]
        self.assertEqual([c for c, _ in release_profile.RELEASE_PROFILE_SHAPES], commands)
        self.assertEqual(
            set(timed),
            {(command, f"profile:{leg}") for command in commands for leg, _ in leg_binaries.PROFILE_LEGS},
        )
        self.assertEqual(set(timed.values()), {1})
        for spec in specs:
            with self.subTest(spec=spec.label):
                self.assertEqual((spec.input, spec.regime), ("control", "warm"))

    def test_the_provider_row_is_the_scaling_query_at_the_apparatus_count(self):
        # `parallel-scan-throughput`'s one-partition row word for word, but
        # for the allowance it states: the figure's one container is the
        # register's, which `PARALLEL_BUDGET` would overrun.
        script = shapes._script(shapes.PARALLEL_SCAN)
        axis = shapes._script(f"{shapes.PARALLEL_SCAN}-jobs-{shapes.SWEEP_JOBS}")
        allowance = f" -c 'SET pgdump.memory = {memory_model.stated_allowance(shapes.PARALLEL_BUDGET)}'"
        self.assertIn(allowance, axis)
        self.assertEqual(axis.replace(allowance, ""), script)
        self.assertNotIn("pgdump.memory", script)
        self.assertEqual(shapes._dfcli_partitions(script), {str(shapes.SWEEP_JOBS)})
        self.assertIn(shapes.PARALLEL_SCAN_SQL, script)
        builder, _, timed = script.partition(" && ")
        self.assertEqual(f"{builder} && ", shapes.PROVIDER_BUILDER)
        self.assertTrue(timed.startswith("time "))
        self.assertIsNone(register.FIGURES_BY_ID["release-profile"].memory)

    def _render(self, digests):
        figure = "release-profile"
        specs = release_profile._release_profile_specs()
        reported = {
            s.key(figure): {"result_rows": "1", "result_first": "100", "result_digest": d}
            for s, d in zip((s for s in specs if s.command == shapes.PARALLEL_SCAN), digests)
        }
        raw = {
            "readings": {s.key(figure): [1.0] * 5 for s in specs},
            "reported": reported,
            "input_sizes": {"control": 3221227790},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(dry_run=True), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = figure
            return release_profile.run_release_profile(session)

    def test_the_provider_row_renders_when_every_leg_answers_alike(self):
        body = self._render(["ab", "ab", "ab"])
        self.assertIn(f"| {release_profile.RELEASE_PROFILE_SHAPES[-1][1]} |", body)
        self.assertIn(f"{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS}", body)

    def test_legs_answering_differently_are_refused(self):
        # One source in three builds: a different answer is a scan that lost
        # or repeated rows under the timer.
        for digests in (["ab", "ab", "cd"], ["ab", "ab"]):
            with self.subTest(digests=digests), self.assertRaises(RuntimeError):
                self._render(digests)

    def test_no_leg_is_the_shipped_binary(self):
        # Every column built alike from the tree measured: the shipped binary
        # is another build of `release`, which a later decision may move.
        binaries = {s.binary for s in release_profile._release_profile_specs()}
        self.assertNotIn("pgdt", binaries)
        self.assertEqual([s.binary for s in release_profile._release_profile_specs()].count("none"), 1)

    def test_each_leg_is_a_profile_inheriting_release_that_pins_both_knobs(self):
        # A leg pinning only one knob would move with `release` on the other,
        # and the default leg would stop being Cargo's default the day
        # `release` states a profile of its own.
        manifest = tomllib.loads((config.REPO / "Cargo.toml").read_text())
        profiles = manifest["profile"]
        want = {
            "release-default": (False, 16),
            "release-thin": ("thin", 16),
            "release-fat": ("fat", 1),
        }
        self.assertEqual([leg for leg, _ in leg_binaries.PROFILE_LEGS], list(want))
        for leg, (lto, units) in want.items():
            with self.subTest(leg=leg):
                self.assertEqual(profiles[leg]["inherits"], "release")
                self.assertEqual(profiles[leg]["lto"], lto)
                self.assertEqual(profiles[leg]["codegen-units"], units)

    def test_the_shipped_profile_is_the_fat_leg_restated(self):
        # D105: `release` states exactly the knobs `release-fat` pins, so the
        # default leg's gap to that leg is the gap to what ships.
        profiles = tomllib.loads((config.REPO / "Cargo.toml").read_text())["profile"]
        knobs = ("lto", "codegen-units")
        self.assertEqual(leg_binaries.SHIPPED_PROFILE_LEG, "release-fat")
        self.assertEqual(
            {k: profiles["release"].get(k) for k in knobs},
            {k: profiles[leg_binaries.SHIPPED_PROFILE_LEG][k] for k in knobs},
        )

    def test_its_declared_paths_include_where_the_legs_live(self):
        fig = register.SELECTABLE_BY_ID["release-profile"]
        for path in (
            "Cargo.toml",
            "crates/pgdt/build.rs",
            "crates/pgdt/Cargo.toml",
            "crates/pgdt-query/src/prune.rs",
            *declaration.DATAFUSION,
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)
        for path in register.FIGURES_BY_ID["allocator"].depends:
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_a_version_string_yields_its_profile(self):
        out = "pgdt 0.1.0 (unreleased) (allocator: mimalloc) (profile: release-thin) (datafusion: 55.1.0)\n"
        with unittest.mock.patch.object(host, "run", return_value=out):
            self.assertEqual(leg_binaries.binary_profile(Path("/pgdt")), "release-thin")

    def test_a_binary_naming_no_profile_or_an_unknown_one_is_refused(self):
        for out in ("pgdt 0.1.0 (allocator: mimalloc)\n", "pgdt 0.1.0 (profile: unknown)\n"):
            with self.subTest(out=out), unittest.mock.patch.object(host, "run", return_value=out):
                with self.assertRaises(RuntimeError):
                    leg_binaries.binary_profile(Path("/pgdt"))

    def test_an_unknown_leg_is_never_built(self):
        with self.assertRaises(ValueError):
            leg_binaries.ensure_profile_binary(config.Config(), "release", lambda _: None)

    def test_a_leg_builds_its_profile_at_the_default_features_in_its_own_target_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp)
            calls = []

            def fake_run(argv, cwd=None, capture=False, quiet=False):
                calls.append(list(argv))
                write_profile_leg(cfg, "release-fat")
                return ""

            with unittest.mock.patch.object(host, "run", fake_run), \
                 unittest.mock.patch.object(leg_binaries, "binary_profile", return_value="release-fat"), \
                 unittest.mock.patch.object(leg_binaries, "binary_allocator", return_value="mimalloc"):
                out = leg_binaries.ensure_profile_binary(cfg, "release-fat", lambda _: None)
                # Twice in one process is one build.
                leg_binaries.ensure_profile_binary(cfg, "release-fat", lambda _: None)
            self.assertEqual(len(calls), 1)
            argv = calls[0]
            self.assertIn("--profile=release-fat", argv)
            self.assertNotIn("--no-default-features", argv)
            self.assertFalse(any(a.startswith("--features") for a in argv))
            target = image.image_target_dir(cfg, "release-fat").resolve()
            self.assertIn(f"{target}:/state/target", argv)
            self.assertEqual(out, cfg.out_dir / "pgdt-release-fat")

    def test_a_leg_reporting_another_profile_or_allocator_is_refused(self):
        for profile, allocator in (("release", "mimalloc"), ("release-thin", "system")):
            with self.subTest(profile=profile, allocator=allocator), \
                 tempfile.TemporaryDirectory() as tmp:
                leg_binaries._PROFILE_BUILT.clear()
                cfg = self._cfg(tmp)

                def fake_run(argv, cwd=None, capture=False, quiet=False, cfg=cfg):
                    write_profile_leg(cfg, "release-thin")
                    return ""

                with unittest.mock.patch.object(host, "run", fake_run), \
                     unittest.mock.patch.object(leg_binaries, "binary_profile", return_value=profile), \
                     unittest.mock.patch.object(leg_binaries, "binary_allocator", return_value=allocator):
                    with self.assertRaises(RuntimeError):
                        leg_binaries.ensure_profile_binary(cfg, "release-thin", lambda _: None)
                self.assertFalse((cfg.out_dir / "pgdt-release-thin").exists())
                self.assertNotIn("release-thin", leg_binaries._PROFILE_BUILT)

    def test_a_profile_leg_resolves_to_a_binary(self):
        session = timing.Session(config.Config(dry_run=True), None, lambda _: None)
        self.assertEqual(
            session.binary_path("profile:release-thin"),
            config.Config().out_dir / "pgdt-release-thin",
        )


class PurgeDelay(unittest.TestCase):
    """`purge-delay` and `purge-delay-resident`: the shipped binary under
    each leg's `MIMALLOC_PURGE_DELAY`, timed on the throughput shapes and
    read for peak resident over `long-line-resident`'s legs.

    Each assertion is a way to publish a plausible table of the wrong
    comparison: a leg whose variable never reached the container or
    mimalloc, a shipped leg stating a value, two legs sharing a key, or a
    resident leg read in another arrangement than `long-line-resident`'s."""

    FIGURES = ("purge-delay", "purge-delay-resident")

    def test_both_are_taken_and_borrow_nothing(self):
        # Taken from a commit and published outside a sweep, which standing
        # in no sharing edge allows.
        for figure in self.FIGURES:
            with self.subTest(figure=figure):
                fig = register.SELECTABLE_BY_ID[figure]
                self.assertIn(fig, register.FIGURES)
                self.assertNotIn(fig, register.UNTAKEN)
                self.assertEqual(fig.shares, ())
                self.assertEqual(borrowing.entangled_with(figure), [])
                self.assertIn("Cargo.lock", fig.depends)
                self.assertIn("crates/pgdt/Cargo.toml", fig.depends)

    def test_the_shipped_leg_states_nothing_and_each_other_its_delay(self):
        self.assertEqual(leg_binaries.purge_delay_legs(), (None, *leg_binaries.PURGE_DELAY_LEGS))
        self.assertIsNone(leg_binaries.purge_delay_env(None))
        for delay in leg_binaries.PURGE_DELAY_LEGS:
            with self.subTest(delay=delay):
                self.assertEqual(
                    leg_binaries.purge_delay_env(delay), f"{leg_binaries.PURGE_DELAY_VAR}={delay}"
                )
        # At or below the shipped default, distinct, zero among them.
        self.assertIn(0, leg_binaries.PURGE_DELAY_LEGS)
        self.assertEqual(len(set(leg_binaries.PURGE_DELAY_LEGS)), len(leg_binaries.PURGE_DELAY_LEGS))
        self.assertTrue(all(d <= leg_binaries.DRY_RUN_PURGE_DELAY for d in leg_binaries.PURGE_DELAY_LEGS))

    def test_a_spec_s_environment_keys_it_only_where_stated(self):
        # An unstated environment keys as before, so every earlier sitting
        # still renders; a stated one is a reading of its own.
        plain = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        self.assertEqual(plain.key("f"), "f/pgdt/control/parse/warm")
        stated = dataclasses.replace(plain, env="MIMALLOC_PURGE_DELAY=10")
        self.assertEqual(stated.key("f"), "f/pgdt/control/parse/warm/env=MIMALLOC_PURGE_DELAY=10")

    def test_every_shape_is_read_at_every_leg_under_one_key_each(self):
        specs = purge_delay._purge_delay_specs()
        timed = [s for s in specs if s.binary != "none"]
        self.assertEqual(
            len(timed), len(purge_delay.PURGE_DELAY_SHAPES) * len(leg_binaries.purge_delay_legs())
        )
        self.assertEqual(len({s.key("purge-delay") for s in specs}), len(specs))
        self.assertEqual([s.command for s in specs].count("dd"), 1)
        self.assertEqual(
            [(c, l) for c, l, _, _ in purge_delay.PURGE_DELAY_SHAPES][: len(release_profile.RELEASE_PROFILE_SHAPES)],
            list(release_profile.RELEASE_PROFILE_SHAPES),
        )
        for spec in timed:
            with self.subTest(spec=spec.label):
                self.assertIn(spec.binary, gating.SHIPPED_BINARIES)
                self.assertEqual(spec.input, "control")
                if "-jobs-" in spec.command:
                    self.assertEqual(
                        (spec.regime, spec.memory), ("warm-parallel", shapes.PARALLEL_MEMORY)
                    )
                else:
                    self.assertEqual((spec.regime, spec.memory), ("warm", None))
        self.assertEqual(
            {s.command.rpartition("-jobs-")[2] for s in timed if "-jobs-" in s.command},
            {"4", "24"},
        )

    def _logged_run(self, spec):
        cfg = config.Config(dry_run=True)
        logged: list[str] = []
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), logged.append)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        session.figure_id = "purge-delay"
        session.time_run(spec)
        (run,) = [line for line in logged if " run --rm " in line]
        return run

    def test_a_leg_s_container_is_given_its_variable_outside_the_shape(self):
        shipped, ms0 = purge_delay._purge_delay_specs()[:2]
        self.assertIsNone(shipped.env)
        self.assertEqual(ms0.env, f"{leg_binaries.PURGE_DELAY_VAR}=0")
        run = self._logged_run(ms0)
        image, _, script = run.partition(config.Config(dry_run=True).image)
        self.assertIn(f"-e {leg_binaries.PURGE_DELAY_VAR}=0", image)
        self.assertNotIn(leg_binaries.PURGE_DELAY_VAR, script)
        self.assertNotIn(leg_binaries.PURGE_DELAY_VAR, self._logged_run(shipped))

    def test_the_binary_s_answer_is_read_and_a_leg_it_did_not_reach_refused(self):
        def answering(delay: str):
            out = (
                "pgdt 0.3.0 (unreleased) (allocator: mimalloc) "
                f"(purge delay: {delay} ms) (profile: release) (datafusion: 55.1.0)\n"
            )
            return unittest.mock.patch.object(host, "run", return_value=out)

        cfg = config.Config()
        with answering("100") as run:
            self.assertEqual(leg_binaries.binary_purge_delay(cfg, Path("/pgdt"), None), 100)
        argv = run.call_args.args[0]
        self.assertIn("--version", argv)
        self.assertFalse(any(a.startswith(leg_binaries.PURGE_DELAY_VAR) for a in argv))
        self.assertIn("-m", argv)
        with answering("10") as run:
            self.assertEqual(leg_binaries.binary_purge_delay(cfg, Path("/pgdt"), 10), 10)
        self.assertIn(f"{leg_binaries.PURGE_DELAY_VAR}=10", run.call_args.args[0])
        with answering("100"), self.assertRaises(RuntimeError):
            leg_binaries.binary_purge_delay(cfg, Path("/pgdt"), 10)
        with unittest.mock.patch.object(host, "run", return_value="pgdt 0.3.0\n"):
            with self.assertRaises(RuntimeError):
                leg_binaries.binary_purge_delay(cfg, Path("/pgdt"), None)

    def test_a_cost_is_named_resolved_only_where_the_readings_part(self):
        self.assertIn("slower, resolved", purge_delay.purge_delay_cost([1.2, 1.3], [1.0, 1.1]))
        self.assertIn("faster, resolved", purge_delay.purge_delay_cost([1.0, 1.1], [1.2, 1.3]))
        self.assertIn("within the noise", purge_delay.purge_delay_cost([1.0, 1.2], [1.1, 1.3]))

    def _render(self, asked: bool):
        figure = "purge-delay"
        specs = purge_delay._purge_delay_specs()
        reported = {
            s.key(figure): {"result_rows": "1", "result_first": "100", "result_digest": "ab"}
            for s in specs
            if s.binary == "dfcli"
        }
        if asked:
            for delay in leg_binaries.purge_delay_legs():
                # A sitting before `pgdt` set its default: what a render
                # names is what it asked, never today's answer.
                got = 1000 if delay is None else delay
                reported[f"{figure}/purge-delay/{leg_binaries.purge_delay_caption(delay)}"] = {
                    "purge_delay": str(got)
                }
        raw = {
            "readings": {
                s.key(figure): [2.0 if s.env and s.command == "parse" else 1.0] * 5 for s in specs
            },
            "reported": reported,
            "input_sizes": {"control": 3221227790},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(dry_run=True), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = figure
            return purge_delay.run_purge_delay(session)

    def test_a_render_names_what_the_sitting_asked_and_asks_nothing(self):
        body = self._render(asked=True)
        self.assertIn("the shipped leg 1000 ms", body)
        for _, label, _, _ in purge_delay.PURGE_DELAY_SHAPES:
            with self.subTest(row=label):
                self.assertIn(f"| {label} |", body)
        # Every stating leg's `parse` doubled: resolved there alone.
        self.assertEqual(body.count("**slower, resolved**"), len(leg_binaries.PURGE_DELAY_LEGS))
        self.assertIn("asked none", self._render(asked=False))

    def test_the_resident_legs_are_long_line_resident_s_but_its_anchors_at_every_leg(self):
        allowance = 1 << 34
        base = [s for s in long_line._long_line_specs(allowance) if s.sweep is None]
        self.assertLess(len(base), len(long_line._long_line_specs(allowance)))
        specs = purge_delay._purge_delay_resident_specs(allowance)
        self.assertEqual(len(specs), len(base) * len(leg_binaries.purge_delay_legs()))
        self.assertEqual(
            {dataclasses.replace(s, env=None, label="") for s in specs},
            {dataclasses.replace(s, label="") for s in base},
        )
        self.assertEqual(len({s.key("purge-delay-resident") for s in specs}), len(specs))
        for spec in specs:
            with self.subTest(spec=spec.label):
                self.assertEqual(spec.allowance, allowance)
                self.assertIsNone(spec.sweep)
        for spec in long_line._long_factor_specs():
            self.assertIsNone(spec.env)

    def test_the_resident_groups_stage_every_input_read_once(self):
        fig = register.SELECTABLE_BY_ID["purge-delay-resident"]
        self.assertEqual(fig.memory, shapes.LONG_LINE_MEMORY)
        staged = [name for group in fig.warm_groups for name in group]
        self.assertEqual(sorted(staged), sorted(inputs.LONG_LINE_INPUTS))
        specs = [*long_line._long_factor_specs(), *purge_delay._purge_delay_resident_specs(1 << 34)]
        parts = staging.split_specs("purge-delay-resident", specs, fig.warm_groups)
        self.assertEqual(sum(len(part) for _, part in parts), len(specs))

    def test_a_leg_past_its_allowance_is_named(self):
        kib = [1024.0, 2048.0]
        within = purge_delay.purge_delay_allowance_line(10, kib, 4 << 20)
        self.assertIn("every rep within it", within)
        past = purge_delay.purge_delay_allowance_line(10, kib, 1536 << 10)
        self.assertIn("**1 reps past it**", past)
        self.assertIn("no rep was read", purge_delay.purge_delay_allowance_line(None, [], 1))


if __name__ == "__main__":
    unittest.main()
