"""Tests of `measurement.emit`."""

from __future__ import annotations

import inspect
import unittest
import unittest.mock

from measurement import emit
from measurement import replay
from measurement.figures import register


class StampingTheDocument(unittest.TestCase):
    """Which sittings may re-stamp `measurements.md`.

    The defect: `emit` decided this from the *selection*, before the first
    reading, so a sweep that selected 22 figures and took 20 emitted a full
    session stamp claiming every figure below came from it — over 20 tables,
    with the other two still standing in the document from an older sitting the
    stamp had just claimed."""

    def test_a_whole_sweep_that_took_everything_stamps(self):
        self.assertTrue(emit.stamps_the_document(True, []))

    def test_a_whole_sweep_that_lost_a_figure_does_not(self):
        self.assertFalse(
            emit.stamps_the_document(True, [("per-block-quadratic", "exited 2")])
        )

    def test_a_selection_short_of_the_sweep_never_stamps(self):
        self.assertFalse(emit.stamps_the_document(False, []))
        self.assertFalse(emit.stamps_the_document(False, [("x", "boom")]))

    def test_emit_decides_it_after_the_figure_loop_not_before(self):
        """The predicate has to be evaluated where `failures` is populated.

        Asserted on the source because the ordering *is* the fix: computing it
        early type-checks, runs, and reproduces the defect exactly."""
        source = inspect.getsource(emit.emit)
        self.assertLess(
            source.index("failures.append"),
            source.index("stamps_the_document"),
            "emit decides the stamp before a figure can have failed",
        )


class CensoredSittingsBarPublication(unittest.TestCase):
    """What `emit` does with a figure that lost a leg, and what `--render`
    reproduces."""

    def test_emit_bars_publication_and_names_the_figure(self):
        source = inspect.getsource(emit.emit)
        self.assertIn("NOT PUBLISHABLE — a leg was OOM-killed", source)
        self.assertIn("censored_note(kills)", source)
        self.assertIn("failures or censored", source)

    def test_an_untaken_instruments_sitting_can_be_re_rendered(self):
        """`render` reads `SELECTABLE_BY_ID`, which is wider than `FIGURES`.

        It looked its figures up in `FIGURES`, so a diagnostic sitting of an
        untaken instrument came back "unknown figure" — and an untaken
        instrument is the case that needs re-rendering most, being taken
        repeatedly while its renderer is still being written. `UNTAKEN` is
        empty today, so nothing in the register exercises the difference; the
        past sittings that need re-rendering are exactly the ones taken while
        it was not."""
        self.assertIn("by_id = register.SELECTABLE_BY_ID", inspect.getsource(emit.render))
        self.assertEqual(
            {f.id for f in register.SELECTABLE},
            {f.id for f in register.FIGURES} | {f.id for f in register.UNTAKEN},
        )

    def test_a_sitting_records_its_kills_so_a_render_reproduces_the_cells(self):
        # The same reconciliation `test_a_sitting_records_what_a_render_needs`
        # makes for every other recorded field: `render` reads `killed` off the
        # sitting, so `emit` must write it.
        self.assertIn('"killed": session.killed', inspect.getsource(emit.emit))
        self.assertIn('raw.get("killed"', inspect.getsource(replay.ReplaySession.__init__))
        self.assertIn("censored_note(kills)", inspect.getsource(emit.render))


if __name__ == "__main__":
    unittest.main()
