"""Tests of `measurement.parsing`."""

from __future__ import annotations

import json
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import parsing
from measurement import shapes


class BashTime(unittest.TestCase):
    def test_reads_the_real_line(self):
        self.assertAlmostEqual(
            parsing.parse_bash_time("real\t0m0.570s\nuser\t0m0.4s\nsys\t0m0.1s"), 0.570
        )

    def test_minutes_are_added(self):
        self.assertAlmostEqual(parsing.parse_bash_time("real\t1m2.003s\n"), 62.003)

    def test_dd_s_own_chatter_is_not_mistaken_for_a_timing(self):
        text = (
            "3221225472 bytes (3.2 GB, 3.0 GiB) copied, 5.73374 s, 562 MB/s\n"
            "real\t0m5.734s\nuser\t0m0.0s\n"
        )
        self.assertAlmostEqual(parsing.parse_bash_time(text), 5.734)

    def test_no_timing_is_an_error_not_a_zero(self):
        # A labelled run with no number under it is exactly what redirecting
        # stderr inside a timed command produces.
        with self.assertRaises(ValueError):
            parsing.parse_bash_time("some output but no timing\n")

    def test_two_timings_are_an_error(self):
        with self.assertRaises(ValueError):
            parsing.parse_bash_time("real\t0m1.0s\nreal\t0m2.0s\n")


class Criterion(unittest.TestCase):
    def _bench(self, root: Path, directory: str, full_id: str, median_ns: float, which="new"):
        d = root / directory / which
        d.mkdir(parents=True)
        (d / "benchmark.json").write_text(json.dumps({"full_id": full_id}))
        (d / "estimates.json").write_text(
            json.dumps({"median": {"point_estimate": median_ns}, "mean": {"point_estimate": 0}})
        )

    def test_median_comes_from_the_new_estimates(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._bench(root, "array_4_decode", "nested/array_4/decode", 265.1)
            self.assertAlmostEqual(
                parsing.criterion_median_ns(root, "nested/array_4/decode"), 265.1
            )

    def test_the_previous_run_is_not_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._bench(root, "array_4_decode", "nested/array_4/decode", 265.1)
            self._bench(root, "array_4_decode", "nested/array_4/decode", 999.9, which="base")
            self.assertAlmostEqual(
                parsing.criterion_median_ns(root, "nested/array_4/decode"), 265.1
            )

    def test_a_missing_bench_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(FileNotFoundError):
                parsing.criterion_median_ns(Path(tmp), "nested/array_4/decode")


class Reported(unittest.TestCase):
    """`key=value` lines a timed binary prints about its own run."""

    def test_it_reads_the_keys_it_is_given(self):
        got = parsing.parse_reported("workers=8\nplaintext=3221225472\ndelivered=3221225472\n")
        self.assertEqual(got["workers"], "8")
        self.assertEqual(got["delivered"], "3221225472")

    def test_anything_else_is_ignored_rather_than_refused(self):
        # A binary is free to print whatever else it likes; a parser that
        # refused would couple every instrument's output to this one's shape.
        self.assertEqual(parsing.parse_reported("hello\nworkers=2\n= \n"), {"workers": "2"})

    def test_no_report_is_an_empty_report(self):
        self.assertEqual(parsing.parse_reported(""), {})


class OomKillOracle(unittest.TestCase):
    """Telling an OOM kill from any other failure.

    The exit code cannot: `peak-rss` exits `128 + n` for a death by signal `n`,
    but a `SIGKILL` does not say who sent it, and `--rm` has destroyed the
    container before `nerdctl inspect` could be asked. So the oracle is the container's own `memory.events`, read inside it
    after the timed command — and the three things that make that sound are
    asserted here, because each fails by returning a plausible answer about
    something else.
    """

    EVENTS = "low 0\nhigh 0\nmax 41\noom 2\noom_kill 1\noom_group_kill 0\n"

    def test_the_counter_is_read_off_memory_events(self):
        self.assertEqual(parsing.parse_oom_kills(self.EVENTS), 1)

    def test_a_container_that_killed_nothing_answers_zero(self):
        self.assertEqual(parsing.parse_oom_kills(self.EVENTS.replace("oom_kill 1", "oom_kill 0")), 0)

    def test_an_unreadable_counter_is_not_zero(self):
        # The distinction the whole oracle exists for: "nothing was killed" and
        # "the harness cannot tell" are different answers, and reading the
        # second as the first is how a kill becomes a silent apparatus failure.
        self.assertIsNone(parsing.parse_oom_kills("real\t0m1.000s\n"))

    def test_the_oracle_runs_after_the_timed_command_and_preserves_its_status(self):
        # Inside the timer it would be part of the reading; without the saved
        # status the container would exit on `cat`'s success and every failure
        # would read as a pass.
        self.assertTrue(parsing.OOM_ORACLE.startswith("; __st=$?;"))
        self.assertTrue(parsing.OOM_ORACLE.rstrip().endswith("exit $__st"))
        self.assertIn("/sys/fs/cgroup/memory.events", parsing.OOM_ORACLE)

    def test_it_writes_to_stderr_beside_the_other_two_reports(self):
        # `parse_reported` reads a binary's *stdout*; sending the counter there
        # would put `oom_kill 0` in front of it as a `key=value` line.
        self.assertIn(">&2", parsing.OOM_ORACLE)

    def test_it_is_not_part_of_any_command_shape(self):
        # What a figure records as its shape must be exactly what was measured.
        for command in shapes.command_shapes():
            with self.subTest(command=command):
                self.assertNotIn("memory.events", shapes._script(command))

    def test_the_oracle_adds_a_command_but_never_a_timer(self):
        # `parse_bash_time` refuses more than one `real` line, so a shape that
        # acquired a second timed command would lose its figure — and a shape
        # whose timer count the oracle changed is exactly that.
        self.assertNotIn("time ", parsing.OOM_ORACLE)
        for command in shapes.command_shapes():
            with self.subTest(command=command):
                bare = shapes._script(command)
                self.assertEqual((bare + parsing.OOM_ORACLE).count("time "), bare.count("time "))


if __name__ == "__main__":
    unittest.main()
