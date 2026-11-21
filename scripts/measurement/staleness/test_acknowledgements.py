"""Tests of `measurement.staleness.acknowledgements`."""

from __future__ import annotations

import acknowledged
import unittest
import unittest.mock

from measurement import check
from measurement.figures import register
from measurement.staleness import acknowledgements
from measurement.staleness import comments
from measurement.staleness import outside_sections


class CommentOnlyStaleness(unittest.TestCase):
    """What the oracle does to `--stale`'s two path predicates."""

    ACKS = ()

    def test_a_comment_only_commit_settles_a_path_with_no_entry(self):
        got = acknowledgements.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
            comment_only=lambda c, p: True,
        )
        self.assertEqual(got, ["scripts/generate_perf_data.py"])

    def test_a_comment_only_commit_is_not_what_holds_a_path_red(self):
        # An inert entry names the commits actually blocking it, and a commit
        # the oracle settled is not one of them.
        acks = (acknowledged.Acknowledged(commit="aaa", figures=("nested-end-to-end",), why="additive"),)
        got = acknowledgements.inert_excuses(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "bbb", "ccc"]},
            acks,
            comment_only=lambda c, p: c == "bbb",
        )
        self.assertEqual(got, [("scripts/generate_perf_data.py", ["aaa"], ["ccc"])])

    def test_the_oracle_is_on_by_default(self):
        # The weaker answer must not be reachable by forgetting an argument:
        # a caller that passes no oracle gets the real one.
        import inspect

        for fn in (acknowledgements.excused_paths, acknowledgements.inert_excuses):
            with self.subTest(fn=fn.__name__):
                default = inspect.signature(fn).parameters["comment_only"].default
                self.assertIs(default, comments.comment_only_commit)


