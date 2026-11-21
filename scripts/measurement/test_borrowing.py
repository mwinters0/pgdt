"""Tests of `measurement.borrowing`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import borrowing
from measurement import runspec
from measurement import test_support
from measurement import timing
from measurement.figures import nested
from measurement.figures import register
from measurement.figures import throughput
from measurement.staleness import doc_markers
from measurement.staleness import outside_sections


class BorrowGraph(unittest.TestCase):
    """The sharing relation, declared on `Figure` and read by the harness.

    Its whole reason to be declared is the *transitive* case: a note written at
    a `session.borrow` call site names the source it just asked for, and behind
    the reserve table that is one figure where the honest set is two."""

    def _session(self, readings=None, rss=None):
        session = timing.Session.__new__(timing.Session)
        session.readings = dict(readings or {})
        session.rss = dict(rss or {})
        return session

    def test_requires_is_read_off_the_declared_borrows(self):
        # One list, not two: a second declaration of the same fact drifts, and
        # the one that drifts is the one no run function reads.
        fig = register.FIGURES_BY_ID["allocator"]
        self.assertEqual(
            fig.requires, ("scan-throughput-warm", "nested-end-to-end")
        )

    def test_every_borrow_names_a_known_figure(self):
        for fig in register.EVERY_FIGURE:
            for shared in fig.shares:
                with self.subTest(figure=fig.id, source=shared.source):
                    self.assertIn(shared.source, register.EVERY_BY_ID)
                    self.assertTrue(shared.what)

    def test_every_republished_spec_is_one_its_source_takes(self):
        """A borrow is a dictionary lookup that silently returns nothing.

        So every declared key is checked against the specs the source figure
        actually sweeps — the failure it guards against is a rename on one side
        producing a table that measures its own reference column and says it
        shared it."""
        taken = {
            "scan-throughput-warm": {
                spec.key("scan-throughput-warm") for spec in throughput._throughput_specs("warm")
            },
            "nested-end-to-end": {
                spec.key("nested-end-to-end") for spec in nested._nested_specs()
            },
            "per-block-quadratic": {
                runspec.RunSpec(
                    "pgdt", name, "parse-cache-out", "warm", ""
                ).key("per-block-quadratic")
                for name, _ in register._QUADRATIC_ROWS
            },
            # The two resident figures borrow from this one, which is what made
            # `Session.borrow` carry an RSS reading. Both are keyed against the
            # same `parse-rss` rows, so a rename on either side reads here as a
            # figure measuring its own reference cell and calling it shared.
            "peak-rss": {spec.key("peak-rss") for spec in test_support._peak_rss_specs()},
        }
        for fig in register.EVERY_FIGURE:
            for shared in fig.shares:
                for spec in shared.republished:
                    with self.subTest(figure=fig.id, spec=spec.key(shared.source)):
                        self.assertIn(shared.source, taken)
                        self.assertIn(spec.key(shared.source), taken[shared.source])

    def test_the_closure_is_transitive(self):
        # `reserve` borrows `peak-rss` alone, and `rss-attribution` borrows the
        # same readings in turn, so re-taking `reserve` moves all three tables.
        self.assertEqual(register.FIGURES_BY_ID["reserve"].requires, ("peak-rss",))
        self.assertEqual(
            borrowing.sharing_closure("reserve"), ["peak-rss", "rss-attribution"]
        )

    def test_the_closure_is_symmetric(self):
        for fig in register.EVERY_FIGURE:
            for other in borrowing.sharing_closure(fig.id):
                with self.subTest(figure=fig.id, other=other):
                    self.assertIn(fig.id, borrowing.sharing_closure(other))

    def test_a_consumed_reading_is_not_a_closure_edge(self):
        # `cross-file-floor` differences the nested sweep's reps into a per-row
        # cost. That is a derived quantity, not the nested table's number a
        # second time, so re-taking one does not put two numbers in the doc for
        # one measurement -- but it still orders the run.
        self.assertNotIn("cross-file-floor", borrowing.sharing_closure("nested-end-to-end"))
        self.assertEqual(
            register.FIGURES_BY_ID["cross-file-floor"].requires, ("nested-end-to-end",)
        )

    def test_a_satisfied_borrow_is_copied_and_said_to_be_shared(self):
        source = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        readings = {
            spec.key(shared.source): [1.0, 2.0]
            for shared in register.FIGURES_BY_ID["allocator"].shares
            for spec in shared.republished
        }
        session = self._session(readings)
        note = borrowing.share_readings(session, "allocator")
        self.assertEqual(session.readings[source.key("allocator")], [1.0, 2.0])
        self.assertIn("Shared, not measured again", note)
        self.assertNotIn("Partial sweep", note)

    def test_a_resident_set_crosses_the_share_with_the_wall_clock(self):
        """The half `reserve`'s borrow needs, and the half that fails as a `KeyError`.

        `has` reads `readings`, so a borrow that copied the duration alone
        would report the spec satisfied and then leave the borrowing figure's
        renderer asking `get_rss` for a key nothing wrote — an hour into a
        sweep, not at the declaration."""
        source = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        session = self._session(
            {source.key("scan-throughput-warm"): [1.0, 2.0]},
            {source.key("scan-throughput-warm"): [5.9, 6.0]},
        )
        borrowing.share_readings(session, "allocator")
        self.assertEqual(session.rss[source.key("allocator")], [5.9, 6.0])

    def test_a_source_with_no_resident_reading_leaves_the_key_absent(self):
        # An absent `rss` key means "this shape carries no RSS wrapper", which
        # `sweep` is careful to distinguish from an empty one. Manufacturing an
        # empty list here would turn the first fact into the second.
        source = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        session = self._session({source.key("scan-throughput-warm"): [1.0, 2.0]})
        borrowing.share_readings(session, "allocator")
        self.assertNotIn(source.key("allocator"), session.rss)

    def test_an_all_killed_source_leg_crosses_as_an_empty_list(self):
        # The other side of the same distinction: the source opened the key and
        # every rep was censored. That is a fact about the leg and it travels.
        source = runspec.RunSpec("pgdt", "control", "parse", "warm", "")
        session = self._session(
            {source.key("scan-throughput-warm"): [1.0]},
            {source.key("scan-throughput-warm"): []},
        )
        borrowing.share_readings(session, "allocator")
        self.assertEqual(session.rss[source.key("allocator")], [])

    def test_no_republished_spec_is_an_instrument_leg(self):
        """Why `borrow` copies two channels and not four.

        `reported` and `instrument` are read by the figure that *declared* the
        leg, and `RunSpec.instrument` is that declaration rather than a
        property of the run. A borrow that carried them would hand a borrower a
        report it never asked the harness to find — sound only while no shared
        spec is such a leg, which is what this holds."""
        for fig in register.EVERY_FIGURE:
            for shared in fig.shares:
                for spec in shared.republished:
                    with self.subTest(figure=fig.id, spec=spec.key(shared.source)):
                        self.assertFalse(spec.instrument)

    def test_an_unsatisfied_borrow_names_the_whole_closure(self):
        note = borrowing.share_readings(self._session(), "allocator")
        self.assertIn("**Partial sweep**", note)
        for fid in borrowing.sharing_closure("allocator"):
            with self.subTest(figure=fid):
                self.assertIn(fid, note)

    def test_nothing_is_faked_for_an_unsatisfied_borrow(self):
        # The figure measures it instead, which is what the note discloses.
        session = self._session()
        borrowing.share_readings(session, "allocator")
        self.assertEqual(session.readings, {})

    def test_closure_gaps_name_what_a_selection_leaves_out(self):
        # Selection pulls `peak-rss` in, which `reserve` borrows; the table
        # borrowing the same readings is what it leaves out.
        gaps = borrowing.closure_gaps(outside_sections.resolve_selection(["reserve"]))
        self.assertTrue(any(g.startswith("reserve —") for g in gaps))
        self.assertIn("rss-attribution", " ".join(gaps))

    def test_a_whole_closure_leaves_no_gap(self):
        ids = ["allocator", *borrowing.sharing_closure("allocator")]
        self.assertEqual(borrowing.closure_gaps(outside_sections.resolve_selection(ids)), [])

    def test_the_derived_direction_is_read_off_the_same_declaration(self):
        # The reverse of a non-republishing share, which the closure does not
        # carry: `cross-file-floor`'s row 1 is a difference over the nested
        # sweep's reps, so re-taking that sweep strands it.
        self.assertEqual(
            borrowing.derived_consumers("nested-end-to-end"),
            [("cross-file-floor", "row 1's per-rep differences")],
        )

    def test_the_edge_names_both_ends_and_the_quantity(self):
        # What `--check` prints: the consumer, the source whose reps it reads,
        # and what it makes of them.
        self.assertEqual(
            borrowing.derivation_edges(),
            [("cross-file-floor", "nested-end-to-end", "row 1's per-rep differences")],
        )

    def test_a_republished_share_is_not_a_derivation(self):
        # `allocator` publishes `scan-throughput-warm`'s reading as its own
        # number, which is the closure's business and not this edge's.
        self.assertEqual(borrowing.derived_consumers("scan-throughput-warm"), [])

    def test_re_taking_a_source_alone_names_the_table_it_strands(self):
        gaps = borrowing.derivation_gaps(outside_sections.resolve_selection(["nested-end-to-end"]))
        self.assertEqual(len(gaps), 1)
        self.assertIn("nested-end-to-end", gaps[0])
        self.assertIn("cross-file-floor", gaps[0])

    def test_taking_the_consumer_too_leaves_no_derivation_gap(self):
        selection = outside_sections.resolve_selection(["cross-file-floor"])
        self.assertIn("nested-end-to-end", [f.id for f in selection])
        self.assertEqual(borrowing.derivation_gaps(selection), [])

    def test_the_forward_direction_was_never_the_hazard(self):
        # Selection reads every share, republished or not, so a consumer always
        # drags its source in. Only the reverse needed naming.
        self.assertEqual(borrowing.derivation_gaps(outside_sections.resolve_selection(["map-only"])), [])

    def test_alone_takes_exactly_what_is_named(self):
        got = [f.id for f in outside_sections.resolve_selection(["allocator"], alone=True)]
        self.assertEqual(got, ["allocator"])

    def test_alone_refuses_a_reading_it_cannot_measure_for_itself(self):
        with self.assertRaises(SystemExit):
            outside_sections.resolve_selection(["cross-file-floor"], alone=True)

    def test_a_partial_note_is_attributed_to_the_marker_above_it(self):
        text = (
            "<!-- figure: scan-throughput-warm -->\nnothing here\n"
            "<!-- figure: allocator -->\n**Partial sweep**: measured here.\n"
        )
        self.assertEqual(
            doc_markers.partial_sittings(text),
            [("allocator", borrowing.sharing_closure("allocator"))],
        )

    def test_a_doc_with_no_partial_note_reports_none(self):
        self.assertEqual(doc_markers.partial_sittings("<!-- figure: allocator -->\n"), [])


if __name__ == "__main__":
    unittest.main()
