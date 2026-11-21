"""Tests of `measurement.replay`."""

from __future__ import annotations

import contextlib
import inspect
import io
import json
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import emit
from measurement import replay
from measurement import runspec
from measurement import staging


class Render(unittest.TestCase):
    """`--render` rebuilds a sitting's tables from its `raw.json`, measuring
    nothing — which is how a presentation-only renderer change is folded in."""

    def test_a_replay_session_refuses_to_measure(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = {"readings": {}, "input_sizes": {}, "runs": []}
            session = replay.ReplaySession(
                config.Config(), raw, Path(tmp), lambda _m: None
            )
            spec = runspec.RunSpec(
                binary="pgdt", input="control", command="parse", regime="warm", label="x"
            )
            with self.assertRaises(AssertionError):
                session.take(spec, 0)
            with self.assertRaises(AssertionError):
                session.drop_caches()
            # A sweep is a no-op rather than an error: a renderer calls it, and
            # the readings it would take are already loaded.
            self.assertIsNone(session.sweep("f", [spec], 5))

    def test_a_staged_input_is_a_sparse_file_of_the_recorded_size(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = {"readings": {}, "input_sizes": {"control": 4096}, "runs": []}
            session = replay.ReplaySession(
                config.Config(), raw, Path(tmp), lambda _m: None
            )
            path = session.input_path("control", "warm")
            self.assertEqual(path.stat().st_size, 4096)
            self.assertEqual(
                staging.file_size(config.Config(), path, "control"), 4096
            )

    def test_an_input_the_sitting_never_sized_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(), {"readings": {}, "input_sizes": {}, "runs": []},
                Path(tmp), lambda _m: None,
            )
            with self.assertRaises(KeyError):
                session.input_path("control", "warm")

    def test_a_sitting_without_the_recorded_fields_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "raw.json").write_text(json.dumps({"figures": [], "commit": "abc"}))
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = emit.render(config.Config(), run_dir)
            self.assertEqual(rc, 2)
            self.assertIn("predates --render", err.getvalue())

    def test_a_missing_raw_json_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = emit.render(config.Config(), Path(tmp))
            self.assertEqual(rc, 2)

    def test_a_sitting_records_what_a_render_needs(self):
        # The fields `render` reads must be the fields `emit` writes. Both
        # lists live in the source, so a field added to one and not the other
        # is caught here rather than at the next fold-in.
        source = inspect.getsource(emit.emit)
        for field in (
            "allocator",
            "glibc",
            "figure_glibc",
            "whole_sweep",
            "header",
            "input_sizes",
            "rss",
            "reported",
        ):
            self.assertIn(f'"{field}"', source, f"emit does not record {field}")

    def test_a_render_skips_the_figures_the_sitting_failed(self):
        """A failed figure took no readings, so replaying it raises rather than
        rebuilding a table — and the sitting's own record says which those
        were."""
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "raw.json").write_text(
                json.dumps(
                    {
                        "commit": "abcdef1",
                        "figures": ["per-block-quadratic"],
                        "failures": [["per-block-quadratic", "before one_block exited 2"]],
                        "whole_sweep": False,
                        "header": ["# measure.py output"],
                        "allocator": None,
                        "input_sizes": {},
                        "readings": {},
                        "rss": {},
                        "reported": {},
                        "runs": [],
                    }
                )
            )
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = emit.render(config.Config(), run_dir)
            self.assertEqual(rc, 0)
            self.assertIn("0 figure(s)", out.getvalue())


if __name__ == "__main__":
    unittest.main()
