"""Tests of `measurement.preflight`."""

from __future__ import annotations

import contained
import dataclasses
import inspect
import re
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import emit
from measurement import parsing
from measurement import preflight
from measurement.builds import image
from measurement.figures import register


class ApparatusPreflight(unittest.TestCase):
    """The pinned image is the release image's base, so a binary built
    anywhere else may not load in it: before the first reading, `pgdt
    --version` and `peak-rss` around `/bin/true` are started in it, and a
    sweep whose binary does not start there is refused, naming the pin and
    the build that does."""

    LOADER = "/pgdt: /usr/lib/libm.so.6: version `GLIBC_2.45' not found (required by /pgdt)\n"

    def _preflight(self, outcomes):
        """`apparatus_preflight` with each container start answered in turn."""
        started, logged = [], []

        def fake_run(argv, **_kwargs):
            started.append(list(argv))
            code, err = outcomes[len(started) - 1]
            return subprocess.CompletedProcess(argv, code, "", err)

        with unittest.mock.patch.object(subprocess, "run", fake_run), \
                unittest.mock.patch.object(
                    image, "ensure_peak_rss_binary", return_value=Path("/x/peak-rss")
                ):
            problems = preflight.apparatus_preflight(config.Config(), logged.append)
        return problems, started, logged

    def test_both_start_in_the_pinned_image(self):
        problems, started, logged = self._preflight([(0, ""), (0, "maxrss_kib=1024\n")])
        self.assertEqual(problems, [])
        cfg = config.Config()
        self.assertEqual([argv[-2:] for argv in started],
                         [["/pgdt", "--version"], [parsing.PEAK_RSS, "/bin/true"]])
        for argv in started:
            with self.subTest(argv=argv):
                self.assertIn(cfg.image, argv)
                self.assertIn(f"{cfg.bin_pgdt}:/pgdt:ro", argv)
                self.assertIn(f"/x/peak-rss:{parsing.PEAK_RSS}:ro", argv)
        # The floor is logged, and it is no reading of any figure.
        self.assertTrue(any("1.00 MiB" in line and "never published" in line for line in logged))

    def test_a_binary_the_image_cannot_load_is_refused_naming_the_pin(self):
        problems, _, _ = self._preflight([(127, self.LOADER), (0, "maxrss_kib=1024\n")])
        self.assertEqual(len(problems), 1)
        self.assertIn(config.Config().image, problems[0])
        self.assertIn("GLIBC_2.45", problems[0])
        self.assertIn("release image", problems[0])
        self.assertIn("PGDT_MEASURE_BIN", problems[0])

    def test_a_wrapper_that_reports_nothing_is_refused(self):
        problems, _, _ = self._preflight([(0, ""), (0, "")])
        self.assertEqual(len(problems), 1)
        self.assertIn("peak-rss", problems[0])

    def test_a_wrapper_that_cannot_be_built_is_a_problem_not_a_crash(self):
        refused = RuntimeError("`rustup target add x86_64-unknown-linux-musl`")
        with unittest.mock.patch.object(image, "ensure_peak_rss_binary", side_effect=refused):
            problems = preflight.apparatus_preflight(config.Config(), lambda _m: None)
        self.assertEqual(len(problems), 1)
        self.assertIn("rustup target add", problems[0])

    def test_a_dry_run_starts_nothing(self):
        with unittest.mock.patch.object(subprocess, "run") as started:
            problems = preflight.apparatus_preflight(config.Config(dry_run=True), lambda _m: None)
        self.assertEqual(problems, [])
        started.assert_not_called()

    def test_the_sweep_refuses_before_its_first_reading(self):
        # Among the problems `emit` refuses on before anything is run.
        source = inspect.getsource(emit.emit)
        self.assertIn("problems += preflight.apparatus_preflight(cfg, log)", source)
        self.assertLess(
            source.index("apparatus_preflight(cfg, log)"), source.index("Session(cfg, stager")
        )


