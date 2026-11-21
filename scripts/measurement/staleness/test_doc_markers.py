"""Tests of `measurement.staleness.doc_markers`."""

from __future__ import annotations

import acknowledged
import contextlib
import inspect
import io
import json
import re
import release
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import test_support
from measurement import check
from measurement import config
from measurement import drift
from measurement import emit
from measurement import host
from measurement import parsing
from measurement import replay
from measurement import runspec
from measurement import staging
from measurement import timing
from measurement.builds import leg_binaries
from measurement.figures import micro
from measurement.figures import register
from measurement.staleness import doc_markers
from measurement.staleness import outside_sections
from measurement.staleness import stale


class Glibc(unittest.TestCase):
    """Which glibc a figure ran under: the image pinned by digest, each place
    a figure's program runs asked for its glibc, and the answer named in the
    stamp or in the figure's own marker (`measurements.md`, "The apparatus")."""

    DOC = config.REPO / "docs/design/measurements.md"
    PIN = re.compile(r"[^@\s]+:[^@\s]+@sha256:[0-9a-f]{64}")

    def test_the_image_is_pinned_by_digest(self):
        # A tag moves under the register; a digest is the image a figure ran in.
        self.assertRegex(config.Config().image, self.PIN)

    def test_the_image_is_the_release_image_s_base(self):
        # One pin names the glibc every timed binary links against and the one
        # it runs under, so nothing but the Dockerfile moves it.
        dockerfile = release.DOCKERFILE.read_text()
        self.assertEqual(config.Config().image, release.pinned_base(dockerfile))
        self.assertNotIn("PGDT_MEASURE_IMAGE", test_support.harness_source())

    def test_an_image_is_asked_in_a_container_and_nothing_runs_on_the_host(self):
        cfg = config.Config()
        asked = []

        def fake(argv, **_kwargs):
            asked.append(list(argv))
            return "glibc 2.41\n"

        with unittest.mock.patch.object(host, "run", fake):
            self.assertEqual(leg_binaries.glibc_of(cfg, "some:image@sha256:ab"), "2.41")
        self.assertEqual(
            asked,
            [[*cfg.container_argv(), "run", "--rm", "some:image@sha256:ab",
              "getconf", "GNU_LIBC_VERSION"]],
        )
        # No figure's program runs on the host, so there is no host place to ask.
        self.assertFalse(test_support.harness_has("HOST"))

    def test_a_place_that_is_no_glibc_is_refused(self):
        # musl's `getconf` has no GNU_LIBC_VERSION and exits non-zero.
        failed = subprocess.CalledProcessError(1, ["getconf"])
        with unittest.mock.patch.object(host, "run", side_effect=failed):
            with self.assertRaises(RuntimeError) as raised:
                leg_binaries.glibc_of(config.Config(), "alpine:3")
        self.assertIn("alpine:3", str(raised.exception))
        with unittest.mock.patch.object(host, "run", return_value="musl\n"):
            with self.assertRaises(RuntimeError):
                leg_binaries.glibc_of(config.Config(), "alpine:3")

    def test_the_stamp_names_the_glibc_and_still_reads_back(self):
        stamp = emit.session_stamp("deadbee", dirty=True, allocator="system", glibc="2.41")
        self.assertIn("under the `system` allocator and glibc 2.41.", stamp)
        self.assertEqual(doc_markers.stamp_in(stamp), "deadbee")
        self.assertEqual(doc_markers.stamp_glibc(stamp), "2.41")
        self.assertIsNone(doc_markers.stamp_glibc(emit.session_stamp("deadbee", dirty=False)))
        self.assertIn(", under glibc 2.41", emit.taken_against(False, None, "2.41"))

    def test_a_partial_sitting_says_its_glibc_too(self):
        lead = emit.partial_lead(1, "deadbee", False, "system", None, "2.41")
        self.assertIn("glibc 2.41", lead)

    def test_a_marker_names_its_glibc_beside_its_sitting_and_both_read_back(self):
        marker = doc_markers.figure_marker("map-only", "7ee5db5", glibc="2.44")
        self.assertEqual(doc_markers.figure_sittings(marker), {"map-only": "7ee5db5"})
        self.assertEqual(doc_markers.marker_glibcs(marker), {"map-only": "2.44"})
        alone = doc_markers.figure_marker("nested-decode-micro", glibc="2.41 and 2.44")
        self.assertEqual(doc_markers.figure_sittings(alone), {})
        self.assertEqual(doc_markers.marker_glibcs(alone), {"nested-decode-micro": "2.41 and 2.44"})

    def test_a_marker_names_its_glibc_only_where_the_stamp_does_not_speak_for_it(self):
        # A figure of the sweep run in the register's image: the stamp speaks.
        self.assertIsNone(doc_markers.marker_glibc(True, "2.41", "2.41"))
        # Run elsewhere — the host.
        self.assertEqual(doc_markers.marker_glibc(True, "2.41", "2.44"), "2.44")
        # A sitting of its own names its glibc beside its commit, whatever it is.
        self.assertEqual(doc_markers.marker_glibc(False, "2.41", "2.41"), "2.41")
        # Nothing asked, nothing named: `--dry-run`, or a sitting older than this.
        self.assertIsNone(doc_markers.marker_glibc(False, "2.41", None))

    def test_several_places_are_named_together(self):
        glibcs = {"a": "2.44", "b": "2.41", "c": "2.41"}
        self.assertEqual(leg_binaries.glibc_named(["a", "b", "c"], glibcs), "2.41 and 2.44")
        self.assertIsNone(leg_binaries.glibc_named(["a"], {}))

    def test_a_sweep_records_where_the_program_ran_and_not_the_floor(self):
        cfg = config.Config(dry_run=True)
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        leg = runspec.RunSpec("dfcli", "dynfilter", "dfcli-dynamic-filter-topk-unsorted-on",
                              "warm", "")
        floor = runspec.RunSpec("none", "dynfilter", "dd", "warm", "dd floor")
        session.sweep("dynamic-filter-topk", [leg, floor], 1)
        self.assertEqual(session.places["dynamic-filter-topk"], {cfg.image})
        # `--dry-run` asks nothing: it must not need root or a runtime.
        self.assertEqual(session.glibcs, {})
        with unittest.mock.patch.object(release, "ensure_image") as built:
            micro.run_nested_decode_micro(session)
        built.assert_not_called()
        self.assertEqual(
            session.places["nested-decode-micro"], {release.image_tag(release.image_inputs())}
        )

    def test_a_bench_runs_in_the_release_image_and_is_read_where_criterion_wrote(self):
        cfg = config.Config()
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        order: list[str] = []
        ran: list[list[str]] = []
        roots: set[Path] = set()

        def median(root, _full_id):
            roots.add(root)
            return 100.0

        def asked(_cfg, where):
            order.append(f"asked {where}")
            return "2.41"

        def fake_run(argv, **_kwargs):
            order.append("ran")
            ran.append(list(argv))
            return ""

        with unittest.mock.patch.object(release, "ensure_image", return_value="pgdt-release:t"), \
                unittest.mock.patch.object(release, "prepare_state"), \
                unittest.mock.patch.object(leg_binaries, "glibc_of", asked), \
                unittest.mock.patch.object(host, "run", fake_run), \
                unittest.mock.patch.object(parsing, "criterion_median_ns", median):
            micro.run_nested_decode_micro(session)
        # The image is asked its glibc before the bench runs, and is where it ran.
        self.assertEqual(order, ["asked pgdt-release:t", "ran"])
        self.assertEqual(session.places["nested-decode-micro"], {"pgdt-release:t"})
        # Timed, so it is given every core: no cpuset.
        self.assertEqual(
            ran, [release.run_in_image(cfg.container_argv(), "pgdt-release:t",
                                       micro.NESTED_BENCH.step(), reserve=0)],
        )  # fmt: skip
        self.assertNotIn("--cpuset-cpus", ran[0])
        self.assertEqual(
            ran[0][-5:],
            ["bench", "--package=pgdt-codec", "--bench=decoders", "--filter=nested", "--inside"],
        )
        # The estimates are read out of the release state, where the image's
        # `CARGO_TARGET_DIR` puts criterion's output, never the host's `target/`.
        self.assertEqual(roots, {release.STATE / "target" / "criterion" / "nested"})

    def test_a_place_is_asked_once_and_before_its_first_run(self):
        cfg = config.Config()
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
        with unittest.mock.patch.object(leg_binaries, "glibc_of", return_value="2.41") as asked:
            session.ran_in("f", cfg.image)
            session.ran_in("g", cfg.image)
        asked.assert_called_once_with(cfg, cfg.image)
        self.assertEqual(session.places, {"f": {cfg.image}, "g": {cfg.image}})

    def test_a_render_names_what_the_sitting_recorded_and_asks_nothing(self):
        render = inspect.getsource(emit.render)
        self.assertIn('raw.get("figure_glibc"', render)
        self.assertIn("marker_glibc(", render)
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(), {"readings": {}, "runs": []}, Path(tmp), lambda _m: None
            )
            with unittest.mock.patch.object(leg_binaries, "glibc_of") as asked:
                session.ran_in("nested-decode-micro", "pgdt-release:0123456789abcdef")
            asked.assert_not_called()

    def test_the_check_wants_a_glibc_in_the_stamp_and_in_every_sitting_marker(self):
        text = (
            emit.session_stamp("aaaaaaa", dirty=False, allocator="system") + "\n"
            + doc_markers.figure_marker("map-only", "bbbbbbb") + "\n"
            + doc_markers.figure_marker("peak-rss", "ccccccc", glibc="2.41") + "\n"
        )
        problems = doc_markers.glibc_problems(text)
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("session stamp", problems[0])
        self.assertIn("map-only", problems[1])

    def test_the_doc_names_every_glibc_it_must(self):
        self.assertEqual(doc_markers.glibc_problems(self.DOC.read_text()), [])
        self.assertIsNotNone(doc_markers.stamp_glibc(self.DOC.read_text()))

    def test_a_drift_taken_outside_the_stamp_names_the_sweeps_glibc(self):
        with tempfile.TemporaryDirectory() as tmp:
            dirs = []
            for name in ("a", "b"):
                d = Path(tmp) / name
                d.mkdir()
                (d / "raw.json").write_text(
                    json.dumps(
                        {"commit": "abc1234", "date": "2026-09-27", "glibc": "2.41",
                         "readings": {"f/x": [1.0]}}
                    )
                )
                dirs.append(str(d))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                drift.cmd_drift(*dirs)
        self.assertEqual(doc_markers.marker_glibcs(out.getvalue()), {"session-drift": "2.41"})


