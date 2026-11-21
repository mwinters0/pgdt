"""Tests of `measurement.machine_state`."""

from __future__ import annotations

import collections
import dataclasses
import release
import shutil
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import consumers
from measurement import drift
from measurement import inputs
from measurement import machine_state
from measurement import parsing
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.builds import leg_binaries
from measurement.figures import register
from measurement.figures import throughput


class ColdNvme(unittest.TestCase):
    """The third device class, and the only instrument that can price a
    readahead, `fadvise` or chunk-size default.

    What has to hold is that it really is a third *device*: an input read out
    of the SSD cache, or out of page cache, would produce a plausible table of
    the wrong thing — which is the failure mode every check here is aimed at.
    """

    def test_the_figure_reads_from_the_nvme_and_not_the_ssd(self):
        fig = register.FIGURES_BY_ID["scan-throughput-nvme"]
        self.assertEqual(fig.nvme_inputs, ("control", "large_object", "insert_run"))
        # A `cold_inputs` entry would be read in place off the SSD cache, which
        # is the device the cold table above already measures.
        self.assertEqual(fig.cold_inputs, ())
        self.assertEqual(fig.warm_inputs, ())

    def test_it_measures_the_same_three_shapes_and_their_floors(self):
        # The three tables are read against each other as ratios, so they have
        # to be the same rows over the same files.
        cold = register.FIGURES_BY_ID["scan-throughput-cold"]
        self.assertEqual(
            register.FIGURES_BY_ID["scan-throughput-nvme"].nvme_inputs, cold.cold_inputs
        )
        specs = throughput._throughput_specs("cold-nvme")
        self.assertEqual({s.regime for s in specs}, {"cold-nvme"})
        self.assertEqual(
            [(s.input, s.command) for s in specs],
            [(s.input, s.command) for s in throughput._throughput_specs("warm")],
        )

    def test_the_nvme_regime_resolves_under_the_nvme_directory(self):
        cfg = config.Config(dry_run=True, warm_budget=8)
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
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
            cfg = config.Config(cache_dir=root / "ssd", nvme_dir=root / "nvme")
            (root / "ssd").mkdir()
            (root / "ssd" / "control.sql").write_bytes(b"new bytes")
            (root / "ssd" / "control.stamp").write_text(
                inputs.input_stamp(inputs.INPUTS["control"], cfg) + "\n"
            )
            (root / "nvme").mkdir()
            (root / "nvme" / "control.sql").write_bytes(b"old bytes")
            (root / "nvme" / "control.stamp").write_text("a stamp from another generator\n")
            stager = staging.Stager(cfg, lambda _m: None)
            got = stager.nvme_path("control")
            self.assertEqual(got.read_bytes(), b"new bytes")

    def test_a_matching_copy_is_left_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = config.Config(cache_dir=root / "ssd", nvme_dir=root / "nvme")
            stamp = inputs.input_stamp(inputs.INPUTS["control"], cfg)
            for name, body in (("ssd", b"new bytes"), ("nvme", b"already here")):
                (root / name).mkdir()
                (root / name / "control.sql").write_bytes(body)
                (root / name / "control.stamp").write_text(stamp + "\n")
            stager = staging.Stager(cfg, lambda _m: None)
            self.assertEqual(stager.nvme_path("control").read_bytes(), b"already here")

    def test_every_regime_is_gated_for_contention(self):
        # A regime with no limits gates nothing, so a new one nobody added a
        # row for would take every reading it was handed, however busy the
        # machine was. The set is derived rather than written out: the tuple
        # this used to iterate was missing `warm-parallel` from the day that
        # regime landed, so the fourth regime went unchecked by the test whose
        # whole subject it is.
        for regime in consumers.registered_regimes():
            with self.subTest(regime=regime):
                self.assertTrue(machine_state.CONTENTION_LIMITS.get(regime))
        self.assertIn("warm-parallel", consumers.registered_regimes())
        self.assertIsNotNone(
            machine_state.contention_verdict({"cpu_busy_pct": 99.0}, "cold-nvme")
        )
        self.assertIsNone(machine_state.contention_verdict({"cpu_busy_pct": 1.0}, "cold-nvme"))

    def test_the_regime_vocabulary_reconciles_both_ways(self):
        """`REGIMES`, `CONTENTION_LIMITS` and the figures' own `stage`
        declarations name the same four regimes.

        Each of the three is the one a half-landed regime would be missing
        from, and each fails silently on its own: a `stage` token nothing
        declares takes a reading off the wrong device, a `REGIMES` row with no
        gate takes one off a busy machine, and a gate for a regime no figure
        runs is a row nobody will ever notice is wrong."""
        declared = set(machine_state.REGIMES)
        self.assertEqual(set(consumers.registered_regimes()), declared)
        self.assertEqual(set(machine_state.CONTENTION_LIMITS), declared)
        for name, regime in machine_state.REGIMES.items():
            with self.subTest(regime=name):
                self.assertIn(regime.area, machine_state.STAGING_AREAS)

    def test_a_stage_that_names_no_regime_is_declared_as_such(self):
        # `criterion` and `derived` are exempt because they read no staged
        # input; anything else exempted by hand would be a regime hidden from
        # the reconciliation above.
        for name in machine_state.NON_REGIME_STAGES:
            with self.subTest(stage=name):
                self.assertNotIn(name, machine_state.REGIMES)
                # And each is a stage some figure actually carries, so an
                # exemption cannot outlive the thing it exempts.
                self.assertTrue(
                    any(name in f.stage.split("+") for f in register.EVERY_FIGURE)
                )

    def test_an_unknown_regime_is_refused_rather_than_resolved(self):
        """The defect this closed: `input_path` matched two names and returned
        the *warm* path for everything else, while the cache drop fired on any
        name starting with `cold` — so a `cold-parallel` regime added to a
        figure and nowhere else would have dropped the page cache and then read
        from tmpfs, publishing warm readings under a cold heading."""
        with self.assertRaises(ValueError) as raised:
            machine_state.regime_spec("cold-parallel")
        self.assertIn("cold-parallel", str(raised.exception))
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(dry_run=True, cache_dir=Path(tmp) / "ssd")
            session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
            with self.assertRaises(ValueError):
                session.input_path("control", "cold-parallel")

    def test_the_staging_area_a_regime_reads_is_the_one_it_declares(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = config.Config(
                dry_run=True,
                cache_dir=root / "ssd",
                nvme_dir=root / "nvme",
                warm_dir=root / "shm",
                warm_budget=8.0,
            )
            session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
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
            cfg = config.Config(dry_run=True, nvme_dir=Path(tmp) / "nvme")
            stager = staging.Stager(cfg, lambda _m: None)
            figures = [register.FIGURES_BY_ID["scan-throughput-nvme"]]
            stager.plan(figures)
            tiny = collections.namedtuple("usage", "total used free")(0, 0, 1024)
            with unittest.mock.patch.object(shutil, "disk_usage", return_value=tiny):
                problems = stager._nvme_preflight(figures)
        self.assertTrue(any("cold-NVMe" in p for p in problems), problems)

    def test_a_figure_with_no_nvme_inputs_checks_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(dry_run=True, nvme_dir=Path(tmp) / "nvme")
            stager = staging.Stager(cfg, lambda _m: None)
            self.assertEqual(
                stager._nvme_preflight([register.FIGURES_BY_ID["scan-throughput-warm"]]), []
            )
            # Not even a directory: a check that mkdir'd one for a sweep that
            # takes no NVMe figure would leave the area behind on every run.
            self.assertFalse((Path(tmp) / "nvme").exists())

    def test_a_cold_stage_selection_does_not_reach_another_device(self):
        # `--stage` splits on `+` rather than matching as a substring, so
        # `chunk-size`'s `warm+cold+cold-nvme` is selected by `cold` and
        # `scan-throughput-nvme`'s `cold-nvme` is not.
        cold = [f.id for f in register.FIGURES if "cold" in f.stage.split("+")]
        self.assertIn("scan-throughput-cold", cold)
        self.assertIn("chunk-size", cold)
        self.assertNotIn("scan-throughput-nvme", cold)
        nvme = [f.id for f in register.FIGURES if "cold-nvme" in f.stage.split("+")]
        self.assertEqual(
            sorted(nvme), ["chunk-size", "inflate-throughput", "scan-throughput-nvme"]
        )
        # `chunk-size` and `inflate-throughput` are taken in all three regimes,
        # so they are what would catch a selection rule reading `cold` as a
        # prefix of `cold-nvme` in either direction.
        self.assertIn("chunk-size", cold)
        self.assertIn("inflate-throughput", cold)


class Gate(unittest.TestCase):
    LIMITS = {"warm": {"cpu_busy_pct": 25.0}, "cold": {"cpu_busy_pct": 25.0}}

    def test_a_reading_over_a_limit_is_named_with_the_limit_it_broke(self):
        verdict = machine_state.contention_verdict({"cpu_busy_pct": 60.0}, "warm", self.LIMITS)
        self.assertIn("cpu_busy_pct", verdict)
        self.assertIn("25.0", verdict)

    def test_a_reading_under_every_limit_is_clean(self):
        self.assertIsNone(machine_state.contention_verdict({"cpu_busy_pct": 3.0}, "warm", self.LIMITS))

    def test_a_limit_with_no_reading_behind_it_does_not_fire(self):
        # A machine without PSI must not fail every reading for lack of it.
        self.assertIsNone(
            machine_state.contention_verdict({}, "warm", {"warm": {"psi_cpu_some_pct": 1.0}})
        )

    def test_a_regime_with_no_limits_gates_nothing(self):
        self.assertIsNone(machine_state.contention_verdict({"cpu_busy_pct": 99.0}, "warm", {}))

    def test_io_stall_is_gated_in_neither_regime(self):
        # A cold run drops the page cache and reads 3 GiB off the SSD, so it
        # stalls on I/O for a fifth of its window *by construction*. Gating
        # that would fail every cold reading there is.
        for regime in ("cold", "warm"):
            self.assertNotIn("psi_io_some_pct", machine_state.CONTENTION_LIMITS[regime])
        self.assertIsNone(machine_state.contention_verdict({"psi_io_some_pct": 23.0}, "cold"))

    def test_the_armed_limits_clear_the_calibration_sweep_with_headroom(self):
        # Every p95 from the 182-reading fa186ab sweep, which its apparatus
        # lines witness as quiet. A limit that fires here is mis-set.
        for regime, p95 in (
            ("cold", {"cpu_busy_pct": 4.29, "psi_cpu_some_pct": 1.16, "cpu_steal_pct": 0.0}),
            ("warm", {"cpu_busy_pct": 4.52, "psi_cpu_some_pct": 0.20, "cpu_steal_pct": 0.0}),
        ):
            self.assertIsNone(machine_state.contention_verdict(p95, regime), regime)

    def test_an_obviously_busy_machine_is_caught_in_either_regime(self):
        for regime in ("cold", "warm"):
            self.assertIsNotNone(machine_state.contention_verdict({"cpu_busy_pct": 40.0}, regime))
            self.assertIsNotNone(machine_state.contention_verdict({"cpu_steal_pct": 12.0}, regime))


class ApparatusNote(unittest.TestCase):
    def test_the_note_quotes_the_worst_run_not_the_average(self):
        records = [
            {"telemetry": {"cpu_busy_pct": 3.0, "psi_cpu_some_pct": 0.1}},
            {"telemetry": {"cpu_busy_pct": 40.0, "psi_cpu_some_pct": 9.0}},
        ]
        note = machine_state.apparatus_note(records)
        self.assertIn("≤40% busy", note)
        self.assertIn("9.00%", note)

    def test_no_telemetry_emits_no_note(self):
        # A machine exposing none of this emits the table it always did.
        self.assertEqual(machine_state.apparatus_note([{"seconds": 1.0}]), "")

    def test_the_slowest_frequency_is_the_one_reported(self):
        records = [
            {"telemetry": {"freq_busiest_min_mhz": 4500.0}},
            {"telemetry": {"freq_busiest_min_mhz": 2100.0}},
        ]
        self.assertIn("≥2.10 GHz", machine_state.apparatus_note(records))


class Governor(unittest.TestCase):
    def test_pinning_is_off_by_default(self):
        # It measures as a no-op on amd-pstate-epp, and turning it on is an
        # apparatus change that obliges a full re-sweep.
        self.assertFalse(config.Config().pin_governor)

    def test_a_machine_without_cpufreq_is_reported_not_crashed(self):
        with tempfile.TemporaryDirectory() as tmp:
            pin = machine_state.GovernorPin(log=lambda _m: None, root=Path(tmp))
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
            pin = machine_state.GovernorPin(log=lambda _m: None, root=root)
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
            pin = machine_state.GovernorPin(log=lambda _m: None, root=root)
            pin._write_all = lambda values: written.append(dict(values))
            pin.__enter__()
            pin.restore()
            pin.restore()
            self.assertEqual(len(written), 2)


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
        release.parse_cpu_list(t)
        for t in ("0-2,12-14", "3-5,15-17", "6-8,18-20", "9-11,21-23")
    ]
    TIMED = "\nreal\t0m0.012345s\nuser\t0m0.008000s\nsys\t0m0.004000s\n"
    EVENTS = "oom_kill 0\n"

    def test_l3_groups_are_read_off_sysfs_by_lowest_cpu(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for cpu, shared in ((0, "0,2"), (1, "1,3"), (2, "0,2"), (3, "1,3")):
                d = root / f"cpu{cpu}/cache/index3"
                d.mkdir(parents=True)
                (d / "shared_cpu_list").write_text(shared + "\n")
            self.assertEqual(machine_state.l3_groups(root), [frozenset({0, 2}), frozenset({1, 3})])

    def test_the_placement_follows_the_stated_count(self):
        small, large = self.GROUPS[1], self.GROUPS[0] | self.GROUPS[1]
        self.assertEqual(machine_state.placement(1, self.GROUPS), small)
        self.assertEqual(machine_state.placement(6, self.GROUPS), small)
        self.assertEqual(machine_state.placement(7, self.GROUPS), large)
        self.assertEqual(machine_state.placement(12, self.GROUPS), large)
        # The whole machine's subject stays on the whole machine.
        self.assertIsNone(machine_state.placement(16, self.GROUPS))
        self.assertIsNone(machine_state.placement(None, self.GROUPS))
        # The harness never shares a group with a pinned leg.
        self.assertFalse(machine_state.harness_cpus(self.GROUPS) & large)

    def test_another_topology_is_refused_rather_than_guessed(self):
        self.assertIsNone(machine_state.pin_topology_problem(self.GROUPS))
        self.assertIn("4 equal L3 groups", machine_state.pin_topology_problem(self.GROUPS[:2]))
        uneven = [*self.GROUPS[:3], frozenset({9})]
        self.assertIsNotNone(machine_state.pin_topology_problem(uneven))

    def test_a_leg_is_placed_by_the_count_its_script_states(self):
        self.assertEqual(shapes.stated_threads("dd"), 1)
        self.assertEqual(shapes.stated_threads("parse"), shapes.SWEEP_JOBS)
        self.assertEqual(shapes.stated_threads("parse-jobs-24"), 24)
        self.assertEqual(shapes.stated_threads("decode-16"), 16)
        self.assertEqual(shapes.stated_threads(shapes.DYNFILTER_STARTUP), shapes.SWEEP_JOBS)
        for command in shapes.dynfilter_shapes():
            self.assertEqual(shapes.stated_threads(command), shapes.SWEEP_JOBS)
        # Discovery stays unpinned: a cpuset would narrow what it reads.
        flagless = next(c for c in shapes.command_shapes() if c.startswith(shapes.RESERVE_FLAGLESS))
        self.assertIsNone(shapes.stated_threads(flagless))
        # Every shape resolves, so no leg reaches a sitting unplaceable.
        for command in shapes.command_shapes():
            shapes.stated_threads(command)

    def test_the_recorded_apparatus_is_the_default_and_the_only_publishable_one(self):
        cfg = config.Config()
        self.assertEqual(cfg.arms, (machine_state.Arm(),))
        self.assertTrue(cfg.publishable)
        self.assertEqual(cfg.arms[0].name, "unpinned+staged")
        self.assertTrue(config.Config(pin_cpus="off", stage_binaries="on").publishable)
        for pin, stage in (("on", "on"), ("off", "off"), ("alternate", "on"), ("off", "alternate")):
            with self.subTest(pin=pin, stage=stage):
                self.assertFalse(config.Config(pin_cpus=pin, stage_binaries=stage).publishable)
        # `alternate` puts the recorded arm first, so it is the one the tables render.
        both = config.Config(pin_cpus="alternate", stage_binaries="on").arms
        self.assertEqual([a.name for a in both], ["unpinned+staged", "pinned+staged"])
        both = config.Config(stage_binaries="alternate").arms
        self.assertEqual([a.name for a in both], ["unpinned+staged", "unpinned+unstaged"])
        with self.assertRaises(ValueError):
            config.Config(pin_cpus="sometimes").arms

    def test_the_report_format_reads_to_the_microsecond_outside_the_shape(self):
        self.assertAlmostEqual(parsing.parse_bash_time(self.TIMED), 0.012345)
        self.assertEqual(parsing.parse_bash_cpu(self.TIMED), {"user": 0.008, "sys": 0.004})
        # The default's shape, which `TIME_RE` reads, at six places.
        self.assertIn("%6lR", parsing.TIME_FORMAT)
        self.assertNotIn("time ", parsing.TIME_FORMAT)

    def _session(self, **cfg):
        config_ = config.Config(**cfg)
        with unittest.mock.patch.object(machine_state, "l3_groups", return_value=self.GROUPS):
            session = timing.Session(config_, staging.Stager(config_, lambda _m: None), lambda _m: None)
        session.figure_id = "dynamic-filter-topk"
        # A unit test starts no container to ask the image for its glibc.
        self.enterContext(unittest.mock.patch.object(leg_binaries, "glibc_of", return_value="2.41"))
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path(f"/bin/{which}")
        session.stage_binary = lambda src: Path("/dev/shm/pgdt/bin") / src.name
        session.place_harness = lambda cpus: setattr(session, "placed", cpus)
        return session

    def _argv(self, session, spec):
        proc = subprocess.CompletedProcess([], 0, stdout="startup_answer=1\n", stderr=self.TIMED + self.EVENTS)
        with unittest.mock.patch.object(subprocess, "run", return_value=proc) as ran:
            session.time_run(spec)
        return ran.call_args[0][0]

    def test_a_pinned_staged_leg_mounts_from_tmpfs_and_reads_it_before_the_timer(self):
        session = self._session(pin_cpus="on", stage_binaries="on")
        spec = runspec.RunSpec("dfcli", "dynfilter", shapes.DYNFILTER_STARTUP, "warm", "")
        argv = self._argv(session, spec)
        self.assertEqual(argv[argv.index("--cpuset-cpus") + 1], "3-5,15-17")
        self.assertIn("/dev/shm/pgdt/bin/dfcli:/pgdt:ro", argv)
        script = argv[-1]
        self.assertTrue(script.startswith(parsing.TIME_FORMAT + "cat /pgdt >/dev/null; "))
        self.assertLess(script.index("cat /pgdt"), script.index("time "))
        self.assertEqual(session.placed, machine_state.harness_cpus(self.GROUPS))
        record = session.records[-1]
        self.assertEqual((record["arm"], record["cpuset"]), ("pinned+staged", "3-5,15-17"))
        self.assertEqual(record["cpu_seconds"], {"user": 0.008, "sys": 0.004})

    def test_the_recorded_apparatus_stages_and_adds_nothing_else(self):
        session = self._session()
        # A cold regime too: a `drop_caches` evicts a binary read off disk,
        # and none staged on tmpfs.
        for regime in ("warm", "cold"):
            with self.subTest(regime=regime):
                spec = runspec.RunSpec("pgdt", "control", "parse", regime, "")
                session.drop_caches = lambda: None
                argv = self._argv(session, spec)
                self.assertNotIn("--cpuset-cpus", argv)
                self.assertIn("/dev/shm/pgdt/bin/pgdt:/pgdt:ro", argv)
                self.assertEqual(
                    argv[-1],
                    parsing.TIME_FORMAT + "cat /pgdt >/dev/null; "
                    + shapes._script("parse") + parsing.OOM_ORACLE,
                )
        self.assertFalse(hasattr(session, "placed"))

    def test_an_unstaged_leg_mounts_where_cargo_left_it_and_reads_nothing_first(self):
        session = self._session(stage_binaries="off")
        argv = self._argv(session, runspec.RunSpec("pgdt", "control", "parse", "warm", ""))
        self.assertIn("/bin/pgdt:/pgdt:ro", argv)
        self.assertEqual(
            argv[-1], parsing.TIME_FORMAT + shapes._script("parse") + parsing.OOM_ORACLE
        )

    def test_an_unpinned_leg_under_the_pinned_arm_sends_the_harness_home(self):
        session = self._session(pin_cpus="on")
        argv = self._argv(session, runspec.RunSpec("pgdt", "control", "parse-jobs-24", "warm", ""))
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
        spec = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
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
        text = drift.arms_table(raw)
        self.assertIn("-50.00%", text)
        self.assertIn("spread wider than unpinned+staged's on 0 of 1", text)
        with self.assertRaises(ValueError):
            drift.arms_table({"commit": "abc", "date": "d"})

    def test_two_sittings_price_each_arms_drift_against_the_criterion(self):
        a = self._raw({"f/a": [1.0], "f/b": [1.0]}, {"f/a": [1.0], "f/b": [1.0]})
        b = self._raw({"f/a": [1.10], "f/b": [0.90]}, {"f/a": [1.04], "f/b": [0.96]})
        text = drift.arm_drift_table(a, b)
        self.assertIn("**10.00%**", text)
        self.assertIn("**4.00%**", text)
        self.assertIn("0.40×", text)

    def test_a_second_arm_never_reaches_a_censored_bound(self):
        session = self._session()
        spec = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        want = dataclasses.asdict(spec)
        session.records = [
            {"killed": True, "figure": "f", "spec": want, "maxrss_bound_kib": 1,
             "seconds_to_kill": 1.0, "secondary_arm": False},
            {"killed": True, "figure": "f", "spec": want, "maxrss_bound_kib": 2,
             "seconds_to_kill": 2.0, "secondary_arm": True},
        ]
        self.assertEqual(session.censored_bounds("f", spec), [(1, 1.0)])

    def test_the_startup_leg_answers_before_its_table_does(self):
        script = shapes._script(shapes.DYNFILTER_STARTUP)
        builder, _, timed = script.partition(" && ")
        self.assertTrue(builder.startswith("/pgdt parse --source /dump.sql "))
        self.assertIn(shapes.GATHER_STATISTICS, builder)
        self.assertTrue(timed.startswith(f"time {shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS} "))
        self.assertIn(f"'{shapes.STARTUP_SQL}'", timed)
        self.assertEqual(script.count("time "), 1)
        self.assertIn(shapes.DYNFILTER_STARTUP, shapes.command_shapes())
        self.assertEqual(shapes.statistics_flag_problems(), [])


if __name__ == "__main__":
    unittest.main()
