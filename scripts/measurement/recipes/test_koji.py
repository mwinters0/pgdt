"""Tests of `measurement.recipes.koji`."""

from __future__ import annotations

import release
import unittest
import unittest.mock

from measurement import config
from measurement import shapes
from measurement.recipes import koji


class KojiRecipe(unittest.TestCase):
    """koji is never run from here, but the invocation is owned here — three
    hand-maintained copies is how a documented command was found that could
    not execute. Each assertion below is a mistake that has cost a run."""

    def _recipe(self, wrap=False, jobs=shapes.SWEEP_JOBS) -> str:
        return koji.koji_recipe(config.Config(), "pgdt-koji", wrap, jobs)

    def test_pgdt_is_pid_one(self):
        # A compound command cannot be exec'd, so nothing may be appended to
        # report the exit status: `sh` would take the signal and not forward
        # it, the runtime's SIGKILL would follow, and the interrupt guard would
        # never run.
        self.assertIn("sh -c 'exec /pgdt parse", self._recipe())

    def test_nothing_follows_the_parse_inside_the_shell(self):
        for line in self._recipe().splitlines():
            if "exec /pgdt parse" in line:
                with self.subTest(line=line):
                    self.assertNotIn("; echo", line)

    def test_the_cgroup_limit_is_part_of_the_apparatus(self):
        self.assertIn("-m 512m --memory-swap 512m", self._recipe())

    def test_the_binary_is_the_register_s_in_the_register_s_image(self):
        # The binary it mounts is the release image's build, and it runs in
        # that image's base: a host build need not load there.
        recipe = self._recipe()
        self.assertIn(f"uv run release.py build --target {release.NATIVE_TARGET}", recipe)
        self.assertIn(f'-v "{config.REGISTER_BIN}:/pgdt:ro"', recipe)
        self.assertIn(config.register_image(), recipe)
        self.assertNotIn("cargo build", recipe)

    def test_the_cache_lands_in_the_mounted_volume(self):
        # The dump is mounted read-only, so the colocated default would land in
        # the container's ephemeral layer and die with it — an hour of scanning
        # lost with no error, because the write itself succeeds.
        self.assertIn("--cache /out/", self._recipe())
        self.assertNotIn("--cache /dump.sql", self._recipe())

    def test_the_dump_is_read_only(self):
        self.assertIn(":/dump.sql:ro", self._recipe())

    def test_the_exit_status_is_read_from_inspect(self):
        self.assertIn("{{.State.ExitCode}}", self._recipe())

    def test_the_run_s_own_resident_set_can_be_read(self):
        # koji's record carried an RSS row that no run since has measured,
        # because the recipe captured no memory figure at all. `VmHWM` off the
        # host pid is the instrument: the cgroup's `memory.peak` is charged the
        # page cache of a 784 GB read and reports the limit.
        recipe = self._recipe()
        self.assertIn("VmHWM", recipe)
        self.assertIn("{{.State.Pid}}", recipe)

    def test_the_resident_set_capture_is_outside_the_exec_d_command(self):
        # It is read from the host while the scan runs; adding anything to the
        # container's command would cost `exec` and with it the interrupt guard.
        for line in self._recipe().splitlines():
            if "exec /pgdt parse" in line:
                with self.subTest(line=line):
                    self.assertNotIn("VmHWM", line)

    def test_the_wrap_recipe_stops_reports_resumes_and_compares(self):
        wrap = self._recipe(wrap=True)
        for fragment in ("nerdctl stop", "info --cache", "--detail", "cmp "):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, wrap)

    def test_the_wrap_resumes_the_identical_command(self):
        wrap = self._recipe(wrap=True)
        legs = [ln for ln in wrap.splitlines() if "exec /pgdt parse" in ln]
        self.assertEqual(len(legs), 2)
        self.assertEqual(legs[0], legs[1])

    def test_the_scan_states_its_worker_count(self):
        # Nothing this module builds inherits the CLI's `--jobs` default, koji
        # included — a 784 GB scan that cannot say how many workers read it is
        # not a regression check against anything.
        self.assertIn(f"--jobs {shapes.SWEEP_JOBS}", self._recipe())

    def test_the_worker_count_is_the_callers_to_state(self):
        # koji's parallel leg is a leg at some count against a serial one, so
        # the count is a parameter here where every other invocation pins it.
        self.assertIn("--jobs 8", self._recipe(jobs=8))
        self.assertNotIn("--jobs 1 ", self._recipe(jobs=8))

    def test_the_wrap_legs_agree_on_the_count(self):
        # Resuming under a different arrangement is a second variable in a
        # check that has one. `test_the_wrap_resumes_the_identical_command`
        # holds the whole line; this says which part of it matters.
        legs = [ln for ln in self._recipe(wrap=True, jobs=8).splitlines() if "--jobs" in ln]
        self.assertEqual(len(legs), 2)
        for line in legs:
            with self.subTest(line=line):
                self.assertIn("--jobs 8", line)

    def test_every_continued_line_carries_its_continuation(self):
        # A dropped trailing backslash silently splits one command into two.
        for recipe in (self._recipe(), self._recipe(wrap=True)):
            lines = recipe.splitlines()
            for i, line in enumerate(lines[:-1]):
                if line.strip().startswith("-v ") and not lines[i + 1].strip().startswith("-v "):
                    with self.subTest(line=line):
                        self.assertTrue(line.rstrip().endswith("\\"), line)


if __name__ == "__main__":
    unittest.main()