class Stamp(unittest.TestCase):
    def test_a_stamp_wrapped_across_lines_still_yields_a_commit(self):
        # The stamp is prose in a hard-wrapped doc, so it is never one line.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(
                "**Session stamp.** Every figure below was taken by `scripts/measure.py` on\n"
                "2026-08-28, against commit `3739c26`. One sweep, one apparatus.\n"
            )
            self.assertEqual(doc_markers.stamped_commit(doc), "3739c26")

    def test_the_stamp_does_not_reach_across_the_document(self):
        # A bounded run, so "measure.py" in one paragraph cannot bind to a
        # commit hash mentioned much later.
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text("measure.py\n" + ("filler line\n" * 40) + "commit `abc1234`\n")
            self.assertIsNone(doc_markers.stamped_commit(doc))

    def test_the_stamp_line_yields_a_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(
                "**Session stamp.** Every figure below was taken by `scripts/measure.py` on "
                "2026-08-28, against commit `a376d47`.\n"
            )
            self.assertEqual(doc_markers.stamped_commit(doc), "a376d47")

    def test_an_unstamped_doc_yields_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text("# Measurements\n")
            self.assertIsNone(doc_markers.stamped_commit(doc))

    def test_the_stamp_the_harness_emits_is_the_one_it_can_read_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "measurements.md"
            doc.write_text(emit.session_stamp("deadbee", dirty=False) + "\n")
            self.assertEqual(doc_markers.stamped_commit(doc), "deadbee")

    def test_a_tree_dirty_under_a_measured_path_says_so(self):
        self.assertIn("uncommitted", emit.session_stamp("deadbee", dirty=True))

    def test_a_clean_stamp_claims_nothing_extra(self):
        self.assertNotIn("uncommitted", emit.session_stamp("deadbee", dirty=False))


