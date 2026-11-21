"""Tests of `measurement.builds.image`."""

from __future__ import annotations

import release
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import host
from measurement import test_support
from measurement.builds import image


class ShippedBinary(unittest.TestCase):
    """The binary every `pgdt` figure is timed against, built rather than
    found.

    The failure each of these holds shut is the same one and it is silent: a
    sitting emits a full table, a session stamp naming `git rev-parse HEAD`,
    and a verdict about a library that was never executed. Only one figure in
    the register — the reserve instrument, whose resolved budgets fingerprint
    the charge model — could have noticed from its own numbers.
    """

    def setUp(self):
        # Module state, so one test's build would otherwise satisfy the next.
        image._PGDT_BUILT = False
        test_support.without_the_release_image(self)

    def _cfg(self, **kw):
        return config.Config(bin_pgdt=config.REGISTER_BIN, **kw)

    def test_the_default_binary_is_the_one_the_release_build_writes(self):
        # `ensure_pgdt_binary` compares the two to decide whether it may claim
        # to have built what it is about to time, so a `Config` default that
        # drifted from the build's output path would turn the build into a
        # no-op and put the old check back with no sign of it.
        self.assertEqual(config.Config().bin_pgdt, config.REGISTER_BIN)
        self.assertEqual(
            config.REGISTER_BIN,
            release.STATE / "target" / release.NATIVE_TARGET / "release" / "pgdt",
        )

    def test_the_shipped_binary_is_built_once_per_process(self):
        calls = []

        def fake_run(argv, cwd=None, capture=False, quiet=False):
            calls.append((list(argv), cwd))
            return ""

        with unittest.mock.patch.object(host, "run", fake_run):
            out = image.ensure_pgdt_binary(self._cfg(), lambda _: None)
            image.ensure_pgdt_binary(self._cfg(), lambda _: None)
        self.assertEqual(out, config.REGISTER_BIN)
        self.assertEqual(len(calls), 1)
        argv, cwd = calls[0]
        # The release build itself, in the release image: one target, the
        # package's default features, the release state's target directory.
        self.assertEqual(
            argv[argv.index("pgdt-release:t") + 1:],
            ["python3", "scripts/release.py", "build",
             f"--target={release.NATIVE_TARGET}", "--package=pgdt", "--inside"],
        )
        self.assertIn(f"{(release.STATE / 'target').resolve()}:/state/target", argv)
        self.assertEqual(cwd, config.REPO)

    def test_an_existing_binary_is_rebuilt_anyway(self):
        # The whole point, and it is asserted as an absence: nothing on the
        # path to the build consults the file. `target/release/pgdt` survives
        # between sessions, so short-circuiting on its existence is what timed
        # a charge model three commits stale against the harness's repaired
        # mirror of it and reported the difference as an over-bill.
        calls = []

        def fake_run(argv, cwd=None, capture=False, quiet=False):
            calls.append(list(argv))
            return ""

        def never(self):
            raise AssertionError("the build short-circuited on the file's existence")

        with unittest.mock.patch.object(host, "run", fake_run), \
             unittest.mock.patch.object(Path, "exists", never):
            image.ensure_pgdt_binary(self._cfg(), lambda _: None)
        self.assertEqual(len(calls), 1)

    def test_a_dry_run_announces_and_builds_nothing(self):
        # It measures nothing and must run where no binary exists.
        said = []

        def fake_run(argv, cwd=None, capture=False, quiet=False):
            raise AssertionError("a dry run built the binary")

        with unittest.mock.patch.object(host, "run", fake_run):
            image.ensure_pgdt_binary(self._cfg(dry_run=True), said.append)
        self.assertEqual(len(said), 1)
        self.assertIn("dry-run", said[0])

    def test_an_overridden_binary_is_never_built(self):
        # `cargo build --release` writes exactly one path, so a harness that
        # ran it and then timed some other file would be asserting a
        # provenance it does not have. There the binary is the caller's.
        def fake_run(argv, cwd=None, capture=False, quiet=False):
            raise AssertionError("the harness built a binary it does not own")

        with tempfile.TemporaryDirectory() as tmp:
            elsewhere = Path(tmp) / "pgdt"
            with unittest.mock.patch.object(host, "run", fake_run):
                out = image.ensure_pgdt_binary(
                    config.Config(bin_pgdt=elsewhere), lambda _: None
                )
            self.assertEqual(out, elsewhere)


if __name__ == "__main__":
    unittest.main()
