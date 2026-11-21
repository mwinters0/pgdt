"""Tests of `measurement.figures.release_profile`."""

from __future__ import annotations

import inspect
import re
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import emit
from measurement import host
from measurement import parsing
from measurement import preflight
from measurement import replay
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import declaration
from measurement.figures import register
from measurement.figures import release_profile


class InstructionCounter(unittest.TestCase):
    """`count-instructions` under the `release-profile` figure: every `pgdt`
    leg counted in the run its wall clock is, the count read back or the rep
    refused, and the downstream-build rule read off both.

    What the counter counts, how it exits and that every thread is in its
    count are the crate's own tests
    (`crates/count-instructions/tests/count_instructions.rs`); what is held
    here is that the harness builds it, mounts it where it counts, and reads
    the rule the way `roadmap.md`, "A downstream build is within 3% of ours"
    states it."""

    FIGURE = "release-profile"

    def setUp(self):
        leg_binaries._PROFILE_BUILT.clear()
        leg_binaries._PROFILE_ANNOUNCED.clear()

    def test_the_counted_figure_is_release_profile_and_declares_the_counter(self):
        counting = {fig.id for fig in register.FIGURES if fig.counts_instructions}
        self.assertEqual(counting, {self.FIGURE})
        declaring = {
            fig.id for fig in register.FIGURES if set(declaration.COUNTER_INSTRUMENT) <= set(fig.depends)
        }
        self.assertEqual(declaring, counting)
        self.assertTrue((config.REPO / declaration.COUNTER_INSTRUMENT[0]).is_dir())

    def test_the_counter_is_built_static_beside_peak_rss(self):
        target = image.peak_rss_target()
        self.assertEqual(
            image.count_instructions_path(),
            config.REPO / "target" / target / "release" / "count-instructions",
        )
        asked = []
        with tempfile.TemporaryDirectory() as sysroot:
            (Path(sysroot) / "lib" / "rustlib" / target).mkdir(parents=True)

            def fake(argv, **_kwargs):
                asked.append(list(argv))
                return sysroot + "\n"

            with unittest.mock.patch.object(host, "run", fake), \
                    unittest.mock.patch.object(image, "_COUNTER_BUILT", False):
                image.ensure_count_instructions_binary(config.Config(), lambda _m: None)
                image.ensure_count_instructions_binary(config.Config(), lambda _m: None)
        builds = [argv for argv in asked if argv[:2] == ["cargo", "build"]]
        self.assertEqual(
            builds,
            [["cargo", "build", "--release", "-p", "count-instructions", "--target", target]],
        )

    def test_the_counter_wraps_each_shape_s_timed_pgdt_and_nothing_else(self):
        # Inside the timer and behind the environment the program is given,
        # which its child inherits; never the untimed builder ahead of it.
        for command, _ in release_profile.RELEASE_PROFILE_SHAPES:
            with self.subTest(command=command):
                script = shapes._script(command)
                counted = parsing.counted_script(script)
                self.assertEqual(counted.count(parsing.COUNT_INSTRUCTIONS), 1)
                self.assertEqual(counted.replace(f"{parsing.COUNT_INSTRUCTIONS} ", ""), script)
                timed = counted[counted.index("time "):]
                self.assertRegex(
                    timed, rf"^time (?:[A-Z_]+=\S+ )*{re.escape(parsing.COUNT_INSTRUCTIONS)} /pgdt "
                )

    def test_a_shape_with_no_one_timed_pgdt_is_refused(self):
        for script in (shapes._script("dd"), "time /pgdt a && time /pgdt b"):
            with self.subTest(script=script), self.assertRaises(ValueError):
                parsing.counted_script(script)

    def test_the_count_is_read_off_the_counter_s_own_line(self):
        self.assertEqual(
            parsing.parse_instructions("real\t0m0.5s\ninstructions_u=1234567\n"), 1234567
        )

    def test_no_count_two_counts_or_a_multiplexed_one_is_an_error_not_a_reading(self):
        for text in (
            "real\t0m0.5s\n",
            "instructions_u=1\ninstructions_u=2\n",
            "instructions_multiplexed=50/100\n",
        ):
            with self.subTest(text=text), self.assertRaises(ValueError):
                parsing.parse_instructions(text)

    def _logged_runs(self, specs):
        cfg = config.Config(dry_run=True)
        logged: list[str] = []
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), logged.append)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        session.figure_id = self.FIGURE
        for spec in specs:
            session.time_run(spec)
        return session, [line for line in logged if " run --rm " in line]

    def test_a_counted_leg_carries_the_counter_and_its_capability_and_the_floor_neither(self):
        specs = release_profile._release_profile_specs()
        floor = specs[-1]
        self.assertEqual(floor.binary, "none")
        session, runs = self._logged_runs([specs[0], floor])
        cap = " ".join(f"--cap-add {c}" for c in parsing.COUNT_INSTRUCTIONS_CAPS)
        mount = f"{parsing.COUNT_INSTRUCTIONS}:ro"
        self.assertIn(cap, runs[0])
        self.assertIn(mount, runs[0])
        self.assertIn(f"time {parsing.COUNT_INSTRUCTIONS} /pgdt ", runs[0])
        self.assertNotIn(cap, runs[1])
        self.assertNotIn(mount, runs[1])
        self.assertEqual(session.records, [])  # a dry run records nothing

    def test_an_uncounted_figure_s_leg_carries_neither(self):
        cfg = config.Config(dry_run=True)
        logged: list[str] = []
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), logged.append)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        session.figure_id = "allocator"
        session.time_run(runspec.RunSpec("pgdt", "control", "parse", "warm", ""))
        (run,) = [line for line in logged if " run --rm " in line]
        self.assertNotIn("--cap-add", run)
        self.assertNotIn(parsing.COUNT_INSTRUCTIONS, run)

    def test_a_counted_run_records_its_count_beside_its_reading(self):
        cfg = config.Config()
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        session.figure_id = self.FIGURE
        self.enterContext(
            unittest.mock.patch.object(
                image, "ensure_count_instructions_binary", return_value=Path("/dev/null")
            )
        )
        stderr = "instructions_u=4200000000\n\nreal\t0m0.012s\nuser\t0m0.008s\nsys\t0m0.004s\noom_kill 0\n"
        proc = subprocess.CompletedProcess([], 0, stdout="", stderr=stderr)
        with unittest.mock.patch.object(subprocess, "run", lambda *a, **k: proc):
            session.time_run(release_profile._release_profile_specs()[0])
        self.assertEqual(session._last_instructions, 4200000000)
        self.assertEqual(session.records[0]["instructions_u"], 4200000000)
        missing = subprocess.CompletedProcess([], 0, stdout="", stderr=stderr.split("\n", 1)[1])
        with unittest.mock.patch.object(subprocess, "run", lambda *a, **k: missing):
            with self.assertRaises(ValueError):
                session.time_run(release_profile._release_profile_specs()[0])

    def test_the_preflight_counts_true_in_the_pinned_image_under_the_capability(self):
        started = []

        def fake_run(argv, **_kwargs):
            started.append(list(argv))
            return subprocess.CompletedProcess(argv, 0, "", "instructions_u=120000\n")

        with unittest.mock.patch.object(subprocess, "run", fake_run), \
                unittest.mock.patch.object(
                    image, "ensure_count_instructions_binary", return_value=Path("/x/ci")
                ):
            problems = preflight.counter_preflight(config.Config(), lambda _m: None)
        self.assertEqual(problems, [])
        (argv,) = started
        self.assertEqual(argv[-2:], [parsing.COUNT_INSTRUCTIONS, "/bin/true"])
        self.assertIn(config.Config().image, argv)
        self.assertIn(f"/x/ci:{parsing.COUNT_INSTRUCTIONS}:ro", argv)
        self.assertIn("PERFMON", argv)

    def test_a_counter_the_kernel_refuses_is_a_problem_before_the_first_reading(self):
        refused = subprocess.CompletedProcess(
            [], 1, "", "count-instructions: perf_event_open: Operation not permitted (os error 1)\n"
        )
        with unittest.mock.patch.object(subprocess, "run", lambda *a, **k: refused), \
                unittest.mock.patch.object(
                    image, "ensure_count_instructions_binary", return_value=Path("/x/ci")
                ):
            problems = preflight.counter_preflight(config.Config(), lambda _m: None)
        self.assertEqual(len(problems), 1)
        self.assertIn("perf_event_open", problems[0])

    def test_the_sweep_runs_the_preflight_only_where_a_figure_counts(self):
        source = inspect.getsource(emit.emit)
        self.assertIn("if any(fig.counts_instructions for fig in figures):", source)
        self.assertLess(
            source.index("counter_preflight(cfg, log)"), source.index("Session(cfg, stager")
        )

    # -- the rule's verdict --------------------------------------------------

    WITHIN = ([2.87, 2.90, 2.88, 2.85, 2.87], [2.81, 2.81, 2.75, 2.80, 2.79])
    PAST = ([3.10, 3.12, 3.11, 3.09, 3.10], [2.95, 2.96, 2.94, 2.95, 2.96])
    MEETING = ([3.10, 3.12, 3.11, 2.90, 3.10], [2.95, 2.96, 2.94, 2.95, 2.96])
    MORE = ([10_100, 10_101, 10_102], [10_000, 10_001, 10_002])
    LEVEL = ([10_000, 10_003, 10_001], [10_001, 10_002, 10_000])

    def test_a_gap_within_the_order_s_small_passes(self):
        verdict = release_profile.downstream_verdict("x", self.WITHIN, self.MORE)
        self.assertIn("passes", verdict)
        self.assertNotIn("owes", verdict)

    def test_a_gap_past_it_that_the_readings_do_not_resolve_passes(self):
        self.assertIn("passes", release_profile.downstream_verdict("x", self.MEETING, self.MORE))

    def test_a_resolved_failure_with_more_instructions_owes_the_remedy(self):
        verdict = release_profile.downstream_verdict("x", self.PAST, self.MORE)
        self.assertIn("**fails**", verdict)
        self.assertIn("owes the remedy", verdict)

    def test_a_resolved_failure_with_the_counts_level_is_layout(self):
        verdict = release_profile.downstream_verdict("x", self.PAST, self.LEVEL)
        self.assertIn("**fails**", verdict)
        self.assertIn("layout, which owes nothing", verdict)

    def test_a_failure_with_no_count_is_unattributed(self):
        self.assertIn("unattributed", release_profile.downstream_verdict("x", self.PAST, None))

    def test_the_gap_is_against_the_default_leg_s_median(self):
        # The `pgdt sql` row at `2df93720` (`runs/measure-20261011T013910/`),
        # whose gap the rule's reading quoted: 1 - 2.804 / 2.869 is 2.27%,
        # where against the shipped leg's median it would be 2.32%.
        walls = (
            [2.868, 2.904, 2.875, 2.851, 2.869],
            [2.81, 2.81, 2.753, 2.804, 2.792],
        )
        self.assertIn("passes — +2.27% in wall clock, resolved", release_profile.downstream_verdict("x", walls, None))

    # -- rendering ---------------------------------------------------------

    def _render(self, instructions):
        specs = release_profile._release_profile_specs()
        raw = {
            "readings": {s.key(self.FIGURE): [1.0] * 5 for s in specs},
            "reported": {
                s.key(self.FIGURE): {"result_rows": "1", "result_first": "100", "result_digest": "ab"}
                for s in specs
                if s.command == shapes.PARALLEL_SCAN
            },
            "instructions": instructions(specs),
            "input_sizes": {"control": 3221227790},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(dry_run=True), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = self.FIGURE
            return release_profile.run_release_profile(session)

    def test_a_counted_sitting_renders_its_counts_and_a_verdict_per_shape(self):
        body = self._render(
            lambda specs: {
                s.key(self.FIGURE): [4_000_000_000] * 5 for s in specs if s.binary != "none"
            }
        )
        self.assertIn("| `instructions:u`, the same runs |", body)
        self.assertIn("**4.000 G** (4.000–4.000)", body)
        self.assertIn("Per-rep `instructions:u` (G):", body)
        for _, label in release_profile.RELEASE_PROFILE_SHAPES:
            with self.subTest(label=label):
                self.assertIn(f"- {label}: passes", body)

    def test_a_sitting_from_before_the_count_still_renders_and_says_so(self):
        body = self._render(lambda specs: {})
        self.assertIn("This sitting took no `instructions:u`", body)
        self.assertNotIn("Per-rep `instructions:u`", body)

    def test_a_sitting_counting_some_legs_and_not_others_is_refused(self):
        with self.assertRaises(RuntimeError):
            self._render(lambda specs: {specs[0].key(self.FIGURE): [1] * 5})


if __name__ == "__main__":
    unittest.main()