class Markers(unittest.TestCase):
    """The doc addresses a figure by id, never by heading — so a heading is
    free to quote a number and free to be rewritten when the number moves."""

    def _doc(self, tmp: str, text: str) -> Path:
        doc = Path(tmp) / "measurements.md"
        doc.write_text(text)
        return doc

    def test_a_marker_is_read_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(tmp, "## Anything at all\n\n<!-- figure: map-only -->\n")
            self.assertEqual(doc_markers.markers_in(doc), ["map-only"])

    def test_the_emitted_marker_is_the_one_that_is_read_back(self):
        # What emit() writes above each table must be what --check finds after
        # the paste, trailing prose in the comment included.
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(
                tmp,
                "<!-- figure: nested-end-to-end — reproduce with `cd scripts && "
                "uv run measure.py --figure nested-end-to-end` -->\n",
            )
            self.assertEqual(doc_markers.markers_in(doc), ["nested-end-to-end"])

    def test_a_heading_that_quotes_a_number_is_not_an_address(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(
                tmp,
                "## A typed query over nested columns costs 6.6 µs a row more than a string one\n\n"
                "<!-- figure: nested-end-to-end -->\n",
            )
            self.assertEqual(doc_markers.markers_in(doc), ["nested-end-to-end"])

    def test_no_marker_is_no_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(doc_markers.markers_in(self._doc(tmp, "# Measurements\n")), [])

    def test_every_label_begins_with_its_marker_s_heading(self):
        # The label renders as the sitting's heading, so one left quoting the
        # number its heading was rewritten from prints it at the next sweep.
        text = (config.REPO / "docs/design/measurements.md").read_text()
        self.assertEqual(doc_markers.section_label_problems(text), [])

    def test_a_label_quoting_a_number_its_heading_no_longer_quotes_is_reported(self):
        fig = register.ALL_BY_ID["nested-end-to-end"]
        text = "## A heading rewritten when its number moved\n\n<!-- figure: nested-end-to-end -->\n"
        self.assertEqual(
            doc_markers.section_label_problems(text),
            [
                f"nested-end-to-end: label {fig.section!r}, "
                "heading 'A heading rewritten when its number moved'"
            ],
        )

    def test_a_suffix_past_the_heading_is_a_label(self):
        # `map-only` shares `per-block-quadratic`'s heading and says so.
        heading = register.ALL_BY_ID["per-block-quadratic"].section
        self.assertTrue(register.ALL_BY_ID["map-only"].section.startswith(heading + " "))
        text = f"## {heading}\n\n<!-- figure: map-only -->\n"
        self.assertEqual(doc_markers.section_label_problems(text), [])

    def test_the_heading_is_the_nearest_above_the_marker_outside_a_fence(self):
        fig = register.ALL_BY_ID["map-only"]
        text = f"## {fig.section}\n\n```sh\n# not a heading\n```\n\n<!-- figure: map-only -->\n"
        self.assertEqual(doc_markers.section_label_problems(text), [])

    def test_a_marker_under_no_heading_is_reported(self):
        self.assertEqual(
            doc_markers.section_label_problems("<!-- figure: map-only -->\n"),
            ["map-only sits under no heading"],
        )

    def test_check_fails_on_a_label_its_heading_left_behind(self):
        fig = register.ALL_BY_ID["nested-end-to-end"]
        with unittest.mock.patch.object(fig, "section", "A typed query costs 13.2 µs"):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = check.cmd_check(config.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("Labels not beginning with their marker's heading", out.getvalue())

    def test_the_doc_and_the_register_agree_exactly(self):
        """Bidirectional, now that every figure has been folded in: a rename on
        either side breaks this rather than going unnoticed."""
        markers = doc_markers.markers_in(config.REPO / "docs/design/measurements.md")
        self.assertEqual(sorted(markers), sorted(f.id for f in register.ALL_FIGURES))

    def test_no_figure_is_marked_twice(self):
        # One figure is one table.
        markers = doc_markers.markers_in(config.REPO / "docs/design/measurements.md")
        self.assertEqual(len(markers), len(set(markers)))

    def test_the_doc_s_session_stamp_names_a_commit(self):
        # `--stale` defaults to it, so an unparseable stamp silently disarms
        # the only mechanism that says a figure has gone stale.
        stamp = doc_markers.stamped_commit(config.REPO / "docs/design/measurements.md")
        self.assertIsNotNone(stamp)


class RegisterBoundary(unittest.TestCase):
    """The doc carries sections the harness does not own, and the session stamp
    claims nothing about them. A figure announces itself with a marker and an
    apparatus line; a section outside the register announced itself with a
    prose sentence paragraphs from the numbers, which nothing checked."""

    DOC = config.REPO / "docs/design/measurements.md"

    def _doc(self, tmp: str, text: str) -> Path:
        doc = Path(tmp) / "measurements.md"
        doc.write_text(text)
        return doc

    def test_the_doc_declares_every_section_the_harness_disowns(self):
        # Both ways: a declaration naming nothing, and a disowned section that
        # declares nothing, are the two halves of the same drift.
        declared = {i for i, _ in doc_markers.outside_register_sections(self.DOC.read_text())}
        self.assertEqual(declared, set(outside_sections.NOT_OURS))

    def test_no_declared_section_carries_a_figure_marker(self):
        for oid, figs in doc_markers.outside_register_sections(self.DOC.read_text()):
            with self.subTest(section=oid):
                self.assertEqual(figs, [])

    def test_a_declaration_covers_its_own_subheadings(self):
        text = (
            "## Outside\n\n<!-- outside-register: koji -->\n\n"
            "### A subsection of it\n\n<!-- figure: map-only -->\n"
        )
        self.assertEqual(doc_markers.outside_register_sections(text), [("koji", ["map-only"])])

    def test_a_declaration_stops_at_the_next_section(self):
        text = (
            "## Outside\n\n<!-- outside-register: koji -->\n\n"
            "## A figure of its own\n\n<!-- figure: map-only -->\n"
        )
        self.assertEqual(doc_markers.outside_register_sections(text), [("koji", [])])

    def test_a_comment_inside_a_code_fence_does_not_end_a_section(self):
        # measurements.md publishes a `sh` block whose first line is `# maps to
        # EOF …`, which reads as a level-1 heading.
        text = (
            "## Outside\n\n<!-- outside-register: koji -->\n\n"
            "```sh\n# maps to EOF (the table never matches)\npgdt parse\n```\n\n"
            "<!-- figure: map-only -->\n"
        )
        self.assertEqual(doc_markers.outside_register_sections(text), [("koji", ["map-only"])])

    def test_headings_skip_fenced_blocks(self):
        text = "# Real\n\n```sh\n# not a heading\n```\n\n## Also real\n"
        self.assertEqual([level for _, level in doc_markers.headings(text)], [1, 2])

    def test_the_stamp_is_scoped_to_the_register(self):
        # The claim was "every figure below", over a document that prints
        # readings the harness did not take.
        self.assertIn("<!-- figure:", emit.session_stamp("deadbee", dirty=False))

    def test_the_stamp_s_scope_clause_is_not_read_as_a_figure_marker(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._doc(tmp, emit.session_stamp("deadbee", dirty=False) + "\n")
            self.assertEqual(doc_markers.markers_in(doc), [])

    def test_the_doc_s_own_stamp_is_scoped_too(self):
        head = self.DOC.read_text().split("## The apparatus")[0]
        self.assertIn("`<!-- figure: … -->` marker, and no other", head)


class OutsideInvalidation(unittest.TestCase):
    """A declared section carries the same invalidation edge a figure does.

    Being outside the register says the harness cannot re-take the readings. It
    was also saying, by omission, that nothing would ever be told when they
    went wrong: `--stale` walked `ALL_BY_ID`, so koji could not go red however
    far the scanner moved under it. `Outside.depends` is that edge, and the
    commit it is measured from lives in the section's own marker — in the
    document, because `session-drift` declares the harness's source and a run's
    provenance recorded here would mark a figure stale for saying where another
    reading came from."""

    DOC = config.REPO / "docs/design/measurements.md"

    def test_every_section_that_publishes_a_reading_declares_an_edge(self):
        # `benches` is the one that legitimately declares nothing, and it is
        # the same sentence that puts it outside the register: it quotes no
        # number in the doc, so there is nothing a diff could falsify.
        publishing = {oid for oid, o in outside_sections.NOT_OURS.items() if o.depends}
        self.assertEqual(publishing, {"koji"})
        self.assertEqual(outside_sections.NOT_OURS["benches"].depends, ())

    def test_koji_s_edge_covers_what_a_koji_run_concludes(self):
        # The block list, the offsets and the row/byte totals come off the read
        # path, the scanner, the map and the cache; the `info --detail` report
        # they are compared as text against comes off the preamble, the type
        # resolution and the CLI -- and that last one is not hypothetical, the
        # two compared reports differing by one line of user-defined-type
        # detail the report had gained in between.
        for path in (
            "crates/pgdt-io/src/source.rs",
            "crates/pgdt-dump/src/scan.rs",
            "crates/pgdt-dump/src/map.rs",
            "crates/pgdt-dump/src/cache.rs",
            "crates/pgdt-dump/src/preamble.rs",
            "crates/pgdt-codec/src/resolve.rs",
            "crates/pgdt-codec/src/pgtype.rs",
            "crates/pgdt/src/main.rs",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    stale.declared_hits(outside_sections.NOT_OURS["koji"], [path]), [path]
                )

    def test_the_attribution_is_no_longer_a_section_the_harness_disowns(self):
        # A registered figure carries no `Outside` row and its section no
        # `outside-register` marker, which is the reconciliation `--check` makes
        # both ways.
        self.assertNotIn("rss-attribution", outside_sections.NOT_OURS)
        self.assertIn("rss-attribution", register.FIGURES_BY_ID)
        self.assertNotIn(
            "outside-register: rss-attribution",
            (config.REPO / "docs/design/measurements.md").read_text(),
        )

    def test_one_predicate_answers_for_a_figure_and_a_section(self):
        # `declared_hits` is what `--stale` intersects a diff with, and an
        # `Outside` declares its edge in the same field a `Figure` does so that
        # no second authority over "what moves a reading" is written.
        self.assertEqual(
            stale.declared_hits(outside_sections.NOT_OURS["koji"], ["docs/design/roadmap.md"]), []
        )

    def test_the_marker_carries_the_commit_and_is_read_back(self):
        text = "<!-- outside-register: koji — taken at `f5768e7` — nothing here is a figure -->"
        self.assertEqual(doc_markers.outside_sittings(text), {"koji": "f5768e7"})
        self.assertEqual(doc_markers.OUTSIDE_RE.findall(text), ["koji"])

    def test_a_section_with_no_reading_declares_no_commit(self):
        text = "<!-- outside-register: benches — tripwires, not figures -->"
        self.assertEqual(doc_markers.outside_sittings(text), {})

    def test_a_commit_in_the_prose_under_a_section_is_not_its_provenance(self):
        # Same reason a figure's sitting went inside its marker: a sibling can
        # go missing on its own, and its absence is silent.
        text = "<!-- outside-register: koji -->\n\nThe last run was at commit `f5768e7`.\n"
        self.assertEqual(doc_markers.outside_sittings(text), {})

    def test_the_doc_s_own_declarations_all_stand(self):
        self.assertEqual(doc_markers.outside_sitting_problems(self.DOC.read_text()), [])

    def _about(self, text: str, oid: str) -> list[str]:
        """The refusals naming one section. Every declared section is walked on
        every call — a fragment that omits one is a section declaring nothing —
        so a test about one of them says which."""
        return [line for line in doc_markers.outside_sitting_problems(text) if line.startswith(oid)]

    def test_an_edge_with_no_commit_to_measure_it_from_is_refused(self):
        # The state koji was in before this landed, reached from the other
        # side: a live-looking edge that `--stale` has no range to intersect.
        problems = self._about("<!-- outside-register: koji -->", "koji")
        self.assertEqual(len(problems), 1)
        self.assertIn("no commit", problems[0])

    def test_a_commit_no_edge_reads_is_refused(self):
        problems = self._about(
            "<!-- outside-register: benches — taken at `f5768e7` -->", "benches"
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("no invalidation edge", problems[0])

    def test_a_commit_that_is_not_one_is_refused(self):
        problems = doc_markers.outside_sitting_problems(
            self.DOC.read_text(), resolve=lambda rev: None
        )
        self.assertEqual(len(problems), 1)
        for line in problems:
            self.assertIn("not a commit in this repository", line)

    def test_a_provenance_line_for_a_section_nobody_disowns_is_refused(self):
        problems = self._about(
            "<!-- outside-register: koji — taken at `f5768e7` -->\n"
            "<!-- outside-register: gone — taken at `f5768e7` -->",
            "gone",
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("no section this harness disowns", problems[0])

    def test_a_declared_section_is_argued_from_its_own_marker(self):
        # Never from the session stamp, which is scoped to the register and
        # says nothing about a section outside it.
        bases = doc_markers.outside_bases(self.DOC.read_text())
        self.assertEqual(set(bases), {"koji"})
        self.assertNotEqual(bases["koji"], doc_markers.stamp_in(self.DOC.read_text()))

    def test_since_asks_one_question_of_every_section(self):
        bases = doc_markers.outside_bases(self.DOC.read_text(), "deadbee")
        self.assertEqual(set(bases.values()), {"deadbee"})

    def test_an_acknowledgement_may_name_a_declared_section(self):
        # The harness cannot re-take koji, so an entry is the only discharge
        # that does not cost an hour on the HDD -- and `--check` must not read
        # that entry as naming nothing.
        acks = [acknowledged.Acknowledged(commit="aaa", figures=("koji",), why="x")]
        unknown, _ = check.acknowledgement_problems(
            acks, {**register.ALL_BY_ID, **outside_sections.NOT_OURS}, []
        )
        self.assertEqual(unknown, [])
        typo = [acknowledged.Acknowledged(commit="aaa", figures=("kojii",), why="x")]
        unknown, _ = check.acknowledgement_problems(
            typo, {**register.ALL_BY_ID, **outside_sections.NOT_OURS}, []
        )
        self.assertEqual(len(unknown), 1)


if __name__ == "__main__":
    unittest.main()