class Acknowledgements(unittest.TestCase):
    """A commit that touched a declared path and moved no reading.

    The mechanism's whole risk is that it becomes a way to wave staleness away,
    so what these tests hold are the refusals: a dirty path is never excused, a
    path is excused only when *every* commit that touched it is, and an excuse
    is scoped to the figures it names.
    """

    ACKS = (
        acknowledged.Acknowledged(commit="aaa", figures=("nested-end-to-end",), why="additive"),
        acknowledged.Acknowledged(commit="bbb", figures=(), why="touches no figure's subject"),
    )

    def test_a_path_whose_only_commit_is_excused_is_excused(self):
        got = acknowledgements.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, ["scripts/generate_perf_data.py"])

    def test_an_excuse_does_not_reach_a_figure_it_does_not_name(self):
        got = acknowledgements.excused_paths(
            "scan-throughput-warm",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_an_empty_figure_list_excuses_every_figure(self):
        got = acknowledgements.excused_paths(
            "scan-throughput-warm",
            ["scripts/measure.py"],
            {"scripts/measure.py": ["bbb"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, ["scripts/measure.py"])

    def test_a_blanket_entry_does_not_reach_a_declared_section(self):
        # Empty means every figure and no declared section. A blanket entry is
        # written about the readings a sweep takes, which are durations; koji's
        # are a byte-for-byte comparison of what a scan concludes, so excusing
        # it has to be its author's decision rather than an inheritance.
        got = acknowledgements.excused_paths(
            "koji",
            ["scripts/measure.py"],
            {"scripts/measure.py": ["bbb"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_a_declared_section_is_excused_by_being_named(self):
        # The permission itself is unchanged: the harness cannot re-take koji,
        # so an entry naming it is the only discharge short of an hour on the HDD.
        acks = (*self.ACKS, acknowledged.Acknowledged(commit="ddd", figures=("koji",), why="comments"))
        got = acknowledgements.excused_paths(
            "koji",
            ["scripts/measure.py"],
            {"scripts/measure.py": ["ddd"]},
            set(),
            acks,
        )
        self.assertEqual(got, ["scripts/measure.py"])

    def test_one_unexamined_commit_keeps_the_path_stale(self):
        # The failure this exists against: a path changed by an excused commit
        # and an unexamined one is stale on the strength of the second.
        got = acknowledgements.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "ccc"]},
            set(),
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_an_uncommitted_path_is_never_excused(self):
        # There is no commit to point at, so nobody has read the diff.
        got = acknowledgements.excused_paths(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa"]},
            {"scripts/generate_perf_data.py"},
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_a_path_with_no_commits_in_range_is_not_excused(self):
        got = acknowledgements.excused_paths(
            "nested-end-to-end", ["scripts/generate_perf_data.py"], {}, set(), self.ACKS
        )
        self.assertEqual(got, [])

    def test_an_inert_entry_is_named_with_what_holds_the_path_red(self):
        # The failure this exists against: an entry that excuses one commit on
        # a path another commit also touched vanishes from every output, which
        # reads as a missing entry and has been mistaken for one.
        got = acknowledgements.inert_excuses(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["aaa", "ccc"]},
            self.ACKS,
        )
        self.assertEqual(got, [("scripts/generate_perf_data.py", ["aaa"], ["ccc"])])

    def test_a_path_nothing_excuses_gets_no_commentary(self):
        got = acknowledgements.inert_excuses(
            "nested-end-to-end",
            ["scripts/generate_perf_data.py"],
            {"scripts/generate_perf_data.py": ["ccc"]},
            self.ACKS,
        )
        self.assertEqual(got, [])

    def test_the_live_register_names_only_figures_and_declared_sections(self):
        # The names `--check` admits: a declared section is acknowledged by
        # name (`excuses`), so an entry naming `koji` is no typo.
        known = set(register.ALL_BY_ID) | set(outside_sections.NOT_OURS)
        for ack in acknowledged.ACKNOWLEDGED:
            for fid in ack.figures:
                with self.subTest(commit=ack.commit, figure=fid):
                    self.assertIn(fid, known)

    def test_every_live_entry_carries_its_evidence(self):
        # An entry may legitimately have no mechanical check, but it may not
        # have no *reason*: `why` is what a reader weighs when the excuse is
        # the only thing between a figure and a re-take.
        for ack in acknowledged.ACKNOWLEDGED:
            with self.subTest(commit=ack.commit):
                self.assertTrue(ack.why.strip())

    def test_a_spent_entry_is_reported_rather_than_kept(self):
        acks = (acknowledged.Acknowledged(commit="aaa", figures=(), why="x"),)
        unknown, spent = check.acknowledgement_problems(acks, register.ALL_BY_ID, ["aaa"])
        self.assertEqual(unknown, [])
        self.assertEqual(spent, ["aaa"])

    def test_an_entry_naming_no_figure_is_reported(self):
        acks = (acknowledged.Acknowledged(commit="aaa", figures=("gone",), why="x"),)
        unknown, spent = check.acknowledgement_problems(acks, register.ALL_BY_ID, [])
        self.assertEqual(unknown, ["aaa names gone"])
        self.assertEqual(spent, [])

    def test_spentness_is_read_off_each_figure_s_own_base(self):
        # An entry recorded between the stamp and a figure taken later is spent
        # for that figure and live for every figure read from the stamp, so it
        # is deleted only when every figure it covers has moved past it.
        bases = {fid: "stamp" for fid in register.ALL_BY_ID}
        bases["peak-rss"] = "later"
        acks = (
            acknowledged.Acknowledged(commit="aaa", figures=("peak-rss", "map-only"), why="x"),
            acknowledged.Acknowledged(commit="bbb", figures=("peak-rss",), why="x"),
        )
        # `aaa` is inside `map-only`'s range and behind `peak-rss`'s; `bbb`
        # covers only the figure that has moved past it.
        spent = check.spent_acknowledgements(
            acks, bases, ancestor=lambda commit, rev: rev == "later"
        )
        self.assertEqual(spent, ["bbb"])

    def test_an_entry_covering_every_figure_needs_every_base_past_it(self):
        bases = {fid: "stamp" for fid in register.ALL_BY_ID}
        bases["peak-rss"] = "later"
        acks = (acknowledged.Acknowledged(commit="aaa", figures=(), why="x"),)
        self.assertEqual(
            check.spent_acknowledgements(acks, bases, ancestor=lambda c, rev: rev == "later"),
            [],
        )
        self.assertEqual(
            check.spent_acknowledgements(acks, bases, ancestor=lambda c, rev: True), ["aaa"]
        )

    def test_a_blanket_entry_s_spentness_ignores_the_declared_sections(self):
        # `--check` passes the union of both bases, so an entry *naming* koji is
        # spent against koji's own marker. A blanket entry covers no section, so
        # a section left behind may not hold it alive past every figure.
        bases = {fid: "later" for fid in register.ALL_BY_ID}
        bases |= {oid: "stamp" for oid in outside_sections.NOT_OURS}
        acks = (acknowledged.Acknowledged(commit="aaa", figures=(), why="x"),)
        self.assertEqual(
            check.spent_acknowledgements(acks, bases, ancestor=lambda c, rev: rev == "later"),
            ["aaa"],
        )
        named = (acknowledged.Acknowledged(commit="aaa", figures=("koji",), why="x"),)
        self.assertEqual(
            check.spent_acknowledgements(named, bases, ancestor=lambda c, rev: rev == "later"),
            [],
        )

    def test_a_figure_with_no_base_spends_nothing(self):
        acks = (acknowledged.Acknowledged(commit="aaa", figures=("map-only",), why="x"),)
        self.assertEqual(
            check.spent_acknowledgements(acks, {"map-only": None}, ancestor=lambda c, r: True),
            [],
        )


if __name__ == "__main__":
    unittest.main()