class MemoryPreflight(unittest.TestCase):
    """Each container a figure's legs run in is held to the ceiling
    `contained.py` holds its own `--memory` to, before the first reading."""

    #: The machine the register is taken on: `MemTotal`, 31.26 GiB.
    HOST = 32_773_996 * 1024

    def _figure(self, memory):
        return dataclasses.replace(register.FIGURES_BY_ID["long-line-resident"], memory=memory)

    def test_a_figure_s_container_past_half_the_machine_is_refused_by_name(self):
        problems = preflight.memory_preflight(config.Config(), [self._figure("16g")], self.HOST)
        self.assertEqual(len(problems), 1)
        self.assertIn("long-line-resident's container 16g", problems[0])
        self.assertIn(f"{contained.MAX_FRACTION:.0%}", problems[0])

    def test_one_under_half_passes(self):
        self.assertEqual(
            preflight.memory_preflight(config.Config(), [self._figure("15g")], self.HOST), []
        )

    def test_the_sweep_s_own_container_is_held_too(self):
        problems = preflight.memory_preflight(
            config.Config(memory="20g"), [self._figure(None)], self.HOST
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("PGDT_MEASURE_MEMORY", problems[0])

    def test_every_figure_fits_the_register_s_machine(self):
        # Every container a figure states runs on the machine its table is
        # taken on; `long-line-resident`'s is the largest.
        self.assertEqual(
            preflight.memory_preflight(config.Config(), register.EVERY_FIGURE, self.HOST), []
        )

    def test_the_sweep_refuses_before_its_first_reading(self):
        source = inspect.getsource(emit.emit)
        self.assertIn("problems += preflight.memory_preflight(cfg, figures, contained.machine_bytes())", source)
        self.assertLess(
            source.index("memory_preflight(cfg, figures"), source.index("Session(cfg, stager")
        )


class MountPreflight(unittest.TestCase):
    """A container reads its mounts from the runtime's mount namespace, not
    the harness's, so before the first reading a nonce is written into each
    staging directory and read back inside a container: a directory whose
    probe the container does not see is refused, naming it."""

    def _preflight(self, dirs, hidden=(), code=0):
        """`mount_preflight` over `dirs`, the container answering by reading
        each mounted probe off the host — except under `hidden`, which it
        sees as another namespace's copy would: without the probe."""
        started = []

        def fake_run(argv, **_kwargs):
            started.append(list(argv))
            mounts = [a.split(":")[0] for a, prev in zip(argv[1:], argv) if prev == "-v"]
            name = re.search(r"cat /probe/0/(\S+)", argv[-1]).group(1)
            lines = [
                "" if Path(m) in hidden else (Path(m) / name).read_text() for m in mounts
            ]
            return subprocess.CompletedProcess(argv, code, "\n".join(lines) + "\n", "no such image")

        with unittest.mock.patch.object(subprocess, "run", fake_run):
            problems = preflight.mount_preflight(config.Config(), dirs)
        return problems, started

    def test_directories_the_container_sees_pass_and_keep_no_probe(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            dirs = [Path(a), Path(b), Path(a)]
            problems, started = self._preflight(dirs)
            self.assertEqual(problems, [])
            self.assertEqual(len(started), 1, "one container start for every directory")
            self.assertIn(config.Config().image, started[0])
            self.assertEqual(sum(arg == "-v" for arg in started[0]), 2, "each directory once")
            self.assertEqual(list(Path(a).iterdir()) + list(Path(b).iterdir()), [])

    def test_a_directory_the_container_sees_another_copy_of_is_refused(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            problems, _ = self._preflight([Path(a), Path(b)], hidden={Path(b)})
            self.assertEqual(len(problems), 1)
            self.assertIn(b, problems[0])
            self.assertIn("private /tmp or /var/tmp", problems[0])
            self.assertEqual(list(Path(b).iterdir()), [], "the probe is removed either way")

    def test_a_probe_that_cannot_start_is_one_problem(self):
        with tempfile.TemporaryDirectory() as a:
            problems, _ = self._preflight([Path(a)], code=125)
            self.assertEqual(len(problems), 1)
            self.assertIn("exit 125", problems[0])

    def test_a_dry_run_starts_nothing(self):
        with unittest.mock.patch.object(subprocess, "run") as started:
            problems = preflight.mount_preflight(config.Config(dry_run=True), [Path("/nonexistent")])
        self.assertEqual(problems, [])
        started.assert_not_called()

    def test_the_sweep_probes_every_staging_area_before_its_first_reading(self):
        source = inspect.getsource(emit.emit)
        for area in ("cfg.cache_dir", "cfg.warm_dir", "cfg.nvme_dir"):
            self.assertIn(area, source[source.index("mount_preflight("):])
        self.assertLess(source.index("mount_preflight("), source.index("Session(cfg, stager"))


if __name__ == "__main__":
    unittest.main()
