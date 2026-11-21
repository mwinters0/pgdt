"""Tests of `measurement.staleness.sitting_checks`."""

from __future__ import annotations

import contextlib
import io
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import measure
from measurement import borrowing
from measurement import config
from measurement import emit
from measurement.figures import register
from measurement.staleness import doc_markers
from measurement.staleness import sitting_checks


class Sittings(unittest.TestCase):
    """A figure may be published outside the sweep, and then its own marker
    carries the commit it was taken at. Every reader of the session stamp
    argues from that commit instead — half-applying it leaves a mechanism
    reasoning from a commit the doc itself says is not the figure's.

    **The cases below fabricate a sitting and name a real register entry as
    their example, so that entry's other properties are premises they do not
    state.** `test_the_unentangled_example_is_still_unentangled` states them
    instead, and is what a register move fails against — asserting the premise
    beside the test is the durable form of that lesson, because the failure
    then names the premise that went rather than a refusal these cases were
    never testing. A rule saying so elsewhere would be a third copy enforced by
    nothing. The shape is specific: it bites where an assertion picks **one
    element of a multi-cause result**, `sitting_problems` being the only such
    producer here — an example whose property is asserted whole, as at
    `test_a_figure_with_no_nvme_inputs_checks_nothing`, fails legibly on its
    own."""

    DOC = config.REPO / "docs/design/measurements.md"

    def test_the_emitted_marker_declares_the_sitting_and_is_read_back(self):
        marker = doc_markers.figure_marker("peak-rss", "7ee5db5")
        self.assertEqual(doc_markers.figure_sittings(marker), {"peak-rss": "7ee5db5"})
        self.assertEqual(doc_markers.MARKER_RE.findall(marker), ["peak-rss"])

    def test_a_figure_from_the_sweep_declares_nothing(self):
        # The datum is present only where it differs from the stamp: a figure
        # that declares nothing came from the sweep.
        self.assertEqual(doc_markers.figure_sittings(doc_markers.figure_marker("map-only")), {})

    def test_the_stamp_s_own_scope_clause_is_not_a_sitting(self):
        stamp = emit.session_stamp("deadbee", dirty=False)
        self.assertEqual(doc_markers.figure_sittings(stamp), {})

    def test_a_sitting_is_read_only_out_of_its_own_marker(self):
        # A commit named in the prose *under* a figure is not its provenance;
        # the marker is, which is why the datum went inside it.
        text = (
            "<!-- figure: peak-rss — reproduce with `x` -->\n\n"
            "This figure was taken alone, at commit `7ee5db5`.\n"
        )
        self.assertEqual(doc_markers.figure_sittings(text), {})

    def test_the_doc_carries_exactly_the_sittings_the_register_permits(self):
        problems = sitting_checks.sitting_problems(
            doc_markers.figure_sittings(self.DOC.read_text()),
            doc_markers.stamped_commit(self.DOC),
        )
        self.assertEqual(problems, [])

    def test_a_base_is_the_figure_s_own_sitting_where_it_declares_one(self):
        text = emit.session_stamp("aaaaaaa", dirty=False) + "\n" + doc_markers.figure_marker(
            "peak-rss", "bbbbbbb"
        )
        bases = sitting_checks.figure_bases(text)
        self.assertEqual(bases["peak-rss"], "bbbbbbb")
        self.assertEqual(bases["map-only"], "aaaaaaa")
        self.assertEqual(set(bases), set(register.ALL_BY_ID))

    def test_an_explicit_since_overrides_every_figure(self):
        # `--since` asks one deliberate question of the whole document.
        text = emit.session_stamp("aaaaaaa", dirty=False) + "\n" + doc_markers.figure_marker(
            "peak-rss", "bbbbbbb"
        )
        bases = sitting_checks.figure_bases(text, "ccccccc")
        self.assertEqual(set(bases.values()), {"ccccccc"})

    def test_an_unstamped_doc_leaves_a_figure_with_no_base(self):
        bases = sitting_checks.figure_bases("# Measurements\n")
        self.assertIsNone(bases["map-only"])

    def test_a_figure_that_shares_a_reading_may_not_be_published_alone(self):
        # What one sitting buys is differencing, so the condition is the borrow
        # graph: `allocator`'s reference column *is* two other tables' rows.
        self.assertEqual(borrowing.entangled_with("map-only"), [])
        self.assertIn("scan-throughput-warm", borrowing.entangled_with("allocator"))

    def test_the_unentangled_example_is_still_unentangled(self):
        """`map-only` is the figure the fabricated sittings below are written
        against, and it is load-bearing that it stands in no edge.

        `peak-rss` used to be that figure, and the register move that put
        `reserve` and `rss-attribution` into `FIGURES` gave it two — so seven
        assertions started failing on a refusal that fired before the one they
        were checking, which reads as a doc problem and is not one. Asserting
        the premise here is what makes the next such move fail with a sentence
        that says which premise went."""
        self.assertEqual(borrowing.entangled_with("map-only"), [])
        self.assertEqual(
            borrowing.entangled_with("peak-rss"), ["reserve", "rss-attribution"]
        )

    def test_a_derivation_entangles_in_both_directions(self):
        # Not a closure edge, and still an edge: `cross-file-floor`'s first row
        # is a difference over `nested-end-to-end`'s reps.
        self.assertIn("cross-file-floor", borrowing.entangled_with("nested-end-to-end"))
        self.assertIn("nested-end-to-end", borrowing.entangled_with("cross-file-floor"))

    def test_an_entangled_sitting_is_refused_by_the_doc_and_by_the_run(self):
        problems = sitting_checks.sitting_problems({"allocator": "bbbbbbb"}, "aaaaaaa")
        self.assertEqual(len(problems), 1)
        self.assertIn("scan-throughput-warm", problems[0])
        refusals = borrowing.publication_refusals([register.ALL_BY_ID["allocator"]])
        self.assertEqual(len(refusals), 1)
        self.assertIn("only a sweep", refusals[0])

    def test_a_figure_standing_in_no_edge_may_be_taken_on_its_own(self):
        self.assertEqual(borrowing.publication_refusals([register.ALL_BY_ID["map-only"]]), [])

    def _cli_stderr(self, argv: list[str]) -> str:
        """`main` up to its first refusal, with nothing measured.

        `emit` is stubbed because the question is which refusals fire before
        it; a missing release binary is a refusal of its own and is allowed to
        be the one that ends the run."""
        err = io.StringIO()
        with unittest.mock.patch.object(emit, "emit", return_value=0):
            with contextlib.redirect_stderr(err), contextlib.suppress(SystemExit):
                measure.main(argv)
        return err.getvalue()

    def test_an_entangled_figure_is_refused_before_the_measurement_is_spent(self):
        self.assertIn(
            "publish outside the document's session stamp",
            self._cli_stderr(["--figure", "allocator"]),
        )

    def test_the_refusal_stops_firing_for_a_diagnostic_sitting(self):
        # Not an exemption clause: `--alone` marks the run unpublishable, so
        # the guard -- which asks only of a publishable run -- has no
        # publication left to refuse.
        self.assertNotIn(
            "publish outside the document's session stamp",
            self._cli_stderr(["--figure", "allocator", "--alone"]),
        )

    def test_a_diagnostic_sitting_is_not_told_to_fold_its_tables_in(self):
        reason = config.Config(alone=True).unpublishable_reason
        lead = emit.partial_lead(1, "deadbee", dirty=False, allocator=None, unpublishable=reason)
        self.assertIn("do not enter the document", lead)
        self.assertNotIn("fold each table in", lead)

    def test_a_publishable_partial_sitting_still_says_how_to_fold_it_in(self):
        lead = emit.partial_lead(1, "deadbee", dirty=False, allocator=None, unpublishable=None)
        self.assertIn("fold each table in", lead)
        self.assertIn("does not stamp the document", lead)

    def test_a_sitting_that_repeats_the_stamp_is_a_marker_that_should_not_be_there(self):
        problems = sitting_checks.sitting_problems(
            {"map-only": "aaaaaaa"},
            "aaaaaaa",
            resolve=lambda rev: rev * 5,
            ancestor=lambda a, b: True,
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("stamp's own commit", problems[0])

    def test_a_sitting_older_than_the_stamp_is_refused(self):
        # An older sitting cannot legitimately exist — a sweep replaces every
        # table at once — so it is a marker a sweep left behind or a hand edit,
        # and either puts --stale back on the wrong commit.
        problems = sitting_checks.sitting_problems(
            {"map-only": "bbbbbbb"},
            "aaaaaaa",
            resolve=lambda rev: rev * 5,
            ancestor=lambda a, b: False,
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("does not descend", problems[0])

    def test_a_descendant_sitting_passes(self):
        self.assertEqual(
            sitting_checks.sitting_problems(
                {"map-only": "bbbbbbb"},
                "aaaaaaa",
                resolve=lambda rev: rev * 5,
                ancestor=lambda a, b: True,
            ),
            [],
        )

    def test_a_sitting_naming_no_commit_is_refused(self):
        problems = sitting_checks.sitting_problems(
            {"map-only": "bbbbbbb"}, "aaaaaaa", resolve=lambda rev: None
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("not a commit", problems[0])

    def test_a_sitting_with_no_stamp_to_be_outside_of_is_refused(self):
        problems = sitting_checks.sitting_problems({"map-only": "bbbbbbb"}, None)
        self.assertEqual(len(problems), 1)
        self.assertIn("session stamp names no commit", problems[0])

    def test_the_accounting_sentence_is_generated_not_counted(self):
        whole = sitting_checks.sitting_accounting([])
        self.assertIn(f"All {len(register.ALL_FIGURES)} figures", whole)
        one = sitting_checks.sitting_accounting([("peak-rss", "7ee5db5")])
        self.assertIn(f"{len(register.ALL_FIGURES) - 1} of the {len(register.ALL_FIGURES)}", one)
        self.assertIn("`peak-rss` (`7ee5db5`)", one)
        self.assertIn("The other carries its own", one)
        two = sitting_checks.sitting_accounting([("a", "1234567"), ("b", "2345678")])
        self.assertIn("The other 2 carry their own", two)

    def test_the_doc_carries_the_sentence_the_harness_generates(self):
        # Hand-maintaining the count is what went wrong silently: the sentence
        # claiming every marker was false for as long as one figure stood
        # outside the sweep and said so only in prose.
        text = self.DOC.read_text()
        wanted = sitting_checks.sitting_accounting(sorted(doc_markers.figure_sittings(text).items()))
        self.assertIn(" ".join(wanted.split()), " ".join(text.split()))

    def test_the_stamp_carries_that_sentence_too(self):
        stamp = emit.session_stamp("deadbee", dirty=False, outside=[("peak-rss", "7ee5db5")])
        self.assertIn(sitting_checks.sitting_accounting([("peak-rss", "7ee5db5")]), stamp)
        # And the stamp is still the one `stamped_commit` reads back: the
        # accounting's own shas must not be mistaken for the sitting's.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(stamp + "\n")
            self.assertEqual(doc_markers.stamped_commit(doc), "deadbee")


if __name__ == "__main__":
    unittest.main()
